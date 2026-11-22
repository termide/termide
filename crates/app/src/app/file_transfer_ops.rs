//! Remote file transfer operation handlers.
//!
//! Contains handlers for remote file system operations:
//! - Upload operations (local → remote)

use std::path::{Path, PathBuf};

use crate::PanelExt;

use super::App;

impl App {
    /// Handle pending upload operation from Editor (regular Ctrl+S save of remote file)
    pub(super) fn handle_pending_upload(
        &mut self,
        temp_path: PathBuf,
        remote_path: termide_vfs::VfsPath,
        vfs_manager: std::sync::Arc<termide_vfs::VfsManager>,
    ) {
        if self.ask_if_remote_changed(&temp_path, false) {
            return;
        }

        // Set uploading flag on active editor
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                editor.set_uploading(true);
            }
        }

        // Create upload request via OperationManager
        let request =
            termide_file_ops::OperationRequest::upload(temp_path.clone(), remote_path.clone());

        // Start upload via OperationManager
        match self.state.start_operation_now(request, vfs_manager) {
            Ok(operation_id) => {
                log::info!("Started editor upload operation {}", operation_id);

                // Track in operations panel instead of showing modal
                self.state.track_operation(
                    operation_id,
                    crate::state::OperationType::CopyUpload,
                    temp_path.display().to_string(),
                    remote_path.to_url_string(),
                    1,
                    0,
                );

                self.state.editor_uploads.push(crate::state::EditorUpload {
                    operation_id,
                    editor_path: temp_path,
                    close_after: false,
                });
            }
            Err(e) => {
                log::error!("Failed to start upload operation: {}", e);
                self.show_error_modal(termide_i18n::t().op_upload_failed_fmt(&e.to_string()));
                // Clear uploading flag
                if let Some(panel) = self.layout_manager.active_panel_mut() {
                    if let Some(editor) = panel.as_editor_mut() {
                        editor.set_uploading(false);
                    }
                }
            }
        }
    }

    /// The editor whose file (the local copy of a remote file) is `path`.
    fn editor_with_path_mut(&mut self, path: &Path) -> Option<&mut termide_panel_editor::Editor> {
        self.layout_manager
            .iter_all_panels_mut()
            .filter_map(|panel| panel.as_editor_mut())
            .find(|editor| editor.file_path() == Some(path))
    }

    /// Before uploading the save of the editor with local copy
    /// `temp_path`: when the file changed on the server since it was
    /// downloaded or last uploaded, ask whether to overwrite it instead of
    /// doing so unseen. The edits are unsaved again meanwhile, so declining
    /// keeps them and closing still asks. Returns whether it asked.
    pub(super) fn ask_if_remote_changed(&mut self, temp_path: &Path, close_after: bool) -> bool {
        let Some(editor) = self.editor_with_path_mut(temp_path) else {
            return false;
        };
        let Some(server_mtime) = editor.remote_changed_on_server() else {
            return false;
        };
        editor.upload_failed();
        let t = termide_i18n::t();
        let modal = termide_modal::ConfirmModal::new(
            t.editor_close_external(),
            t.editor_remote_changed_question(),
        )
        .defaulting_to_no();
        self.state.set_pending_action(
            termide_state::PendingAction::OverwriteRemoteSave {
                editor_path: temp_path.to_path_buf(),
                server_mtime,
                close_after,
            },
            termide_modal::ActiveModal::Confirm(Box::new(modal)),
        );
        true
    }

    /// The user chose to overwrite the newer file on the server: take its
    /// time as the base, save again and upload.
    pub(super) fn overwrite_remote_save(
        &mut self,
        editor_path: PathBuf,
        server_mtime: std::time::SystemTime,
        close_after: bool,
    ) {
        let Some(editor) = self.editor_with_path_mut(&editor_path) else {
            return;
        };
        editor.set_remote_mtime(Some(server_mtime));
        match editor.save() {
            Ok(Some((temp_path, remote_path, vfs_manager))) => {
                self.queue_remote_editor_upload(
                    temp_path,
                    remote_path,
                    vfs_manager,
                    close_after.then_some(editor_path),
                );
            }
            Ok(None) => {}
            Err(e) => {
                self.show_error_modal(termide_i18n::t().status_error_save(&e.to_string()));
            }
        }
    }

    /// After the editor's upload succeeded: its next save is checked
    /// against the server's new time.
    pub(super) fn refresh_editor_remote_mtime(&mut self, editor_path: &Path) {
        if let Some(editor) = self.editor_with_path_mut(editor_path) {
            editor.refresh_remote_mtime();
        }
    }
}
