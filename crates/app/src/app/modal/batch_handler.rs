//! Batch file operation handling: shared helpers and the progress/tracking engine.

// Note: PanelExt is used for FileManager batch operations (copy/move/delete/rename).

use std::path::{Path, PathBuf};

use super::super::App;
use crate::state::{
    ActiveModal, BatchOperation, BatchOperationType, ConflictMode, PendingAction,
    PendingRemoteDelete, SourceLocation,
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

/// Whether the file name of `path` is not UTF-8. Such a name has no
/// faithful text form, so it is never sent to a server: written lossily,
/// two of them could land on one remote name.
pub(in crate::app::modal) fn name_is_not_utf8(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.to_str().is_none())
}

/// Whether a local batch source can be handed to the copy worker: a file,
/// a directory, or a symlink, which the worker copies as a link — even a
/// dangling one. Pipes, sockets and devices are not.
pub(in crate::app::modal) fn is_copyable_local_source(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() || m.is_dir() || m.is_symlink())
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

/// A batch item bound for a remote destination, as `remote_batch_target`
/// resolves it.
pub(in crate::app::modal) struct RemoteBatchTarget {
    /// Reaches the destination (and a remote source).
    pub(in crate::app::modal) manager: std::sync::Arc<termide_vfs::VfsManager>,
    pub(in crate::app::modal) source: OperationPath,
    /// The destination as typed, on the connection that reaches it.
    pub(in crate::app::modal) destination: VfsPath,
}

