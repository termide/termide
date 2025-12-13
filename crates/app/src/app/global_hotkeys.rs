//! Global hotkey handling for the application.
//!
//! Uses the HotkeyProcessor trait to handle Alt+key combinations
//! for navigation, panel management, and quick actions.

use anyhow::Result;
use crossterm::event::KeyEvent;

use termide_app_event::{HotkeyAction, HotkeyProcessor};

use super::App;
use crate::state::{ActiveModal, PendingAction};
use termide_i18n as i18n;

impl App {
    /// Handle global hotkeys (Alt+key combinations)
    ///
    /// Returns `Some(())` if the hotkey was handled, `None` to pass to panel.
    pub(super) fn handle_global_hotkeys(&mut self, key: KeyEvent) -> Result<Option<()>> {
        // Check if this is a global hotkey
        if let Some(action) = self.hotkey_processor.process_hotkey(&key) {
            self.execute_hotkey_action(action)?;
            return Ok(Some(()));
        }

        // Escape - close panel (without modifiers)
        let captures = self
            .layout_manager
            .active_panel_mut()
            .map(|p| p.captures_escape())
            .unwrap_or(false);

        if self.hotkey_processor.should_escape_close(&key, captures) {
            self.handle_close_panel_request(0)?;
            return Ok(Some(()));
        }

        Ok(None) // Not handled, pass to panel
    }

    /// Execute a hotkey action
    fn execute_hotkey_action(&mut self, action: HotkeyAction) -> Result<()> {
        match action {
            // Menu
            HotkeyAction::ToggleMenu => {
                self.state.toggle_menu();
            }

            // Panel creation
            HotkeyAction::NewFileManager => {
                self.handle_new_file_manager()?;
            }
            HotkeyAction::NewTerminal => {
                self.handle_new_terminal()?;
            }
            HotkeyAction::NewEditor => {
                self.handle_new_editor()?;
            }
            HotkeyAction::NewDebug => {
                self.handle_new_debug()?;
            }
            HotkeyAction::OpenHelp => {
                self.handle_new_help()?;
            }
            HotkeyAction::OpenPreferences => {
                self.open_config_in_editor()?;
            }

            // Navigation
            HotkeyAction::PrevGroup => {
                self.navigate_to_prev_group();
            }
            HotkeyAction::NextGroup => {
                self.navigate_to_next_group();
            }
            HotkeyAction::PrevInGroup => {
                self.navigate_to_prev_panel_in_group();
            }
            HotkeyAction::NextInGroup => {
                self.navigate_to_next_panel_in_group();
            }
            HotkeyAction::GoToPanel(panel_num) => {
                self.navigate_to_group(panel_num);
            }

            // Panel management
            HotkeyAction::ClosePanel => {
                self.handle_close_panel_request(0)?;
            }
            HotkeyAction::ToggleStacking => {
                self.toggle_panel_stacking();
            }
            HotkeyAction::SwapPanelLeft => {
                self.handle_swap_panel_left()?;
            }
            HotkeyAction::SwapPanelRight => {
                self.handle_swap_panel_right()?;
            }
            HotkeyAction::MoveToFirst => {
                self.move_panel_to_first();
            }
            HotkeyAction::MoveToLast => {
                self.move_panel_to_last();
            }
            HotkeyAction::ResizePanel(delta) => {
                self.handle_resize_panel(delta)?;
            }

            // Application
            HotkeyAction::RequestQuit => {
                self.handle_quit_request()?;
            }
        }
        Ok(())
    }

    /// Handle quit request with confirmation if needed
    pub(super) fn handle_quit_request(&mut self) -> Result<()> {
        // Always save session before quit
        self.auto_save_session();

        if self.has_panels_requiring_confirmation() {
            let t = i18n::t();
            let modal = termide_modal::ConfirmModal::new(t.modal_yes(), t.app_quit_confirm());
            self.state.set_pending_action(
                PendingAction::QuitApplication,
                ActiveModal::Confirm(Box::new(modal)),
            );
        } else {
            self.state.quit();
        }
        Ok(())
    }

    /// Check if session should be saved and save if needed
    fn check_and_save_session(&mut self) {
        if self.state.should_save_session() {
            self.auto_save_session();
            self.state.update_last_session_save();
        }
    }

    /// Navigate to previous group with session save
    fn navigate_to_prev_group(&mut self) {
        self.layout_manager.prev_group();
        self.check_and_save_session();
    }

    /// Navigate to next group with session save
    fn navigate_to_next_group(&mut self) {
        self.layout_manager.next_group();
        self.check_and_save_session();
    }

    /// Navigate to previous panel in group with session save
    fn navigate_to_prev_panel_in_group(&mut self) {
        self.layout_manager.prev_panel_in_group();
        self.check_and_save_session();
    }

    /// Navigate to next panel in group with session save
    fn navigate_to_next_panel_in_group(&mut self) {
        self.layout_manager.next_panel_in_group();
        self.check_and_save_session();
    }

    /// Navigate to specific group by number (1-indexed)
    fn navigate_to_group(&mut self, group_num: usize) {
        // Convert from 1-indexed (user-facing) to 0-indexed (internal)
        let index = group_num.saturating_sub(1);
        self.layout_manager.set_focus(index);
        self.check_and_save_session();
    }

    /// Toggle panel stacking mode
    fn toggle_panel_stacking(&mut self) {
        let terminal_width = self.state.terminal.width;
        if let Err(e) = self.layout_manager.toggle_panel_stacking(terminal_width) {
            self.state
                .set_error(format!("Cannot toggle stacking: {}", e));
        } else {
            self.auto_save_session();
        }
    }

    /// Move panel to first group
    fn move_panel_to_first(&mut self) {
        let terminal_width = self.state.terminal.width;
        if let Err(e) = self
            .layout_manager
            .move_panel_to_first_group(terminal_width)
        {
            self.state.set_error(format!("Cannot move panel: {}", e));
        } else {
            self.auto_save_session();
        }
    }

    /// Move panel to last group
    fn move_panel_to_last(&mut self) {
        let terminal_width = self.state.terminal.width;
        if let Err(e) = self.layout_manager.move_panel_to_last_group(terminal_width) {
            self.state.set_error(format!("Cannot move panel: {}", e));
        } else {
            self.auto_save_session();
        }
    }
}
