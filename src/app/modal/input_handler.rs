use anyhow::Result;
use std::path::PathBuf;

use super::super::App;
use crate::{
    i18n,
    panels::{editor::Editor, file_manager::FileManager},
};

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
            // Get FileManager and create file
            if let Some(fm_panel) = self.layout_manager.file_manager_mut() {
                // Use Any trait for downcast
                use std::any::Any;
                let panel_any: &mut dyn Any = &mut **fm_panel;
                if let Some(fm) = panel_any.downcast_mut::<FileManager>() {
                    match fm.create_file(name.clone()) {
                        Ok(_) => {
                            crate::logger::info(format!("File created: {}", name));
                            self.state.set_info(t.status_file_created(name));
                            // Refresh directory contents
                            let _ = fm.load_directory();
                        }
                        Err(e) => {
                            crate::logger::error(format!("File creation error '{}': {}", name, e));
                            self.state
                                .set_error(t.status_error_create_file(&e.to_string()));
                        }
                    }
                } else {
                    crate::logger::error("FileManager panel could not be accessed".to_string());
                }
            } else {
                crate::logger::error("FileManager not found".to_string());
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
            // Get FileManager and create directory
            if let Some(fm_panel) = self.layout_manager.file_manager_mut() {
                use std::any::Any;
                let panel_any: &mut dyn Any = &mut **fm_panel;
                if let Some(fm) = panel_any.downcast_mut::<FileManager>() {
                    match fm.create_directory(name.clone()) {
                        Ok(_) => {
                            crate::logger::info(format!("Directory created: {}", name));
                            self.state.set_info(t.status_dir_created(name));
                            // Refresh directory contents
                            let _ = fm.load_directory();
                        }
                        Err(e) => {
                            crate::logger::error(format!(
                                "Directory creation error '{}': {}",
                                name, e
                            ));
                            self.state
                                .set_error(t.status_error_create_dir(&e.to_string()));
                        }
                    }
                } else {
                    crate::logger::error("FileManager panel could not be accessed".to_string());
                }
            } else {
                crate::logger::error("FileManager not found".to_string());
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
                use std::any::Any;
                let panel_any: &mut dyn Any = &mut **panel;
                if let Some(editor) = panel_any.downcast_mut::<Editor>() {
                    // User enters path - parse it directly
                    let file_path = PathBuf::from(filename);
                    match editor.save_file_as(file_path.clone()) {
                        Ok(_) => {
                            crate::logger::info(format!("File saved as: {}", filename));
                            self.state.set_info(t.status_file_saved(filename));
                        }
                        Err(e) => {
                            crate::logger::error(format!("Save error '{}': {}", filename, e));
                            self.state.set_error(t.status_error_save(&e.to_string()));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
