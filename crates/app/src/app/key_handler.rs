//! Main keyboard event handling for the application.
//!
//! Dispatches key events to modals, menus, global hotkeys, or active panels.

// Note: PanelExt is still used for panel-specific resource extraction
// (take_config_update, dir_size_receiver) which don't fit the command pattern.
#![allow(deprecated)]

use anyhow::Result;

use super::App;
use crate::state::{ActiveModal, PendingAction};
use crate::PanelExt;
use termide_i18n as i18n;
use termide_logger as logger;

impl App {
    /// Handle keyboard event
    pub(super) fn handle_key_event(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        // Translate Cyrillic to Latin for hotkeys
        let key = termide_keyboard::translate_hotkey(key);

        // Log key event for debugging
        logger::debug(format!(
            "Key event: code={:?}, modifiers={:?}",
            key.code, key.modifiers
        ));

        // Clear status message on any key press
        if self.state.ui.status_message.is_some() {
            self.state.clear_status();
        }

        // If modal window is open, handle it
        if self.state.has_modal() {
            return self.handle_modal_key(key);
        }

        // If menu is open, handle menu navigation
        if self.state.ui.menu_open {
            return self.handle_menu_key(key);
        }

        // Handle global hotkeys
        #[allow(clippy::redundant_pattern_matching)]
        if let Some(_) = self.handle_global_hotkeys(key)? {
            return Ok(());
        }

        // Pass event to active panel and collect results
        let (events, modal_request, config_update) =
            if let Some(panel) = self.layout_manager.active_panel_mut() {
                // handle_key returns Vec<PanelEvent>
                let events = panel.handle_key(key);

                // Legacy methods still in use
                let modal_request = panel.take_modal_request();
                let config_update = if let Some(editor) = panel.as_editor_mut() {
                    editor.take_config_update()
                } else {
                    None
                };

                (events, modal_request, config_update)
            } else {
                (vec![], None, None)
            };

        // Process panel events (new event-based architecture)
        self.process_panel_events(events)?;

        // Apply config update if present (legacy, still used by Editor)
        if let Some(new_config) = config_update {
            self.state.config = new_config.clone();
            self.state.set_theme(&new_config.general.theme);
            self.state.set_info("Config saved and applied".to_string());
        }

        // Handle modal window request from panel (legacy, still used)
        if let Some((action, modal)) = modal_request {
            self.handle_modal_request(action, modal)?;
        }

        Ok(())
    }

    /// Handle modal request from panel
    fn handle_modal_request(
        &mut self,
        mut action: PendingAction,
        mut modal: ActiveModal,
    ) -> Result<()> {
        // Update panel_index for actions that need it (placeholder value)
        match &mut action {
            PendingAction::CreateFile { panel_index, .. }
            | PendingAction::CreateDirectory { panel_index, .. }
            | PendingAction::DeletePath { panel_index, .. }
            | PendingAction::CopyPath { panel_index, .. }
            | PendingAction::MovePath { panel_index, .. }
            | PendingAction::SaveFileAs { panel_index, .. }
            | PendingAction::ClosePanel { panel_index }
            | PendingAction::CloseEditorWithSave { panel_index }
            | PendingAction::CloseEditorExternal { panel_index }
            | PendingAction::CloseEditorConflict { panel_index }
            | PendingAction::OverwriteDecision { panel_index, .. } => {
                *panel_index = 0; // Placeholder value, not used with LayoutManager
            }
            PendingAction::BatchFileOperation { .. }
            | PendingAction::ContinueBatchOperation { .. }
            | PendingAction::RenameWithPattern { .. }
            | PendingAction::Search
            | PendingAction::Replace
            | PendingAction::NextPanel
            | PendingAction::PrevPanel
            | PendingAction::QuitApplication => {
                // These actions don't require panel_index update
            }
        }

        // For Copy/Move - find all other FM panels and create suggestions
        let is_copy = matches!(action, PendingAction::CopyPath { .. });

        match &mut action {
            PendingAction::CopyPath {
                target_directory,
                sources,
                ..
            }
            | PendingAction::MovePath {
                target_directory,
                sources,
                ..
            } => {
                if target_directory.is_none() && !sources.is_empty() {
                    modal = self.prepare_copy_move_modal(sources, target_directory, is_copy);
                }
            }
            _ => {}
        }

        // Handle navigation actions without modal window
        match action {
            PendingAction::NextPanel => {
                self.layout_manager.next_group();
                return Ok(());
            }
            PendingAction::PrevPanel => {
                self.layout_manager.prev_group();
                return Ok(());
            }
            _ => {}
        }

        self.state.set_pending_action(action, modal);

        // Check if there's a channel receiver for directory size in panel
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(fm) = panel.as_file_manager_mut() {
                if let Some(rx) = fm.dir_size_receiver.take() {
                    self.state.dir_size_receiver = Some(rx);
                }
            }
        }

        Ok(())
    }

    /// Prepare modal for copy/move operations
    fn prepare_copy_move_modal(
        &mut self,
        sources: &[std::path::PathBuf],
        target_directory: &mut Option<std::path::PathBuf>,
        is_copy: bool,
    ) -> ActiveModal {
        // Get source directory to exclude from default selection
        let source_dir = sources[0].parent().map(|p| p.to_path_buf());
        let source_dir_str = source_dir.as_ref().map(|p| p.display().to_string());

        // Find all unique paths from other panels
        let options = self.find_all_other_panel_paths();
        let unique_paths_count = options.len();

        // Filter out source directory for default selection
        let default_dir = options
            .iter()
            .find(|opt| source_dir_str.as_ref() != Some(&opt.value))
            .map(|opt| std::path::PathBuf::from(&opt.value))
            .or_else(|| source_dir.clone())
            .unwrap_or_else(|| std::path::PathBuf::from("/"));

        *target_directory = Some(default_dir.clone());

        let default_dest = format!("{}/", default_dir.display());

        // Prepare title and prompt
        let t = i18n::t();
        let (title, prompt) = if sources.len() == 1 {
            let source_name = sources[0]
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?");

            if is_copy {
                (
                    t.modal_copy_single_title(source_name),
                    t.modal_copy_single_prompt(source_name),
                )
            } else {
                (
                    t.modal_move_single_title(source_name),
                    t.modal_move_single_prompt(source_name),
                )
            }
        } else if is_copy {
            (
                t.modal_copy_multiple_title(sources.len()),
                t.modal_copy_multiple_prompt(sources.len()),
            )
        } else {
            (
                t.modal_move_multiple_title(sources.len()),
                t.modal_move_multiple_prompt(sources.len()),
            )
        };

        // Choose modal based on number of unique paths
        if unique_paths_count >= 2 {
            let new_modal =
                termide_modal::EditableSelectModal::new(title, prompt, &default_dest, options);
            ActiveModal::EditableSelect(Box::new(new_modal))
        } else {
            let new_modal = termide_modal::InputModal::with_default(title, prompt, &default_dest);
            ActiveModal::Input(Box::new(new_modal))
        }
    }
}
