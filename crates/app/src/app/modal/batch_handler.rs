//! Batch file operation handling: shared helpers and the progress/tracking engine.

// Note: PanelExt is used for FileManager batch operations (copy/move/delete/rename).
#![allow(deprecated)]

use std::path::{Path, PathBuf};

use super::super::App;
use crate::state::{
    ActiveModal, BatchOperation, BatchOperationType, ConflictMode, PendingAction,
    PendingRemoteDelete,
};
use crate::PanelExt;
use termide_file_ops::{OperationPath, OperationRequest};
use termide_i18n as i18n;
use termide_modal::ConflictModal;
use termide_ui::path_utils;
use termide_vfs::VfsPath;

/// Extract the file name from a path, defaulting to "file" if unavailable.
pub(in crate::app::modal) fn source_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string())
}

/// Create a copy or move OperationRequest based on operation type.
pub(in crate::app::modal) fn make_copy_or_move_request(
    source: OperationPath,
    dest: OperationPath,
    is_move: bool,
) -> OperationRequest {
    if is_move {
        OperationRequest::r#move(vec![source], dest)
    } else {
        OperationRequest::copy(vec![source], dest)
    }
}

/// Calculate total size of all sources (files and directories recursively).
fn scan_sources_total_bytes(sources: &[PathBuf]) -> u64 {
    fn dir_size(path: &Path) -> u64 {
        let mut total = 0u64;
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if meta.is_dir() {
                    total += dir_size(&entry.path());
                } else {
                    total += meta.len();
                }
            }
        }
        total
    }

    let mut total = 0u64;
    for source in sources {
        match std::fs::metadata(source) {
            Ok(meta) if meta.is_dir() => total += dir_size(source),
            Ok(meta) => total += meta.len(),
            Err(_) => {}
        }
    }
    total
}

/// Format VfsPath for display in progress modal
fn format_vfs_path_for_display(vfs_path: &VfsPath, file_path: &Path) -> String {
    if vfs_path.is_local() {
        file_path.display().to_string()
    } else {
        let mut result = String::new();

        // Add username@host
        if let Some(ref user) = vfs_path.username {
            result.push_str(user);
            result.push('@');
        }

        if let Some(ref host) = vfs_path.host {
            result.push_str(host);
        }

        // Add port if non-standard
        if let Some(port) = vfs_path.port {
            let default_port = vfs_path.default_port();
            if Some(port) != default_port {
                result.push(':');
                result.push_str(&port.to_string());
            }
        }

        // Add path (no colon separator)
        let full_path = vfs_path
            .path
            .join(file_path.file_name().unwrap_or_default());
        result.push_str(&full_path.display().to_string());

        result
    }
}

impl App {
    /// Start a sub-operation within a batch and store continuation state.
    ///
    /// On success, stores the operation ID and sets pending action for continuation.
    /// On failure, logs the error, increments error count, advances to next file,
    /// and still stores the pending action for continuation.
    pub(in crate::app::modal) fn start_batch_sub_operation(
        &mut self,
        request: termide_file_ops::OperationRequest,
        vfs_manager: std::sync::Arc<termide_vfs::VfsManager>,
        mut operation: BatchOperation,
    ) {
        // Extract tracking info before starting (request is consumed)
        let source_display = request
            .sources
            .first()
            .map(|s| s.display())
            .unwrap_or_default();
        let dest_display = request
            .destination
            .as_ref()
            .map(|d| d.display())
            .unwrap_or_default();
        let op_type = Self::tracking_op_type(&request);

        match self.state.start_operation_now(request, vfs_manager) {
            Ok(op_id) => {
                self.state.batch.sub_operation_id = Some(op_id);

                // If no batch tracking card exists, create one and open the panel.
                // Use start_batch_tracking() to get a synthetic ID so that
                // untrack_operation(real_id) on sub-op completion won't remove it.
                if self.state.batch.tracking_id.is_none() {
                    let batch_id = self.state.start_batch_tracking(
                        op_type,
                        source_display,
                        dest_display,
                        1,
                        0,
                    );
                    let _ = self.open_operations_panel_with_focus(batch_id);
                }

                self.state.pending_action =
                    Some(PendingAction::ContinueBatchOperation { operation });
            }
            Err(e) => {
                log::error!("Failed to start operation: {}", e);
                operation.increment_error();
                operation.advance();
                self.state.pending_action =
                    Some(PendingAction::ContinueBatchOperation { operation });
            }
        }
    }

