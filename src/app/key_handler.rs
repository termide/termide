use anyhow::Result;
use crossterm::event::KeyCode;
use std::path::PathBuf;

use super::App;
use crate::{
    constants::DEFAULT_FM_WIDTH,
    i18n,
    panels::{
        debug::Debug, editor::Editor, file_manager::FileManager, terminal_pty::Terminal,
        welcome::Welcome,
    },
    state::{ActiveModal, PendingAction},
    ui::menu::MENU_ITEM_COUNT,
};

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
        #[allow(clippy::redundant_pattern_matching)]
        if let Some(_) = self.handle_global_hotkeys(key)? {
            return Ok(());
        }

        // Pass event to active panel and collect results
        let (file_to_open, modal_request, config_update, status_message) =
            if let Some(panel) = self.layout_manager.active_panel_mut() {
                panel.handle_key(key)?;

                // Collect results from panel
                let file_to_open = panel.take_file_to_open();
                let modal_request = panel.take_modal_request();

                // Check config update and status message (only for Editor)
                let (config_update, status_message) = {
                    use crate::panels::editor::Editor;
                    use std::any::Any;
                    if let Some(editor) = (&mut **panel as &mut dyn Any).downcast_mut::<Editor>() {
                        (editor.take_config_update(), editor.take_status_message())
                    } else {
                        (None, None)
                    }
                };

                (file_to_open, modal_request, config_update, status_message)
            } else {
                (None, None, None, None)
            };

        // Apply config update if present
        if let Some(new_config) = config_update {
            self.state.config = new_config.clone();
            self.state.set_theme(&new_config.theme);
            self.state.set_info("Config saved and applied".to_string());
        }

        // Display status message if present
        if let Some(message) = status_message {
            self.state.set_info(message);
        }

        // Handle file opening in editor
        if let Some(file_path) = file_to_open {
            self.close_welcome_panels();
            let filename = file_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?");
            let t = i18n::t();
            crate::logger::info(format!("Attempting to open file: {}", filename));

            match Editor::open_file_with_config(file_path.clone(), self.state.editor_config()) {
                Ok(editor_panel) => {
                    self.add_panel(Box::new(editor_panel));
                    self.auto_save_session();
                    crate::logger::info(format!("File '{}' opened in editor", filename));
                    self.state.set_info(t.editor_file_opened(filename));
                }
                Err(e) => {
                    let error_msg = t.status_error_open_file(filename, &e.to_string());
                    crate::logger::error(format!("Error opening '{}': {}", filename, e));
                    self.state.set_error(error_msg);
                }
            }
        }

        // Handle modal window
        if let Some((mut action, mut modal)) = modal_request {
            // Note: panel_index is kept for compatibility but not actively used with LayoutManager
            // Active panel is tracked by LayoutManager (active group + expanded panel)
            match &mut action {
                PendingAction::CreateFile { panel_index, .. }
                | PendingAction::CreateDirectory { panel_index, .. }
                | PendingAction::DeletePath { panel_index, .. }
                | PendingAction::CopyPath { panel_index, .. }
                | PendingAction::MovePath { panel_index, .. }
                | PendingAction::SaveFileAs { panel_index, .. }
                | PendingAction::ClosePanel { panel_index }
                | PendingAction::CloseEditorWithSave { panel_index }
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
                        // Find all unique paths from other panels (FM, Terminal, Editor)
                        let options = self.find_all_other_panel_paths();
                        let unique_paths_count = options.len();

                        // Determine default directory based on available paths
                        let default_dir = if !options.is_empty() {
                            // Use first option as default (parse value back to PathBuf)
                            std::path::PathBuf::from(&options[0].value)
                        } else {
                            // Use parent directory of first source
                            sources[0]
                                .parent()
                                .map(|p| p.to_path_buf())
                                .unwrap_or_else(|| std::path::PathBuf::from("/"))
                        };

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

                        // Choose modal based on number of unique paths:
                        // 0-1 paths: simple InputModal
                        // 2+ paths: EditableSelectModal with list
                        modal = if unique_paths_count >= 2 {
                            // Multiple unique paths - show editable select
                            let new_modal = crate::ui::modal::EditableSelectModal::new(
                                title,
                                prompt,
                                &default_dest,
                                options,
                            );
                            ActiveModal::EditableSelect(Box::new(new_modal))
                        } else {
                            // 0 or 1 unique path - simple input modal
                            let new_modal = crate::ui::modal::InputModal::with_default(
                                title,
                                prompt,
                                &default_dest,
                            );
                            ActiveModal::Input(Box::new(new_modal))
                        };
                    }
                }
                _ => {}
            }

            // Handle navigation actions without modal window
            match action {
                PendingAction::NextPanel => {
                    // Navigate to next group horizontally
                    self.layout_manager.next_group();
                    return Ok(());
                }
                PendingAction::PrevPanel => {
                    // Navigate to previous group horizontally
                    self.layout_manager.prev_group();
                    return Ok(());
                }
                _ => {}
            }

            self.state.set_pending_action(action, modal);

            // Check if there's a channel receiver for directory size in panel
            if let Some(panel) = self.layout_manager.active_panel_mut() {
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
                    self.state.close_menu();
                    if self.has_panels_requiring_confirmation() {
                        let t = i18n::t();
                        let modal = crate::ui::modal::ConfirmModal::new(
                            t.modal_yes(),
                            t.app_quit_confirm(),
                        );
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
        }
        Ok(())
    }

    /// Handle panel close request with confirmation if needed
    /// NOTE: panel_index parameter is obsolete with LayoutManager
    pub(crate) fn handle_close_panel_request(&mut self, _panel_index: usize) -> Result<()> {
        crate::logger::debug("Panel close requested");
        // Check if confirmation is required before closing active panel
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(_message) = panel.needs_close_confirmation() {
                crate::logger::warn("Close requested for panel with unsaved changes");
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
                    let action = PendingAction::CloseEditorWithSave { panel_index: 0 }; // placeholder
                    self.state
                        .set_pending_action(action, ActiveModal::Select(Box::new(modal)));
                    return Ok(());
                } else {
                    // For other panels show simple confirmation
                    let t = i18n::t();
                    let modal = crate::ui::modal::ConfirmModal::new(t.modal_yes(), &_message);
                    let action = PendingAction::ClosePanel { panel_index: 0 }; // placeholder
                    self.state
                        .set_pending_action(action, ActiveModal::Confirm(Box::new(modal)));
                    return Ok(());
                }
            }
        }

        // Close active panel without confirmation
        self.close_panel_at_index(0); // panel_index is obsolete, function uses active panel
        Ok(())
    }

    /// Create new terminal
    fn handle_new_terminal(&mut self) -> Result<()> {
        crate::logger::debug("Opening new Terminal panel");
        self.close_welcome_panels();
        // Get directory from FM
        let working_dir = self
            .layout_manager
            .file_manager_mut()
            .and_then(|p| p.get_working_directory());

        // Create new terminal
        let width = self.state.terminal.width;
        let height = self.state.terminal.height;
        let term_height = height.saturating_sub(3);
        let term_width = if self.layout_manager.has_file_manager() {
            let fm_width = DEFAULT_FM_WIDTH;
            width.saturating_sub(fm_width).saturating_sub(2)
        } else {
            width.saturating_sub(2)
        };

        if let Ok(terminal_panel) = Terminal::new_with_cwd(term_height, term_width, working_dir) {
            self.add_panel(Box::new(terminal_panel));
            self.auto_save_session();
        }
        Ok(())
    }

    /// Create new file manager
    fn handle_new_file_manager(&mut self) -> Result<()> {
        crate::logger::debug("Opening new FileManager panel");
        self.close_welcome_panels();
        // Get working directory from current active panel
        let working_dir = self
            .layout_manager
            .active_panel_mut()
            .and_then(|p| p.get_working_directory())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));

        let fm_panel = FileManager::new_with_path(working_dir);
        self.add_panel(Box::new(fm_panel));
        self.auto_save_session();
        Ok(())
    }

    /// Create new editor
    fn handle_new_editor(&mut self) -> Result<()> {
        crate::logger::debug("Opening new Editor panel");
        self.close_welcome_panels();
        let editor_panel = Editor::with_config(self.state.editor_config());
        self.add_panel(Box::new(editor_panel));
        self.auto_save_session();
        Ok(())
    }

    /// Create new debug panel (singleton - only one instance allowed)
    fn handle_new_debug(&mut self) -> Result<()> {
        // Check if Debug panel already exists and focus it
        if self.focus_existing_debug_panel() {
            crate::logger::debug("Switching focus to existing Log panel");
            return Ok(());
        }

        // No existing Debug panel found, create new one
        crate::logger::debug("Opening new Log panel");
        self.close_welcome_panels();
        let debug_panel = Debug::new();
        self.add_panel(Box::new(debug_panel));
        self.auto_save_session();
        Ok(())
    }

    /// Find and focus existing Debug panel if it exists
    /// Returns true if Debug panel was found and focused
    fn focus_existing_debug_panel(&mut self) -> bool {
        use crate::panels::debug::Debug;
        use std::any::Any;

        // Iterate through all panel groups
        for (group_idx, group) in self.layout_manager.panel_groups.iter_mut().enumerate() {
            // Check each panel in the group
            for (panel_idx, panel) in group.panels().iter().enumerate() {
                let panel_any: &dyn Any = &**panel;
                if panel_any.is::<Debug>() {
                    // Found Debug panel - set it as expanded and focus the group
                    group.set_expanded(panel_idx);
                    self.layout_manager.focus =
                        crate::layout_manager::FocusTarget::Group(group_idx);
                    return true;
                }
            }
        }

        false
    }

    /// Open or switch to help panel (Welcome)
    fn handle_new_help(&mut self) -> Result<()> {
        crate::logger::debug("Opening new Help/Welcome panel");
        // TODO: Check if Welcome panel already exists in some group
        // For now, just create new
        let welcome = Welcome::new();
        self.add_panel(Box::new(welcome));
        self.auto_save_session();
        Ok(())
    }

    /// Open config file in editor
    fn open_config_in_editor(&mut self) -> Result<()> {
        use crate::config::Config;
        use crate::panels::editor::Editor;

        let config_path = match Config::config_file_path() {
            Ok(path) => path,
            Err(e) => {
                crate::logger::warn(format!("Failed to get config path: {}", e));
                self.state
                    .set_error(format!("Failed to get config path: {}", e));
                return Ok(());
            }
        };

        // TODO: Check if config file is already open in some editor panel
        // For now, just open it

        self.close_welcome_panels();

        match Editor::open_file_with_config(config_path.clone(), self.state.editor_config()) {
            Ok(editor_panel) => {
                self.add_panel(Box::new(editor_panel));
                self.auto_save_session();
            }
            Err(e) => {
                self.state
                    .set_error(format!("Failed to open config: {}", e));
            }
        }

        Ok(())
    }

    /// Check if any panel requires close confirmation (unsaved changes, running processes)
    fn has_panels_requiring_confirmation(&self) -> bool {
        // Check if any panel has unsaved changes or running processes
        for panel in std::iter::once(&self.layout_manager.file_manager)
            .flatten()
            .chain(
                self.layout_manager
                    .panel_groups
                    .iter()
                    .flat_map(|g| g.panels().iter()),
            )
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

    /// Close all Welcome panels (called before opening new panel)
    pub(super) fn close_welcome_panels(&mut self) {
        crate::logger::debug("Closing Welcome panel(s)");
        // Iterate through all groups and close Welcome panels
        // Use reverse iteration to avoid index shifting issues when removing
        let mut groups_to_remove = Vec::new();

        for group_idx in (0..self.layout_manager.panel_groups.len()).rev() {
            if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                // Find panels to remove in this group
                let mut panels_to_remove = Vec::new();

                for panel_idx in (0..group.len()).rev() {
                    if let Some(panel) = group.panels().get(panel_idx) {
                        if panel.is_welcome_panel() {
                            panels_to_remove.push(panel_idx);
                        }
                    }
                }

                // Remove the panels
                for panel_idx in panels_to_remove {
                    group.remove_panel(panel_idx);
                }

                // If group is now empty, mark it for removal
                if group.is_empty() {
                    groups_to_remove.push(group_idx);
                }
            }
        }

        // Remove empty groups
        let groups_were_removed = !groups_to_remove.is_empty();
        for group_idx in groups_to_remove {
            self.layout_manager.panel_groups.remove(group_idx);
        }

        // Adjust focus if needed
        if let crate::layout_manager::FocusTarget::Group(idx) = self.layout_manager.focus {
            if !self.layout_manager.panel_groups.is_empty()
                && idx >= self.layout_manager.panel_groups.len()
            {
                self.layout_manager.focus = crate::layout_manager::FocusTarget::Group(
                    self.layout_manager.panel_groups.len() - 1,
                );
            }
        }

        // Пропорционально перераспределить ширины после удаления групп
        if groups_were_removed {
            let terminal_width = self.state.terminal.width;
            let fm_width = if self.layout_manager.has_file_manager() {
                DEFAULT_FM_WIDTH
            } else {
                0
            };
            let available_width = terminal_width.saturating_sub(fm_width);
            self.layout_manager
                .redistribute_widths_proportionally(available_width);
        }
    }

    /// Move panel to previous group
    fn handle_swap_panel_left(&mut self) -> Result<()> {
        let terminal_width = self.state.terminal.width;
        let fm_width = if self.layout_manager.has_file_manager() {
            DEFAULT_FM_WIDTH
        } else {
            0
        };
        let available_width = terminal_width.saturating_sub(fm_width);

        self.layout_manager
            .move_panel_to_prev_group(available_width)?;
        self.auto_save_session();
        Ok(())
    }

    /// Move panel to next group
    fn handle_swap_panel_right(&mut self) -> Result<()> {
        let terminal_width = self.state.terminal.width;
        let fm_width = if self.layout_manager.has_file_manager() {
            DEFAULT_FM_WIDTH
        } else {
            0
        };
        let available_width = terminal_width.saturating_sub(fm_width);

        self.layout_manager
            .move_panel_to_next_group(available_width)?;
        self.auto_save_session();
        Ok(())
    }

    /// Change active group width
    fn handle_resize_panel(&mut self, delta: i16) -> Result<()> {
        use crate::layout_manager::FocusTarget;

        // Игнорировать для FileManager
        if matches!(self.layout_manager.focus, FocusTarget::FileManager) {
            return Ok(());
        }

        if let Some(group_idx) = self.layout_manager.active_group_index() {
            // Нельзя ресайзить если это единственная группа (некому адаптироваться)
            if self.layout_manager.panel_groups.len() <= 1 {
                return Ok(());
            }

            // Рассчитать доступную ширину для групп панелей
            let terminal_width = self.state.terminal.width;
            let fm_width = if self.layout_manager.has_file_manager() {
                DEFAULT_FM_WIDTH
            } else {
                0
            };
            let available_width = terminal_width.saturating_sub(fm_width);

            // Заморозить все auto-width группы перед ресайзом
            let actual_widths = self.layout_manager.calculate_actual_widths(available_width);
            for (idx, group) in self.layout_manager.panel_groups.iter_mut().enumerate() {
                if group.width.is_none() {
                    group.width = Some(actual_widths.get(idx).copied().unwrap_or(20));
                }
            }

            // Получить текущую ширину активной группы (теперь гарантированно Some)
            let current_width = self.layout_manager.panel_groups[group_idx].width.unwrap();

            // Вычислить желаемую новую ширину активной группы
            let desired_new_width = ((current_width as i16 + delta).clamp(20, 300)) as u16;
            let actual_delta = desired_new_width as i16 - current_width as i16;

            // Если delta = 0 (достигли границы), ничего не делаем
            if actual_delta == 0 {
                return Ok(());
            }

            // Собрать все остальные группы с их индексами и ширинами
            let other_groups: Vec<(usize, u16)> = self
                .layout_manager
                .panel_groups
                .iter()
                .enumerate()
                .filter(|(idx, _)| *idx != group_idx)
                .map(|(idx, g)| (idx, g.width.unwrap()))
                .collect();

            // Рассчитать общую ширину остальных групп
            let total_other_width: u16 = other_groups.iter().map(|(_, w)| *w).sum();

            if total_other_width == 0 {
                return Ok(()); // Защита от деления на 0
            }

            // Распределить -actual_delta пропорционально по остальным группам
            let mut remaining_delta = -actual_delta;
            let mut new_widths: Vec<(usize, u16)> = Vec::new();

            for (i, &(idx, width)) in other_groups.iter().enumerate() {
                let is_last = i == other_groups.len() - 1;

                let delta_for_this = if is_last {
                    // Последняя группа получает весь остаток для точной zero-sum
                    remaining_delta
                } else {
                    // Пропорциональная доля от -actual_delta
                    let proportion = width as f64 / total_other_width as f64;
                    ((-actual_delta as f64) * proportion).round() as i16
                };

                let new_width = ((width as i16 + delta_for_this).clamp(20, 300)) as u16;
                new_widths.push((idx, new_width));

                // Вычесть использованное изменение из остатка
                let actual_change = new_width as i16 - width as i16;
                remaining_delta -= actual_change;
            }

            // Применить новую ширину к активной группе
            self.layout_manager.panel_groups[group_idx].width = Some(desired_new_width);

            // Применить новые ширины к остальным группам
            for (idx, new_width) in new_widths {
                self.layout_manager.panel_groups[idx].width = Some(new_width);
            }

            // Проверить и скорректировать баланс если clamping нарушил zero-sum
            let total_new_width: u16 = self
                .layout_manager
                .panel_groups
                .iter()
                .map(|g| g.width.unwrap_or(20))
                .sum();

            if total_new_width != available_width {
                // Clamping нарушил баланс - скорректировать активную группу
                let other_widths_sum: u16 = self
                    .layout_manager
                    .panel_groups
                    .iter()
                    .enumerate()
                    .filter(|(idx, _)| *idx != group_idx)
                    .map(|(_, g)| g.width.unwrap_or(20))
                    .sum();

                let corrected_width = available_width.saturating_sub(other_widths_sum);
                self.layout_manager.panel_groups[group_idx].width =
                    Some(corrected_width.clamp(20, 300));
            }
            self.auto_save_session();
        }
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
                // Alt+L - open Debug (Log panel)
                KeyCode::Char('l') | KeyCode::Char('L') => {
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
                    if self.has_panels_requiring_confirmation() {
                        let t = i18n::t();
                        let modal = crate::ui::modal::ConfirmModal::new(
                            t.modal_yes(),
                            t.app_quit_confirm(),
                        );
                        self.state.set_pending_action(
                            PendingAction::QuitApplication,
                            ActiveModal::Confirm(Box::new(modal)),
                        );
                    } else {
                        self.state.quit();
                    }
                    return Ok(Some(()));
                }
                // Alt+X / Alt+Delete - close panel
                KeyCode::Char('x') | KeyCode::Char('X') | KeyCode::Delete => {
                    self.handle_close_panel_request(0)?; // Parameter is unused - works with active panel
                    return Ok(Some(()));
                }
                // Alt+1..9 - go to panel by number
                // TODO: Implement panel selection by number with LayoutManager
                KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                    let _panel_num = c.to_digit(10).unwrap() as usize;
                    // TODO: Select group or panel within group by number
                    return Ok(Some(()));
                }
                // Alt+Left or Alt+, - go to previous panel/group
                KeyCode::Left | KeyCode::Char(',') | KeyCode::Char('<') => {
                    // Navigate to previous group
                    self.layout_manager.prev_group();
                    return Ok(Some(()));
                }
                // Alt+Right or Alt+. - go to next panel/group
                KeyCode::Right | KeyCode::Char('.') | KeyCode::Char('>') => {
                    // Navigate to next group
                    self.layout_manager.next_group();
                    return Ok(Some(()));
                }
                // Alt+Up - go to previous panel in current group (vertical navigation)
                KeyCode::Up => {
                    self.layout_manager.prev_panel_in_group();
                    return Ok(Some(()));
                }
                // Alt+Down - go to next panel in current group (vertical navigation)
                KeyCode::Down => {
                    self.layout_manager.next_panel_in_group();
                    return Ok(Some(()));
                }
                // Alt+W - go to previous panel in group (alternative to Alt+Up)
                KeyCode::Char('w') | KeyCode::Char('W') => {
                    self.layout_manager.prev_panel_in_group();
                    return Ok(Some(()));
                }
                // Alt+S - go to next panel in group (alternative to Alt+Down)
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    self.layout_manager.next_panel_in_group();
                    return Ok(Some(()));
                }
                // Alt+A - go to previous group (alternative to Alt+Left)
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    self.layout_manager.prev_group();
                    return Ok(Some(()));
                }
                // Alt+D - go to next group (alternative to Alt+Right)
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    self.layout_manager.next_group();
                    return Ok(Some(()));
                }
                // Alt+Backspace - toggle panel stacking (smart merge/unstack)
                KeyCode::Backspace => {
                    // Рассчитать доступную ширину для групп панелей
                    let terminal_width = self.state.terminal.width;
                    let fm_width = if self.layout_manager.has_file_manager() {
                        DEFAULT_FM_WIDTH
                    } else {
                        0
                    };
                    let available_width = terminal_width.saturating_sub(fm_width);

                    if let Err(e) = self.layout_manager.toggle_panel_stacking(available_width) {
                        self.state
                            .set_error(format!("Cannot toggle stacking: {}", e));
                    } else {
                        self.auto_save_session();
                    }
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
                // Alt+Home - move panel to first group
                KeyCode::Home => {
                    let terminal_width = self.state.terminal.width;
                    let fm_width = if self.layout_manager.has_file_manager() {
                        DEFAULT_FM_WIDTH
                    } else {
                        0
                    };
                    let available_width = terminal_width.saturating_sub(fm_width);

                    if let Err(e) = self
                        .layout_manager
                        .move_panel_to_first_group(available_width)
                    {
                        self.state.set_error(format!("Cannot move panel: {}", e));
                    } else {
                        self.auto_save_session();
                    }
                    return Ok(Some(()));
                }
                // Alt+End - move panel to last group
                KeyCode::End => {
                    let terminal_width = self.state.terminal.width;
                    let fm_width = if self.layout_manager.has_file_manager() {
                        DEFAULT_FM_WIDTH
                    } else {
                        0
                    };
                    let available_width = terminal_width.saturating_sub(fm_width);

                    if let Err(e) = self
                        .layout_manager
                        .move_panel_to_last_group(available_width)
                    {
                        self.state.set_error(format!("Cannot move panel: {}", e));
                    } else {
                        self.auto_save_session();
                    }
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
            let captures = self
                .layout_manager
                .active_panel_mut()
                .map(|p| p.captures_escape())
                .unwrap_or(false);
            if !captures {
                self.handle_close_panel_request(0)?; // panel_index is obsolete
                return Ok(Some(()));
            }
            // Otherwise Escape is passed to panel's handle_key
        }

        Ok(None) // Not handled, pass to panel
    }
}
