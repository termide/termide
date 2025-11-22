use anyhow::Result;
use std::path::PathBuf;

use super::super::App;
use crate::{
    i18n,
    panels::editor::Editor,
    state::{ActiveModal, PendingAction},
};

impl App {
    /// Handle editor closure with saving
    pub(in crate::app) fn handle_close_editor_with_save(
        &mut self,
        panel_index: usize,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(selected) = value.downcast_ref::<Vec<usize>>() {
            if selected.is_empty() {
                // Cancel or Esc - do nothing
                return Ok(());
            }

            match selected[0] {
                0 => {
                    // Save and close
                    self.state.log_info("Selected: Save and close editor");
                    if let Some(panel) = self.panels.get_mut(panel_index) {
                        use std::any::Any;
                        let panel_any: &mut dyn Any = &mut **panel;
                        if let Some(editor) = panel_any.downcast_mut::<Editor>() {
                            // Try to save
                            if editor.has_file_path() {
                                // File already has path - just save
                                let t = i18n::t();
                                if let Err(e) = editor.save() {
                                    self.state.log_error(format!("Save error: {}", e));
                                    self.state.set_error(t.status_error_save(&e.to_string()));
                                    return Ok(());
                                }
                                self.state.log_success("File saved before closing");
                            } else {
                                // Unnamed file - need to request name
                                let t = i18n::t();
                                let modal = crate::ui::modal::InputModal::new(
                                    t.modal_save_as_title(),
                                    t.modal_enter_filename(),
                                );
                                let current_dir =
                                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
                                let action = PendingAction::SaveFileAs {
                                    panel_index,
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
                    self.close_panel_at_index(panel_index);
                }
                1 => {
                    // Close without saving
                    self.state.log_info("Selected: Close without saving");
                    self.close_panel_at_index(panel_index);
                }
                2 | _ => {
                    // Cancel - do nothing
                    self.state.log_info("Selected: Cancel closing");
                }
            }
        }
        Ok(())
    }

    /// Handle file overwrite decision
    pub(in crate::app) fn handle_overwrite_decision(
        &mut self,
        panel_index: usize,
        source: PathBuf,
        destination: PathBuf,
        is_move: bool,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(choice) = value.downcast_ref::<crate::ui::modal::OverwriteChoice>() {
            use crate::{i18n, panels::file_manager::FileManager, ui::modal::OverwriteChoice};

            let item_name = source.file_name().and_then(|n| n.to_str()).unwrap_or("?");

            // Determine final destination path
            let final_dest = if destination.is_dir() {
                destination.join(source.file_name().unwrap_or_default())
            } else {
                destination.clone()
            };

            // Check overwrite conditions
            let should_proceed = match choice {
                OverwriteChoice::Replace => true,
                OverwriteChoice::ReplaceIfNewer => {
                    // Compare modification time
                    if let (Ok(src_meta), Ok(dst_meta)) = (source.metadata(), final_dest.metadata())
                    {
                        if let (Ok(src_time), Ok(dst_time)) =
                            (src_meta.modified(), dst_meta.modified())
                        {
                            src_time > dst_time
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                }
                OverwriteChoice::ReplaceIfLarger => {
                    // Compare file sizes
                    if let (Ok(src_meta), Ok(dst_meta)) = (source.metadata(), final_dest.metadata())
                    {
                        src_meta.len() > dst_meta.len()
                    } else {
                        false
                    }
                }
                OverwriteChoice::Skip => false,
            };

            if should_proceed {
                // Execute operation
                if let Some(panel) = self.panels.get_mut(panel_index) {
                    use std::any::Any;
                    let panel_any: &mut dyn Any = &mut **panel;
                    if let Some(fm) = panel_any.downcast_mut::<FileManager>() {
                        let result = if is_move {
                            fm.move_path(source.clone(), destination.clone())
                        } else {
                            fm.copy_path(source.clone(), destination.clone())
                        };

                        match result {
                            Ok(_) => {
                                let t = i18n::t();
                                let action_name = if is_move {
                                    t.action_moved()
                                } else {
                                    t.action_copied()
                                };
                                self.state
                                    .log_success(format!("'{}' {}", item_name, action_name));
                                self.state
                                    .set_info(t.status_item_actioned(&item_name, action_name));

                                // Refresh FM panels
                                if is_move {
                                    if let Some(parent) = source.parent() {
                                        self.refresh_fm_panels(parent);
                                    }
                                }
                                if let Some(parent) = destination.parent() {
                                    self.refresh_fm_panels(parent);
                                }
                            }
                            Err(e) => {
                                let t = i18n::t();
                                let action_name = if is_move {
                                    t.action_moving()
                                } else {
                                    t.action_copying()
                                };
                                self.state
                                    .log_error(format!("Ошибка {}: {}", action_name, e));
                                self.state
                                    .set_error(t.status_error_action(action_name, &e.to_string()));
                            }
                        }
                    }
                }
            } else {
                let t = i18n::t();
                self.state
                    .log_info(format!("Operation '{}' skipped", item_name));
                self.state.set_info(t.status_operation_skipped(&item_name));
            }
        }
        Ok(())
    }
}
