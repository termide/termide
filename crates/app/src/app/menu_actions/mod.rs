//! Menu actions and panel creation for the application.
//!
//! Handles menu navigation and creating new panels.

// Note: PanelExt is used for editor save operations that require concrete type access.

mod ai;
mod bookmarks;
mod command_palette;
mod commands;
mod commands_exec;
mod operation_action;
mod outline;
mod panel_action;
mod projects;
mod settings;
mod stash;
mod tools;

use anyhow::Result;
use crossterm::event::KeyCode;

use super::parked_projects::ProjectStep;
use super::App;
use crate::state::{ActiveModal, PendingAction, ProjectsOrigin};
use termide_i18n as i18n;
use termide_theme::Theme;
use termide_ui_render::menu::{
    project_button_of, AI_MENU_INDEX, BOOKMARKS_MENU_INDEX, COMMANDS_MENU_INDEX,
    INDICATOR_CLOCK_INDEX, INDICATOR_CPU_INDEX, INDICATOR_DISK_INDEX, INDICATOR_NET_INDEX,
    INDICATOR_RAM_INDEX, OPTIONS_MENU_INDEX, PROJECTS_MENU_INDEX, PROJECT_BUTTON_BASE,
    WINDOWS_MENU_INDEX,
};
use termide_ui_render::{OPTIONS_SUBMENU_LANGUAGE, OPTIONS_SUBMENU_THEMES};

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
        // The hotkey table holds the commands' keys too: a file edited by
        // hand is read here, and its keys must take effect with it.
        self.state.cache.hotkey_table = None;
        reg
    }

    /// Switch to next root menu item and open its submenu
    pub(super) fn switch_to_next_menu(&mut self) -> Result<()> {
        self.state.ui.close_all_submenus();
        self.step_menu_item(true);
        self.execute_menu_action()
    }

    /// Switch to previous root menu item and open its submenu
    pub(super) fn switch_to_prev_menu(&mut self) -> Result<()> {
        self.state.ui.close_all_submenus();
        self.step_menu_item(false);
        self.execute_menu_action()
    }

    /// Move the menu bar selection to the next (or previous) position in
    /// the order the bar shows them, the project buttons on screen included.
    fn step_menu_item(&mut self, forward: bool) {
        let order = self.menu_bar().nav_order();
        self.state.step_menu_item(&order, forward);
    }

    /// Handle keyboard event in menu
    pub(super) fn handle_menu_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        let button = self.state.ui.selected_menu_item.and_then(project_button_of);
        if let Some(button) = button {
            if let Some(step) = self.project_step_of(&key) {
                self.move_project_button(button, step);
                return Ok(());
            }
            if matches!(key.code, KeyCode::Delete | KeyCode::F(8)) {
                if let Some(root) = self.listed_open_roots().get(button).cloned() {
                    self.state.close_menu();
                    self.confirm_close_project(root, ProjectsOrigin::Button(button));
                }
                return Ok(());
            }
        }
        match key.code {
            KeyCode::Esc => {
                self.state.close_menu();
            }
            KeyCode::Left => {
                self.step_menu_item(false);
                self.execute_menu_action()?;
            }
            KeyCode::Right => {
                self.step_menu_item(true);
                self.execute_menu_action()?;
            }
            KeyCode::Enter => {
                // Enter on a project button switches to its project; moving
                // onto one only selects it.
                if let Some(index) = self.state.ui.selected_menu_item.and_then(project_button_of) {
                    self.state.close_menu();
                    return self.switch_to_open_project(index);
                }
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
                PROJECTS_MENU_INDEX => {
                    self.state.open_projects_submenu();
                }
                WINDOWS_MENU_INDEX => {
                    self.state.open_tools_submenu();
                }
                COMMANDS_MENU_INDEX => {
                    self.state.open_commands_submenu();
                }
                AI_MENU_INDEX => {
                    self.state.open_ai_submenu();
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
                _ => {
                    if let Some(index) = project_button_of(menu_index) {
                        self.select_project_button(index);
                    }
                }
            }
        }
        Ok(())
    }

    /// Select project button `index`: nothing opens, the status bar shows
    /// the project's full path, which its button may have cut.
    fn select_project_button(&mut self, index: usize) {
        self.state.close_indicator_modal();
        if let Some(view) = self.state.open_projects.get(index) {
            let path = termide_core::util::shorten_home_path(&view.root.display().to_string());
            self.state.set_info(path);
        }
        self.state.needs_redraw = true;
    }

    /// Move the project of button `index` along the bar, the selection with
    /// it.
    fn move_project_button(&mut self, index: usize, step: ProjectStep) {
        let Some(root) = self.listed_open_roots().get(index).cloned() else {
            return;
        };
        if let Some(place) = self.move_open_project(&root, step) {
            self.state.ui.selected_menu_item = Some(PROJECT_BUTTON_BASE + place);
            self.select_project_button(place);
        }
    }

    /// Open the menu bar at project button `index`, or the last one when
    /// fewer are left, after a project was closed from it. With one project
    /// left there are no buttons, and the menu stays closed.
    pub(in crate::app) fn reopen_menu_bar_at_button(&mut self, index: usize) {
        let count = self.open_projects.count();
        if count < 2 {
            return;
        }
        let index = index.min(count - 1);
        self.state.open_menu(Some(PROJECT_BUTTON_BASE + index));
        self.select_project_button(index);
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

        let bar = self.menu_bar();
        let anchor_x = match menu_index {
            INDICATOR_NET_INDEX => bar.net.start,
            INDICATOR_CPU_INDEX => bar.cpu.start,
            INDICATOR_RAM_INDEX => bar.ram.start,
            INDICATOR_CLOCK_INDEX => bar.clock.start,
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

        let item_count = termide_ui_render::get_options_items(
            self.detach_available(),
            Some(&self.state.config.general.keybindings),
        )
        .len();

        match navigate_submenu(&key, &mut self.state.ui.options_submenu, item_count, &[]) {
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
    pub(in crate::app) fn execute_submenu_action(&mut self) -> Result<()> {
        // Dispatch on the item's key, not its position: the Detach entry is
        // only present in a detachable instance, so a positional match would
        // fire Quit where Detach was chosen.
        let items = termide_ui_render::get_options_items(
            self.detach_available(),
            Some(&self.state.config.general.keybindings),
        );
        let Some(key) = items
            .get(self.state.ui.options_submenu.selected)
            .map(|item| item.key.clone())
        else {
            return Ok(());
        };

        match key.as_str() {
            "themes" => {
                let theme_names = Theme::all_theme_names();
                let current_idx = theme_names
                    .iter()
                    .position(|n| n == self.state.theme.name)
                    .unwrap_or(0);
                self.state.ui.theme_preview_original = Some(self.state.theme.name.to_string());
                self.state.open_nested_submenu(current_idx);
            }
            "language" => {
                use termide_ui_render::find_current_language_index;
                let current_idx = find_current_language_index();
                self.state.ui.language_preview_original = Some(i18n::current_language());
                self.state.open_nested_submenu(current_idx);
            }
            "edit_preferences" => {
                self.state.close_menu();
                self.open_settings_modal();
            }
            "help" => {
                self.state.close_menu();
                self.handle_new_help()?;
            }
            "detach_instance" => {
                self.state.close_menu();
                self.handle_detach_instance();
            }
            "quit" => {
                self.state.close_menu();
                if let Some(message) = self.quit_confirmation() {
                    let t = i18n::t();
                    let modal = termide_modal::ConfirmModal::new(t.app_quit_title(), message);
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
