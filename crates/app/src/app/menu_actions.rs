//! Menu actions and panel creation for the application.
//!
//! Handles menu navigation and creating new panels.

// Note: PanelExt is used for editor save operations that require concrete type access.
#![allow(deprecated)]

use anyhow::Result;
use crossterm::event::KeyCode;
use std::path::PathBuf;

use super::App;
use crate::state::{ActiveModal, PendingAction};
use crate::PanelExt;
use termide_app_core::Panel;

/// Result of generic submenu keyboard navigation.
enum SubmenuNavAction {
    /// User pressed Esc/Left — close submenu
    Close,
    /// User pressed Enter/Right — execute selected action
    Execute,
    /// Navigation handled (Up/Down) or no-op
    None,
}

/// Handle generic submenu keyboard navigation.
/// Updates selection on Up/Down and returns the action for Esc/Enter.
fn navigate_submenu(
    key: &crossterm::event::KeyEvent,
    submenu: &mut termide_state::SubmenuState,
    item_count: usize,
) -> SubmenuNavAction {
    match key.code {
        KeyCode::Esc | KeyCode::Left => SubmenuNavAction::Close,
        KeyCode::Up => {
            submenu.select_prev(item_count);
            SubmenuNavAction::None
        }
        KeyCode::Down => {
            submenu.select_next(item_count);
            SubmenuNavAction::None
        }
        KeyCode::Right | KeyCode::Enter => SubmenuNavAction::Execute,
        _ => SubmenuNavAction::None,
    }
}
use termide_config::Config;
use termide_i18n as i18n;

use termide_panel_file_manager::FileManager;
use termide_panel_terminal::Terminal;
use termide_theme::Theme;
use termide_ui_render::menu::MENU_ITEM_COUNT;

