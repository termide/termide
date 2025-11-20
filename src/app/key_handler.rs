use anyhow::Result;
use crossterm::event::KeyCode;
use std::path::PathBuf;

use crate::{
    i18n,
    panels::{
        debug::Debug,
        editor::Editor,
        file_manager::FileManager,
        terminal_pty::Terminal,
        welcome::Welcome,
    },
    state::{ActiveModal, LayoutMode, PendingAction},
    ui::menu::MENU_ITEM_COUNT,
};
use super::App;

impl App {
    /// Handle keyboard event
    pub(super) fn handle_key_event(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        // Translate Cyrillic to Latin for hotkeys
        let key = crate::keyboard::translate_hotkey(key);

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
        if let Some(_) = self.handle_global_hotkeys(key)? {
            return Ok(());
        }

        // Pass event to active panel and collect results
        let (file_to_open, modal_request, config_update) = if let Some(panel) = self.panels.get_mut(self.state.active_panel) {
            panel.handle_key(key)?;

            // Collect results from panel
            let file_to_open = panel.take_file_to_open();
            let modal_request = panel.take_modal_request();

            // Check config update (only for Editor)
            let config_update = {
                use std::any::Any;
                use crate::panels::editor::Editor;
                if let Some(editor) = (&mut **panel as &mut dyn Any).downcast_mut::<Editor>() {
                    editor.take_config_update()
                } else {
                    None
                }
            };

            (file_to_open, modal_request, config_update)
        } else {
            (None, None, None)
        };

        // Apply config update if present
        if let Some(new_config) = config_update {
            self.state.config = new_config.clone();
            self.state.set_theme(&new_config.theme);
            self.state.set_info("Config saved and applied".to_string());
        }

        // Handle file opening in editor
        if let Some(file_path) = file_to_open {
            self.close_welcome_panels();
            let filename = file_path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?");
            let t = i18n::t();
            self.state.log_info(format!("Attempting to open file: {}", filename));

            match Editor::open_file(file_path.clone()) {
                Ok(editor_panel) => {
                    self.panels.add_panel(Box::new(editor_panel));
                    let new_panel_index = self.panels.count().saturating_sub(1);
                    self.state.set_active_panel(new_panel_index);
                    self.state.log_success(format!("File '{}' opened in editor", filename));
                    self.state.set_info(t.editor_file_opened(filename));
                }
                Err(e) => {
                    let error_msg = t.status_error_open_file(filename, &e.to_string());
                    self.state.log_error(format!("Error opening '{}': {}", filename, e));
                    self.state.set_error(error_msg);
                }
            }
        }

        // Handle modal window
        if let Some((mut action, mut modal)) = modal_request {
            // Update panel_index in action
            match &mut action {
                PendingAction::CreateFile { panel_index, .. } |
                PendingAction::CreateDirectory { panel_index, .. } |
                PendingAction::DeletePath { panel_index, .. } |
                PendingAction::CopyPath { panel_index, .. } |
                PendingAction::MovePath { panel_index, .. } |
                PendingAction::SaveFileAs { panel_index, .. } |
                PendingAction::ClosePanel { panel_index } |
                PendingAction::CloseEditorWithSave { panel_index } |
                PendingAction::OverwriteDecision { panel_index, .. } => {
                    *panel_index = self.state.active_panel;
                }
                PendingAction::BatchFileOperation { .. } |
                PendingAction::ContinueBatchOperation { .. } |
                PendingAction::RenameWithPattern { .. } |
                PendingAction::Search |
                PendingAction::Replace |
                PendingAction::ReplaceStep2 { .. } |
                PendingAction::NextPanel |
                PendingAction::PrevPanel => {
                    // These actions don't require panel_index update
                    // since it's already set in BatchOperation or not used
                }
            }

            // For Copy/Move - find another FM panel and set target_directory
            let is_copy = matches!(action, PendingAction::CopyPath { .. });

            match &mut action {
                PendingAction::CopyPath { target_directory, sources, .. } |
                PendingAction::MovePath { target_directory, sources, .. } => {
                    if target_directory.is_none() && !sources.is_empty() {
                        // Find another FM panel
                        let other_fm_dir = self.find_other_fm_directory(self.state.active_panel);

                        // If no other FM found, use parent directory of first source
                        let default_dir = other_fm_dir.or_else(|| {
                            sources[0].parent().map(|p| p.to_path_buf())
                        });

                        if let Some(dir) = default_dir {
                            *target_directory = Some(dir.clone());

                            let default_dest = format!("{}/", dir.display());

                            // Recreate modal with new default
                            let t = i18n::t();
                            let new_modal = if sources.len() == 1 {
                                let source_name = sources[0].file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("?");

                                if is_copy {
                                    crate::ui::modal::InputModal::with_default(
                                        t.modal_copy_title(),
                                        &t.modal_copy_single_prompt(source_name),
                                        &default_dest,
                                    )
                                } else {
                                    crate::ui::modal::InputModal::with_default(
                                        t.modal_move_title(),
                                        &t.modal_move_single_prompt(source_name),
                                        &default_dest,
                                    )
                                }
                            } else {
                                if is_copy {
                                    crate::ui::modal::InputModal::with_default(
                                        t.modal_copy_title(),
                                        &t.modal_copy_multiple_prompt(sources.len()),
                                        &default_dest,
                                    )
                                } else {
                                    crate::ui::modal::InputModal::with_default(
                                        t.modal_move_title(),
                                        &t.modal_move_multiple_prompt(sources.len()),
                                        &default_dest,
                                    )
                                }
                            };

                            modal = ActiveModal::Input(Box::new(new_modal));
                        }
                    }
                }
                _ => {}
            }

            // Handle navigation actions without modal window
            match action {
                PendingAction::NextPanel => {
                    // Find next FM panel (cyclically)
                    let count = self.panels.count();
                    for i in 1..=count {
                        let idx = (self.state.active_panel + i) % count;
                        if self.is_file_manager_panel(idx) {
                            self.state.set_active_panel(idx);
                            break;
                        }
                    }
                    return Ok(());
                }
                PendingAction::PrevPanel => {
                    // Find previous FM panel (cyclically)
                    let count = self.panels.count();
                    for i in 1..=count {
                        let idx = (self.state.active_panel + count - i) % count;
                        if self.is_file_manager_panel(idx) {
                            self.state.set_active_panel(idx);
                            break;
                        }
                    }
                    return Ok(());
                }
                _ => {}
            }

            self.state.set_pending_action(action, modal);

            // Check if there's a channel receiver for directory size in panel
            if let Some(panel) = self.panels.get_mut(self.state.active_panel) {
                // Try to get FileManager and take channel receiver
                use crate::panels::file_manager::FileManager;
                use std::any::Any;

                if let Some(fm) = (&mut **panel as &mut dyn Any).downcast_mut::<FileManager>() {
                    if let Some(rx) = fm.dir_size_receiver.take() {
                        self.state.dir_size_receiver = Some(rx);
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle keyboard event in menu
    pub(super) fn handle_menu_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.close_menu();
            }
            KeyCode::Left => {
                self.state.prev_menu_item(MENU_ITEM_COUNT);
            }
            KeyCode::Right => {
                self.state.next_menu_item(MENU_ITEM_COUNT);
            }
            KeyCode::Enter => {
                self.execute_menu_action()?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Execute action for selected menu item
    pub(super) fn execute_menu_action(&mut self) -> Result<()> {
        if let Some(menu_index) = self.state.ui.selected_menu_item {
            match menu_index {
                0 => {
                    // Files - open new file manager panel
                    self.handle_new_file_manager()?;
                    self.state.close_menu();
                }
                1 => {
                    // Terminal - open new terminal panel
                    self.handle_new_terminal()?;
                    self.state.close_menu();
                }
                2 => {
                    // Editor - open new editor panel
                    self.handle_new_editor()?;
                    self.state.close_menu();
                }
                3 => {
                    // Debug - open debug panel
                    self.handle_new_debug()?;
                    self.state.close_menu();
                }
                4 => {
                    // Preferences - open config file in editor
                    self.state.close_menu();
                    self.open_config_in_editor()?;
                }
                5 => {
                    // Help - show help
                    self.state.close_menu();
                    self.handle_new_help()?;
                }
                6 => {
                    // Quit - exit
                    self.state.quit();
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Handle panel close request with confirmation if needed
    fn handle_close_panel_request(&mut self, panel_index: usize) -> Result<()> {
        // Cannot close FM (panel 0) in MultiPanel mode
        if panel_index == 0 && self.state.layout_mode == LayoutMode::MultiPanel {
            return Ok(()); // Ignore
        }

        // Check if confirmation is required before closing
        if let Some(panel) = self.panels.get(panel_index) {
            if let Some(_message) = panel.needs_close_confirmation() {
                // Check if panel is editor
                use std::any::Any;
                let panel_any: &dyn Any = &**panel;

                if panel_any.is::<Editor>() {
                    // For editor show window with three options
                    use crate::ui::modal::SelectModal;
                    let t = i18n::t();
                    let modal = SelectModal::single(
                        t.editor_close_unsaved(),
                        t.editor_close_unsaved_question(),
                        vec![
                            t.editor_save_and_close().to_string(),
                            t.editor_close_without_saving().to_string(),
                            t.editor_cancel().to_string(),
                        ],
                    );
                    let action = PendingAction::CloseEditorWithSave { panel_index };
                    self.state.set_pending_action(action, ActiveModal::Select(Box::new(modal)));
                    return Ok(());
                } else {
                    // For other panels show simple confirmation
                    let t = i18n::t();
                    let modal = crate::ui::modal::ConfirmModal::new(
                        t.modal_yes(),
                        &_message,
                    );
                    let action = PendingAction::ClosePanel { panel_index };
                    self.state.set_pending_action(action, ActiveModal::Confirm(Box::new(modal)));
                    return Ok(());
                }
            }
        }

        // Close panel without confirmation
        self.close_panel_at_index(panel_index);
        Ok(())
    }

    /// Create new terminal
    fn handle_new_terminal(&mut self) -> Result<()> {
        self.close_welcome_panels();
        // Get directory from FM (panel 0)
        let working_dir = self.panels.get(0).and_then(|p| p.get_working_directory());

        // Create new terminal
        let width = self.state.terminal.width;
        let height = self.state.terminal.height;
        let term_height = height.saturating_sub(3);
        let term_width = if self.state.layout_mode == LayoutMode::MultiPanel {
            let fm_width = self.state.layout_info.fm_width.unwrap_or(30);
            width.saturating_sub(fm_width).saturating_sub(2)
        } else {
            width.saturating_sub(2)
        };

        if let Ok(terminal_panel) = Terminal::new_with_cwd(term_height, term_width, working_dir) {
            self.panels.add_panel(Box::new(terminal_panel));
            let new_panel_index = self.panels.count().saturating_sub(1);
            self.state.set_active_panel(new_panel_index);
        }
        Ok(())
    }

    /// Create new file manager
    fn handle_new_file_manager(&mut self) -> Result<()> {
        self.close_welcome_panels();
        // Get working directory from current panel
        let working_dir = self.panels.get(self.state.active_panel)
            .and_then(|p| p.get_working_directory())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));

        let fm_panel = FileManager::new_with_path(working_dir);
        self.panels.add_panel(Box::new(fm_panel));
        let new_panel_index = self.panels.count().saturating_sub(1);
        self.state.set_active_panel(new_panel_index);
        Ok(())
    }

    /// Create new editor
    fn handle_new_editor(&mut self) -> Result<()> {
        self.close_welcome_panels();
        let editor_panel = Editor::new();
        self.panels.add_panel(Box::new(editor_panel));
        let new_panel_index = self.panels.count().saturating_sub(1);
        self.state.set_active_panel(new_panel_index);
        Ok(())
    }

    /// Create new debug panel
    fn handle_new_debug(&mut self) -> Result<()> {
        self.close_welcome_panels();
        let debug_panel = Debug::new();
        self.panels.add_panel(Box::new(debug_panel));
        let new_panel_index = self.panels.count().saturating_sub(1);
        self.state.set_active_panel(new_panel_index);
        Ok(())
    }

    /// Open or switch to help panel (Welcome)
    fn handle_new_help(&mut self) -> Result<()> {
        // Check if Welcome panel already exists
        for i in 0..self.panels.count() {
            if let Some(panel) = self.panels.get(i) {
                if panel.is_welcome_panel() {
                    // Already exists - just switch focus
                    self.state.set_active_panel(i);
                    return Ok(());
                }
            }
        }

        // No Welcome - create new
        let welcome = Welcome::new();
        self.panels.add_panel(Box::new(welcome));
        let new_panel_index = self.panels.count().saturating_sub(1);
        self.state.set_active_panel(new_panel_index);
        Ok(())
    }

    /// Open config file in editor
    fn open_config_in_editor(&mut self) -> Result<()> {
        use crate::config::Config;
        use crate::panels::editor::Editor;

        let config_path = match Config::config_file_path() {
            Ok(path) => path,
            Err(e) => {
                self.state.set_error(format!("Failed to get config path: {}", e));
                return Ok(());
            }
        };

        // Check if config file is already open in some editor
        for i in 0..self.panels.count() {
            if let Some(panel) = self.panels.get(i) {
                // Check if panel is editor with this file
                use std::any::Any;
                if let Some(editor) = (&**panel as &dyn Any).downcast_ref::<Editor>() {
                    if editor.file_path() == Some(&config_path) {
                        // File already open - just switch focus
                        self.state.set_active_panel(i);
                        return Ok(());
                    }
                }
            }
        }

        self.close_welcome_panels();

        match Editor::open_file(config_path.clone()) {
            Ok(editor_panel) => {
                self.panels.add_panel(Box::new(editor_panel));
                let new_panel_index = self.panels.count().saturating_sub(1);
                self.state.set_active_panel(new_panel_index);
            }
            Err(e) => {
                self.state.set_error(format!("Failed to open config: {}", e));
            }
        }

        Ok(())
    }

    /// Check if panel is FileManager
    fn is_file_manager_panel(&self, index: usize) -> bool {
        if let Some(panel) = self.panels.get(index) {
            use std::any::Any;
            (&**panel as &dyn Any).is::<FileManager>()
        } else {
            false
        }
    }

    /// Close all Welcome panels (called before opening new panel)
    fn close_welcome_panels(&mut self) {
        // Collect welcome panel indices (in reverse order for correct removal)
        let welcome_indices: Vec<usize> = (0..self.panels.count())
            .filter(|&i| {
                if let Some(panel) = self.panels.get(i) {
                    panel.is_welcome_panel()
                } else {
                    false
                }
            })
            .collect();

        // Remove in reverse order so indices don't shift
        for index in welcome_indices.into_iter().rev() {
            self.panels.close_panel(index);
        }

        // Adjust active_panel if needed
        if self.state.active_panel >= self.panels.count() && self.panels.count() > 0 {
            self.state.set_active_panel(self.panels.count() - 1);
        }
    }

    /// Move panel left (swap with previous)
    fn handle_swap_panel_left(&mut self) -> Result<()> {
        let current = self.state.active_panel;
        // FM (panel 0) is fixed, so swap is only possible if current > 1
        if current > 1 {
            self.panels.swap_panels(current, current - 1);
            self.state.set_active_panel(current - 1);
        }
        Ok(())
    }

    /// Move panel right (swap with next)
    fn handle_swap_panel_right(&mut self) -> Result<()> {
        let current = self.state.active_panel;
        // FM (panel 0) is fixed
        if current > 0 && current < self.panels.count() - 1 {
            self.panels.swap_panels(current, current + 1);
            self.state.set_active_panel(current + 1);
        }
        Ok(())
    }

    /// Change active panel width
    fn handle_resize_panel(&mut self, delta: i16) -> Result<()> {
        let current = self.state.active_panel;
        self.state.adjust_panel_weight(current, delta);
        Ok(())
    }

    /// Handle global hotkeys
    fn handle_global_hotkeys(&mut self, key: crossterm::event::KeyEvent) -> Result<Option<()>> {
        use crossterm::event::{KeyCode, KeyModifiers};

        // All hotkeys now use Alt
        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                // Alt+M - activate/deactivate menu
                KeyCode::Char('m') | KeyCode::Char('M') => {
                    self.state.toggle_menu();
                    return Ok(Some(()));
                }
                // Alt+F - open Files panel
                KeyCode::Char('f') | KeyCode::Char('F') => {
                    self.handle_new_file_manager()?;
                    return Ok(Some(()));
                }
                // Alt+T - open Terminal
                KeyCode::Char('t') | KeyCode::Char('T') => {
                    self.handle_new_terminal()?;
                    return Ok(Some(()));
                }
                // Alt+E - open Editor
                KeyCode::Char('e') | KeyCode::Char('E') => {
                    self.handle_new_editor()?;
                    return Ok(Some(()));
                }
                // Alt+D - open Debug
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    self.handle_new_debug()?;
                    return Ok(Some(()));
                }
                // Alt+P - open Preferences (config file in editor)
                KeyCode::Char('p') | KeyCode::Char('P') => {
                    self.open_config_in_editor()?;
                    return Ok(Some(()));
                }
                // Alt+H - open Help
                KeyCode::Char('h') | KeyCode::Char('H') => {
                    self.handle_new_help()?;
                    return Ok(Some(()));
                }
                // Alt+Q - quit
                KeyCode::Char('q') | KeyCode::Char('Q') => {
                    self.state.quit();
                    return Ok(Some(()));
                }
                // Alt+X / Alt+Backspace - close panel
                KeyCode::Char('x') | KeyCode::Char('X') | KeyCode::Backspace => {
                    self.handle_close_panel_request(self.state.active_panel)?;
                    return Ok(Some(()));
                }
                // Alt+Delete - close application
                KeyCode::Delete => {
                    self.state.quit();
                    return Ok(Some(()));
                }
                // Alt+1..9 - go to panel by number
                KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                    let panel_num = c.to_digit(10).unwrap() as usize;
                    if panel_num <= self.panels.count() {
                        self.state.set_active_panel(panel_num - 1);
                    }
                    return Ok(Some(()));
                }
                // Alt+Left or Alt+, - go to previous panel
                KeyCode::Left | KeyCode::Char(',') | KeyCode::Char('<') => {
                    self.state.prev_panel(self.panels.count());
                    return Ok(Some(()));
                }
                // Alt+Right or Alt+. - go to next panel
                KeyCode::Right | KeyCode::Char('.') | KeyCode::Char('>') => {
                    self.state.next_panel(self.panels.count());
                    return Ok(Some(()));
                }
                // Alt+PgUp - move panel left (swap)
                KeyCode::PageUp => {
                    self.handle_swap_panel_left()?;
                    return Ok(Some(()));
                }
                // Alt+PgDn - move panel right (swap)
                KeyCode::PageDown => {
                    self.handle_swap_panel_right()?;
                    return Ok(Some(()));
                }
                // Alt+Minus - decrease panel width
                KeyCode::Char('-') => {
                    self.handle_resize_panel(-1)?;
                    return Ok(Some(()));
                }
                // Alt+Plus - increase panel width
                KeyCode::Char('+') | KeyCode::Char('=') => {
                    self.handle_resize_panel(1)?;
                    return Ok(Some(()));
                }
                _ => {}
            }
        }

        // Escape - close panel (without modifiers)
        // But if panel captures Escape (e.g., terminal with running program),
        // then Escape is passed to panel
        if key.code == KeyCode::Esc && key.modifiers.is_empty() {
            let captures = self.panels.get(self.state.active_panel)
                .map(|p| p.captures_escape())
                .unwrap_or(false);
            if !captures {
                self.handle_close_panel_request(self.state.active_panel)?;
                return Ok(Some(()));
            }
            // Otherwise Escape is passed to panel's handle_key
        }

        Ok(None) // Not handled, pass to panel
    }
}
