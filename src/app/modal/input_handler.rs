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
        panel_index: usize,
        _directory: PathBuf,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(name) = value.downcast_ref::<String>() {
            let t = i18n::t();
            self.state.log_info(format!(
                "Attempting to create file: {} (panel_index={})",
                name, panel_index
            ));
            // Get FileManager panel and create file
            if let Some(panel) = self.panels.get_mut(panel_index) {
                // Use Any trait for downcast
                use std::any::Any;
                let panel_any: &mut dyn Any = &mut **panel;
                if let Some(fm) = panel_any.downcast_mut::<FileManager>() {
                    match fm.create_file(name.clone()) {
                        Ok(_) => {
                            self.state.log_success(format!("File '{}' created", name));
                            self.state.set_info(t.status_file_created(name));
                            // Refresh directory contents
                            let _ = fm.load_directory();
                        }
                        Err(e) => {
                            self.state
                                .log_error(format!("File creation error '{}': {}", name, e));
                            self.state
                                .set_error(t.status_error_create_file(&e.to_string()));
                        }
                    }
                } else {
                    self.state
                        .log_error(format!("Panel {} is not FileManager", panel_index));
                }
            } else {
                self.state
                    .log_error(format!("Panel with index {} not found", panel_index));
            }
        }
        Ok(())
    }

    /// Handle directory creation
    pub(in crate::app) fn handle_create_directory(
        &mut self,
        panel_index: usize,
        _directory: PathBuf,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(name) = value.downcast_ref::<String>() {
            let t = i18n::t();
            self.state.log_info(format!(
                "Attempting to create directory: {} (panel_index={})",
                name, panel_index
            ));
            // Get FileManager panel and create directory
            if let Some(panel) = self.panels.get_mut(panel_index) {
                use std::any::Any;
                let panel_any: &mut dyn Any = &mut **panel;
                if let Some(fm) = panel_any.downcast_mut::<FileManager>() {
                    match fm.create_directory(name.clone()) {
                        Ok(_) => {
                            self.state
                                .log_success(format!("Directory '{}' created", name));
                            self.state.set_info(t.status_dir_created(name));
                            // Refresh directory contents
                            let _ = fm.load_directory();
                        }
                        Err(e) => {
                            self.state
                                .log_error(format!("Directory creation error '{}': {}", name, e));
                            self.state
                                .set_error(t.status_error_create_dir(&e.to_string()));
                        }
                    }
                } else {
                    self.state
                        .log_error(format!("Panel {} is not FileManager", panel_index));
                }
            } else {
                self.state
                    .log_error(format!("Panel with index {} not found", panel_index));
            }
        }
        Ok(())
    }

    /// Handle saving file with new name
    pub(in crate::app) fn handle_save_file_as(
        &mut self,
        panel_index: usize,
        _directory: PathBuf,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(filename) = value.downcast_ref::<String>() {
            let t = i18n::t();
            self.state
                .log_info(format!("Attempting to save file: {}", filename));
            // Get Editor panel and save file
            if let Some(panel) = self.panels.get_mut(panel_index) {
                use std::any::Any;
                let panel_any: &mut dyn Any = &mut **panel;
                if let Some(editor) = panel_any.downcast_mut::<Editor>() {
                    // User enters path - parse it directly
                    let file_path = PathBuf::from(filename);
                    match editor.save_file_as(file_path.clone()) {
                        Ok(_) => {
                            self.state.log_success(format!("File '{}' saved", filename));
                            self.state.set_info(t.status_file_saved(filename));
                        }
                        Err(e) => {
                            self.state
                                .log_error(format!("Save error '{}': {}", filename, e));
                            self.state.set_error(t.status_error_save(&e.to_string()));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
