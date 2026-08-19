//! Operation manager event handler.
//!
//! Handles events from the unified operation manager for file operations.

use termide_file_ops::{OperationEvent, OperationPath, OperationPhase, OperationResult};
use termide_panel_editor::{Editor, FileState};

use crate::state::{ActiveModal, PendingAction};
use crate::PanelExt;

use super::App;

impl App {
    /// Poll the unified operation manager for events (new file-ops system).
    /// This handles events from the centralized operation manager which will
    /// eventually replace the individual operation handles.
    pub(super) fn poll_operation_manager(&mut self) {
        let events = self.state.poll_operations();
        let mut should_refresh_file_managers = false;

        for event in events {
            match event {
                OperationEvent::Started(id) => {
                    let _ = id;
                    self.state.needs_redraw = true;
                }
                OperationEvent::Progress(id, progress) => {
                    // Resolve to batch tracking ID if this is a sub-operation
                    let tracking_id = if self.state.batch.sub_operation_id == Some(id) {
                        self.state.batch.tracking_id.unwrap_or(id)
                    } else {
                        id
                    };

                    // Update active operation progress for operations panel
                    let is_batch = self.state.batch.sub_operation_id == Some(id)
                        && self.state.batch.tracking_id.is_some();
                    if let Some(op) = self.state.active_operations.get_mut(&tracking_id) {
                        if is_batch {
                            // For batch: add offset from previously completed sub-ops
                            // so bytes don't reset to 0 when a new sub-op starts.
                            op.progress.bytes_transferred =
                                op.batch_bytes_offset + progress.bytes_transferred;
                            // Track current sub-op's total for offset shift on completion
                            op.batch_current_file_total = progress.total_bytes;
                            // Update total_bytes only if the accumulated value is larger
                            let accumulated_total = op.batch_bytes_offset + progress.total_bytes;
                            if accumulated_total > op.progress.total_bytes {
                                op.progress.total_bytes = accumulated_total;
                            }
                            // Update file counts only when the sub-op reports larger
                            // values (e.g., folder scanning discovered many files).
                            if progress.total_files > op.progress.total_files {
                                op.progress.total_files = progress.total_files;
                            }
                            if progress.files_completed > op.progress.files_completed {
                                op.progress.files_completed = progress.files_completed;
                            }
                        } else {
                            op.progress.bytes_transferred = progress.bytes_transferred;
                            op.progress.total_bytes = progress.total_bytes;
                            op.progress.files_completed = progress.files_completed;
                            if progress.total_files > 0 {
                                op.progress.total_files = progress.total_files;
                            }
                        }
                        // Use accumulated bytes for speed tracking so it doesn't
                        // reset between batch sub-operations.
                        op.speed_tracker.update(op.progress.bytes_transferred);
                        op.is_scanning = matches!(progress.phase, OperationPhase::Scanning);

                        // Update op_type for cross-protocol (remote→remote) transfers
                        // based on current phase reported by CrossProtocolWorker
                        if let Some(ref item) = progress.current_item {
                            let is_move = matches!(
                                op.op_type,
                                crate::state::OperationType::MoveUpload
                                    | crate::state::OperationType::MoveDownload
                            );
                            if item.starts_with("Downloading") {
                                op.op_type = if is_move {
                                    crate::state::OperationType::MoveDownload
                                } else {
                                    crate::state::OperationType::CopyDownload
                                };
                            } else if item.starts_with("Uploading") {
                                op.op_type = if is_move {
                                    crate::state::OperationType::MoveUpload
                                } else {
                                    crate::state::OperationType::CopyUpload
                                };
                            }
                        }
                    }

                    self.state.needs_redraw = true;
                }
                OperationEvent::Completed(id, result) => {
                    // Check if this operation is part of a BatchOperation
                    let has_batch = matches!(
                        self.state.pending_action,
                        Some(PendingAction::ContinueBatchOperation { .. })
                    );

                    match result {
                        OperationResult::Success | OperationResult::SuccessWithPath(_) => {
                            should_refresh_file_managers = true;

                            // Handle remote delete for move operations (delete source after download)
                            if let Some(pending_delete) = self.state.batch.pending_delete.take() {
                                // Start async delete operation (fire and forget)
                                let delete_op = pending_delete
                                    .vfs_manager
                                    .delete(&pending_delete.vfs_source);
                                std::thread::spawn(move || {
                                    if let Err(e) = delete_op.recv() {
                                        log::error!(
                                            "Failed to delete remote source after move: {}",
                                            e
                                        );
                                    }
                                });
                            }

                            // Handle pending editor download (open editor after remote file download)
                            if let Some(pending_download) =
                                self.state.pending_editor_download.take()
                            {
                                if pending_download.operation_id == id {
                                    let (size, mtime) =
                                        match std::fs::metadata(&pending_download.temp_path) {
                                            Ok(meta) => (meta.len(), meta.modified().ok()),
                                            Err(_) => (0, None),
                                        };

                                    match Editor::open_file_with_config(
                                        pending_download.temp_path.clone(),
                                        pending_download.config,
                                    ) {
                                        Ok(mut editor) => {
                                            editor.set_file_state(FileState::from_remote(
                                                pending_download.remote_path.clone(),
                                                pending_download.temp_path,
                                                mtime,
                                                size,
                                            ));
                                            editor.set_vfs_manager(pending_download.vfs_manager);

                                            if let Some(lsp) = &mut self.state.lsp_manager {
                                                editor.init_lsp(lsp);
                                            }

                                            self.add_panel(Box::new(editor));
                                            self.auto_save_session();

                                            let filename = pending_download
                                                .remote_path
                                                .file_name()
                                                .and_then(|n| n.to_str())
                                                .unwrap_or("remote file");
                                            self.state
                                                .set_info(format!("File {} opened", filename));
                                        }
                                        Err(e) => {
                                            log::error!("Failed to open downloaded file: {}", e);
                                            self.show_error_modal(format!(
                                                "Failed to open downloaded file: {}",
                                                e
                                            ));
                                            if let Err(e) =
                                                std::fs::remove_file(&pending_download.temp_path)
                                            {
                                                log::warn!(
                                                    "Failed to remove temp file {}: {}",
                                                    pending_download.temp_path.display(),
                                                    e
                                                );
                                            }
                                        }
                                    }
                                } else {
                                    self.state.pending_editor_download = Some(pending_download);
                                }
                            }

                            // Handle batch upload continuation
                            if let Some(mut batch_upload) = self.state.batch.pending_upload.take() {
                                // Delete local source if this was a move operation
                                if batch_upload.is_move {
                                    if let Err(e) =
                                        std::fs::remove_file(&batch_upload.current_source)
                                    {
                                        log::warn!("Failed to delete source after move: {}", e);
                                    }
                                }

                                // Check if there are more files to upload
                                batch_upload.current_index += 1;
                                if batch_upload.current_index < batch_upload.all_sources.len() {
                                    // Start next file upload
                                    let next_source = batch_upload.all_sources
                                        [batch_upload.current_index]
                                        .clone();
                                    let source_name = next_source
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| "file".to_string());

                                    // Parse remote base path and join with filename
                                    if let Ok(remote_base) =
                                        termide_vfs::parse_vfs_url(&batch_upload.dest_base_url)
                                    {
                                        let final_remote = remote_base.join(&source_name);
                                        // Keep an exact copy of the next-file destination so
                                        // the cancel-cleanup modal targets the right partial.
                                        let next_remote_target = final_remote.clone();

                                        // Create upload request for next file
                                        let request = termide_file_ops::OperationRequest::upload(
                                            next_source.clone(),
                                            final_remote,
                                        );

                                        // Update batch state
                                        batch_upload.current_source = next_source;
                                        batch_upload.current_remote_target =
                                            Some(next_remote_target);

                                        // Start upload for next file
                                        match self.state.start_operation_now(
                                            request,
                                            batch_upload.vfs_manager.clone(),
                                        ) {
                                            Ok(_) => {
                                                // Put back for next tick
                                                self.state.batch.pending_upload =
                                                    Some(batch_upload);
                                            }
                                            Err(e) => {
                                                log::error!("Failed to start next upload: {}", e);
                                                self.state.close_modal();
                                                self.show_error_modal(format!(
                                                    "Upload failed: {}",
                                                    e
                                                ));
                                            }
                                        }
                                    } else {
                                        // Failed to parse URL - abort
                                        self.state.close_modal();
                                        self.show_error_modal(
                                            "Failed to parse remote URL".to_string(),
                                        );
                                    }
                                } else {
                                    // All files uploaded!
                                    self.state.close_modal();
                                    let total = batch_upload.all_sources.len();
                                    if total == 1 {
                                        self.state.set_info("File uploaded".to_string());
                                    } else {
                                        self.state.set_info(format!("{} files uploaded", total));
                                    }
                                }
                            }

                            // Continue batch operation if pending
                            if has_batch {
                                // Accumulate bytes from completed sub-op into offset
                                if let Some(batch_id) = self.state.batch.tracking_id {
                                    if let Some(op) =
                                        self.state.active_operations.get_mut(&batch_id)
                                    {
                                        op.batch_bytes_offset = op.progress.bytes_transferred;
                                    }
                                }

                                if let Some(PendingAction::ContinueBatchOperation {
                                    mut operation,
                                }) = self.state.pending_action.take()
                                {
                                    // Track completed destination for cancel cleanup
                                    if let Some(source) = operation.current_source() {
                                        let filename =
                                            source.file_name().unwrap_or_default().to_os_string();
                                        let dest_path = operation.destination.join(filename);
                                        operation.add_completed_destination(dest_path);
                                    }
                                    operation.increment_success();
                                    operation.advance();
                                    // Update file count immediately so the card reflects completion
                                    self.state.update_batch_progress(
                                        operation.current_index,
                                        operation.total_count(),
                                    );
                                    self.process_batch_operation(operation);
                                }
                            }

                            // Skip file manager refresh for editor uploads (file already existed)
                            if self.state.skip_refresh_after_upload {
                                self.state.skip_refresh_after_upload = false;
                                should_refresh_file_managers = false;
                                self.clear_any_editor_uploading_flag();
                            }

                            // Handle close editor after upload (for "save and close" flow)
                            if let Some(editor_path) = self.state.close_editor_after_upload.take() {
                                self.close_editor_by_path(&editor_path);
                            }

                            // Untrack completed operation from operations panel
                            self.state.untrack_operation(id);
                        }
                        OperationResult::PartialSuccess {
                            completed,
                            skipped,
                            failed,
                            ..
                        } => {
                            should_refresh_file_managers = true;

                            // Continue batch operation if pending
                            if has_batch {
                                // Accumulate bytes from completed sub-op into offset
                                if let Some(batch_id) = self.state.batch.tracking_id {
                                    if let Some(op) =
                                        self.state.active_operations.get_mut(&batch_id)
                                    {
                                        op.batch_bytes_offset = op.progress.bytes_transferred;
                                    }
                                }

                                if let Some(PendingAction::ContinueBatchOperation {
                                    mut operation,
                                }) = self.state.pending_action.take()
                                {
                                    // Track completed destination for cancel cleanup
                                    if completed > 0 {
                                        if let Some(source) = operation.current_source() {
                                            let filename = source
                                                .file_name()
                                                .unwrap_or_default()
                                                .to_os_string();
                                            let dest_path = operation.destination.join(filename);
                                            operation.add_completed_destination(dest_path);
                                        }
                                    }
                                    // Add completed count to batch
                                    for _ in 0..completed {
                                        operation.increment_success();
                                    }
                                    for _ in 0..skipped {
                                        operation.increment_skipped();
                                    }
                                    for _ in 0..failed {
                                        operation.increment_error();
                                    }
                                    operation.advance();
                                    self.state.update_batch_progress(
                                        operation.current_index,
                                        operation.total_count(),
                                    );
                                    self.process_batch_operation(operation);
                                }
                            } else if skipped > 0 || failed > 0 {
                                self.state.set_info(format!(
                                    "Operation completed: {} done, {} skipped, {} failed",
                                    completed, skipped, failed
                                ));
                            }

                            // Untrack partially completed operation from operations panel
                            self.state.untrack_operation(id);
                        }
                        OperationResult::Failed(err) => {
                            log::error!("Operation {} failed: {}", id, err);

                            // Clear pending remote delete (don't delete source if download failed)
                            self.state.batch.pending_delete = None;

                            // Clear pending editor download on failure
                            if let Some(pending) = self.state.pending_editor_download.take() {
                                if pending.operation_id == id {
                                    if let Err(e) = std::fs::remove_file(&pending.temp_path) {
                                        log::warn!(
                                            "Failed to remove temp file {}: {}",
                                            pending.temp_path.display(),
                                            e
                                        );
                                    }
                                } else {
                                    self.state.pending_editor_download = Some(pending);
                                }
                            }

                            // Clear editor upload flags on failure
                            if self.state.skip_refresh_after_upload {
                                self.state.skip_refresh_after_upload = false;
                                self.clear_any_editor_uploading_flag();
                            }
                            if let Some(editor_path) = self.state.close_editor_after_upload.take() {
                                self.clear_editor_uploading_flag(&editor_path);
                            }

                            // Clear pending batch upload (don't continue if upload failed)
                            if self.state.batch.pending_upload.take().is_some() {
                                self.state.close_modal();
                            }

                            // Continue batch operation if pending
                            if has_batch {
                                // Accumulate bytes from completed sub-op into offset
                                if let Some(batch_id) = self.state.batch.tracking_id {
                                    if let Some(op) =
                                        self.state.active_operations.get_mut(&batch_id)
                                    {
                                        op.batch_bytes_offset = op.progress.bytes_transferred;
                                    }
                                }

                                if let Some(PendingAction::ContinueBatchOperation {
                                    mut operation,
                                }) = self.state.pending_action.take()
                                {
                                    operation.increment_error();
                                    operation.advance();
                                    self.state.update_batch_progress(
                                        operation.current_index,
                                        operation.total_count(),
                                    );
                                    self.process_batch_operation(operation);
                                }
                            } else {
                                self.show_error_modal(format!("Operation failed: {}", err));
                            }

                            // Untrack failed operation from operations panel
                            self.state.untrack_operation(id);
                        }
                        OperationResult::Cancelled => {
                            // Cancel may still have left bytes on the
                            // server / disk; the remote / local listing
                            // is out of sync until we reload.
                            should_refresh_file_managers = true;

                            // Clear pending remote delete (don't delete source if download cancelled)
                            self.state.batch.pending_delete = None;

                            // Clear pending editor download on cancel
                            if let Some(pending) = self.state.pending_editor_download.take() {
                                if pending.operation_id == id {
                                    if let Err(e) = std::fs::remove_file(&pending.temp_path) {
                                        log::warn!(
                                            "Failed to remove temp file {}: {}",
                                            pending.temp_path.display(),
                                            e
                                        );
                                    }
                                } else {
                                    self.state.pending_editor_download = Some(pending);
                                }
                            }

                            // Clear editor upload flags on cancel
                            if self.state.skip_refresh_after_upload {
                                self.state.skip_refresh_after_upload = false;
                                self.clear_any_editor_uploading_flag();
                            }
                            if let Some(editor_path) = self.state.close_editor_after_upload.take() {
                                self.clear_editor_uploading_flag(&editor_path);
                            }

                            // Clear pending batch upload (don't continue if upload cancelled).
                            // The exact remote target of the file that was in
                            // flight at cancel time is stored on the pending
                            // upload — for multi-file batches this is the
                            // *only* partial file; files already finished in
                            // the batch are intact on the server and stay
                            // untouched.
                            let partial_remote =
                                self.state.batch.pending_upload.take().and_then(|pending| {
                                    let path = pending.current_remote_target.clone()?;
                                    let filename = path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .or_else(|| {
                                            pending
                                                .current_source
                                                .file_name()
                                                .map(|n| n.to_string_lossy().into_owned())
                                        })?;
                                    Some((path, pending.vfs_manager, filename))
                                });
                            if partial_remote.is_some() {
                                self.state.close_modal();
                            }

                            // For batch operations, show cleanup modal
                            if has_batch {
                                // Remove batch tracking card from operations panel
                                self.state.finish_batch_tracking();

                                if let Some(PendingAction::ContinueBatchOperation { operation }) =
                                    self.state.pending_action.take()
                                {
                                    // Show cleanup modal similar to check_local_copy_progress
                                    let all_dest_paths = operation.completed_destinations.clone();
                                    let buttons = if all_dest_paths.is_empty() {
                                        vec!["OK".to_string()]
                                    } else {
                                        vec!["Delete copied".to_string(), "Keep copied".to_string()]
                                    };
                                    let modal = termide_modal::ChoiceModal::buttons_only(
                                        "Operation Cancelled",
                                        buttons,
                                    );
                                    self.state.active_modal =
                                        Some(ActiveModal::Choice(Box::new(modal)));
                                    self.state.pending_action =
                                        Some(PendingAction::CancelCopyCleanup {
                                            partial_path: std::path::PathBuf::new(),
                                            all_dest_paths,
                                            is_directory: false,
                                            batch_operation: Some(Box::new(operation)),
                                        });
                                }
                            } else if let Some((partial_path, vfs_manager, filename)) =
                                partial_remote
                            {
                                // Remote upload was cancelled mid-flight. Only the
                                // currently-in-flight file is partial; the modal
                                // names it explicitly so the user knows what they
                                // are agreeing to delete.
                                let modal = termide_modal::ConfirmModal::new(
                                    "Upload was cancelled",
                                    // Leading blank line creates a one-row
                                    // gap between the title bar and the
                                    // question itself.
                                    format!("\nDelete partial upload '{filename}'?"),
                                );
                                self.state.set_pending_action(
                                    PendingAction::CleanupPartialRemote {
                                        path: partial_path,
                                        vfs_manager,
                                    },
                                    ActiveModal::Confirm(Box::new(modal)),
                                );
                            } else {
                                self.state.set_info("Operation cancelled".to_string());
                            }

                            // Untrack cancelled operation from operations panel
                            self.state.untrack_operation(id);
                        }
                    }
                }
                OperationEvent::Paused(_id) => {
                    // Redraw so the Operations panel reflects the paused state.
                    self.state.needs_redraw = true;
                }
                OperationEvent::Resumed(_id) => {
                    // Redraw so the Operations panel reflects the resumed state.
                    self.state.needs_redraw = true;
                }
                OperationEvent::ConflictDetected(id, conflict_info) => {
                    // Convert OperationPath to PathBuf for ConflictModal
                    let source_path = match &conflict_info.source {
                        OperationPath::Local(p) => p.clone(),
                        OperationPath::Remote(vfs_path) => vfs_path.path.clone(),
                    };
                    let dest_path = match &conflict_info.destination {
                        OperationPath::Local(p) => p.clone(),
                        OperationPath::Remote(vfs_path) => vfs_path.path.clone(),
                    };

                    // Show ConflictModal
                    let modal = termide_modal::ConflictModal::new(
                        &source_path,
                        &dest_path,
                        conflict_info.remaining_items,
                    );
                    self.state.set_pending_action(
                        PendingAction::ResolveOperationConflict { operation_id: id },
                        ActiveModal::Conflict(Box::new(modal)),
                    );
                    self.state.needs_redraw = true;
                }
            }
        }

        // Refresh file managers after successful operations
        if should_refresh_file_managers {
            for panel in self.layout_manager.iter_all_panels_mut() {
                if let Some(fm) = panel.as_file_manager_mut() {
                    fm.clear_selection();
                    let _ = fm.load_directory();
                }
            }
        }
    }
}
