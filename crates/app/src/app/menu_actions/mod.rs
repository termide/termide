//! Menu actions and panel creation for the application.
//!
//! Handles menu navigation and creating new panels.

// Note: PanelExt is used for editor save operations that require concrete type access.
#![allow(deprecated)]

mod bookmarks;
mod command_palette;
mod commands;
mod commands_exec;
mod operation_action;
mod outline;
mod panel_action;
mod sessions;
mod settings;
mod stash;
mod tools;

use anyhow::Result;
use crossterm::event::KeyCode;

use super::App;
use crate::state::{ActiveModal, PendingAction};
use termide_i18n as i18n;
use termide_theme::Theme;
use termide_ui_render::menu::{
    BOOKMARKS_MENU_INDEX, COMMANDS_MENU_INDEX, INDICATOR_CLOCK_INDEX, INDICATOR_CPU_INDEX,
    INDICATOR_DISK_INDEX, INDICATOR_NET_INDEX, INDICATOR_RAM_INDEX, MENU_TOTAL_COUNT,
    OPTIONS_MENU_INDEX, SESSIONS_MENU_INDEX, WINDOWS_MENU_INDEX,
};
use termide_ui_render::{
    OPTIONS_SUBMENU_HELP, OPTIONS_SUBMENU_LANGUAGE, OPTIONS_SUBMENU_PREFERENCES,
    OPTIONS_SUBMENU_QUIT, OPTIONS_SUBMENU_THEMES,
};

/// Result of generic submenu keyboard navigation.
enum SubmenuNavAction {
    /// User pressed Esc — close submenu
    Close,
    /// User pressed Enter — execute selected action
    Execute,
    /// User pressed Right — open submenu or go to next root menu
    Right,
    /// User pressed Left — close nested or go to prev root menu
    Left,
    /// User pressed F2 — rename selected item
    Rename,
    /// User pressed F4 — edit selected item
    Edit,
    /// User pressed Delete — delete selected item
    Delete,
    /// Navigation handled (Up/Down) or no-op
    None,
}

/// Handle generic submenu keyboard navigation.
/// Updates selection on Up/Down and returns the action for Esc/Enter.
/// `separators` lists indices of separator items that should be skipped.
fn navigate_submenu(
    key: &crossterm::event::KeyEvent,
    submenu: &mut termide_state::SubmenuState,
    item_count: usize,
    separators: &[usize],
) -> SubmenuNavAction {
    match key.code {
        KeyCode::Esc => SubmenuNavAction::Close,
        KeyCode::Left => SubmenuNavAction::Left,
        KeyCode::Up => {
            for _ in 0..item_count {
                submenu.select_prev(item_count);
                if !separators.contains(&submenu.selected) {
                    break;
                }
            }
            SubmenuNavAction::None
        }
        KeyCode::Down => {
            for _ in 0..item_count {
                submenu.select_next(item_count);
                if !separators.contains(&submenu.selected) {
                    break;
                }
            }
            SubmenuNavAction::None
        }
        KeyCode::Enter => SubmenuNavAction::Execute,
        KeyCode::Right => SubmenuNavAction::Right,
        KeyCode::F(2) => SubmenuNavAction::Rename,
        KeyCode::F(4) => SubmenuNavAction::Edit,
        KeyCode::Delete | KeyCode::F(8) => SubmenuNavAction::Delete,
        _ => SubmenuNavAction::None,
    }
}

impl App {
    /// Get cached CommandsRegistry, loading from disk on first access.
    pub(super) fn commands_registry(
        &mut self,
    ) -> Option<termide_config::commands::CommandsRegistry> {
        if let Some(ref reg) = self.state.cache.commands_registry {
            return Some(reg.clone());
        }
        let reg = termide_config::commands::CommandsRegistry::load_merged(Some(&self.project_root));
        self.state.cache.commands_registry = reg.clone();
        reg
    }

    /// Switch to next root menu item and open its submenu
    pub(super) fn switch_to_next_menu(&mut self) -> Result<()> {
        self.state.ui.close_all_submenus();
        self.state.next_menu_item(MENU_TOTAL_COUNT);
        self.execute_menu_action()
    }

