//! Tools menu actions — panel creation via Tools submenu.

use anyhow::Result;
use std::sync::Arc;

use super::super::App;
use termide_ui_render::{
    TOOLS_SUBMENU_DIAGNOSTICS, TOOLS_SUBMENU_EDITOR, TOOLS_SUBMENU_FILES, TOOLS_SUBMENU_GIT_LOG,
    TOOLS_SUBMENU_GIT_STATUS, TOOLS_SUBMENU_JOURNAL, TOOLS_SUBMENU_OPEN, TOOLS_SUBMENU_OPERATIONS,
    TOOLS_SUBMENU_OUTLINE, TOOLS_SUBMENU_SEPARATOR, TOOLS_SUBMENU_TERMINAL,
};

impl App {
    /// Handle keyboard event in Tools submenu
    pub(in crate::app) fn handle_tools_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        // If shell picker nested submenu is open, delegate to it
        if self.state.ui.tools_nested.open {
            return self.handle_tools_nested_submenu_key(key);
        }

        use super::navigate_submenu;
        use super::SubmenuNavAction;
        use termide_ui_render::TOOLS_SUBMENU_ITEM_COUNT;

        match navigate_submenu(
            &key,
            &mut self.state.ui.tools_submenu,
            TOOLS_SUBMENU_ITEM_COUNT,
            &[TOOLS_SUBMENU_SEPARATOR],
        ) {
            SubmenuNavAction::Close => self.state.close_menu(),
            SubmenuNavAction::Execute => self.execute_tools_submenu_action()?,
            SubmenuNavAction::Right => {
                // Terminal has a nested (shell picker) submenu
                if self.state.ui.tools_submenu.selected == TOOLS_SUBMENU_TERMINAL {
                    self.execute_tools_submenu_action()?;
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

    /// Handle keyboard event in Tools nested submenu (shell picker)
    fn handle_tools_nested_submenu_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        use super::{navigate_submenu, SubmenuNavAction};

        let item_count = self.state.cache.shells.len();
        if item_count == 0 {
            self.state.close_tools_nested_submenu();
            return Ok(());
        }

        match navigate_submenu(&key, &mut self.state.ui.tools_nested, item_count, &[]) {
            SubmenuNavAction::Close => self.state.close_tools_nested_submenu(),
            SubmenuNavAction::Execute => {
                if let Some(shell) = self
                    .state
                    .cache
                    .shells
                    .get(self.state.ui.tools_nested.selected)
                {
                    let shell_path = shell.path.clone();
                    // Copy-on-write: mutate in-place if single owner, else clone
                    {
                        let config = Arc::make_mut(&mut self.state.config);
                        config.terminal.default_shell = Some(shell_path.clone());
                    }
                    if let Err(e) = self.save_shell_preference(&shell_path) {
                        log::warn!("Failed to save shell preference: {}", e);
                    }
                    self.state.close_menu();
                    self.handle_new_terminal_with_shell(Some(&shell_path))?;
                }
            }
            SubmenuNavAction::Right => self.switch_to_next_menu()?,
            SubmenuNavAction::Left => self.state.close_tools_nested_submenu(),
            SubmenuNavAction::Rename
            | SubmenuNavAction::Edit
            | SubmenuNavAction::Delete
            | SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// Execute action for selected Tools submenu item
    pub(in crate::app) fn execute_tools_submenu_action(&mut self) -> Result<()> {
        match self.state.ui.tools_submenu.selected {
            TOOLS_SUBMENU_TERMINAL => {
                // Toggle: if the shell-picker nested submenu is already open, close it.
                if self.state.ui.tools_nested.open {
                    self.state.close_tools_nested_submenu();
                } else {
                    self.state.open_tools_nested_submenu(0);
                    let default_idx = self
                        .state
                        .config
                        .terminal
                        .default_shell
                        .as_ref()
                        .and_then(|default| {
                            self.state
                                .cache
                                .shells
                                .iter()
                                .position(|s| s.path == *default)
                        })
                        .unwrap_or(0);
                    self.state.ui.tools_nested.selected = default_idx;
                }
            }
            TOOLS_SUBMENU_FILES => {
                self.state.close_menu();
                self.handle_new_file_manager()?;
            }
            TOOLS_SUBMENU_EDITOR => {
                self.state.close_menu();
                self.handle_new_editor()?;
            }
            TOOLS_SUBMENU_GIT_STATUS => {
                self.state.close_menu();
                self.handle_open_git_status()?;
            }
            TOOLS_SUBMENU_GIT_LOG => {
                self.state.close_menu();
                self.handle_open_git_log()?;
            }
            TOOLS_SUBMENU_JOURNAL => {
                self.state.close_menu();
                self.handle_new_journal()?;
            }
            TOOLS_SUBMENU_DIAGNOSTICS => {
                self.state.close_menu();
                self.handle_open_diagnostics()?;
            }
            TOOLS_SUBMENU_OPERATIONS => {
                self.state.close_menu();
                self.open_operations_panel()?;
            }
            TOOLS_SUBMENU_OUTLINE => {
                self.state.close_menu();
                self.handle_open_outline()?;
            }
            TOOLS_SUBMENU_OPEN => {
                // Universal opener: the entered value is routed by type — a file
                // (by extension), a directory (file manager), a database URL
                // (DB viewer), or an http(s) address (fetched into a viewer).
                self.state.close_menu();
                let base = std::env::current_dir().unwrap_or_default();
                self.event_show_input(
                    termide_i18n::t().tools_open_prompt().to_string(),
                    String::new(),
                    termide_core::InputAction::ViewPath { base_dir: base },
                );
            }
            _ => {}
        }
        Ok(())
    }
}
