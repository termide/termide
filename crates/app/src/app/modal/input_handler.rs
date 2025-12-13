//! Input modal result handling.

// Note: PanelExt is used for FileManager file operations (create file/dir).
#![allow(deprecated)]

use anyhow::Result;
use std::path::PathBuf;

use super::super::App;
use crate::PanelExt;
use termide_i18n as i18n;

impl App {
    /// Handle file creation
    pub(in crate::app) fn handle_create_file(
        &mut self,
        _panel_index: usize, // obsolete with LayoutManager
        _directory: PathBuf,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(name) = value.downcast_ref::<String>() {
            let t = i18n::t();
            // Get active FileManager and create file
            let result = if let Some(panel) = self.layout_manager.active_panel_mut() {
                if let Some(fm) = panel.as_file_manager_mut() {
                    let result = fm.create_file(name.clone());
                    if result.is_ok() {
                        termide_logger::info(format!("File created: {}", name));
                        // Refresh directory contents
                        let _ = fm.load_directory();
                    }
                    Some(result)
                } else {
                    termide_logger::error("FileManager panel could not be accessed".to_string());
                    None
                }
            } else {
                termide_logger::error("FileManager not found".to_string());
                None
            };

            // Update status after FM borrow is dropped
            if let Some(result) = result {
                match result {
                    Ok(_) => {
                        self.state.set_info(t.status_file_created(name));
                    }
                    Err(e) => {
                        termide_logger::error(format!("File creation error '{}': {}", name, e));
                        self.state
                            .set_error(t.status_error_create_file(&e.to_string()));
                    }
                }
            }
        }
        Ok(())
    }

    /// Handle directory creation
    pub(in crate::app) fn handle_create_directory(
        &mut self,
        _panel_index: usize, // obsolete with LayoutManager
        _directory: PathBuf,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(name) = value.downcast_ref::<String>() {
            let t = i18n::t();
            // Get active FileManager and create directory
            let result = if let Some(panel) = self.layout_manager.active_panel_mut() {
                if let Some(fm) = panel.as_file_manager_mut() {
                    let result = fm.create_directory(name.clone());
                    if result.is_ok() {
                        termide_logger::info(format!("Directory created: {}", name));
                        // Refresh directory contents
                        let _ = fm.load_directory();
                    }
                    Some(result)
                } else {
                    termide_logger::error("FileManager panel could not be accessed".to_string());
                    None
                }
            } else {
                termide_logger::error("FileManager not found".to_string());
                None
            };

            // Update status after FM borrow is dropped
            if let Some(result) = result {
                match result {
                    Ok(_) => {
                        self.state.set_info(t.status_dir_created(name));
                    }
                    Err(e) => {
                        termide_logger::error(format!(
                            "Directory creation error '{}': {}",
                            name, e
                        ));
                        self.state
                            .set_error(t.status_error_create_dir(&e.to_string()));
                    }
                }
            }
        }
        Ok(())
    }

    /// Handle saving file with new name
    pub(in crate::app) fn handle_save_file_as(
        &mut self,
        _panel_index: usize, // obsolete with LayoutManager
        _directory: PathBuf,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(filename) = value.downcast_ref::<String>() {
            let t = i18n::t();
            // Get active Editor panel and save file
            if let Some(panel) = self.layout_manager.active_panel_mut() {
                if let Some(editor) = panel.as_editor_mut() {
                    // User enters path - parse it directly
                    let file_path = PathBuf::from(filename);
                    match editor.save_file_as(file_path) {
                        Ok(_) => {
                            termide_logger::info(format!("File saved as: {}", filename));
                            self.state.set_info(t.status_file_saved(filename));
                        }
                        Err(e) => {
                            termide_logger::error(format!("Save error '{}': {}", filename, e));
                            self.state.set_error(t.status_error_save(&e.to_string()));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