    /// Switch to previous root menu item and open its submenu
    pub(super) fn switch_to_prev_menu(&mut self) -> Result<()> {
        self.state.ui.close_all_submenus();
        self.state.prev_menu_item(MENU_TOTAL_COUNT);
        self.execute_menu_action()
    }

    /// Handle keyboard event in menu
    pub(super) fn handle_menu_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.close_menu();
            }
            KeyCode::Left => {
                self.state.prev_menu_item(MENU_TOTAL_COUNT);
                self.execute_menu_action()?;
            }
            KeyCode::Right => {
                self.state.next_menu_item(MENU_TOTAL_COUNT);
                self.execute_menu_action()?;
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
                SESSIONS_MENU_INDEX => {
                    self.state.open_sessions_submenu();
                }
                WINDOWS_MENU_INDEX => {
                    self.state.open_tools_submenu();
                }
                COMMANDS_MENU_INDEX => {
                    self.state.open_commands_submenu();
                }
                BOOKMARKS_MENU_INDEX => {
                    self.state.open_bookmarks_submenu();
                }
                OPTIONS_MENU_INDEX => {
                    self.state.open_submenu();
                }
                INDICATOR_NET_INDEX
                | INDICATOR_CPU_INDEX
                | INDICATOR_RAM_INDEX
                | INDICATOR_CLOCK_INDEX
                | INDICATOR_DISK_INDEX => {
                    self.open_indicator_as_submenu(menu_index);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Open an indicator modal positioned as a dropdown under the indicator.
    pub(super) fn open_indicator_as_submenu(&mut self, menu_index: usize) {
        self.state.close_indicator_modal();

        if menu_index == INDICATOR_DISK_INDEX {
            use crate::state::ResourceModalKind;
            let t = termide_i18n::t();
            let lines = self.build_disk_modal_lines();
            // Use terminal width as anchor — clamping in render will right-align the modal
            let anchor_x = self.state.terminal.width;
            // Bottom edge = status bar row (last row)
            let anchor_y = self.state.terminal.height.saturating_sub(1);
            let modal = termide_modal::InfoModal::new_rich(t.resource_disk_title(), lines)
                .without_button()
                .with_anchor_bottom(anchor_x, anchor_y);
            self.state.active_modal = Some(termide_modal::ActiveModal::Info(Box::new(modal)));
            self.state.resource_modal_kind = Some(ResourceModalKind::Disk);
            self.state.last_resource_modal_refresh = Some(std::time::Instant::now());
            self.state.needs_redraw = true;
            return;
        }

        let (net_range, cpu_range, ram_range, clock_range) = self.get_indicator_ranges();
        let anchor_x = match menu_index {
            INDICATOR_NET_INDEX => net_range.start,
            INDICATOR_CPU_INDEX => cpu_range.start,
            INDICATOR_RAM_INDEX => ram_range.start,
            INDICATOR_CLOCK_INDEX => clock_range.start,
            _ => 0,
        };

        if menu_index == INDICATOR_CLOCK_INDEX {
            let modal = termide_modal::CalendarModal::new().with_anchor(anchor_x, 1);
            self.state.active_modal = Some(termide_modal::ActiveModal::Calendar(Box::new(modal)));
            self.state.needs_redraw = true;
        } else {
            let kind = match menu_index {
                INDICATOR_NET_INDEX => crate::state::ResourceModalKind::Network,
                INDICATOR_CPU_INDEX => crate::state::ResourceModalKind::Cpu,
                INDICATOR_RAM_INDEX => crate::state::ResourceModalKind::Ram,
                _ => return,
            };
            self.open_resource_modal_at(kind, Some((anchor_x, 1)));
        }
    }

    /// Check if any panel requires close confirmation
    pub(super) fn has_panels_requiring_confirmation(&self) -> bool {
        // Check if any panel has unsaved changes or running processes
        for panel in self
            .layout_manager
            .panel_groups
            .iter()
            .flat_map(|g| g.panels().iter())
        {
            if panel.needs_close_confirmation().is_some() {
                return true;
            }
        }

        // Check if there's an active batch file operation
        #[allow(clippy::collapsible_match)]
        if let Some(pending) = &self.state.pending_action {
            match pending {
                PendingAction::BatchFileOperation { .. }
                | PendingAction::ContinueBatchOperation { .. } => {
                    return true;
                }
                _ => {}
            }
        }

        false
    }

    // =========================================================================
    // Submenu handling
    // =========================================================================

    /// Handle keyboard event in submenu (Options dropdown)
    pub(super) fn handle_submenu_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        // If nested submenu is open, delegate to nested handler
        if self.state.ui.nested_submenu.open {
            return self.handle_nested_submenu_key(key);
        }

        use termide_ui_render::OPTIONS_SUBMENU_ITEM_COUNT;

        match navigate_submenu(
            &key,
            &mut self.state.ui.options_submenu,
            OPTIONS_SUBMENU_ITEM_COUNT,
            &[],
        ) {
            SubmenuNavAction::Close => self.state.close_menu(),
            SubmenuNavAction::Execute => self.execute_submenu_action()?,
            SubmenuNavAction::Right => {
                let sel = self.state.ui.options_submenu.selected;
                if sel == OPTIONS_SUBMENU_THEMES || sel == OPTIONS_SUBMENU_LANGUAGE {
                    self.execute_submenu_action()?;
                } else {
                    self.switch_to_next_menu()?;
                }
            }
            SubmenuNavAction::Left => self.switch_to_prev_menu()?,
            SubmenuNavAction::Rename
            | SubmenuNavAction::Edit
            | SubmenuNavAction::Delete
            | SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// Execute action for selected Options submenu item
    fn execute_submenu_action(&mut self) -> Result<()> {
        match self.state.ui.options_submenu.selected {
            OPTIONS_SUBMENU_THEMES => {
                let theme_names = Theme::all_theme_names();
                let current_idx = theme_names
                    .iter()
                    .position(|n| n == self.state.theme.name)
                    .unwrap_or(0);
                self.state.ui.theme_preview_original = Some(self.state.theme.name.to_string());
                self.state.open_nested_submenu(current_idx);
            }
            OPTIONS_SUBMENU_LANGUAGE => {
                use termide_ui_render::find_current_language_index;
                let current_idx = find_current_language_index();
                self.state.ui.language_preview_original = Some(i18n::current_language());
                self.state.open_nested_submenu(current_idx);
            }
            OPTIONS_SUBMENU_PREFERENCES => {
                self.state.close_menu();
                self.open_settings_modal();
            }
            OPTIONS_SUBMENU_HELP => {
                self.state.close_menu();
                self.handle_new_help()?;
            }
            OPTIONS_SUBMENU_QUIT => {
                self.state.close_menu();
                if self.has_panels_requiring_confirmation() {
                    let t = i18n::t();
                    let modal =
                        termide_modal::ConfirmModal::new(t.app_quit_title(), t.app_quit_confirm());
                    self.state.set_pending_action(
                        PendingAction::QuitApplication,
                        ActiveModal::Confirm(Box::new(modal)),
                    );
                } else {
                    self.state.quit();
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle keyboard event in nested submenu (Themes or Language list)
    fn handle_nested_submenu_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        // Determine which nested submenu is open based on parent submenu item
        match self.state.ui.options_submenu.selected {
            OPTIONS_SUBMENU_THEMES => self.handle_themes_nested_submenu_key(key),
            OPTIONS_SUBMENU_LANGUAGE => self.handle_language_nested_submenu_key(key),
            _ => Ok(()),
        }
    }

    /// Navigate nested submenu selection up/down with wrapping.
    fn navigate_nested_submenu(&mut self, key_code: KeyCode, count: usize) {
        match key_code {
            KeyCode::Up => {
                if self.state.ui.nested_submenu.selected > 0 {
                    self.state.ui.nested_submenu.selected -= 1;
                } else {
                    self.state.ui.nested_submenu.selected = count.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if count > 0 {
                    self.state.ui.nested_submenu.selected =
                        (self.state.ui.nested_submenu.selected + 1) % count;
                }
            }
            _ => {}
        }
    }
}
