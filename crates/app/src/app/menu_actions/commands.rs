//! Commands menu actions — commands dropdown navigation and command management.

use anyhow::Result;
use termide_config::commands::{decode_command_menu_key, CommandMenuKeyKind};
use termide_config::{GlobalKeybindings, KeyBinding};
use termide_modal::ReservedHotkey;

use super::super::App;

fn push_reserved_hotkeys(reserved: &mut Vec<ReservedHotkey>, binding: &Option<KeyBinding>) {
    let Some(binding) = binding else {
        return;
    };
    match binding {
        KeyBinding::Single(key) => reserved.push(ReservedHotkey {
            binding: key.clone(),
        }),
        KeyBinding::Multiple(keys) => {
            for key in keys {
                reserved.push(ReservedHotkey {
                    binding: key.clone(),
                });
            }
        }
    }
}

fn collect_global_reserved_hotkeys(kb: &GlobalKeybindings) -> Vec<ReservedHotkey> {
    let mut reserved = Vec::new();
    push_reserved_hotkeys(&mut reserved, &kb.toggle_menu);
    push_reserved_hotkeys(&mut reserved, &kb.new_file_manager);
    push_reserved_hotkeys(&mut reserved, &kb.new_terminal);
    push_reserved_hotkeys(&mut reserved, &kb.new_editor);
    push_reserved_hotkeys(&mut reserved, &kb.new_journal);
    push_reserved_hotkeys(&mut reserved, &kb.open_help);
    push_reserved_hotkeys(&mut reserved, &kb.open_preferences);
    push_reserved_hotkeys(&mut reserved, &kb.open_sessions);
    push_reserved_hotkeys(&mut reserved, &kb.new_session);
    push_reserved_hotkeys(&mut reserved, &kb.open_git_status);
    push_reserved_hotkeys(&mut reserved, &kb.open_outline);
    push_reserved_hotkeys(&mut reserved, &kb.open_diagnostics);
    push_reserved_hotkeys(&mut reserved, &kb.open_git_log);
    push_reserved_hotkeys(&mut reserved, &kb.open_bookmark_add);
    push_reserved_hotkeys(&mut reserved, &kb.open_command_palette);
    push_reserved_hotkeys(&mut reserved, &kb.prev_group);
    push_reserved_hotkeys(&mut reserved, &kb.next_group);
    push_reserved_hotkeys(&mut reserved, &kb.prev_panel);
    push_reserved_hotkeys(&mut reserved, &kb.next_panel);
    push_reserved_hotkeys(&mut reserved, &kb.goto_panel_1);
    push_reserved_hotkeys(&mut reserved, &kb.goto_panel_2);
    push_reserved_hotkeys(&mut reserved, &kb.goto_panel_3);
    push_reserved_hotkeys(&mut reserved, &kb.goto_panel_4);
    push_reserved_hotkeys(&mut reserved, &kb.goto_panel_5);
    push_reserved_hotkeys(&mut reserved, &kb.goto_panel_6);
    push_reserved_hotkeys(&mut reserved, &kb.goto_panel_7);
    push_reserved_hotkeys(&mut reserved, &kb.goto_panel_8);
    push_reserved_hotkeys(&mut reserved, &kb.goto_panel_9);
    push_reserved_hotkeys(&mut reserved, &kb.close_panel);
    push_reserved_hotkeys(&mut reserved, &kb.toggle_stack);
    push_reserved_hotkeys(&mut reserved, &kb.swap_left);
    push_reserved_hotkeys(&mut reserved, &kb.swap_right);
    push_reserved_hotkeys(&mut reserved, &kb.move_first);
    push_reserved_hotkeys(&mut reserved, &kb.move_last);
    push_reserved_hotkeys(&mut reserved, &kb.resize_smaller);
    push_reserved_hotkeys(&mut reserved, &kb.resize_larger);
    push_reserved_hotkeys(&mut reserved, &kb.panel_action_menu);
    push_reserved_hotkeys(&mut reserved, &kb.quit);
    reserved
}