impl RemoteBatchTarget {
    /// The request that copies or moves the source onto `dest`: a local
    /// source is uploaded.
    pub(in crate::app::modal) fn request(&self, dest: VfsPath, is_move: bool) -> OperationRequest {
        match &self.source {
            OperationPath::Local(path) => {
                let mut request = OperationRequest::upload(path.clone(), dest);
                request.is_move = is_move;
                request
            }
            remote => {
                make_copy_or_move_request(remote.clone(), OperationPath::Remote(dest), is_move)
            }
        }
    }
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
            FO::Trash => crate::state::OperationType::Trash,
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
        base.with_path(path)
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
                if let Some(fm) = panel.downcast_ref::<termide_panel_file_manager::FileManager>() {
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

    /// The VFS side of a batch item read through the VFS: the manager, the
    /// connection base (also used for a remote destination) and the VFS path
    /// of `source`. A recorded location is authoritative; an unrecorded one
    /// falls back to whichever panel shows a remote location.
    pub(in crate::app::modal) fn remote_batch_source(
        &self,
        operation: &BatchOperation,
        source: &Path,
    ) -> Option<(std::sync::Arc<termide_vfs::VfsManager>, VfsPath, VfsPath)> {
        if let SourceLocation::Vfs { manager, directory } = &operation.source_location {
            // Sources are full paths on that location, so an entry from an
            // expanded subdirectory keeps its own directory.
            return Some((
                manager.clone(),
                directory.clone(),
                directory.with_path(source),
            ));
        }
        let (manager, directory) = self.find_remote_file_manager_info()?;
        let vfs_source = directory.join(source_name(source));
        Some((manager, directory, vfs_source))
    }

    /// The two ends of a batch item bound for a remote destination: a remote
    /// source keeps the connection it is read through, a local one is
    /// uploaded through the panel connected to the destination. None when no
    /// panel holds the connection.
    pub(in crate::app::modal) fn remote_batch_target(
        &self,
        operation: &BatchOperation,
        source: &Path,
    ) -> Option<RemoteBatchTarget> {
        let destination =
            termide_vfs::parse_vfs_url(&operation.destination.to_string_lossy()).ok()?;
        let (manager, connection, source) = if operation.source_is_remote(source) {
            let (manager, connection, vfs_source) = self.remote_batch_source(operation, source)?;
            (manager, connection, OperationPath::Remote(vfs_source))
        } else {
            let (manager, connection) = self.find_connected_vfs_manager(&destination)?;
            (
                manager,
                connection,
                OperationPath::Local(source.to_path_buf()),
            )
        };
        Some(RemoteBatchTarget {
            manager,
            source,
            destination: connection.with_path(destination.path),
        })
    }

    /// Where on the server the item named `name` lands, and whether something
    /// is there already, asked of the server. A renamed item goes into the
    /// destination directory under its new name.
    pub(in crate::app::modal) fn remote_item_destination(
        &self,
        target: &RemoteBatchTarget,
        operation: &BatchOperation,
        name: &str,
        renamed: bool,
    ) -> (VfsPath, bool) {
        if renamed {
            let dest = target.destination.join(name);
            let exists = target.manager.exists(&dest).recv().unwrap_or(false);
            return (dest, exists);
        }
        self.resolve_remote_dest(
            &target.destination,
            name,
            &operation.destination.to_string_lossy(),
            operation.sources.len() > 1,
            &target.manager,
        )
    }

    /// Copy or move the current item to a remote destination, asking first
    /// when the server already has something under its name.
    pub(in crate::app::modal) fn process_remote_batch_item(
        &mut self,
        mut operation: BatchOperation,
        source: PathBuf,
        name: &str,
        renamed: bool,
    ) {
        let Some(target) = self.remote_batch_target(&operation, &source) else {
            log::error!(
                "No connection to the remote destination of {}",
                source.display()
            );
            operation.increment_error();
            operation.advance();
            self.state.pending_action = Some(PendingAction::ContinueBatchOperation { operation });
            return;
        };
        let (dest, exists) = self.remote_item_destination(&target, &operation, name, renamed);
        if exists {
            let shown = PathBuf::from(dest.to_url_string());
            match self.settle_batch_conflict(operation, &source, &shown) {
                Some(proceed) => operation = proceed,
                None => return,
            }
        }
        let is_move = operation.operation_type == BatchOperationType::Move;
        let request = target.request(dest, is_move);
        self.start_batch_sub_operation(request, target.manager, operation);
    }

    /// A batch item whose destination is taken: asks, or skips, as the
    /// batch's conflict mode says, and hands the batch back only when the
    /// item is to be overwritten.
    fn settle_batch_conflict(
        &mut self,
        mut operation: BatchOperation,
        source: &Path,
        dest: &Path,
    ) -> Option<BatchOperation> {
        match operation.conflict_mode {
            ConflictMode::Ask => {
                let remaining_items = operation
                    .sources
                    .len()
                    .saturating_sub(operation.current_index + 1);
                let modal = ConflictModal::new(source, dest, remaining_items);
                self.state.pending_action =
                    Some(PendingAction::ContinueBatchOperation { operation });
                self.state.active_modal = Some(ActiveModal::Conflict(Box::new(modal)));
                None
            }
            ConflictMode::SkipAll => {
                operation.increment_skipped();
                operation.advance();
                // Store and return to allow UI update
                self.state.pending_action =
                    Some(PendingAction::ContinueBatchOperation { operation });
                None
            }
            ConflictMode::OverwriteAll => Some(operation),
        }
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
            .map(|p| !operation.source_is_remote(p))
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

            // Start batch tracking in Operations panel; the size of local
            // sources is added up off the UI thread, a remote source's comes
            // from its own operations.
            let batch_id = self.state.start_batch_tracking(
                op_type,
                source_display,
                dest_display,
                operation.total_count(),
                0,
            );
            if !is_remote_operation {
                self.state.batch.size_scan = Some(crate::state::SizeScan::spawn(
                    batch_id,
                    operation.sources.clone(),
                ));
            }

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

        // The name as text, for a rename pattern and a remote destination.
        // A local copy without a pattern keeps the name as it is, so only a
        // remote destination refuses a name that is not UTF-8.
        let item_name = source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        let dest_str = operation.destination.to_string_lossy().into_owned();
        let is_remote_dest = termide_vfs::is_vfs_url(&dest_str);

        if is_remote_dest && name_is_not_utf8(&source) {
            log::error!(
                "Not uploading {}: its name is not valid UTF-8",
                source.display()
            );
            self.state
                .set_error(i18n::t().op_name_not_utf8_fmt(&source.display().to_string()));
            operation.increment_error();
            operation.advance();
            self.state.pending_action = Some(PendingAction::ContinueBatchOperation { operation });
            return;
        }

        // Apply the rename pattern, if set (counter first, then the pattern).
        let new_name = if operation.rename_pattern.is_some() {
            let counter = operation.get_and_increment_rename_counter();
            let metadata = source.metadata().ok();
            let created = metadata.as_ref().and_then(|m| m.created().ok());
            let modified = metadata.as_ref().and_then(|m| m.modified().ok());
            operation
                .rename_pattern
                .as_ref()
                .map(|pattern| pattern.apply(&item_name, counter, created, modified))
        } else {
            None
        };

        // Update batch tracking file-level progress in Operations panel
        if self.state.batch.tracking_id.is_some() {
            self.state
                .update_batch_progress(operation.current_index, operation.total_count());
            self.state.needs_redraw = true;
        }

        // A remote destination is asked of the server, not of the local disk.
        if is_remote_dest {
            let renamed = new_name.is_some();
            let name = new_name.unwrap_or(item_name);
            self.process_remote_batch_item(operation, source, &name, renamed);
            return;
        }

        let final_dest = match &new_name {
            Some(new_name) => path_utils::resolve_rename_destination_path(
                &operation.destination,
                new_name,
                operation.destination_is_directory(),
            ),
            None => path_utils::resolve_batch_destination_path(
                &source,
                &operation.destination,
                operation.sources.len() == 1,
                operation.destination_is_directory(),
            ),
        };

        // An entry pasted onto itself is no conflict: the worker gives a copy
        // a fresh name and leaves a move where it is.
        let onto_itself = !operation.source_is_remote(&source)
            && termide_file_ops::destination_overlap(&source, &final_dest)
                == termide_file_ops::DestinationOverlap::SameAsSource;

        // A dangling symlink at the destination is a conflict too.
        if std::fs::symlink_metadata(&final_dest).is_ok() && !onto_itself {
            match self.settle_batch_conflict(operation, &source, &final_dest) {
                Some(proceed) => operation = proceed,
                None => return,
            }
        }

        // A remote source is downloaded to the local destination.
        if operation.source_is_remote(&source) {
            if let Some((vfs_manager, _, vfs_source)) =
                self.remote_batch_source(&operation, &source)
            {
                operation.note_item_destination(&final_dest);
                let request = OperationRequest::download(vfs_source.clone(), final_dest);
                if operation.operation_type == BatchOperationType::Move {
                    self.state.batch.pending_delete = Some(PendingRemoteDelete {
                        vfs_source,
                        vfs_manager: vfs_manager.clone(),
                    });
                }
                self.start_batch_sub_operation(request, vfs_manager, operation);
                return;
            }
        }

        // Local file, directory or symlink (copied as a link, dangling or
        // not) - use OperationManager for async copy with progress.
        // Applies to both Copy and Move (move may need copy+delete for cross-filesystem)
        if is_copyable_local_source(&source) {
            use termide_file_ops::ConflictMode as FileOpsConflictMode;

            let is_move = operation.operation_type == BatchOperationType::Move;
            let worker_conflict_mode = match operation.conflict_mode {
                ConflictMode::OverwriteAll => FileOpsConflictMode::OverwriteAll,
                ConflictMode::SkipAll => FileOpsConflictMode::SkipAll,
                _ => FileOpsConflictMode::Ask,
            };
            operation.note_item_destination(&final_dest);
            let worker_dest = if source.is_dir() && final_dest.is_dir() {
                final_dest
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| final_dest.clone())
            } else {
                final_dest
            };
            let request = make_copy_or_move_request(
                OperationPath::Local(source),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A name that is not UTF-8 is refused for a server rather than sent
    /// lossily, where two such names would become one "?"-like name.
    #[cfg(unix)]
    #[test]
    fn a_name_that_is_not_utf8_is_spotted() {
        use std::os::unix::ffi::OsStrExt;
        let bad = Path::new("/tmp").join(std::ffi::OsStr::from_bytes(b"caf\xe9.txt"));
        assert!(name_is_not_utf8(&bad));
        assert!(!name_is_not_utf8(Path::new("/tmp/café.txt")));
        assert!(!name_is_not_utf8(Path::new("/")));
    }
}
