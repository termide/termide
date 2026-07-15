//! Copy/move entry points and remote transfer setup for batch operations.

// Note: PanelExt is used for FileManager batch operations (copy/move/delete/rename).
#![allow(deprecated)]

use anyhow::Result;
use std::path::PathBuf;

use super::super::App;
use super::batch_handler::{make_copy_or_move_request, source_name};
use crate::state::{ActiveModal, BatchOperation, BatchOperationType, PendingAction};
use crate::PanelExt;
use termide_file_ops::{OperationPath, OperationRequest};
use termide_modal::ConflictModal;
use termide_ui::path_utils;
use termide_vfs::VfsPath;

impl App {
    pub(in crate::app::modal) fn resolve_local_destination_input(
        &mut self,
        target_directory: Option<&PathBuf>,
        dest_str: &str,
    ) -> Option<(PathBuf, bool)> {
        let base_dir = if let Some(target_dir) = target_directory {
            target_dir.clone()
        } else if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(fm) = panel.as_file_manager_mut() {
                fm.get_current_directory()
            } else {
                log::error!("Active panel is not FileManager");
                return None;
            }
        } else {
            log::error!("No active panel found");
            return None;
        };

        Some(path_utils::resolve_local_destination_input(
            &base_dir, dest_str,
        ))
    }

    /// Parse a VFS URL, logging and setting error on failure.
    fn parse_remote_url(&mut self, remote_url: &str) -> Option<VfsPath> {
        match termide_vfs::parse_vfs_url(remote_url) {
            Ok(path) => Some(path),
            Err(e) => {
                log::error!("Invalid remote URL '{}': {}", remote_url, e);
                self.show_error_modal(format!("Invalid remote URL: {}", e));
                None
            }
        }
    }

    /// Common method for handling file operations (Copy/Move)
    fn handle_file_operation(
        &mut self,
        operation_type: BatchOperationType,
        sources: Vec<PathBuf>,
        target_directory: Option<PathBuf>,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        // Extract destination string first to check if it's a remote URL
        let destination_str: Option<String> = if let Some(confirmed) = value.downcast_ref::<bool>()
        {
            if !confirmed {
                return Ok(()); // Operation cancelled by user
            }
            // Use target_directory as string for Ctrl+V confirmation
            target_directory.as_ref().map(|p| p.display().to_string())
        } else if let Some(s) = value.downcast_ref::<String>() {
            Some(s.clone())
        } else {
            return Ok(()); // Invalid response type
        };

        let Some(mut dest_str) = destination_str else {
            return Ok(());
        };

        // If the user typed a bare relative name (e.g. "newname.txt" from
        // the rename modal) while the source panel is remote, the value
        // alone is not a VFS URL. Join it with the remote `target_directory`
        // URL so the downstream `is_vfs_url` check sees the full URL and
        // routes through the remote ops path — otherwise the operation
        // falls through to local fs and creates surprises like a local
        // directory named after the new file.
        if let Some(td) = target_directory.as_ref() {
            let td_str = td.display().to_string();
            if termide_vfs::is_vfs_url(&td_str) && !termide_vfs::is_vfs_url(&dest_str) {
                let base = td_str.trim_end_matches('/');
                let tail = dest_str.trim_start_matches('/');
                dest_str = format!("{base}/{tail}");
            }
        }

        // Check if destination is a remote VFS URL (e.g., sftp://user@host/path)
        if termide_vfs::is_vfs_url(&dest_str) {
            // Check if the ACTIVE panel (source) is remote — that means same-server copy.
            // Use active panel, not find_remote_file_manager_info() which searches ALL panels
            // and would incorrectly match the destination panel for local→remote uploads.
            let active_is_remote = self
                .layout_manager
                .active_panel()
                .and_then(|p| {
                    p.as_any()
                        .downcast_ref::<termide_panel_file_manager::FileManager>()
                })
                .map(|fm| fm.is_remote())
                .unwrap_or(false);

            if active_is_remote {
                if let Some((vfs_manager, vfs_current_path)) = self.find_remote_file_manager_info()
                {
                    return self.start_remote_to_remote_operation(
                        operation_type,
                        sources,
                        &dest_str,
                        vfs_manager,
                        vfs_current_path,
                    );
                }
            }
            // Local-to-remote upload
            return self.start_upload_operation(operation_type, sources, &dest_str);
        }

        let Some((absolute_destination, destination_is_directory)) =
            self.resolve_local_destination_input(target_directory.as_ref(), &dest_str)
        else {
            return Ok(());
        };

        // Create and start batch operation
        let batch_op = BatchOperation::new(operation_type, sources, absolute_destination)
            .with_destination_directory(destination_is_directory);

        self.process_batch_operation(batch_op);
        Ok(())
    }

    /// Start upload operation for local-to-remote file transfer
    fn start_upload_operation(
        &mut self,
        operation_type: BatchOperationType,
        sources: Vec<PathBuf>,
        remote_url: &str,
    ) -> Result<()> {
        use crate::state::OperationType;

        if sources.is_empty() {
            return Ok(());
        }

        // Parse the remote URL
        let remote_path = match self.parse_remote_url(remote_url) {
            Some(path) => path,
            None => return Ok(()),
        };

        // Find the destination panel that is connected to this remote
        let (vfs_manager, connected_path) = match self.find_connected_vfs_manager(&remote_path) {
            Some(result) => result,
            None => {
                log::error!(
                    "No active connection to remote host: {}",
                    remote_path.log_safe_key()
                );
                self.show_error_modal("No active connection to remote host".to_string());
                return Ok(());
            }
        };

        // Normalize remote_path to use connection info from the connected panel
        // (user URL may omit username/port that were resolved from SSH config)
        let remote_path = Self::vfs_path_with_connection(&connected_path, remote_path.path);

        let is_move = operation_type == BatchOperationType::Move;
        let total_files = sources.len();

        // Start with the first file (clone to avoid borrow issues)
        let source = sources[0].clone();
        let src_name = source_name(&source);

        // Determine final remote destination path using VFS stat
        let (final_remote, dest_exists) = self.resolve_remote_dest(
            &remote_path,
            &src_name,
            remote_url,
            sources.len() > 1,
            &vfs_manager,
        );

        if dest_exists {
            // Show conflict modal
            let remaining = sources.len().saturating_sub(1);
            let modal = ConflictModal::new(
                &source,
                &PathBuf::from(final_remote.to_url_string()),
                remaining,
            );
            self.state.pending_action = Some(PendingAction::ContinueBatchOperation {
                operation: BatchOperation::new(operation_type, sources, PathBuf::from(remote_url)),
            });
            self.state.active_modal = Some(ActiveModal::Conflict(Box::new(modal)));
            return Ok(());
        }

        // Determine operation type for display
        let op_type = if is_move {
            OperationType::MoveUpload
        } else {
            OperationType::CopyUpload
        };

        // Extract display strings before moving values
        let source_display = source.display().to_string();
        let dest_display = final_remote.to_url_string();
        // Snapshot the exact remote destination — we need it later if
        // the user cancels and we have to ask whether to delete the
        // partial file. Cloning here is cheaper than reconstructing
        // the URL post-cancel (and avoids the double-filename bug
        // when remote_url already ends with the filename for single
        // source uploads).
        let cancel_target = final_remote.clone();

        // Create upload operation request
        let request = OperationRequest::upload(source.clone(), final_remote);

        // Store batch upload state for continuation
        self.state.batch.pending_upload = Some(crate::state::PendingBatchUpload {
            all_sources: sources,
            current_index: 0,
            dest_base_url: remote_url.to_string(),
            vfs_manager: vfs_manager.clone(),
            is_move,
            current_source: source,
            current_remote_target: Some(cancel_target),
        });

        // Start tracked upload operation (opens Operations panel)
        match self.start_tracked_operation(
            request,
            vfs_manager,
            op_type,
            source_display,
            dest_display,
            total_files,
            0, // bytes will be updated during progress
        ) {
            Ok(operation_id) => {
                // Store operation ID for pause/resume
                self.state.active_operation_id = Some(operation_id);
            }
            Err(e) => {
                log::error!("Failed to start upload operation: {}", e);
                self.state.batch.pending_upload = None;
                self.show_error_modal(format!("Upload failed: {}", e));
            }
        }

        Ok(())
    }

    /// Start a remote-to-remote copy/move on the same server.
    ///
    /// Sources are server-side absolute paths (from `get_selected_paths()` where
    /// `current_path` is `/`). The destination is a VFS URL like `sftp://user@host/path/`.
    /// We construct proper VFS paths for both source and destination and use
    /// `CrossProtocolWorker::RemoteToRemote` which downloads to temp then uploads.
    fn start_remote_to_remote_operation(
        &mut self,
        operation_type: BatchOperationType,
        sources: Vec<PathBuf>,
        remote_url: &str,
        vfs_manager: std::sync::Arc<termide_vfs::VfsManager>,
        vfs_current_path: termide_vfs::VfsPath,
    ) -> Result<()> {
        use crate::state::OperationType;

        if sources.is_empty() {
            return Ok(());
        }

        let parsed_dest = match self.parse_remote_url(remote_url) {
            Some(path) => path,
            None => return Ok(()),
        };

        // Normalize destination to use connection info from the connected panel
        // (user URL may omit username/port that were resolved from SSH config)
        let remote_dest = Self::vfs_path_with_connection(&vfs_current_path, parsed_dest.path);

        let is_move = operation_type == BatchOperationType::Move;
        let total_files = sources.len();

        // Build VFS source path from the server-side PathBuf
        let source = &sources[0];
        let src_name = source_name(source);
        let vfs_source = vfs_current_path.join(&src_name);

        // Resolve destination using VFS stat
        let (vfs_dest, dest_exists) = self.resolve_remote_dest(
            &remote_dest,
            &src_name,
            remote_url,
            sources.len() > 1,
            &vfs_manager,
        );

        if dest_exists {
            // Show conflict modal
            let remaining = sources.len().saturating_sub(1);
            let modal =
                ConflictModal::new(source, &PathBuf::from(vfs_dest.to_url_string()), remaining);
            self.state.pending_action = Some(PendingAction::ContinueBatchOperation {
                operation: BatchOperation::new(operation_type, sources, PathBuf::from(remote_url)),
            });
            self.state.active_modal = Some(ActiveModal::Conflict(Box::new(modal)));
            return Ok(());
        }

        let op_type = if is_move {
            OperationType::Move
        } else {
            OperationType::Copy
        };

        // Extract display strings before moving values into the request
        let source_url = vfs_source.to_url_string();
        let dest_url = vfs_dest.to_url_string();

        // Create remote-to-remote copy/move request
        let request = make_copy_or_move_request(
            OperationPath::Remote(vfs_source),
            OperationPath::Remote(vfs_dest),
            is_move,
        );

        match self.start_tracked_operation(
            request,
            vfs_manager,
            op_type,
            source_url,
            dest_url,
            total_files,
            0,
        ) {
            Ok(operation_id) => {
                self.state.active_operation_id = Some(operation_id);
            }
            Err(e) => {
                log::error!("Failed to start remote copy operation: {}", e);
                self.show_error_modal(format!("Remote copy failed: {}", e));
            }
        }

        Ok(())
    }

    /// Resolve remote destination path using VFS stat.
    ///
    /// Logic (mirrors local `resolve_batch_destination_path`):
    /// 1. If URL ends with '/' or multiple sources — treat as directory, append filename
    /// 2. Otherwise, stat the path on server:
    ///    - If it's a directory — append filename (copy INTO directory)
    ///    - If it exists as a file — conflict (return dest path + exists=true)
    ///    - If doesn't exist — use as-is (rename)
    fn resolve_remote_dest(
        &self,
        remote_dest: &VfsPath,
        source_name: &str,
        remote_url: &str,
        is_multi_source: bool,
        vfs_manager: &std::sync::Arc<termide_vfs::VfsManager>,
    ) -> (VfsPath, bool) {
        // Multiple sources or trailing slash — always directory
        if is_multi_source || remote_url.ends_with('/') {
            let final_path = remote_dest.join(source_name);
            let exists = vfs_manager.exists(&final_path).recv().unwrap_or_else(|e| {
                log::error!("resolve_remote_dest: VFS channel error: {}", e);
                false
            });
            return (final_path, exists);
        }

        // Single source, no trailing slash — check what dest is on server
        match vfs_manager.metadata(remote_dest).recv() {
            Ok(meta) if meta.file_type.is_dir() => {
                // Dest is an existing directory — copy INTO it
                let final_path = remote_dest.join(source_name);
                let exists = vfs_manager.exists(&final_path).recv().unwrap_or_else(|e| {
                    log::error!("resolve_remote_dest: VFS channel error: {}", e);
                    false
                });
                (final_path, exists)
            }
            Ok(_) => {
                // Dest exists and is a file — conflict (overwrite)
                (remote_dest.clone(), true)
            }
            Err(_e) => {
                // Dest doesn't exist — use as-is (rename)
                (remote_dest.clone(), false)
            }
        }
    }

    /// Find VFS manager from an existing connected FileManager panel.
    /// Returns None if no panel is connected to the target remote.
    /// Also returns the panel's VfsPath for connection info normalization.
    fn find_connected_vfs_manager(
        &self,
        remote_path: &termide_vfs::VfsPath,
    ) -> Option<(
        std::sync::Arc<termide_vfs::VfsManager>,
        termide_vfs::VfsPath,
    )> {
        for group in &self.layout_manager.panel_groups {
            for panel in group.panels() {
                if let Some(fm) = panel
                    .as_any()
                    .downcast_ref::<termide_panel_file_manager::FileManager>()
                {
                    if fm.is_remote() {
                        let fm_path = fm.vfs_state().current_path();
                        // Check if same connection (protocol + host + username)
                        if fm_path.connection_key() == remote_path.connection_key() {
                            return Some((fm.vfs_state().manager_arc(), fm_path.clone()));
                        }
                    }
                }
            }
        }
        None
    }

    /// Handle file copying
    pub(in crate::app) fn handle_copy_path(
        &mut self,
        sources: Vec<PathBuf>,
        target_directory: Option<PathBuf>,
        create_symlink: bool,
        create_relative_symlink: bool,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if create_symlink {
            return self.handle_create_symlinks(
                sources,
                target_directory,
                create_relative_symlink,
                value,
            );
        }
        self.handle_file_operation(BatchOperationType::Copy, sources, target_directory, value)
    }

    /// Handle file moving
    pub(in crate::app) fn handle_move_path(
        &mut self,
        sources: Vec<PathBuf>,
        target_directory: Option<PathBuf>,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        self.handle_file_operation(BatchOperationType::Move, sources, target_directory, value)
    }
}
