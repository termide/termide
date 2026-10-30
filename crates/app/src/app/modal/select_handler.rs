//! Select modal result handling.

// Note: PanelExt is used for editor-specific operations (go_to_line, save, replace).

use anyhow::Result;
use std::path::PathBuf;

use super::super::App;
use crate::state::{ActiveModal, PendingAction};
use crate::PanelExt;
use std::sync::Arc;
use termide_i18n as i18n;

impl App {
    /// Force-save the active editor and handle the remote-upload / close flow.
    ///
    /// Returns `true` if the helper took ownership of the close flow (either
    /// because an error was shown or an async upload was queued — in both
    /// cases the caller should return immediately). Returns `false` for a
    /// successful local save where the caller should proceed to close the
    /// panel synchronously.
    fn force_save_active_editor(&mut self) -> Result<bool> {
        let editor_file_path = self
            .layout_manager
            .active_panel()
            .and_then(|p| p.as_any().downcast_ref::<termide_panel_editor::Editor>())
            .and_then(|e| e.file_path().map(|p| p.to_path_buf()));

        let upload_info = {
            let mut result = None;
            if let Some(panel) = self.layout_manager.active_panel_mut() {
                if let Some(editor) = panel.as_editor_mut() {
                    match editor.force_save() {
                        Err(e) => {
                            log::error!("Force save error: {}", e);
                            let t = i18n::t();
                            self.show_error_modal(t.status_error_save(&e.to_string()));
                            return Ok(true); // Error shown; caller returns.
                        }
                        Ok(Some(info)) => {
                            result = Some(info);
                        }
                        Ok(None) => {}
                    }
                }
            }
            result
        };

        if let Some((temp_path, remote_path, vfs_manager)) = upload_info {
            // Remote file — queue async upload; panel closes when upload completes.
            self.queue_remote_editor_upload(temp_path, remote_path, vfs_manager, editor_file_path);
            return Ok(true);
        }

        Ok(false) // Local save succeeded; caller should close the panel synchronously.
    }

