//! Remote file transfer operation handlers.
//!
//! Contains handlers for remote file system operations:
//! - Upload operations (local → remote)

use std::path::PathBuf;

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

                // Skip file manager refresh - file already exists, we're just overwriting
                self.state.skip_refresh_after_upload = true;
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
}