    /// Map an OperationRequest to a tracking OperationType.
    fn tracking_op_type(
        request: &termide_file_ops::OperationRequest,
    ) -> crate::state::OperationType {
        use termide_file_ops::OperationType as FO;
        let is_remote_src = request
            .sources
            .first()
            .map(|s| s.is_remote())
            .unwrap_or(false);
        let is_remote_dst = request
            .destination
            .as_ref()
            .map(|d| d.is_remote())
            .unwrap_or(false);

        match request.op_type {
            FO::Copy | FO::Move if is_remote_src && !is_remote_dst => {
                if request.is_move {
                    crate::state::OperationType::MoveDownload
                } else {
                    crate::state::OperationType::CopyDownload
                }
            }
            FO::Copy | FO::Move if !is_remote_src && is_remote_dst => {
                if request.is_move {
                    crate::state::OperationType::MoveUpload
                } else {
                    crate::state::OperationType::CopyUpload
                }
            }
            FO::Copy | FO::Move if is_remote_src && is_remote_dst => {
                if request.is_move {
                    crate::state::OperationType::MoveUpload
                } else {
                    crate::state::OperationType::CopyUpload
                }
            }
            FO::Delete => crate::state::OperationType::Delete,
            _ => {
                if request.is_move {
                    crate::state::OperationType::Move
                } else {
                    crate::state::OperationType::Copy
                }
            }
        }
    }

    /// Build a VfsPath using connection info from another VfsPath but with a different path.
    pub(in crate::app::modal) fn vfs_path_with_connection(
        base: &VfsPath,
        path: PathBuf,
    ) -> VfsPath {
        VfsPath {
            protocol: base.protocol,
            host: base.host.clone(),
            port: base.port,
            username: base.username.clone(),
            path,
        }
    }