    /// Start an async upload of the editor's temp file to the remote path via
    /// `OperationManager`. If `close_after` is `Some(path)`, the editor panel
    /// with that file path will close once the upload completes (handled in
    /// the operation manager event loop).
    ///
    /// Sets the editor's "uploading" flag (spinner in header), tracks the
    /// operation in the Operations panel, and shows an error modal on failure.
    /// Returns `true` if the upload was queued successfully.
    pub(in crate::app) fn queue_remote_editor_upload(
        &mut self,
        temp_path: std::path::PathBuf,
        remote_path: termide_vfs::VfsPath,
        vfs_manager: Arc<termide_vfs::VfsManager>,
        close_after: Option<std::path::PathBuf>,
    ) -> bool {
        // Set uploading flag on the active editor (spinner in header)
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                editor.set_uploading(true);
            }
        }

        let total_bytes = std::fs::metadata(&temp_path).map(|m| m.len()).unwrap_or(0);
        let source_display = temp_path.display().to_string();
        let request = termide_file_ops::OperationRequest::upload(temp_path, remote_path.clone());

        match self.state.start_operation_now(request, vfs_manager) {
            Ok(operation_id) => {
                self.state.track_operation(
                    operation_id,
                    crate::state::OperationType::CopyUpload,
                    source_display,
                    remote_path.to_url_string(),
                    1,
                    total_bytes,
                );
                if close_after.is_some() {
                    self.state.close_editor_after_upload = close_after;
                }
                // Skip file-manager refresh — file already exists at destination
                self.state.skip_refresh_after_upload = true;
                true
            }
            Err(e) => {
                log::error!("Failed to start upload: {}", e);
                self.show_error_modal(format!("Upload failed: {}", e));
                if let Some(panel) = self.layout_manager.active_panel_mut() {
                    if let Some(editor) = panel.as_editor_mut() {
                        editor.set_uploading(false);
                    }
                }
                false
            }
        }
    }

    /// Handle editor closure with saving
    pub(in crate::app) fn handle_close_editor_with_save(
        &mut self,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        // Store info needed for LSP notification (before mutable borrow)
        let mut lsp_info: Option<(String, PathBuf)> = None;

        if let Some(&selected) = value.downcast_ref::<usize>() {
            // The binary hex editor reuses this Save / Don't save / Cancel
            // dialog; handle it up-front (its save path differs from the editor).
            if self.layout_manager.active_panel().map(|p| p.name()) == Some("binary") {
                match selected {
                    0 => {
                        if let Some(bin) = self.layout_manager.active_panel_mut().and_then(|p| {
                            p.as_any_mut()
                                .downcast_mut::<termide_panel_binary::BinaryPanel>()
                        }) {
                            if let Err(e) = bin.save() {
                                self.show_error_modal(format!("Save failed: {e}"));
                                return Ok(());
                            }
                        }
                        self.close_panel_at_index();
                    }
                    1 => self.close_panel_at_index(),
                    _ => {}
                }
                return Ok(());
            }
            match selected {
                0 => {
                    // Save and close
                    // Capture editor file path before mutable borrow for save
                    let editor_file_path = self
                        .layout_manager
                        .active_panel()
                        .and_then(|p| p.as_any().downcast_ref::<termide_panel_editor::Editor>())
                        .and_then(|e| e.file_path().map(|p| p.to_path_buf()));

                    if let Some(panel) = self.layout_manager.active_panel_mut() {
                        if let Some(editor) = panel.as_editor_mut() {
                            // Try to save
                            if editor.has_file_path() {
                                // File already has path - just save
                                let t = i18n::t();

                                match editor.save() {
                                    Err(e) => {
                                        log::error!("Save error: {}", e);
                                        self.show_error_modal(t.status_error_save(&e.to_string()));
                                        return Ok(());
                                    }
                                    Ok(Some((temp_path, remote_path, vfs_manager))) => {
                                        // Remote file — queue async upload; panel closes in the
                                        // operation-manager event loop when upload completes.
                                        self.queue_remote_editor_upload(
                                            temp_path,
                                            remote_path,
                                            vfs_manager,
                                            editor_file_path,
                                        );
                                        return Ok(());
                                    }
                                    Ok(None) => {
                                        // Local file - saved synchronously
                                    }
                                }

                                // Collect LSP info for didSave notification
                                if let Some(lang) = editor.lsp_language() {
                                    if let Some(path) = editor.file_path() {
                                        lsp_info = Some((lang.to_string(), path.to_path_buf()));
                                    }
                                }
                            } else {
                                // Unnamed file - need to request name
                                let t = i18n::t();
                                let modal =
                                    termide_modal::InputModal::new(t.modal_save_as_title(), "");
                                let current_dir =
                                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
                                let action = PendingAction::SaveFileAs {
                                    directory: current_dir,
                                };
                                self.state.set_pending_action(
                                    action,
                                    ActiveModal::Input(Box::new(modal)),
                                );
                                // After saving file will remain open, need to close separately
                                // This is simplification, but for full implementation need more complex PendingAction
                                return Ok(());
                            }
                        }
                    }
                    // Close panel after saving
                    self.close_panel_at_index();
                }
                1 => {
                    // Close without saving
                    self.close_panel_at_index();
                }
                _ => {
                    // Cancel - do nothing
                }
            }
        }

        // Send LSP didSave notification (triggers full analysis for semantic errors)
        if let Some((lang, file_path)) = lsp_info {
            if let Some(ref lsp_manager) = self.state.lsp_manager {
                lsp_manager.did_save(&lang, &file_path, None);
            }
        }

        Ok(())
    }

    /// Handle editor closure with external changes (file changed on disk)
    pub(in crate::app) fn handle_close_editor_external(
        &mut self,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(&selected) = value.downcast_ref::<usize>() {
            match selected {
                0 => {
                    // Overwrite disk with current content
                    if self.force_save_active_editor()? {
                        return Ok(());
                    }
                    self.close_panel_at_index();
                }
                1 => {
                    // Keep disk version (just close)
                    self.close_panel_at_index();
                }
                2 => {
                    // Reload into editor (don't close)
                    if let Some(panel) = self.layout_manager.active_panel_mut() {
                        if let Some(editor) = panel.as_editor_mut() {
                            let t = i18n::t();
                            if let Err(e) = editor.reload_from_disk() {
                                log::error!("Reload error: {}", e);
                                self.show_error_modal(t.status_error_reload(&e.to_string()));
                            } else {
                                self.state.set_info(t.status_file_reloaded().to_string());
                            }
                        }
                    }
                    // Don't close - user wants to continue editing
                }
                _ => {
                    // Cancel - do nothing
                }
            }
        }
        Ok(())
    }

    /// Handle editor closure with conflict (local + external changes)
    pub(in crate::app) fn handle_close_editor_conflict(
        &mut self,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(&selected) = value.downcast_ref::<usize>() {
            match selected {
                0 => {
                    // Overwrite disk with my changes
                    if self.force_save_active_editor()? {
                        return Ok(());
                    }
                    self.close_panel_at_index();
                }
                1 => {
                    // Reload from disk (discard local changes)
                    if let Some(panel) = self.layout_manager.active_panel_mut() {
                        if let Some(editor) = panel.as_editor_mut() {
                            let t = i18n::t();
                            if let Err(e) = editor.reload_from_disk() {
                                log::error!("Reload error: {}", e);
                                self.show_error_modal(t.status_error_reload(&e.to_string()));
                                return Ok(());
                            }
                        }
                    }
                    self.close_panel_at_index();
                }
                _ => {
                    // Cancel - do nothing
                }
            }
        }
        Ok(())
    }

    /// Handle cancelled copy/move operation cleanup
    ///
    /// For directory copy (is_directory=true):
    /// - partial_path = the file that was being copied when cancelled
    /// - all_dest_paths[0] = the destination directory
    /// - Options: Delete partial (file only), Delete all (entire dir), Keep all
    ///
    /// For file copy (is_directory=false):
    /// - partial_path = the partial file
    /// - all_dest_paths = previously completed files (empty for single file)
    /// - Options: Delete (partial), [Delete all], Keep
    pub(in crate::app) fn handle_cancel_copy_cleanup(
        &mut self,
        partial_path: PathBuf,
        all_dest_paths: Vec<PathBuf>,
        is_directory: bool,
        _batch_operation: Option<Box<termide_state::BatchOperation>>,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        // ChoiceModal returns usize (button index)
        if let Some(&selected) = value.downcast_ref::<usize>() {
            if is_directory {
                // Directory copy cancellation - always 3 options:
                // 0 = Keep all (keep everything as is)
                // 1 = Delete partial (only the interrupted file)
                // 2 = Delete all (entire destination directory)
                match selected {
                    1 => {
                        // Delete only the interrupted file
                        if partial_path.exists() {
                            if let Err(e) = std::fs::remove_file(&partial_path) {
                                self.show_error_modal(format!("Failed to delete: {}", e));
                            } else {
                                self.state.set_info("Partial file deleted".to_string());
                            }
                        }
                    }
                    2 => {
                        // Delete entire destination directory
                        if let Some(dest_dir) = all_dest_paths.first() {
                            if dest_dir.exists() {
                                if let Err(e) = std::fs::remove_dir_all(dest_dir) {
                                    self.show_error_modal(format!("Failed to delete: {}", e));
                                } else {
                                    self.state.set_info("Directory deleted".to_string());
                                }
                            }
                        }
                    }
                    _ => {
                        // Keep all (0 or any other)
                    }
                }
            } else {
                // File copy cancellation
                let has_multiple = !all_dest_paths.is_empty();

                match (selected, has_multiple) {
                    (0, _) => {
                        // Delete partial file only
                        if partial_path.exists() {
                            if let Err(e) = std::fs::remove_file(&partial_path) {
                                self.show_error_modal(format!("Failed to delete: {}", e));
                            } else {
                                self.state.set_info("File deleted".to_string());
                            }
                        }
                    }
                    (1, true) => {
                        // Delete all copied files
                        let mut deleted = 0;
                        let mut errors = 0;

                        // Delete partial file first
                        if partial_path.exists() {
                            if std::fs::remove_file(&partial_path).is_ok() {
                                deleted += 1;
                            } else {
                                errors += 1;
                            }
                        }

                        // Delete all completed files
                        for dest in &all_dest_paths {
                            if dest.exists() {
                                let result = if dest.is_dir() {
                                    std::fs::remove_dir_all(dest)
                                } else {
                                    std::fs::remove_file(dest)
                                };
                                if result.is_ok() {
                                    deleted += 1;
                                } else {
                                    errors += 1;
                                }
                            }
                        }

                        if errors > 0 {
                            self.show_error_modal(format!(
                                "Deleted {} items, {} errors",
                                deleted, errors
                            ));
                        } else {
                            self.state.set_info(format!("Deleted {} items", deleted));
                        }
                    }
                    _ => {
                        // Keep everything
                    }
                }
            }
        }

        // Refresh file manager panels to show changes
        if let Some(parent) = partial_path.parent() {
            self.refresh_fm_panels(parent);
        }
        for dest in &all_dest_paths {
            if let Some(parent) = dest.parent() {
                self.refresh_fm_panels(parent);
            }
        }

        Ok(())
    }
}
