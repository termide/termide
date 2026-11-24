//! Operation manager event handler.
//!
//! Handles events from the unified operation manager for file operations.

use termide_file_ops::{OperationEvent, OperationPath, OperationPhase, OperationResult};
use termide_panel_editor::{Editor, FileState};

use crate::state::{ActiveModal, PendingAction};
use crate::PanelExt;

use super::App;

impl App {
    /// Report the sources of moves from a remote that could not be deleted
    /// there: the move then only copied, and the user must know it.
    pub(super) fn check_remote_move_deletes(&mut self) {
        if self.state.remote_move_deletes.is_empty() {
            return;
        }
        let mut failed = Vec::new();
        self.state
            .remote_move_deletes
            .retain(|(source, delete)| match delete.poll() {
                None => true,
                Some(Ok(())) => false,
                Some(Err(error)) => {
                    log::error!("Failed to delete remote source {source} after move: {error}");
                    failed.push(format!("{source}: {error}"));
                    false
                }
            });
        for error in failed {
            self.state.needs_redraw = true;
            self.show_error_modal(termide_i18n::t().op_move_source_not_deleted_fmt(&error));
        }
    }

    /// Give a batch the size of its local sources once the walk adding it
    /// up is done; its sub-operations' own totals only ever raise it.
    pub(super) fn check_batch_size_scan(&mut self) {
        let Some(scan) = &self.state.batch.size_scan else {
            return;
        };
        let Some(total) = scan.poll() else {
            return;
        };
        let batch_id = scan.batch_id;
        self.state.batch.size_scan = None;
        if let Some(op) = self.state.active_operations.get_mut(&batch_id) {
            if total > op.progress.total_bytes {
                op.progress.total_bytes = total;
                self.state.needs_redraw = true;
            }
        }
    }

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
                    // A project's own: handled whole, ahead of the batch
                    // bookkeeping below, which would take it for one of its.
                    if self.complete_project_transfer(id, &result) {
                        should_refresh_file_managers = true;
                        self.state.needs_redraw = true;
                        continue;
                    }
                    // What waits on an operation is settled by that
                    // operation alone, never by another one ending first:
                    // a batch by its current step, a batch upload by its
                    // current file, an editor's save by its upload.
                    let is_batch_step = self.state.batch.sub_operation_id == Some(id);
                    let has_batch = is_batch_step
                        && matches!(
                            self.state.pending_action,
                            Some(PendingAction::ContinueBatchOperation { .. })
                        );
                    let is_upload_step = self
                        .state
                        .batch
                        .pending_upload
                        .as_ref()
                        .is_some_and(|upload| upload.operation_id == Some(id));
                    let editor_upload = self
                        .state
                        .editor_uploads
                        .iter()
                        .position(|upload| upload.operation_id == id)
                        .map(|i| self.state.editor_uploads.remove(i));

