//! Command palette — searchable command launcher.

use anyhow::Result;

use super::super::App;

impl App {
    /// Open the command palette modal.
    pub(in crate::app) fn handle_open_command_palette(&mut self) -> Result<()> {
        use termide_modal::{ActiveModal, CommandEntry, CommandPaletteModal};
        use termide_state::PendingAction;

        let t = termide_i18n::t();
        let kb = &self.state.config.general.keybindings;

        let kb_str = |b: &Option<termide_config::KeyBinding>| {
            b.as_ref()
                .map(|k| k.display().to_string())
                .unwrap_or_default()
        };

        // Build paired lists: action name strings and display entries.
        // Order: Panels, Git, Navigation, Panel Management, Application.
        let commands: Vec<(&str, CommandEntry)> = vec![
            (
                "new_editor",
                CommandEntry {
                    label: t.help_desc_new_editor().into(),
                    category: t.palette_category_panels(),
                    keybinding: kb_str(&kb.new_editor),
                },
            ),
            (
                "new_file_manager",
                CommandEntry {
                    label: t.help_desc_new_file_manager().into(),
                    category: t.palette_category_panels(),
                    keybinding: kb_str(&kb.new_file_manager),
                },
            ),
            (
                "new_terminal",
                CommandEntry {
                    label: t.help_desc_new_terminal().into(),
                    category: t.palette_category_panels(),
                    keybinding: kb_str(&kb.new_terminal),
                },
            ),
            (
                "new_journal",
                CommandEntry {
                    label: t.help_desc_new_journal().into(),
                    category: t.palette_category_panels(),
                    keybinding: kb_str(&kb.new_journal),
                },
            ),
            (
                "open_help",
                CommandEntry {
                    label: t.help_desc_help().into(),
                    category: t.palette_category_panels(),
                    keybinding: kb_str(&kb.open_help),
                },
            ),
            (
                "open_path",
                CommandEntry {
                    label: t.help_desc_open_path().into(),
                    category: t.palette_category_panels(),
                    keybinding: kb_str(&kb.open_path),
                },
            ),
            (
                "open_preferences",
                CommandEntry {
                    label: t.help_desc_open_preferences().into(),
                    category: t.palette_category_panels(),
                    keybinding: kb_str(&kb.open_preferences),
                },
            ),
            (
                "open_git_status",
                CommandEntry {
                    label: t.help_desc_open_git_status().into(),
                    category: t.palette_category_git(),
                    keybinding: kb_str(&kb.open_git_status),
                },
            ),
            (
                "open_git_log",
                CommandEntry {
                    label: t.help_desc_open_git_log().into(),
                    category: t.palette_category_git(),
                    keybinding: kb_str(&kb.open_git_log),
                },
            ),
            (
                "open_projects",
                CommandEntry {
                    label: t.help_desc_open_projects().into(),
                    category: t.palette_category_navigation(),
                    keybinding: kb_str(&kb.open_projects),
                },
            ),
            (
                "switch_directory",
                CommandEntry {
                    label: t.help_desc_switch_directory().into(),
                    category: t.palette_category_navigation(),
                    keybinding: kb_str(
                        &self.state.config.file_manager.keybindings.switch_directory,
                    ),
                },
            ),
            (
                "open_outline",
                CommandEntry {
                    label: t.help_desc_open_outline().into(),
                    category: t.palette_category_navigation(),
                    keybinding: kb_str(&kb.open_outline),
                },
            ),
            (
                "open_agent",
                CommandEntry {
                    label: t.help_desc_open_agent().into(),
                    category: t.palette_category_panels(),
                    keybinding: kb_str(&kb.open_agent),
                },
            ),
            (
                "open_diagnostics",
                CommandEntry {
                    label: t.help_desc_open_diagnostics().into(),
                    category: t.palette_category_navigation(),
                    keybinding: kb_str(&kb.open_diagnostics),
                },
            ),
            (
                "open_bookmark_add",
                CommandEntry {
                    label: t.help_desc_open_bookmark_add().into(),
                    category: t.palette_category_navigation(),
                    keybinding: kb_str(&kb.open_bookmark_add),
                },
            ),
            (
                "close_panel",
                CommandEntry {
                    label: t.help_desc_close_panel().into(),
                    category: t.palette_category_panel_management(),
                    keybinding: kb_str(&kb.close_panel),
                },
            ),
            (
                "toggle_stack",
                CommandEntry {
                    label: t.help_desc_toggle_stack().into(),
                    category: t.palette_category_panel_management(),
                    keybinding: kb_str(&kb.toggle_stack),
                },
            ),
            (
                "swap_left",
                CommandEntry {
                    label: t.help_desc_swap_left().into(),
                    category: t.palette_category_panel_management(),
                    keybinding: kb_str(&kb.swap_left),
                },
            ),
            (
                "swap_right",
                CommandEntry {
                    label: t.help_desc_swap_right().into(),
                    category: t.palette_category_panel_management(),
                    keybinding: kb_str(&kb.swap_right),
                },
            ),
            (
                "move_first",
                CommandEntry {
                    label: t.help_desc_move_first().into(),
                    category: t.palette_category_panel_management(),
                    keybinding: kb_str(&kb.move_first),
                },
            ),
            (
                "move_last",
                CommandEntry {
                    label: t.help_desc_move_last().into(),
                    category: t.palette_category_panel_management(),
                    keybinding: kb_str(&kb.move_last),
                },
            ),
            (
                "detach_instance",
                CommandEntry {
                    label: t.help_desc_detach_instance().into(),
                    category: t.palette_category_application(),
                    keybinding: kb_str(&kb.detach_instance),
                },
            ),
            (
                "lock_vault",
                CommandEntry {
                    label: t.palette_lock_vault().into(),
                    category: t.palette_category_application(),
                    keybinding: String::new(),
                },
            ),
            (
                "quit",
                CommandEntry {
                    label: t.help_desc_quit().into(),
                    category: t.palette_category_application(),
                    keybinding: kb_str(&kb.quit),
                },
            ),
            (
                "menu",
                CommandEntry {
                    label: t.help_desc_menu().into(),
                    category: t.palette_category_application(),
                    keybinding: kb_str(&kb.toggle_menu),
                },
            ),
        ];

        let (actions, entries): (Vec<&str>, Vec<CommandEntry>) = commands.into_iter().unzip();

        let mut actions: Vec<String> = actions.into_iter().map(String::from).collect();
        let mut entries = entries;

        // Add commands from registry
        if let Some(registry) = self.commands_registry() {
            for (command, key_str) in registry.commands_with_hotkeys() {
                let display_name = command
                    .metadata
                    .as_ref()
                    .and_then(|m| m.display_name.as_deref())
                    .unwrap_or(&command.name);
                let command_key = termide_config::commands::encode_command_menu_key(
                    termide_config::commands::CommandMenuKeyKind::Command,
                    &command.name,
                    command.is_project,
                );
                actions.push(format!("run_command:{command_key}"));
                entries.push(CommandEntry {
                    label: t.command_run_label().replace("{name}", display_name),
                    category: t.palette_category_commands(),
                    keybinding: key_str.to_string(),
                });
            }
        }

        self.command_palette_actions = Some(actions);

        let modal = CommandPaletteModal::new(entries);
        self.state.set_pending_action(
            PendingAction::CommandPalette,
            ActiveModal::CommandPalette(Box::new(modal)),
        );

        Ok(())
    }
}
