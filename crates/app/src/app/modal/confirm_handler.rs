//! Confirm modal result handling.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

use super::super::App;
use crate::state::OperationType;
use termide_file_ops::{OperationPath, OperationRequest};
use termide_ui::path_utils;
use termide_vfs::{VfsManager, VfsPath};

impl App {
    /// Handle deletion of files/directories
    pub(in crate::app) fn handle_delete_path(
        &mut self,
        paths: Vec<PathBuf>,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(confirmed) = value.downcast_ref::<bool>() {
            if *confirmed && !paths.is_empty() {
                let source_display = if paths.len() == 1 {
                    path_utils::get_file_name_str(&paths[0]).to_string()
                } else {
                    termide_i18n::t().status_item_count(paths.len())
                };
                let sources: Vec<OperationPath> =
                    paths.into_iter().map(OperationPath::Local).collect();
                let vfs_manager = Arc::new(VfsManager::new());
                self.start_delete_operation(sources, vfs_manager, source_display);
            }
        }
        Ok(())
    }

    /// Handle deletion of remote files/directories
    pub(in crate::app) fn handle_delete_remote_path(
        &mut self,
        paths: Vec<VfsPath>,
        vfs_manager: Arc<VfsManager>,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(confirmed) = value.downcast_ref::<bool>() {
            if *confirmed && !paths.is_empty() {
                let source_display = if paths.len() == 1 {
                    paths[0]
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "file".to_string())
                } else {
                    termide_i18n::t().status_item_count(paths.len())
                };
                let sources: Vec<OperationPath> =
                    paths.into_iter().map(OperationPath::Remote).collect();
                self.start_delete_operation(sources, vfs_manager, source_display);
            }
        }
        Ok(())
    }

    /// Start a tracked delete operation (shared by local and remote delete handlers).
    fn start_delete_operation(
        &mut self,
        sources: Vec<OperationPath>,
        vfs_manager: Arc<VfsManager>,
        source_display: String,
    ) {
        log::info!("Starting async delete of {}", source_display);
        let paths_count = sources.len();
        let request = OperationRequest::delete(sources);

        match self.start_tracked_operation(
            request,
            vfs_manager,
            OperationType::Delete,
            source_display,
            String::new(),
            paths_count,
            0,
        ) {
            Ok(_operation_id) => {}
            Err(e) => {
                log::error!("Failed to start delete operation: {}", e);
                self.show_error_modal(termide_i18n::t().status_delete_failed(&e.to_string()));
            }
        }
    }

    /// Handle panel closure
    pub(in crate::app) fn handle_close_panel(
        &mut self,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(confirmed) = value.downcast_ref::<bool>() {
            if *confirmed {
                // Terminate processes in active panel (for terminal); an
                // agent's terminal is handed back to the agent, not killed.
                if let Some(panel) = self.layout_manager.active_panel_mut() {
                    if crate::app::agent_panel::terminals::ends_on_close(&**panel) {
                        panel.kill_processes();
                    }
                }
                // Close active panel
                self.close_panel_at_index();
            }
        }
        Ok(())
    }
}