    /// Find a remote file manager panel (searches all panels, not just active).
    /// Returns (vfs_manager, vfs_current_path) if found.
    pub(in crate::app::modal) fn find_remote_file_manager_info(
        &self,
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
                        return Some((
                            fm.vfs_state().manager_arc(),
                            fm.vfs_state().current_path().clone(),
                        ));
                    }
                }
            }
        }
        None
    }

    /// Handle batch file operation (copy/move)
    pub(in crate::app) fn process_batch_operation(&mut self, mut operation: BatchOperation) {
        // Show progress modal for:
        // 1. Multi-file operations (total_count > 1), OR
        // 2. Single remote file operations (need network transfer feedback), OR
        // 3. Single directory operations (recursive copy/move can take time), OR
        // 4. Single file > 1MB (large file transfer needs progress feedback)

        // Check if this is a remote operation based on actual operation data:
        // - destination is a VFS URL (e.g., sftp://...), OR
        // - source doesn't exist locally (server-side path)
        let dest_str_check = operation.destination.to_string_lossy();
        let is_remote_dest_check = termide_vfs::is_vfs_url(&dest_str_check);
        let source_is_local = operation
            .current_source()
            .map(|p| p.exists())
            .unwrap_or(false);
        let is_remote_operation = is_remote_dest_check || !source_is_local;

        // Check if source is a directory or large file (>1MB)
        let needs_progress = operation
            .current_source()
            .and_then(|p| {
                p.metadata().ok().map(|meta| {
                    meta.is_dir() || meta.len() > 1_048_576 // 1MB
                })
            })
            .unwrap_or(false);

        // Start Operations-panel tracking when the operation begins.
        // Only once: at the first item, when tracking isn't already active.
        let should_start_tracking = operation.current_index == 0
            && (operation.total_count() > 1 || is_remote_operation || needs_progress)
            && self.state.batch.tracking_id.is_none();

        if should_start_tracking {
            use crate::state::OperationType;

            // Close any existing modal (e.g., destination selection) before showing progress
            self.state.close_modal();

            // Get source display for tracking
            let source_display = if is_remote_operation {
                // For remote operations, find the VFS path for nice display
                if let Some((_, vfs_path)) = self.find_remote_file_manager_info() {
                    if let Some(source_file) = operation.current_source() {
                        format_vfs_path_for_display(&vfs_path, source_file)
                    } else {
                        String::new()
                    }
                } else {
                    operation
                        .current_source()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                }
            } else {
                operation
                    .current_source()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            };

            let dest_display = operation.destination.display().to_string();

            let op_type = match (operation.operation_type, is_remote_operation) {
                (BatchOperationType::Copy, true) => OperationType::CopyDownload,
                (BatchOperationType::Move, true) => OperationType::MoveDownload,
                (BatchOperationType::Copy, false) => OperationType::Copy,
                (BatchOperationType::Move, false) => OperationType::Move,
            };

            // Pre-scan all sources to get total bytes for progress display
            let total_bytes = if !is_remote_operation {
                scan_sources_total_bytes(&operation.sources)
            } else {
                0 // Remote sources: byte progress comes from individual operations
            };

            // Start batch tracking in Operations panel
            let batch_id = self.state.start_batch_tracking(
                op_type,
                source_display,
                dest_display,
                operation.total_count(),
                total_bytes,
            );

            // Open Operations panel with focus on the new batch operation
            let _ = self.open_operations_panel_with_focus(batch_id);

            // Store operation as pending action to allow UI to render panel
            // before starting actual file operations
            self.state.pending_action = Some(PendingAction::ContinueBatchOperation { operation });
            return;
        }

        // Check if operation is paused - keep modal open and don't process next file
        if operation.pause_state == termide_state::PauseState::Paused {
            // Store operation and return - will resume when user unpauses
            self.state.pending_action = Some(PendingAction::ContinueBatchOperation { operation });
            return;
        }

        // Check if operation is complete
        if operation.is_complete() {
            // Finish batch tracking in Operations panel
            self.state.finish_batch_tracking();

            // Bell signal on completion
            self.state.bell();

            // Show final results
            self.show_batch_results(&operation);

            // Refresh ALL file manager panels after batch operation
            // (both source and destination might need refresh)
            if operation.success_count > 0 {
                // Get last successful filename for cursor positioning
                let last_filename = operation.last_successful_filename();
                let dest_path = operation.destination_path();

                for group in &mut self.layout_manager.panel_groups {
                    for panel in group.panels_mut() {
                        if let Some(fm) = panel.as_file_manager_mut() {
                            fm.clear_selection();

                            // Set cursor target BEFORE reload for destination panel
                            if fm.current_path() == dest_path {
                                if let Some(ref name) = last_filename {
                                    fm.set_newly_created(name.clone());
                                }
                            }

                            // Force reload by bypassing debounce to ensure file list updates
                            let _ = fm.force_reload_directory();
                        }
                    }
                }
            }
            return;
        }

        // Get current file
        let Some(source) = operation.current_source().cloned() else {
            return;
        };

        let item_name = path_utils::get_file_name_string(&source);

        let dest_str = operation.destination.to_string_lossy().into_owned();
        let is_remote_dest = termide_vfs::is_vfs_url(&dest_str);

        // Determine target path (considering rename pattern if set).
        // For remote URLs, PathBuf::is_dir() returns false, so standard
        // resolve functions don't work; compute the path component directly.
        let final_dest = if operation.rename_pattern.is_some() {
            // Apply rename pattern (get mutable counter first, then borrow pattern)
            let counter = operation.get_and_increment_rename_counter();
            let metadata = source.metadata().ok();
            let created = metadata.as_ref().and_then(|m| m.created().ok());
            let modified = metadata.as_ref().and_then(|m| m.modified().ok());

            // SAFETY: checked is_some() above; unwrap is safe
            let pattern = operation.rename_pattern.as_ref().unwrap();
            let new_name = pattern.apply(&item_name, counter, created, modified);

            if is_remote_dest {
                let base = termide_vfs::parse_vfs_url(&dest_str)
                    .map(|p| p.path)
                    .unwrap_or_else(|e| {
                        log::warn!("Failed to parse VFS URL: {}", e);
                        operation.destination.clone()
                    });
                base.join(&new_name)
            } else {
                path_utils::resolve_rename_destination_path(
                    &operation.destination,
                    &new_name,
                    operation.destination_is_directory(),
                )
            }
        } else if is_remote_dest {
            // Remote destination: parse URL path and join filename
            let base = termide_vfs::parse_vfs_url(&dest_str)
                .map(|p| p.path)
                .unwrap_or_else(|e| {
                    log::warn!("Failed to parse VFS URL: {}", e);
                    operation.destination.clone()
                });
            base.join(&item_name)
        } else {
            // Standard logic without renaming
            path_utils::resolve_batch_destination_path(
                &source,
                &operation.destination,
                operation.sources.len() == 1,
                operation.destination_is_directory(),
            )
        };

        // Update batch tracking file-level progress in Operations panel
        if self.state.batch.tracking_id.is_some() {
            self.state
                .update_batch_progress(operation.current_index, operation.total_count());
            self.state.needs_redraw = true;
        }

        // Check conflict
        if final_dest.exists() {
            match operation.conflict_mode {
                ConflictMode::Ask => {
                    // Show conflict resolution modal window
                    let remaining_items = operation
                        .sources
                        .len()
                        .saturating_sub(operation.current_index + 1);
                    let modal = ConflictModal::new(&source, &final_dest, remaining_items);
                    self.state.pending_action =
                        Some(PendingAction::ContinueBatchOperation { operation });
                    self.state.active_modal = Some(ActiveModal::Conflict(Box::new(modal)));
                    return;
                }
                ConflictMode::SkipAll => {
                    // Skip file
                    operation.increment_skipped();
                    operation.advance();
                    // Store and return to allow UI update
                    self.state.pending_action =
                        Some(PendingAction::ContinueBatchOperation { operation });
                    return;
                }
                ConflictMode::OverwriteAll => {
                    // Continue with overwrite (processing below)
                }
            }
        }

        // Execute operation - use remote path only when source or destination is actually remote.
        // source.exists() is false for server-side paths (they don't exist locally).
        let needs_remote = is_remote_dest || !source.exists();
        if needs_remote {
            if let Some((vfs_manager, vfs_current_path)) = self.find_remote_file_manager_info() {
                let source_name = source
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let vfs_source = vfs_current_path.join(&source_name);

                let is_move = operation.operation_type == BatchOperationType::Move;

                let request = if is_remote_dest {
                    let vfs_dest = Self::vfs_path_with_connection(&vfs_current_path, final_dest);
                    make_copy_or_move_request(
                        OperationPath::Remote(vfs_source.clone()),
                        OperationPath::Remote(vfs_dest),
                        is_move,
                    )
                } else {
                    let r = OperationRequest::download(vfs_source.clone(), final_dest);
                    if is_move {
                        self.state.batch.pending_delete = Some(PendingRemoteDelete {
                            vfs_source,
                            vfs_manager: vfs_manager.clone(),
                        });
                    }
                    r
                };

                self.start_batch_sub_operation(request, vfs_manager, operation);
                return;
            }
        }

        // Local file or directory - use OperationManager for async copy with progress
        // Applies to both Copy and Move (move may need copy+delete for cross-filesystem)
        if source.is_file() || source.is_dir() {
            use termide_file_ops::ConflictMode as FileOpsConflictMode;

            let is_move = operation.operation_type == BatchOperationType::Move;
            let worker_conflict_mode = match operation.conflict_mode {
                ConflictMode::OverwriteAll => FileOpsConflictMode::OverwriteAll,
                ConflictMode::SkipAll => FileOpsConflictMode::SkipAll,
                _ => FileOpsConflictMode::Ask,
            };
            let worker_dest = if source.is_dir() && final_dest.is_dir() {
                final_dest
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| final_dest.clone())
            } else {
                final_dest.clone()
            };
            let request = make_copy_or_move_request(
                OperationPath::Local(source.clone()),
                OperationPath::Local(worker_dest),
                is_move,
            )
            .with_conflict_mode(worker_conflict_mode);

            let vfs_manager = std::sync::Arc::new(termide_vfs::VfsManager::new());

            self.start_batch_sub_operation(request, vfs_manager, operation);
            return;
        }

        // Unknown source type (symlink?) - skip with error
        log::error!("Unsupported source type: {}", source.display());
        operation.increment_error();

        // Move to next file
        operation.advance();

        // Store operation and return to allow UI update between files
        // This enables:
        // 1. Progress bar to update visually
        // 2. User to pause/cancel between files
        // 3. Spinner animation to work
        self.state.pending_action = Some(PendingAction::ContinueBatchOperation { operation });
    }

    /// Show batch operation final results
    pub(in crate::app) fn show_batch_results(&mut self, operation: &BatchOperation) {
        let total = operation.total_count();
        let success = operation.success_count;
        let errors = operation.error_count;
        let skipped = operation.skipped_count;
        let t = i18n::t();

        let action_name = match operation.operation_type {
            BatchOperationType::Copy => (t.batch_result_file_copied(), t.batch_result_copied()),
            BatchOperationType::Move => (t.batch_result_file_moved(), t.batch_result_moved()),
        };

        if total == 1 {
            if success == 1 {
                // Capitalize the localized action word (e.g., "copied" → "Copied")
                let word = action_name.0;
                let capitalized: String = word
                    .chars()
                    .take(1)
                    .flat_map(|c| c.to_uppercase())
                    .chain(word.chars().skip(1))
                    .collect();
                self.state.set_info(capitalized);
            } else {
                let error_msg = match operation.operation_type {
                    BatchOperationType::Copy => t.batch_result_error_copy(),
                    BatchOperationType::Move => t.batch_result_error_move(),
                };
                self.show_error_modal(error_msg.to_string());
            }
        } else {
            let mut parts = vec![];
            if success > 0 {
                parts.push(format!("{}: {}", action_name.1, success));
            }
            if skipped > 0 {
                parts.push(t.batch_result_skipped_fmt(skipped));
            }
            if errors > 0 {
                parts.push(t.batch_result_errors_fmt(errors));
            }

            self.state.set_info(parts.join(", "));
        }
    }
}