                    match result {
                        OperationResult::Success | OperationResult::SuccessWithPath(_) => {
                            // An editor's save overwrote a file already listed.
                            if editor_upload.is_none() {
                                should_refresh_file_managers = true;
                            }

                            // Handle remote delete for move operations (delete source after download)
                            if let Some(pending_delete) = is_batch_step
                                .then(|| self.state.batch.pending_delete.take())
                                .flatten()
                            {
                                // Delete the source in the background;
                                // `check_remote_move_deletes` reports one
                                // that stays.
                                let delete_op = pending_delete
                                    .vfs_manager
                                    .delete(&pending_delete.vfs_source);
                                self.state
                                    .remote_move_deletes
                                    .push((pending_delete.vfs_source, delete_op));
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
                                                pending_download.remote_mtime,
                                            ));
                                            editor.set_vfs_manager(pending_download.vfs_manager);

                                            if let Some(lsp) = &mut self.state.lsp_manager {
                                                editor.init_lsp(lsp);
                                            }

                                            self.add_panel(Box::new(editor));
                                            self.auto_save_layout();

                                            let filename = pending_download
                                                .remote_path
                                                .file_name()
                                                .map(|n| n.to_string_lossy().into_owned())
                                                .unwrap_or_else(|| {
                                                    pending_download.remote_path.to_string()
                                                });
                                            self.state.set_info(
                                                termide_i18n::t().op_file_opened_fmt(&filename),
                                            );
                                        }
                                        Err(e) => {
                                            log::error!("Failed to open downloaded file: {}", e);
                                            self.show_error_modal(
                                                termide_i18n::t()
                                                    .binary_open_failed_fmt(&e.to_string()),
                                            );
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
                            if let Some(mut batch_upload) = is_upload_step
                                .then(|| self.state.batch.pending_upload.take())
                                .flatten()
                            {
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
                                        // On the connection the first file went
                                        // through: the URL may leave out the user
                                        // or port the panel resolved.
                                        let final_remote = match &batch_upload.current_remote_target
                                        {
                                            Some(connection) => connection
                                                .with_path(remote_base.path.join(&source_name)),
                                            None => remote_base.join(&source_name),
                                        };

                                        // Something already there is asked about,
                                        // as for the first file: the rest of the
                                        // batch goes on as a batch operation.
                                        if batch_upload
                                            .vfs_manager
                                            .exists(&final_remote)
                                            .recv()
                                            .unwrap_or(false)
                                        {
                                            self.ask_about_upload_conflict(
                                                batch_upload,
                                                next_source,
                                                &final_remote,
                                            );
                                        } else {
                                            // Keep an exact copy of the next-file destination so
                                            // the cancel-cleanup modal targets the right partial.
                                            let next_remote_target = final_remote.clone();

                                            // Create upload request for next file
                                            let request =
                                                termide_file_ops::OperationRequest::upload(
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
                                                Ok(next_id) => {
                                                    // Put back for next tick
                                                    batch_upload.operation_id = Some(next_id);
                                                    self.state.batch.pending_upload =
                                                        Some(batch_upload);
                                                }
                                                Err(e) => {
                                                    log::error!(
                                                        "Failed to start next upload: {}",
                                                        e
                                                    );
                                                    self.state.close_modal();
                                                    self.show_error_modal(
                                                        termide_i18n::t()
                                                            .op_upload_failed_fmt(&e.to_string()),
                                                    );
                                                }
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
                                    let t = termide_i18n::t();
                                    if total == 1 {
                                        self.state.set_info(t.op_file_uploaded().to_string());
                                    } else {
                                        self.state.set_info(t.op_files_uploaded_fmt(total));
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
                                    // Track what this copy created for cancel cleanup
                                    operation.record_item_destination();
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

                            if let Some(upload) = editor_upload {
                                self.clear_editor_uploading_flag(&upload.editor_path);
                                // "Save and close"
                                if upload.close_after {
                                    self.close_editor_by_path(&upload.editor_path);
                                } else {
                                    self.refresh_editor_remote_mtime(&upload.editor_path);
                                }
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
                                    // Track what this copy created for cancel cleanup
                                    if completed > 0 {
                                        operation.record_item_destination();
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
                                self.state.set_info(
                                    termide_i18n::t().op_completed_fmt(completed, skipped, failed),
                                );
                            }

                            // Untrack partially completed operation from operations panel
                            self.state.untrack_operation(id);
                        }
                        OperationResult::Failed(err) => {
                            log::error!("Operation {} failed: {}", id, err);

                            // Clear pending remote delete (don't delete source if download failed)
                            if is_batch_step {
                                self.state.batch.pending_delete = None;
                            }

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

                            // The edits reached only the local copy.
                            if let Some(upload) = editor_upload {
                                self.editor_upload_failed(&upload.editor_path);
                            }

                            // Clear pending batch upload (don't continue if upload failed)
                            if is_upload_step && self.state.batch.pending_upload.take().is_some() {
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
                                self.show_error_modal(
                                    termide_i18n::t().op_failed_fmt(&err.to_string()),
                                );
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
                            if is_batch_step {
                                self.state.batch.pending_delete = None;
                            }

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

                            // The edits reached only the local copy.
                            if let Some(upload) = editor_upload {
                                self.editor_upload_failed(&upload.editor_path);
                            }

                            // Clear pending batch upload (don't continue if upload cancelled).
                            // The exact remote target of the file that was in
                            // flight at cancel time is stored on the pending
                            // upload — for multi-file batches this is the
                            // *only* partial file; files already finished in
                            // the batch are intact on the server and stay
                            // untouched.
                            let partial_remote = is_upload_step
                                .then(|| self.state.batch.pending_upload.take())
                                .flatten()
                                .and_then(|pending| {
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
                                })
                                // SFTP writes into a hidden sibling it removes
                                // itself: the path holds the untouched original.
                                .filter(|(path, ..)| {
                                    path.protocol != termide_vfs::VfsProtocol::Sftp
                                });
                            if partial_remote.is_some() {
                                self.state.close_modal();
                            }

                            // For batch operations, show cleanup modal
                            if has_batch {
                                // Remove batch tracking card from operations panel
                                self.state.finish_batch_tracking();

                                if let Some(PendingAction::ContinueBatchOperation {
                                    mut operation,
                                }) = self.state.pending_action.take()
                                {
                                    // The item cut short was new too: its
                                    // remains are as much the batch's own.
                                    operation.record_item_destination();
                                    // Show cleanup modal similar to check_local_copy_progress
                                    let all_dest_paths = operation.completed_destinations.clone();
                                    let buttons = super::modal::cancel_copy_buttons(
                                        !all_dest_paths.is_empty(),
                                    );
                                    let modal = termide_modal::ChoiceModal::buttons_only(
                                        termide_i18n::t().op_cancelled_title(),
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
                                let t = termide_i18n::t();
                                let modal = termide_modal::ConfirmModal::new(
                                    t.op_upload_cancelled_title(),
                                    // Leading blank line creates a one-row
                                    // gap between the title bar and the
                                    // question itself.
                                    format!("\n{}", t.op_delete_partial_upload_fmt(&filename)),
                                );
                                self.state.set_pending_action(
                                    PendingAction::CleanupPartialRemote {
                                        path: partial_path,
                                        vfs_manager,
                                    },
                                    ActiveModal::Confirm(Box::new(modal)),
                                );
                            } else {
                                self.state
                                    .set_info(termide_i18n::t().op_cancelled().to_string());
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

                    self.ask_worker_conflict(
                        termide_state::WorkerConflict {
                            operation_id: id,
                            source: source_path,
                            destination: dest_path,
                            remaining_items: conflict_info.remaining_items,
                        },
                        None,
                    );
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
