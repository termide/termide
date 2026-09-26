//! Command palette — searchable command launcher.

use anyhow::Result;

use super::super::App;

impl App {
    /// Open the command palette modal.
    pub(in crate::app) fn handle_open_command_palette(&mut self) -> Result<()> {
        use termide_modal::{ActiveModal, CommandEntry, CommandPaletteModal};
        use termide_state::PendingAction;

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
                    label: "New Editor".into(),
                    category: "Panels",
                    keybinding: kb_str(&kb.new_editor),
                },
            ),
            (
                "new_file_manager",
                CommandEntry {
                    label: "New File Manager".into(),
                    category: "Panels",
                    keybinding: kb_str(&kb.new_file_manager),
                },
            ),
            (
                "new_terminal",
                CommandEntry {
                    label: "New Terminal".into(),
                    category: "Panels",
                    keybinding: kb_str(&kb.new_terminal),
                },
            ),
            (
                "new_journal",
                CommandEntry {
                    label: "New Journal".into(),
                    category: "Panels",
                    keybinding: kb_str(&kb.new_journal),
                },
            ),
            (
                "open_help",
                CommandEntry {
                    label: "Open Help".into(),
                    category: "Panels",
                    keybinding: kb_str(&kb.open_help),
                },
            ),
            (
                "open_path",
                CommandEntry {
                    label: "Open Path or URL".into(),
                    category: "Panels",
                    keybinding: kb_str(&kb.open_path),
                },
            ),
            (
                "open_preferences",
                CommandEntry {
                    label: "Open Preferences".into(),
                    category: "Panels",
                    keybinding: kb_str(&kb.open_preferences),
                },
            ),
            (
                "open_git_status",
                CommandEntry {
                    label: "Open Git Status".into(),
                    category: "Git",
                    keybinding: kb_str(&kb.open_git_status),
                },
            ),
            (
                "open_git_log",
                CommandEntry {
                    label: "Open Git Log".into(),
                    category: "Git",
                    keybinding: kb_str(&kb.open_git_log),
                },
            ),
            (
                "open_projects",
                CommandEntry {
                    label: "Open Sessions".into(),
                    category: "Navigation",
                    keybinding: kb_str(&kb.open_projects),
                },
            ),
            (
                "open_projects",
                CommandEntry {
                    label: "Switch Directory".into(),
                    category: "Navigation",
                    keybinding: kb_str(
                        &self.state.config.file_manager.keybindings.switch_directory,
                    ),
                },
            ),
            (
                "open_outline",
                CommandEntry {
                    label: "Open Outline".into(),
                    category: "Navigation",
                    keybinding: kb_str(&kb.open_outline),
                },
            ),
            (
                "open_agent",
                CommandEntry {
                    label: "Open Agent".into(),
                    category: "Panels",
                    keybinding: kb_str(&kb.open_agent),
                },
            ),
            (
                "open_diagnostics",
                CommandEntry {
                    label: "Open Diagnostics".into(),
                    category: "Navigation",
                    keybinding: kb_str(&kb.open_diagnostics),
                },
            ),
            (
                "open_bookmark_add",
                CommandEntry {
                    label: "Add Bookmark".into(),
                    category: "Navigation",
                    keybinding: kb_str(&kb.open_bookmark_add),
                },
            ),
            (
                "close_panel",
                CommandEntry {
                    label: "Close Panel".into(),
                    category: "Panel Management",
                    keybinding: kb_str(&kb.close_panel),
                },
            ),
            (
                "toggle_stack",
                CommandEntry {
                    label: "Toggle Stacking".into(),
                    category: "Panel Management",
                    keybinding: kb_str(&kb.toggle_stack),
                },
            ),
            (
                "swap_left",
                CommandEntry {
                    label: "Move Panel Left".into(),
                    category: "Panel Management",
                    keybinding: kb_str(&kb.swap_left),
                },
            ),
            (
                "swap_right",
                CommandEntry {
                    label: "Move Panel Right".into(),
                    category: "Panel Management",
                    keybinding: kb_str(&kb.swap_right),
                },
            ),
            (
                "move_first",
                CommandEntry {
                    label: "Move to First".into(),
                    category: "Panel Management",
                    keybinding: kb_str(&kb.move_first),
                },
            ),
            (
                "move_last",
                CommandEntry {
                    label: "Move to Last".into(),
                    category: "Panel Management",
                    keybinding: kb_str(&kb.move_last),
                },
            ),
            (
                "detach_instance",
                CommandEntry {
                    label: "Detach Session".into(),
                    category: "Application",
                    keybinding: kb_str(&kb.detach_instance),
                },
            ),
            (
                "quit",
                CommandEntry {
                    label: "Quit".into(),
                    category: "Application",
                    keybinding: kb_str(&kb.quit),
                },
            ),
            (
                "menu",
                CommandEntry {
                    label: "Toggle Menu".into(),
                    category: "Application",
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
                    label: format!("Run command: {}", display_name),
                    category: "Commands",
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