impl App {
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
                    // Sessions - open submenu dropdown (keep menu open)
                    self.state.open_sessions_submenu();
                }
                1 => {
                    // Tools - open submenu dropdown (keep menu open)
                    self.state.open_tools_submenu();
                }
                2 => {
                    // Scripts - open submenu dropdown (keep menu open)
                    self.state.open_scripts_submenu();
                }
                3 => {
                    // Bookmarks - open submenu dropdown (keep menu open)
                    self.state.open_bookmarks_submenu();
                }
                4 => {
                    // Options - open submenu dropdown (keep menu open)
                    self.state.open_submenu();
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Open sessions modal to switch between projects
    pub(super) fn handle_open_sessions_modal(&mut self) -> Result<()> {
        use termide_modal::{SessionItem, SessionsModal};
        use termide_session::{format_relative_time, list_all_sessions};

        let t = i18n::t();

        // Get all sessions
        let sessions = list_all_sessions().unwrap_or_default();

        // Get current project path
        let current_project = std::env::current_dir().unwrap_or_default();

        // Convert to SessionItems
        let items: Vec<SessionItem> = sessions
            .into_iter()
            .map(|info| {
                let is_current = info.project_path == current_project;
                let display_path =
                    termide_core::util::shorten_home_path(&info.project_path.display().to_string());
                let relative_time = format_relative_time(info.modified);

                SessionItem {
                    project_path: info.project_path,
                    display_path,
                    relative_time,
                    is_current,
                }
            })
            .collect();

        // Only show modal if there are other sessions
        if items.iter().any(|item| !item.is_current) {
            // Find index of current session to position cursor there
            let current_idx = items.iter().position(|item| item.is_current).unwrap_or(0);
            let modal = SessionsModal::new(t.sessions_title(), items).with_cursor(current_idx);
            self.state.set_pending_action(
                PendingAction::SwitchSession,
                ActiveModal::Sessions(Box::new(modal)),
            );
        }

        Ok(())
    }

    /// Open directory switcher modal
    pub(super) fn handle_open_directory_switcher(&mut self) -> Result<()> {
        use termide_modal::{DirectoryItem, DirectorySwitcherModal};

        let t = i18n::t();

        // Check if active panel supports directory switching (Terminal or FileManager)
        let panel_supported = self
            .layout_manager
            .active_panel_mut()
            .map(|p| p.as_terminal_mut().is_some() || p.as_file_manager_mut().is_some())
            .unwrap_or(false);

        if !panel_supported {
            self.state
                .set_info(t.directory_switcher_unsupported().to_string());
            return Ok(());
        }

        // For terminal panels, check if there's a running process (cd won't work)
        let has_running_process = self
            .layout_manager
            .active_panel_mut()
            .and_then(|p| p.as_terminal_mut())
            .map(|t| t.has_running_processes())
            .unwrap_or(false);

        if has_running_process {
            self.state
                .set_info(t.directory_switcher_process_running().to_string());
            return Ok(());
        }

        // Get current panel's working directory
        let current_dir = self
            .layout_manager
            .active_panel_mut()
            .and_then(|p| p.get_working_directory());

        // Get all unique paths from all panels
        let panel_paths = self.collect_panel_paths();

        // Get bookmarked directories
        let bookmark_dirs = self.state.bookmarks.directories();

        // Build combined items list
        let mut items: Vec<DirectoryItem> = Vec::new();
        let mut seen_paths = std::collections::HashSet::new();

        // Add panel paths first
        for path in panel_paths {
            let is_current = current_dir.as_ref() == Some(&path);
            let display = termide_core::util::shorten_home_path(&path.display().to_string());
            seen_paths.insert(path.clone());
            items.push(DirectoryItem {
                path,
                display,
                is_current,
                is_bookmark: false,
            });
        }

        // Add bookmarked directories (if not already in list)
        for bookmark in bookmark_dirs {
            let path = PathBuf::from(&bookmark.path);
            if !seen_paths.contains(&path) {
                // Show path instead of display name for consistency
                let display = termide_core::util::shorten_home_path(&bookmark.path);
                let is_current = current_dir.as_ref() == Some(&path);
                items.push(DirectoryItem {
                    path,
                    display,
                    is_current,
                    is_bookmark: true,
                });
            }
        }

        // Sort items alphabetically by display path
        items.sort_by(|a, b| a.display.cmp(&b.display));

        // If no paths available, show info message
        if items.is_empty() {
            self.state
                .set_info(t.directory_switcher_no_paths().to_string());
            return Ok(());
        }

        // Find index of current directory to position cursor there
        let current_idx = items.iter().position(|item| item.is_current).unwrap_or(0);
        let modal = DirectorySwitcherModal::new(t.directory_switcher_title(), items)
            .with_cursor(current_idx);
        self.state.set_pending_action(
            PendingAction::SwitchDirectory,
            ActiveModal::DirectorySwitcher(Box::new(modal)),
        );

        Ok(())
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
        ) {
            SubmenuNavAction::Close => self.state.close_submenu(),
            SubmenuNavAction::Execute => self.execute_submenu_action()?,
            SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// Execute action for selected Options submenu item
    fn execute_submenu_action(&mut self) -> Result<()> {
        match self.state.ui.options_submenu.selected {
            0 => {
                // Themes - open nested submenu with live preview
                let theme_names = Theme::all_theme_names();
                let current_idx = theme_names
                    .iter()
                    .position(|n| n == self.state.theme.name)
                    .unwrap_or(0);
                // Save current theme for restoration on cancel
                self.state.ui.theme_preview_original = Some(self.state.theme.name.to_string());
                self.state.open_nested_submenu(current_idx);
            }
            1 => {
                // Language - open nested submenu with live preview
                use termide_ui_render::find_current_language_index;
                let current_idx = find_current_language_index();
                // Save current language for restoration on cancel
                self.state.ui.language_preview_original = Some(i18n::current_language());
                self.state.open_nested_submenu(current_idx);
            }
            2 => {
                // Manage actions - open actions folder in file manager
                self.state.close_menu();
                self.handle_manage_scripts()?;
            }
            3 => {
                // Manage bookmarks - open bookmarks.toml in editor
                self.state.close_menu();
                self.handle_manage_bookmarks()?;
            }
            4 => {
                // Edit preferences - close menu and open config
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
            0 => self.handle_themes_nested_submenu_key(key),
            1 => self.handle_language_nested_submenu_key(key),
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

    /// Handle keyboard event in Themes nested submenu
    fn handle_themes_nested_submenu_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        let theme_names = Theme::all_theme_names();
        let theme_count = theme_names.len();

        match key.code {
            KeyCode::Esc | KeyCode::Left => {
                // Restore original theme on cancel
                if let Some(original_name) = self.state.ui.theme_preview_original.take() {
                    self.state.theme = Theme::get_by_name(&original_name);
                }
                // Close nested submenu, return to parent
                self.state.close_nested_submenu();
            }
            KeyCode::Up | KeyCode::Down => {
                self.navigate_nested_submenu(key.code, theme_count);
                // Live preview: apply theme on cursor move
                if let Some(name) = theme_names.get(self.state.ui.nested_submenu.selected) {
                    self.state.theme = Theme::get_by_name(name);
                }
            }
            KeyCode::Enter => {
                // Clear preview state - theme is confirmed
                self.state.ui.theme_preview_original = None;
                // Apply selected theme and save preference
                if let Some(name) = theme_names.get(self.state.ui.nested_submenu.selected) {
                    self.apply_theme(name)?;
                }
                // Close all menus
                self.state.close_menu();
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle keyboard event in Language nested submenu
    fn handle_language_nested_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        let languages = i18n::get_language_list();
        let lang_count = languages.len();

        match key.code {
            KeyCode::Esc | KeyCode::Left => {
                // Restore original language on cancel
                if let Some(original_lang) = self.state.ui.language_preview_original.take() {
                    let _ = i18n::set_language(&original_lang);
                }
                // Close nested submenu, return to parent
                self.state.close_nested_submenu();
            }
            KeyCode::Up | KeyCode::Down => {
                self.navigate_nested_submenu(key.code, lang_count);
                // Live preview: apply language on cursor move
                if let Some((code, _)) = languages.get(self.state.ui.nested_submenu.selected) {
                    let _ = i18n::set_language(code);
                }
            }
            KeyCode::Enter => {
                // Clear preview state - language is confirmed
                self.state.ui.language_preview_original = None;
                // Apply selected language and save preference
                if let Some((code, name)) = languages.get(self.state.ui.nested_submenu.selected) {
                    self.apply_language(code, name)?;
                }
                // Close all menus
                self.state.close_menu();
            }
            _ => {}
        }
        Ok(())
    }

    /// Apply language by code and save preference
    pub(super) fn apply_language(&mut self, lang_code: &str, lang_name: &str) -> Result<()> {
        if let Err(e) = i18n::set_language(lang_code) {
            log::warn!("Failed to set language: {}", e);
            self.state
                .set_error(format!("Failed to set language: {}", e));
            return Ok(());
        }

        let t = i18n::t();
        self.state.set_info(t.language_changed(lang_name));

        // Save preference to config file
        if let Err(e) = self.save_language_preference(lang_code) {
            log::warn!("Failed to save language preference: {}", e);
        }

        Ok(())
    }

    /// Save language preference to config file
    fn save_language_preference(&self, lang_code: &str) -> Result<()> {
        let mut config = Config::load()?;
        config.general.language = lang_code.to_string();
        config.save()?;
        Ok(())
    }

    // =========================================================================
    // Sessions submenu handling
    // =========================================================================

    /// Handle keyboard event in Sessions submenu
    pub(super) fn handle_sessions_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        use termide_ui_render::SESSIONS_SUBMENU_ITEM_COUNT;

        match navigate_submenu(
            &key,
            &mut self.state.ui.sessions_submenu,
            SESSIONS_SUBMENU_ITEM_COUNT,
        ) {
            SubmenuNavAction::Close => self.state.close_sessions_submenu(),
            SubmenuNavAction::Execute => self.execute_sessions_submenu_action()?,
            SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// Execute action for selected Sessions submenu item
    pub(super) fn execute_sessions_submenu_action(&mut self) -> Result<()> {
        match self.state.ui.sessions_submenu.selected {
            0 => {
                // New session - open directory picker
                self.state.close_menu();
                self.handle_new_session()?;
            }
            1 => {
                // Switch session - open sessions modal
                self.state.close_menu();
                self.handle_open_sessions_modal()?;
            }
            2 => {
                // Change root path - open directory picker
                self.state.close_menu();
                self.handle_change_root_path()?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Open directory picker for creating new session
    fn handle_new_session(&mut self) -> Result<()> {
        use termide_modal::DirectoryPickerModal;

        let t = i18n::t();
        // Get current project root as starting directory
        let initial_dir = self.project_root.clone();

        let modal = DirectoryPickerModal::new(
            initial_dir,
            t.sessions_new().to_string(),
            t.directory_picker_create().to_string(),
        );
        self.state.set_pending_action(
            PendingAction::NewSession,
            ActiveModal::DirectoryPicker(Box::new(modal)),
        );

        Ok(())
    }

    /// Open directory picker for changing root path of current session
    fn handle_change_root_path(&mut self) -> Result<()> {
        use termide_modal::DirectoryPickerModal;

        let t = i18n::t();
        // Get current project root as starting directory
        let initial_dir = self.project_root.clone();

        let modal = DirectoryPickerModal::new(
            initial_dir,
            t.sessions_change_root().to_string(),
            t.directory_picker_move().to_string(),
        );
        self.state.set_pending_action(
            PendingAction::ChangeRootPath,
            ActiveModal::DirectoryPicker(Box::new(modal)),
        );

        Ok(())
    }

    /// Apply theme by name and save preference
    pub(super) fn apply_theme(&mut self, theme_name: &str) -> Result<()> {
        let new_theme = Theme::get_by_name(theme_name);
        self.state.theme = new_theme;

        let t = i18n::t();
        self.state.set_info(t.theme_changed(theme_name));

        // Save preference to config file
        if let Err(e) = self.save_theme_preference(theme_name) {
            log::warn!("Failed to save theme preference: {}", e);
        }

        Ok(())
    }

    /// Save theme preference to config file
    fn save_theme_preference(&self, theme_name: &str) -> Result<()> {
        let mut config = Config::load()?;
        config.general.theme = theme_name.to_string();
        config.save()?;
        Ok(())
    }

    // =========================================================================
    // Tools submenu handling
    // =========================================================================

    /// Handle keyboard event in Tools submenu
    pub(super) fn handle_tools_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        // If shell picker nested submenu is open, delegate to it
        if self.state.ui.tools_nested.open {
            return self.handle_tools_nested_submenu_key(key);
        }

        use termide_ui_render::TOOLS_SUBMENU_ITEM_COUNT;

        match navigate_submenu(
            &key,
            &mut self.state.ui.tools_submenu,
            TOOLS_SUBMENU_ITEM_COUNT,
        ) {
            SubmenuNavAction::Close => self.state.close_tools_submenu(),
            SubmenuNavAction::Execute => self.execute_tools_submenu_action()?,
            SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// Handle keyboard event in Tools nested submenu (shell picker)
    fn handle_tools_nested_submenu_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        let item_count = self.state.cached_shells.len();
        if item_count == 0 {
            self.state.close_tools_nested_submenu();
            return Ok(());
        }

        match navigate_submenu(&key, &mut self.state.ui.tools_nested, item_count) {
            SubmenuNavAction::Close => self.state.close_tools_nested_submenu(),
            SubmenuNavAction::Execute => {
                if let Some(shell) = self
                    .state
                    .cached_shells
                    .get(self.state.ui.tools_nested.selected)
                {
                    let shell_path = shell.path.clone();
                    // Save as default
                    self.state.config.terminal.default_shell = Some(shell_path.clone());
                    if let Err(e) = self.save_shell_preference(&shell_path) {
                        log::warn!("Failed to save shell preference: {}", e);
                    }
                    self.state.close_menu();
                    self.handle_new_terminal_with_shell(Some(&shell_path))?;
                }
            }
            SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// Execute action for selected Tools submenu item
    pub(super) fn execute_tools_submenu_action(&mut self) -> Result<()> {
        match self.state.ui.tools_submenu.selected {
            0 => {
                // Files - open new file manager panel
                self.state.close_menu();
                self.handle_new_file_manager()?;
            }
            1 => {
                // Terminal - open shell picker submenu (caches shells on open)
                self.state.open_tools_nested_submenu(0);
                // Adjust selection to match the current default shell
                let default_idx = self
                    .state
                    .config
                    .terminal
                    .default_shell
                    .as_ref()
                    .and_then(|default| {
                        self.state
                            .cached_shells
                            .iter()
                            .position(|s| s.path == *default)
                    })
                    .unwrap_or(0);
                self.state.ui.tools_nested.selected = default_idx;
            }
            2 => {
                // Editor - open new editor panel
                self.state.close_menu();
                self.handle_new_editor()?;
            }
            3 => {
                // Git Status - open Git Status panel
                self.state.close_menu();
                self.handle_open_git_status()?;
            }
            4 => {
                // Git Log - open Git Log panel
                self.state.close_menu();
                self.handle_open_git_log()?;
            }
            5 => {
                // Journal - open journal panel
                self.state.close_menu();
                self.handle_new_journal()?;
            }
            6 => {
                // Diagnostics - open diagnostics panel
                self.state.close_menu();
                self.handle_open_diagnostics()?;
            }
            7 => {
                // Operations - open operations panel
                self.state.close_menu();
                self.open_operations_panel()?;
            }
            8 => {
                // Outline - open outline panel
                self.state.close_menu();
                self.handle_open_outline()?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Notify outline panel that a file was opened/switched.
    pub(crate) fn notify_outline_file_opened(&mut self) {
        let editor_info = self.collect_editor_info_for_outline();
        if let Some((path, content, language, cursor_line)) = editor_info {
            self.push_to_outline(path, &content, language.as_deref(), Some(cursor_line));
        }
    }

    /// Re-sync outline after a panel close: rebind to another editor or clear.
    pub(super) fn resync_outline_after_close(&mut self) {
        // 1. Try the now-active panel (may be the next editor in stack)
        if self.collect_editor_info_for_outline().is_some() {
            self.notify_outline_file_opened();
            return;
        }
        // 2. Try any editor remaining in layout
        let has_editor = self
            .layout_manager
            .iter_all_panels_mut()
            .any(|p| p.as_editor().is_some());
        if has_editor {
            self.populate_outline_from_any_editor();
            return;
        }
        // 3. No editors — clear outline
        for group in &mut self.layout_manager.panel_groups {
            for panel in group.panels_mut() {
                if let Some(outline) = panel
                    .as_any_mut()
                    .downcast_mut::<termide_panel_outline::OutlinePanel>()
                {
                    outline.clear();
                    return;
                }
            }
        }
    }

    /// Collect editor data for outline (extracted for reuse).
    ///
    /// Only returns data when the active panel is an editor.
    /// Switching to non-editor panels keeps the outline bound to the last editor.
    fn collect_editor_info_for_outline(
        &mut self,
    ) -> Option<(Option<std::path::PathBuf>, String, Option<String>, usize)> {
        let panel = self.layout_manager.active_panel_mut()?;
        let editor = panel.as_editor_mut()?;
        let path = editor.file_path().map(|p| p.to_path_buf());
        let content = editor.content_string();
        let cursor_line = editor.cursor_line();
        let language = path
            .as_ref()
            .and_then(|p| termide_highlight::detect_language(p))
            .map(|s| s.to_string());
        Some((path, content, language, cursor_line))
    }

    /// Lightweight check for live editing — only compare edit_version, debounced 1s.
    pub(super) fn check_outline_live_edit(&mut self) {
        let needs_repopulate = self
            .layout_manager
            .panel_groups
            .iter_mut()
            .flat_map(|g| g.panels_mut())
            .find_map(|p| {
                p.as_any_mut()
                    .downcast_mut::<termide_panel_outline::OutlinePanel>()
            })
            .is_some_and(|outline| outline.needs_repopulate());
        if needs_repopulate {
            self.populate_outline_from_any_editor();
            return;
        }

        let Some(panel) = self.layout_manager.active_panel_mut() else {
            return;
        };
        let Some(editor) = panel.as_editor_mut() else {
            return;
        };

        let version = editor.edit_version();
        if version == self.outline_last_version {
            // No edits — also sync cursor cheaply
            let cursor = editor.cursor_line();
            if cursor != self.outline_last_cursor {
                self.outline_last_cursor = cursor;
                self.sync_outline_cursor(cursor);
            }
            return;
        }

        // Version changed — check debounce (1 second since last update)
        let now = std::time::Instant::now();
        if let Some(last) = self.outline_last_edit_time {
            if now.duration_since(last) < std::time::Duration::from_secs(1) {
                return; // Too soon, wait
            }
        }

        self.outline_last_version = version;
        self.outline_last_cursor = editor.cursor_line();
        self.outline_last_edit_time = Some(now);

        // Only now clone content
        let content = editor.content_string();
        let path = editor.file_path().map(|p| p.to_path_buf());
        let language = path
            .as_ref()
            .and_then(|p| termide_highlight::detect_language(p))
            .map(|s| s.to_string());
        self.push_to_outline(
            path,
            &content,
            language.as_deref(),
            Some(self.outline_last_cursor),
        );
    }

    /// Sync only cursor position to outline (no content extraction).
    fn sync_outline_cursor(&mut self, cursor_line: usize) {
        for group in &mut self.layout_manager.panel_groups {
            for panel in group.panels_mut() {
                if let Some(outline) = panel
                    .as_any_mut()
                    .downcast_mut::<termide_panel_outline::OutlinePanel>()
                {
                    outline.sync_cursor_line(cursor_line);
                    return;
                }
            }
        }
    }

    /// Re-extract outline symbols when the tracked file changed on disk.
    pub(super) fn notify_outline_on_fs_change(
        &mut self,
        changed_paths: &std::collections::HashSet<std::path::PathBuf>,
    ) {
        if changed_paths.is_empty() {
            return;
        }
        // Check if outline tracks one of the changed files
        let tracked: Option<std::path::PathBuf> = self.find_outline_tracked_file();
        let Some(tracked_path) = tracked else {
            return;
        };
        if !changed_paths.contains(&tracked_path) {
            return;
        }
        // File changed on disk — re-extract from editor's current content
        self.notify_outline_file_opened();
    }

    /// Find the file path currently tracked by the outline panel.
    fn find_outline_tracked_file(&self) -> Option<std::path::PathBuf> {
        for group in &self.layout_manager.panel_groups {
            for panel in group.panels() {
                if let Some(outline) = panel
                    .as_any()
                    .downcast_ref::<termide_panel_outline::OutlinePanel>()
                {
                    return outline.tracked_file().map(|p| p.to_path_buf());
                }
            }
        }
        None
    }

    /// Populate the outline panel from any editor found in the layout.
    /// Used on first open when the outline itself may already be focused.
    pub(super) fn populate_outline_from_any_editor(&mut self) {
        let editor_info: Option<(Option<std::path::PathBuf>, String, Option<String>)> = {
            let mut info = None;
            for panel in self.layout_manager.iter_all_panels_mut() {
                if let Some(editor) = panel.as_editor_mut() {
                    let path = editor.file_path().map(|p| p.to_path_buf());
                    let content = editor.content_string();
                    let language = path
                        .as_ref()
                        .and_then(|p| termide_highlight::detect_language(p))
                        .map(|s| s.to_string());
                    info = Some((path, content, language));
                    break;
                }
            }
            info
        };

        if let Some((path, content, language)) = editor_info {
            self.push_to_outline(path, &content, language.as_deref(), None);
        }
    }

    /// Apply pending outline navigation to the editor (called from tick).
    pub(super) fn apply_outline_navigation(&mut self) {
        // Collect pending navigation from outline panel
        let nav: Option<termide_panel_outline::OutlineNavigation> = {
            let mut result = None;
            for group in &mut self.layout_manager.panel_groups {
                for panel in group.panels_mut() {
                    if let Some(outline) = panel
                        .as_any_mut()
                        .downcast_mut::<termide_panel_outline::OutlinePanel>()
                    {
                        result = outline.take_pending_navigation();
                        break;
                    }
                }
                if result.is_some() {
                    break;
                }
            }
            result
        };

        // Find the matching editor, expand it if collapsed, and navigate
        if let Some(nav) = nav {
            let mut target: Option<(usize, usize)> = None;
            for (gi, group) in self.layout_manager.panel_groups.iter().enumerate() {
                for (pi, panel) in group.panels().iter().enumerate() {
                    if let Some(editor) = panel.as_editor() {
                        if editor.file_path() == Some(&nav.path) {
                            target = Some((gi, pi));
                            break;
                        }
                    }
                }
                if target.is_some() {
                    break;
                }
            }

            if let Some((gi, pi)) = target {
                // Expand the editor panel if it's collapsed
                if let Some(group) = self.layout_manager.panel_groups.get_mut(gi) {
                    group.set_expanded(pi);
                }
                // Now navigate
                if let Some(group) = self.layout_manager.panel_groups.get_mut(gi) {
                    if let Some(panel) = group.panels_mut().get_mut(pi) {
                        if let Some(editor) = panel.as_editor_mut() {
                            editor.goto_position(nav.line, nav.column);
                        }
                    }
                }
            }
        }
    }

    /// Push collected editor data into the outline panel (if it exists).
    fn push_to_outline(
        &mut self,
        path: Option<std::path::PathBuf>,
        content: &str,
        language: Option<&str>,
        cursor_line: Option<usize>,
    ) {
        let mut symbol_lines_for_editor = Vec::new();
        'outer: for group in &mut self.layout_manager.panel_groups {
            for panel in group.panels_mut() {
                if let Some(outline) = panel
                    .as_any_mut()
                    .downcast_mut::<termide_panel_outline::OutlinePanel>()
                {
                    outline.update_content(path, content, language);
                    if let Some(line) = cursor_line {
                        outline.sync_cursor_line(line);
                    }
                    symbol_lines_for_editor = outline.symbol_lines();
                    break 'outer;
                }
            }
        }
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                editor.set_symbol_lines(symbol_lines_for_editor);
            }
        }
    }

    // =========================================================================
    // Scripts submenu handling
    // =========================================================================

    /// Handle keyboard event in Scripts submenu
    pub(super) fn handle_scripts_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        // If nested submenu is open, delegate to nested handler
        if self.state.ui.scripts_nested.open {
            return self.handle_scripts_nested_submenu_key(key);
        }

        let registry = termide_config::scripts::ScriptsRegistry::load();
        let item_count = registry
            .as_ref()
            .map(|r| r.root_items.len() + r.groups.len())
            .unwrap_or(0);

        if item_count == 0 {
            // Empty menu - just close on any key
            if matches!(key.code, KeyCode::Esc | KeyCode::Left) {
                self.state.close_scripts_submenu();
            }
            return Ok(());
        }

        match navigate_submenu(&key, &mut self.state.ui.scripts_submenu, item_count) {
            SubmenuNavAction::Close => self.state.close_scripts_submenu(),
            SubmenuNavAction::Execute => self.execute_scripts_submenu_action()?,
            SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// Execute action for selected Scripts submenu item
    pub(super) fn execute_scripts_submenu_action(&mut self) -> Result<()> {
        let registry = termide_config::scripts::ScriptsRegistry::load();

        // Check if registry is empty - then the only item is "Add script..."
        let is_empty = registry
            .as_ref()
            .map(|r| r.root_items.is_empty() && r.groups.is_empty())
            .unwrap_or(true);

        if is_empty {
            // "Add script..." selected - open scripts folder
            self.state.close_menu();
            self.handle_manage_scripts()?;
            return Ok(());
        }

        let registry = match registry {
            Some(r) => r,
            None => return Ok(()),
        };

        let selected = self.state.ui.scripts_submenu.selected;
        let root_count = registry.root_items.len();

        if selected < root_count {
            // Root item selected - execute the script
            if let Some(script) = registry.root_items.get(selected) {
                self.state.close_menu();
                self.run_script(script)?;
            }
        } else {
            // Group selected - open nested submenu
            let group_idx = selected - root_count;
            if let Some(group) = registry.groups.get(group_idx) {
                self.state.open_scripts_nested_submenu(group.name.clone());
            }
        }

        Ok(())
    }

    /// Handle keyboard event in Scripts nested submenu (group items)
    fn handle_scripts_nested_submenu_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        let registry = termide_config::scripts::ScriptsRegistry::load();
        let group_name = self.state.ui.current_scripts_group.clone();

        let item_count = registry
            .as_ref()
            .and_then(|r| {
                group_name
                    .as_ref()
                    .and_then(|name| r.groups.iter().find(|g| &g.name == name))
                    .map(|g| g.items.len())
            })
            .unwrap_or(0);

        match navigate_submenu(&key, &mut self.state.ui.scripts_nested, item_count) {
            SubmenuNavAction::Close => self.state.close_scripts_nested_submenu(),
            SubmenuNavAction::Execute => self.execute_scripts_nested_action()?,
            SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// Execute action for selected item in Scripts nested submenu
    pub(super) fn execute_scripts_nested_action(&mut self) -> Result<()> {
        let registry = match termide_config::scripts::ScriptsRegistry::load() {
            Some(r) => r,
            None => return Ok(()),
        };

        let group_name = match &self.state.ui.current_scripts_group {
            Some(name) => name.clone(),
            None => return Ok(()),
        };

        let group = match registry.groups.iter().find(|g| g.name == group_name) {
            Some(g) => g,
            None => return Ok(()),
        };

        if let Some(script) = group.items.get(self.state.ui.scripts_nested.selected) {
            self.state.close_menu();
            self.run_script(script)?;
        }

        Ok(())
    }

    /// Run a script
    fn run_script(&mut self, script: &termide_config::scripts::ScriptItem) -> Result<()> {
        use termide_panel_terminal::Terminal;

        let cwd = self.get_focused_panel_cwd();

        if script.is_report {
            // Run in background with output capture, show result in modal
            self.run_report_script(script, &cwd)?;
        } else if script.is_background {
            // Fire-and-forget spawn (no terminal panel)
            log::info!("Running background script '{}' in {:?}", script.name, cwd);
            match std::process::Command::new(&script.path)
                .current_dir(&cwd)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .stdin(std::process::Stdio::null())
                .spawn()
            {
                Ok(_) => {}
                Err(e) => {
                    log::error!("Failed to run background script '{}': {}", script.name, e);
                    self.state.set_error(format!("Failed to run script: {}", e));
                }
            }
        } else {
            // Run in new terminal panel
            log::info!("Running script '{}' in {:?}", script.name, cwd);

            self.close_help_panels();

            let width = self.state.terminal.width;
            let height = self.state.terminal.height;
            let term_height = height.saturating_sub(3);
            let term_width = width.saturating_sub(2);

            let command = script.path.to_string_lossy().to_string();

            match Terminal::new_with_cwd(term_height, term_width, Some(cwd)) {
                Ok(mut terminal) => {
                    let _ = terminal.send_command(&command);
                    self.add_panel(Box::new(terminal));
                    self.auto_save_session();
                }
                Err(e) => {
                    log::error!(
                        "Failed to create terminal for script '{}': {}",
                        script.name,
                        e
                    );
                    self.state.set_error(format!("Failed to run script: {}", e));
                }
            }
        }

        Ok(())
    }

    /// Run a report script in background, capturing output for modal display
    fn run_report_script(
        &mut self,
        script: &termide_config::scripts::ScriptItem,
        cwd: &std::path::Path,
    ) -> Result<()> {
        use crate::state::{ScriptOperationHandle, ScriptOperationResult};

        log::info!("Running report script '{}' in {:?}", script.name, cwd);

        let child = std::process::Command::new(&script.path)
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();

        match child {
            Ok(child) => {
                let script_name = script.name.clone();
                let (tx, rx) = std::sync::mpsc::channel();

                std::thread::spawn(move || {
                    let output = child.wait_with_output();
                    let result = match output {
                        Ok(out) => ScriptOperationResult {
                            script_name: script_name.clone(),
                            success: out.status.success(),
                            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
                            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
                        },
                        Err(e) => ScriptOperationResult {
                            script_name: script_name.clone(),
                            success: false,
                            stdout: String::new(),
                            stderr: e.to_string(),
                        },
                    };
                    let _ = tx.send(result);
                });

                self.state.script_operation_handle = Some(ScriptOperationHandle {
                    receiver: rx,
                    script_name: script.name.clone(),
                });
            }
            Err(e) => {
                log::error!("Failed to run report script '{}': {}", script.name, e);
                self.state.set_error(format!("Failed to run script: {}", e));
            }
        }

        Ok(())
    }

    /// Get the working directory from the focused panel
    fn get_focused_panel_cwd(&self) -> PathBuf {
        // Use the Panel::get_working_directory() method
        if let Some(panel) = self.layout_manager.active_panel() {
            if let Some(cwd) = panel.get_working_directory() {
                return cwd;
            }
        }

        // Fallback to project root
        self.project_root.clone()
    }

    // =========================================================================
    // Bookmarks submenu handling
    // =========================================================================

    /// Handle keyboard event in Bookmarks submenu
    pub(super) fn handle_bookmarks_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        // If nested submenu is open, delegate to nested handler
        if self.state.ui.bookmarks_nested.open {
            return self.handle_bookmarks_nested_submenu_key(key);
        }

        use termide_ui_render::get_bookmarks_item_count;
        let item_count = get_bookmarks_item_count(&self.state.bookmarks);

        match navigate_submenu(&key, &mut self.state.ui.bookmarks_submenu, item_count) {
            SubmenuNavAction::Close => self.state.close_bookmarks_submenu(),
            SubmenuNavAction::Execute => self.execute_bookmarks_submenu_action()?,
            SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// Execute action for selected Bookmarks submenu item
    pub(super) fn execute_bookmarks_submenu_action(&mut self) -> Result<()> {
        let selected = self.state.ui.bookmarks_submenu.selected;

        if selected == 0 {
            // Add current - open add bookmark modal
            self.state.close_menu();
            self.handle_add_bookmark()?;
            return Ok(());
        }

        // Get groups and ungrouped counts
        let named_groups: Vec<String> = self
            .state
            .bookmarks
            .named_groups()
            .keys()
            .cloned()
            .collect();
        let ungrouped = self.state.bookmarks.ungrouped();
        let groups_start = 1;
        let ungrouped_start = groups_start + named_groups.len();

        if selected >= groups_start && selected < ungrouped_start {
            // Group selected - open nested submenu
            let group_idx = selected - groups_start;
            if let Some(group_name) = named_groups.get(group_idx) {
                self.state.open_bookmarks_nested_submenu(group_name.clone());
            }
        } else {
            // Ungrouped bookmark selected - open directly
            let ungrouped_idx = selected - ungrouped_start;
            if let Some(bookmark) = ungrouped.get(ungrouped_idx) {
                let path = bookmark.path.clone();
                let bookmark_type = bookmark.bookmark_type();
                self.state.close_menu();
                self.open_bookmark(&path, bookmark_type)?;
            }
        }

        Ok(())
    }

    /// Handle keyboard event in Bookmarks nested submenu (group items)
    fn handle_bookmarks_nested_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        use termide_ui_render::get_bookmarks_group_items;

        let group_name = self.state.ui.current_bookmarks_group.clone();

        let item_count = group_name
            .as_ref()
            .map(|name| get_bookmarks_group_items(&self.state.bookmarks, name).len())
            .unwrap_or(0);

        match navigate_submenu(&key, &mut self.state.ui.bookmarks_nested, item_count) {
            SubmenuNavAction::Close => self.state.close_bookmarks_nested_submenu(),
            SubmenuNavAction::Execute => self.execute_bookmarks_nested_action()?,
            SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// Execute action for selected item in Bookmarks nested submenu
    pub(super) fn execute_bookmarks_nested_action(&mut self) -> Result<()> {
        let group_name = match &self.state.ui.current_bookmarks_group {
            Some(name) => name.clone(),
            None => return Ok(()),
        };

        let grouped = self.state.bookmarks.grouped();
        let group_bookmarks = match grouped.get(&group_name) {
            Some(bookmarks) => bookmarks,
            None => return Ok(()),
        };

        if let Some(bookmark) = group_bookmarks.get(self.state.ui.bookmarks_nested.selected) {
            let path = bookmark.path.clone();
            let bookmark_type = bookmark.bookmark_type();
            self.state.close_menu();
            self.open_bookmark(&path, bookmark_type)?;
        }

        Ok(())
    }

    /// Handle adding a bookmark
    pub(super) fn handle_add_bookmark(&mut self) -> Result<()> {
        use termide_modal::BookmarkAddModal;

        // Get current path from active panel
        let current_path = self.get_current_bookmark_path();

        // Get existing group names for autocomplete
        let existing_groups = self.state.bookmarks.group_names();

        let modal = BookmarkAddModal::new(current_path, existing_groups);
        self.state.set_pending_action(
            PendingAction::AddBookmark,
            ActiveModal::BookmarkAdd(Box::new(modal)),
        );

        Ok(())
    }

    /// Get current path from active panel for bookmarking
    fn get_current_bookmark_path(&self) -> Option<String> {
        if let Some(panel) = self.layout_manager.active_panel() {
            // Try to get file path from editor
            if let Some(editor) = panel.as_editor() {
                if let Some(path) = editor.file_path() {
                    return Some(path.display().to_string());
                }
            }
            // Fall back to working directory
            if let Some(cwd) = panel.get_working_directory() {
                return Some(cwd.display().to_string());
            }
        }
        None
    }

    /// Handle managing bookmarks - open bookmarks.toml in editor
    pub(super) fn handle_manage_bookmarks(&mut self) -> Result<()> {
        use termide_config::BookmarksConfig;

        self.close_help_panels();

        // Get the bookmarks file path
        let bookmarks_path = match BookmarksConfig::config_file_path() {
            Ok(path) => {
                // Create the file if it doesn't exist
                if !path.exists() {
                    // Ensure parent directory exists
                    if let Some(parent) = path.parent() {
                        if !parent.exists() {
                            if let Err(e) = std::fs::create_dir_all(parent) {
                                log::warn!("Failed to create data directory: {}", e);
                            }
                        }
                    }
                    // Create empty bookmarks file
                    let empty_config = BookmarksConfig::default();
                    if let Err(e) = empty_config.save() {
                        log::warn!("Failed to create bookmarks file: {}", e);
                    }
                }
                path
            }
            Err(e) => {
                log::warn!("Failed to get bookmarks path: {}", e);
                self.state
                    .set_error(format!("Failed to get bookmarks path: {}", e));
                return Ok(());
            }
        };

        let _ = self.open_editor_for_file(bookmarks_path);
        Ok(())
    }

    /// Open a bookmark based on its type
    fn open_bookmark(
        &mut self,
        path: &str,
        bookmark_type: termide_config::BookmarkType,
    ) -> Result<()> {
        use termide_config::BookmarkType;

        match bookmark_type {
            BookmarkType::Directory => {
                // Check if active panel is a file manager - reuse it
                if let Some(panel) = self.layout_manager.active_panel_mut() {
                    if let Some(fm) = panel.as_file_manager_mut() {
                        let _ = fm.navigate_to(PathBuf::from(path));
                        self.state.needs_watcher_registration = true;
                        return Ok(());
                    }
                }
                // No active file manager - create new panel
                self.close_help_panels();
                let fm_panel = FileManager::new_with_path(PathBuf::from(path));
                self.add_panel(Box::new(fm_panel));
                self.auto_save_session();
            }
            BookmarkType::TextFile => {
                // Open in editor
                let _ = self.open_editor_for_file(PathBuf::from(path));
            }
            BookmarkType::ViewerFile | BookmarkType::HttpLink => {
                // Open with external viewer
                let _ = std::process::Command::new("xdg-open").arg(path).spawn();
            }
            BookmarkType::SshConnection => {
                // Open SSH connection in terminal
                // Parse ssh://[user@]host[:port] format into proper ssh command
                let ssh_cmd = {
                    let url_part = path.strip_prefix("ssh://").unwrap_or(path);
                    let mut cmd_parts = vec!["ssh".to_string()];

                    // Split off any path component (ignore it for SSH)
                    let authority = url_part.split('/').next().unwrap_or(url_part);

                    // Parse user@host:port format
                    let (user_host, port) = if let Some(colon_pos) = authority.rfind(':') {
                        // Check if what's after colon looks like a port number
                        let after_colon = &authority[colon_pos + 1..];
                        if after_colon.chars().all(|c| c.is_ascii_digit())
                            && !after_colon.is_empty()
                            && after_colon.parse::<u16>().is_ok_and(|p| p > 0)
                        {
                            (&authority[..colon_pos], Some(after_colon))
                        } else {
                            (authority, None)
                        }
                    } else {
                        (authority, None)
                    };

                    // Add port if specified
                    if let Some(port) = port {
                        cmd_parts.push("-p".to_string());
                        cmd_parts.push(port.to_string());
                    }

                    cmd_parts.push(user_host.to_string());
                    cmd_parts.join(" ")
                };

                let width = self.state.terminal.width;
                let height = self.state.terminal.height;
                let term_height = height.saturating_sub(3);
                let term_width = width.saturating_sub(2);

                self.close_help_panels();
                if let Ok(terminal) = Terminal::new_with_command(term_height, term_width, &ssh_cmd)
                {
                    self.add_panel(Box::new(terminal));
                    self.auto_save_session();
                }
            }
            BookmarkType::SftpPath
            | BookmarkType::FtpPath
            | BookmarkType::SmbPath
            | BookmarkType::NfsPath => {
                // Navigate to remote path using VFS
                if let Some(panel) = self.layout_manager.active_panel_mut() {
                    if let Some(fm) = panel.as_file_manager_mut() {
                        let _ = fm.navigate_to_url(path);
                        self.state.needs_watcher_registration = true;
                        return Ok(());
                    }
                }
                // No active file manager - create new panel and navigate
                self.close_help_panels();
                let mut fm_panel = FileManager::new();
                let _ = fm_panel.navigate_to_url(path);
                self.add_panel(Box::new(fm_panel));
                self.auto_save_session();
            }
            BookmarkType::Unknown => {
                // Try to open as text file
                let _ = self.open_editor_for_file(PathBuf::from(path));
            }
        }

        Ok(())
    }
}
