use anyhow::Result;
use std::path::PathBuf;

use super::super::App;
use crate::{i18n, panels::file_manager::FileManager};

impl App {
    /// Handle deletion of files/directories
    pub(in crate::app) fn handle_delete_path(
        &mut self,
        _panel_index: usize, // obsolete with LayoutManager
        paths: Vec<PathBuf>,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(confirmed) = value.downcast_ref::<bool>() {
            if *confirmed {
                // Get FileManager and delete files/directories
                if let Some(fm_panel) = self.layout_manager.file_manager_mut() {
                    use std::any::Any;
                    let panel_any: &mut dyn Any = &mut **fm_panel;
                    if let Some(fm) = panel_any.downcast_mut::<FileManager>() {
                        let mut success_count = 0;
                        let mut error_count = 0;
                        let total_count = paths.len();

                        // Delete each file/directory
                        for path in &paths {
                            let item_name =
                                path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                            let is_dir = path.is_dir();

                            self.state.log_info(format!(
                                "Attempting to delete {}: {}",
                                if is_dir { "directory" } else { "file" },
                                item_name
                            ));

                            match fm.delete_path(path.clone()) {
                                Ok(_) => {
                                    self.state.log_success(format!(
                                        "{} '{}' deleted",
                                        if is_dir { "Directory" } else { "File" },
                                        item_name
                                    ));
                                    success_count += 1;
                                }
                                Err(e) => {
                                    self.state.log_error(format!(
                                        "Deletion error '{}': {}",
                                        item_name, e
                                    ));
                                    error_count += 1;
                                }
                            }
                        }

                        // Show final message
                        let t = i18n::t();
                        if total_count == 1 {
                            if success_count == 1 {
                                self.state.set_info(t.status_item_deleted().to_string());
                            } else {
                                self.state.set_error(t.status_error_delete().to_string());
                            }
                        } else if error_count == 0 {
                            self.state.set_info(t.status_items_deleted(success_count));
                        } else {
                            self.state.set_info(
                                t.status_items_deleted_with_errors(success_count, error_count),
                            );
                        }

                        // Clear selection after successful deletion
                        if success_count > 0 {
                            fm.clear_selection();
                        }

                        // Refresh directory contents
                        let _ = fm.load_directory();
                    } else {
                        self.state
                            .log_error("FileManager panel could not be accessed".to_string());
                    }
                } else {
                    self.state.log_error("FileManager not found".to_string());
                }
            }
        }
        Ok(())
    }

    /// Handle panel closure
    pub(in crate::app) fn handle_close_panel(
        &mut self,
        _panel_index: usize, // obsolete with LayoutManager
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(confirmed) = value.downcast_ref::<bool>() {
            if *confirmed {
                // Terminate processes in active panel (for terminal)
                if let Some(panel) = self.layout_manager.active_panel_mut() {
                    panel.kill_processes();
                }
                // Close active panel
                self.close_panel_at_index(0); // panel_index is obsolete
            }
        }
        Ok(())
    }
}