impl App {
    fn collect_reserved_command_hotkeys(
        &self,
        registry: &termide_config::commands::CommandsRegistry,
        exclude: Option<(&str, bool)>,
    ) -> Vec<ReservedHotkey> {
        let mut reserved = collect_global_reserved_hotkeys(&self.state.config.general.keybindings);
        for (command, key_str) in registry.commands_with_hotkeys() {
            if exclude.is_some_and(|(name, is_project)| {
                command.name == name && command.is_project == is_project
            }) {
                continue;
            }
            reserved.push(ReservedHotkey {
                binding: key_str.to_string(),
            });
        }
        reserved
    }

    // =========================================================================
    // Commands submenu handling
    // =========================================================================

    /// Handle keyboard event in Commands submenu
    pub(in crate::app) fn handle_commands_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        use super::{navigate_submenu, SubmenuNavAction};

        // If nested submenu is open, delegate to nested handler
        if self.state.ui.commands_nested.open {
            return self.handle_commands_nested_submenu_key(key);
        }

        let registry = self.commands_registry();
        let commands_items = registry
            .as_ref()
            .map(|r| {
                use termide_ui_render::get_commands_items;
                get_commands_items(r)
            })
            .unwrap_or_default();
        let item_count = commands_items.len();
        let separators: Vec<usize> = commands_items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.is_separator)
            .map(|(idx, _)| idx)
            .collect();

        match navigate_submenu(
            &key,
            &mut self.state.ui.commands_submenu,
            item_count,
            &separators,
        ) {
            SubmenuNavAction::Close => self.state.close_menu(),
            SubmenuNavAction::Execute => self.execute_commands_submenu_action()?,
            SubmenuNavAction::Right => {
                let sel = self.state.ui.commands_submenu.selected;
                let has_sub = registry
                    .as_ref()
                    .map(|r| {
                        use termide_ui_render::get_commands_items;
                        get_commands_items(r)
                    })
                    .and_then(|items| items.get(sel).map(|i| i.has_submenu))
                    .unwrap_or(false);
                if has_sub {
                    self.execute_commands_submenu_action()?;
                } else {
                    self.switch_to_next_menu()?;
                }
            }
            SubmenuNavAction::Left => self.switch_to_prev_menu()?,
            SubmenuNavAction::Rename => self.rename_selected_command()?,
            SubmenuNavAction::Edit => self.edit_selected_command()?,
            SubmenuNavAction::Delete => self.delete_selected_command()?,
            SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// Execute action for selected Commands submenu item
    pub(in crate::app) fn execute_commands_submenu_action(&mut self) -> Result<()> {
        let selected = self.state.ui.commands_submenu.selected;

        let registry = match self.commands_registry() {
            Some(r) => r,
            None => return Ok(()),
        };

        // Look up the selected item by index in the rendered menu items
        let items = termide_ui_render::get_commands_items(&registry);
        let item = match items.get(selected) {
            Some(i) if !i.is_separator => i,
            _ => return Ok(()),
        };

        let key = &item.key;

        // Special keys: "Manage commands" or "Add command..."
        if key == termide_ui_render::COMMAND_MANAGE || key == termide_ui_render::COMMAND_ADD_NEW {
            self.state.close_menu();
            self.handle_add_command()?;
            return Ok(());
        }

        let Some(decoded) = decode_command_menu_key(key) else {
            return Ok(());
        };

        match decoded.kind {
            CommandMenuKeyKind::Command => {
                if let Some(command) = registry.find_root_command(&decoded.name, decoded.is_project)
                {
                    self.state.close_menu();
                    self.run_command(command)?;
                }
            }
            CommandMenuKeyKind::Group => {
                if registry
                    .find_group(&decoded.name, decoded.is_project)
                    .is_some()
                {
                    if self.state.ui.commands_nested.open
                        && self.state.ui.current_commands_group.as_deref() == Some(key.as_str())
                    {
                        self.state.close_commands_nested_submenu();
                    } else {
                        self.state.open_commands_nested_submenu(key.clone());
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle keyboard event in Commands nested submenu (group items)
    fn handle_commands_nested_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        use super::{navigate_submenu, SubmenuNavAction};

        let registry = self.commands_registry();
        let group_name = self.state.ui.current_commands_group.clone();

        let item_count = registry
            .as_ref()
            .and_then(|r| {
                group_name
                    .as_ref()
                    .and_then(|name| {
                        let decoded = decode_command_menu_key(name)?;
                        r.find_group(&decoded.name, decoded.is_project)
                    })
                    .map(|g| g.items.len())
            })
            .unwrap_or(0);

        match navigate_submenu(&key, &mut self.state.ui.commands_nested, item_count, &[]) {
            SubmenuNavAction::Close | SubmenuNavAction::Left => {
                self.state.close_commands_nested_submenu();
            }
            SubmenuNavAction::Execute => self.execute_commands_nested_action()?,
            SubmenuNavAction::Right => self.switch_to_next_menu()?,
            SubmenuNavAction::Rename => self.rename_selected_nested_command()?,
            SubmenuNavAction::Edit => self.edit_selected_nested_command()?,
            SubmenuNavAction::Delete => self.delete_selected_nested_command()?,
            SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// Execute action for selected item in Commands nested submenu
    pub(in crate::app) fn execute_commands_nested_action(&mut self) -> Result<()> {
        let registry = match self.commands_registry() {
            Some(r) => r,
            None => return Ok(()),
        };

        let group_name = match &self.state.ui.current_commands_group {
            Some(name) => name.clone(),
            None => return Ok(()),
        };

        let decoded = match decode_command_menu_key(&group_name) {
            Some(decoded) if decoded.kind == CommandMenuKeyKind::Group => decoded,
            _ => return Ok(()),
        };

        let group = match registry.find_group(&decoded.name, decoded.is_project) {
            Some(g) => g,
            None => return Ok(()),
        };

        if let Some(command) = group.items.get(self.state.ui.commands_nested.selected) {
            self.state.close_menu();
            self.run_command(command)?;
        }

        Ok(())
    }

    /// Open selected command config modal (F4 from commands submenu)
    fn edit_selected_command(&mut self) -> Result<()> {
        let selected = self.state.ui.commands_submenu.selected;

        // Index 0: "Add command..." — nothing to edit
        if selected == 0 {
            return Ok(());
        }

        if let Some(registry) = self.commands_registry() {
            let items = termide_ui_render::get_commands_items(&registry);
            if let Some(item) = items.get(selected) {
                if item.is_separator || item.has_submenu {
                    return Ok(());
                }
                let Some(decoded) = decode_command_menu_key(&item.key) else {
                    return Ok(());
                };
                if decoded.kind != CommandMenuKeyKind::Command {
                    return Ok(());
                }
                if let Some(command) = registry.find_root_command(&decoded.name, decoded.is_project)
                {
                    self.state.close_menu();
                    let reserved = self.collect_reserved_command_hotkeys(
                        &registry,
                        Some((&command.name, command.is_project)),
                    );
                    let groups: Vec<String> =
                        registry.groups.iter().map(|g| g.name.clone()).collect();
                    let title = format!("Edit command: {}", command.name);
                    let modal = termide_modal::CommandConfigModal::new_edit(
                        title,
                        command.name.clone(),
                        command.metadata.as_ref().and_then(|m| m.group.clone()),
                        command.is_project,
                        None,
                        groups,
                        command.metadata.clone(),
                    )
                    .with_reserved_hotkeys(reserved);
                    self.state.set_pending_action(
                        termide_state::PendingAction::EditCommand {
                            command_name: command.name.clone(),
                            is_project: command.is_project,
                            group: None,
                            selected,
                        },
                        crate::state::ActiveModal::CommandConfig(Box::new(modal)),
                    );
                }
            }
        }
        Ok(())
    }

    /// Open selected nested command config modal (F4 from commands nested submenu)
    fn edit_selected_nested_command(&mut self) -> Result<()> {
        let registry = match self.commands_registry() {
            Some(r) => r,
            None => return Ok(()),
        };
        let group_name = match &self.state.ui.current_commands_group {
            Some(name) => name.clone(),
            None => return Ok(()),
        };
        let decoded = match decode_command_menu_key(&group_name) {
            Some(decoded) if decoded.kind == CommandMenuKeyKind::Group => decoded,
            _ => return Ok(()),
        };
        if let Some(group) = registry.find_group(&decoded.name, decoded.is_project) {
            let selected = self.state.ui.commands_nested.selected;
            if let Some(command) = group.items.get(selected) {
                self.state.close_menu();
                let reserved = self.collect_reserved_command_hotkeys(
                    &registry,
                    Some((&command.name, command.is_project)),
                );
                let groups: Vec<String> = registry.groups.iter().map(|g| g.name.clone()).collect();
                let title = format!("Edit command: {}", command.name);
                let modal = termide_modal::CommandConfigModal::new_edit(
                    title,
                    command.name.clone(),
                    command.metadata.as_ref().and_then(|m| m.group.clone()),
                    command.is_project,
                    None,
                    groups,
                    command.metadata.clone(),
                )
                .with_reserved_hotkeys(reserved);
                self.state.set_pending_action(
                    termide_state::PendingAction::EditCommand {
                        command_name: command.name.clone(),
                        is_project: command.is_project,
                        group: Some(group_name),
                        selected,
                    },
                    crate::state::ActiveModal::CommandConfig(Box::new(modal)),
                );
            }
        }
        Ok(())
    }

    /// Open the "Add command" modal form
    fn handle_add_command(&mut self) -> Result<()> {
        let registry = self.commands_registry();
        let groups: Vec<String> = registry
            .as_ref()
            .map(|r| r.groups.iter().map(|g| g.name.clone()).collect())
            .unwrap_or_default();
        let reserved = registry
            .as_ref()
            .map(|r| self.collect_reserved_command_hotkeys(r, None))
            .unwrap_or_else(|| {
                collect_global_reserved_hotkeys(&self.state.config.general.keybindings)
            });

        let t = termide_i18n::t();
        let modal = termide_modal::CommandConfigModal::new_create(t.menu_commands_add(), groups)
            .with_reserved_hotkeys(reserved);
        self.state.set_pending_action(
            termide_state::PendingAction::CreateCommand,
            crate::state::ActiveModal::CommandConfig(Box::new(modal)),
        );
        Ok(())
    }

    /// Delete selected command with confirmation
    fn delete_selected_command(&mut self) -> Result<()> {
        let selected = self.state.ui.commands_submenu.selected;
        if let Some(registry) = self.commands_registry() {
            let items = termide_ui_render::get_commands_items(&registry);
            if let Some(item) = items.get(selected) {
                if item.is_separator || item.has_submenu {
                    return Ok(());
                }
                let Some(decoded) = decode_command_menu_key(&item.key) else {
                    return Ok(());
                };
                if decoded.kind != CommandMenuKeyKind::Command {
                    return Ok(());
                }
                if let Some(command) = registry.find_root_command(&decoded.name, decoded.is_project)
                {
                    self.state.close_menu();
                    let t = termide_i18n::t();
                    let message = format!("{} \"{}\"?", t.help_desc_delete_generic(), command.name);
                    let modal = termide_modal::ConfirmModal::new(t.modal_confirm_title(), &message);
                    self.state.set_pending_action(
                        termide_state::PendingAction::DeleteCommand {
                            command_name: command.name.clone(),
                            is_project: command.is_project,
                            selected,
                        },
                        crate::state::ActiveModal::Confirm(Box::new(modal)),
                    );
                }
            }
        }
        Ok(())
    }

    /// Delete selected command in nested submenu with confirmation
    fn delete_selected_nested_command(&mut self) -> Result<()> {
        let registry = match self.commands_registry() {
            Some(r) => r,
            None => return Ok(()),
        };
        let group_name = match &self.state.ui.current_commands_group {
            Some(name) => name.clone(),
            None => return Ok(()),
        };
        let decoded = match decode_command_menu_key(&group_name) {
            Some(decoded) if decoded.kind == CommandMenuKeyKind::Group => decoded,
            _ => return Ok(()),
        };
        if let Some(group) = registry.find_group(&decoded.name, decoded.is_project) {
            let selected = self.state.ui.commands_nested.selected;
            if let Some(command) = group.items.get(selected) {
                self.state.close_menu();
                let t = termide_i18n::t();
                let message = format!("{} \"{}\"?", t.help_desc_delete_generic(), command.name);
                let modal = termide_modal::ConfirmModal::new(t.modal_confirm_title(), &message);
                self.state.set_pending_action(
                    termide_state::PendingAction::DeleteCommand {
                        command_name: command.name.clone(),
                        is_project: command.is_project,
                        selected,
                    },
                    crate::state::ActiveModal::Confirm(Box::new(modal)),
                );
            }
        }
        Ok(())
    }

    /// Rename selected command (F2) — shows InputModal with current filename
    fn rename_selected_command(&mut self) -> Result<()> {
        let selected = self.state.ui.commands_submenu.selected;
        if let Some(registry) = self.commands_registry() {
            let items = termide_ui_render::get_commands_items(&registry);
            if let Some(item) = items.get(selected) {
                if item.is_separator || item.has_submenu {
                    return Ok(());
                }
                let Some(decoded) = decode_command_menu_key(&item.key) else {
                    return Ok(());
                };
                if decoded.kind != CommandMenuKeyKind::Command {
                    return Ok(());
                }
                if let Some(command) = registry.find_root_command(&decoded.name, decoded.is_project)
                {
                    self.state.close_menu();
                    let t = termide_i18n::t();
                    let modal = termide_modal::InputModal::with_default(
                        t.help_desc_rename(),
                        t.help_desc_rename(),
                        &command.name,
                    );
                    self.state.set_pending_action(
                        termide_state::PendingAction::RenameCommand {
                            command_name: command.name.clone(),
                            is_project: command.is_project,
                            group: None,
                            selected,
                        },
                        crate::state::ActiveModal::Input(Box::new(modal)),
                    );
                }
            }
        }
        Ok(())
    }

    /// Rename selected command in nested submenu (F2)
    fn rename_selected_nested_command(&mut self) -> Result<()> {
        let registry = match self.commands_registry() {
            Some(r) => r,
            None => return Ok(()),
        };
        let group_name = match &self.state.ui.current_commands_group {
            Some(name) => name.clone(),
            None => return Ok(()),
        };
        let decoded = match decode_command_menu_key(&group_name) {
            Some(decoded) if decoded.kind == CommandMenuKeyKind::Group => decoded,
            _ => return Ok(()),
        };
        if let Some(group) = registry.find_group(&decoded.name, decoded.is_project) {
            let selected = self.state.ui.commands_nested.selected;
            if let Some(command) = group.items.get(selected) {
                self.state.close_menu();
                let t = termide_i18n::t();
                let modal = termide_modal::InputModal::with_default(
                    t.help_desc_rename(),
                    t.help_desc_rename(),
                    &command.name,
                );
                self.state.set_pending_action(
                    termide_state::PendingAction::RenameCommand {
                        command_name: command.name.clone(),
                        is_project: command.is_project,
                        group: Some(group_name),
                        selected,
                    },
                    crate::state::ActiveModal::Input(Box::new(modal)),
                );
            }
        }
        Ok(())
    }

    /// Reopen commands menu after modal (rename/delete).
    /// If `group` is Some, also opens the nested submenu for that group.
    pub(in crate::app) fn reopen_commands_menu(
        &mut self,
        group: Option<String>,
        fallback_selected: usize,
    ) {
        use termide_ui_render::menu::COMMANDS_MENU_INDEX;
        self.state.ui.menu_open = true;
        self.state.ui.selected_menu_item = Some(COMMANDS_MENU_INDEX);
        self.state.open_commands_submenu();

        if let Some(group_name) = group {
            if let Some(registry) = self.commands_registry() {
                let items = termide_ui_render::get_commands_items(&registry);
                let group_idx = items
                    .iter()
                    .position(|i| i.has_submenu && i.key == group_name)
                    .unwrap_or(fallback_selected);
                self.state.ui.commands_submenu.selected = group_idx;
                self.state.open_commands_nested_submenu(group_name);
            }
        } else {
            self.state.ui.commands_submenu.selected = fallback_selected;
        }
    }
}
