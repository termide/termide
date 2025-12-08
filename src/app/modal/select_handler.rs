use anyhow::Result;
use std::path::PathBuf;

use super::super::App;
use crate::{
    i18n,
    panels::PanelExt,
    path_utils,
    state::{ActiveModal, PendingAction},
};

impl App {
    /// Handle editor closure with saving
    pub(in crate::app) fn handle_close_editor_with_save(
        &mut self,
        _panel_index: usize, // obsolete with LayoutManager
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
                    crate::logger::info("Selected: Save and close editor");
                    if let Some(panel) = self.layout_manager.active_panel_mut() {
                        if let Some(editor) = panel.as_editor_mut() {
                            // Try to save
                            if editor.has_file_path() {
                                // File already has path - just save
                                let t = i18n::t();
                                if let Err(e) = editor.save() {
                                    crate::logger::error(format!("Save error: {}", e));
                                    self.state.set_error(t.status_error_save(&e.to_string()));
                                    return Ok(());
                                }
                                crate::logger::info("File saved before closing");
                            } else {
                                // Unnamed file - need to request name
                                let t = i18n::t();
                                let modal =
                                    crate::ui::modal::InputModal::new(t.modal_save_as_title(), "");
                                let current_dir =
                                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
                                let action = PendingAction::SaveFileAs {
                                    panel_index: 0, // placeholder, obsolete
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
                    self.close_panel_at_index(0); // panel_index is obsolete
                }
                1 => {
                    // Close without saving
                    crate::logger::info("Selected: Close without saving");
                    self.close_panel_at_index(0); // panel_index is obsolete
                }
                _ => {
                    // Cancel - do nothing
                    crate::logger::info("Selected: Cancel closing");
                }
            }
        }
        Ok(())
    }

    /// Handle editor closure with external changes (file changed on disk)
    pub(in crate::app) fn handle_close_editor_external(
        &mut self,
        _panel_index: usize, // obsolete with LayoutManager
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(selected) = value.downcast_ref::<Vec<usize>>() {
            if selected.is_empty() {
                // Cancel or Esc - do nothing
                return Ok(());
            }

            match selected[0] {
                0 => {
                    // Overwrite disk with current content
                    crate::logger::info("Selected: Overwrite disk with current content");
                    if let Some(panel) = self.layout_manager.active_panel_mut() {
                        if let Some(editor) = panel.as_editor_mut() {
                            let t = i18n::t();
                            if let Err(e) = editor.force_save() {
                                crate::logger::error(format!("Force save error: {}", e));
                                self.state.set_error(t.status_error_save(&e.to_string()));
                                return Ok(());
                            }
                        }
                    }
                    self.close_panel_at_index(0);
                }
                1 => {
                    // Keep disk version (just close)
                    crate::logger::info("Selected: Keep disk version, close editor");
                    self.close_panel_at_index(0);
                }
                2 => {
                    // Reload into editor (don't close)
                    crate::logger::info("Selected: Reload file into editor");
                    if let Some(panel) = self.layout_manager.active_panel_mut() {
                        if let Some(editor) = panel.as_editor_mut() {
                            let t = i18n::t();
                            if let Err(e) = editor.reload_from_disk() {
                                crate::logger::error(format!("Reload error: {}", e));
                                self.state.set_error(t.status_error_reload(&e.to_string()));
                            } else {
                                self.state.set_info(t.status_file_reloaded().to_string());
                            }
                        }
                    }
                    // Don't close - user wants to continue editing
                }
                _ => {
                    // Cancel - do nothing
                    crate::logger::info("Selected: Cancel closing");
                }
            }
        }
        Ok(())
    }

    /// Handle editor closure with conflict (local + external changes)
    pub(in crate::app) fn handle_close_editor_conflict(
        &mut self,
        _panel_index: usize, // obsolete with LayoutManager
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(selected) = value.downcast_ref::<Vec<usize>>() {
            if selected.is_empty() {
                // Cancel or Esc - do nothing
                return Ok(());
            }

            match selected[0] {
                0 => {
                    // Overwrite disk with my changes
                    crate::logger::info("Selected: Overwrite disk with local changes");
                    if let Some(panel) = self.layout_manager.active_panel_mut() {
                        if let Some(editor) = panel.as_editor_mut() {
                            let t = i18n::t();
                            if let Err(e) = editor.force_save() {
                                crate::logger::error(format!("Force save error: {}", e));
                                self.state.set_error(t.status_error_save(&e.to_string()));
                                return Ok(());
                            }
                        }
                    }
                    self.close_panel_at_index(0);
                }
                1 => {
                    // Reload from disk (discard local changes)
                    crate::logger::info("Selected: Reload from disk, discard local changes");
                    if let Some(panel) = self.layout_manager.active_panel_mut() {
                        if let Some(editor) = panel.as_editor_mut() {
                            let t = i18n::t();
                            if let Err(e) = editor.reload_from_disk() {
                                crate::logger::error(format!("Reload error: {}", e));
                                self.state.set_error(t.status_error_reload(&e.to_string()));
                                return Ok(());
                            }
                        }
                    }
                    self.close_panel_at_index(0);
                }
                _ => {
                    // Cancel - do nothing
                    crate::logger::info("Selected: Cancel closing");
                }
            }
        }
        Ok(())
    }

    /// Handle file overwrite decision
    pub(in crate::app) fn handle_overwrite_decision(
        &mut self,
        _panel_index: usize, // obsolete with LayoutManager
        source: PathBuf,
        destination: PathBuf,
        is_move: bool,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(choice) = value.downcast_ref::<crate::ui::modal::OverwriteChoice>() {
            use crate::{i18n, ui::modal::OverwriteChoice};

            let item_name = path_utils::get_file_name_str(&source);

            let final_dest = path_utils::resolve_destination_path(&source, &destination);

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
                // Execute operation using FileManager
                if let Some(fm_panel) = self.get_first_file_manager_mut() {
                    if let Some(fm) = fm_panel.as_file_manager_mut() {
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
                                crate::logger::info(format!("'{}' {}", item_name, action_name));
                                self.state
                                    .set_info(t.status_item_actioned(item_name, action_name));

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
                                crate::logger::error(format!("Ошибка {}: {}", action_name, e));
                                self.state
                                    .set_error(t.status_error_action(action_name, &e.to_string()));
                            }
                        }
                    }
                }
            } else {
                let t = i18n::t();
                crate::logger::info(format!("Operation '{}' skipped", item_name));
                self.state.set_info(t.status_operation_skipped(item_name));
            }
        }
        Ok(())
    }
}
