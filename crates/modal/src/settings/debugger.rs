//! The Debug tab: how each debugger starts (`[debug]` in the config). The
//! tab holds the general settings and the list of debuggers; a debugger
//! opens on a page of its own, laid out like an LSP server's — its fields,
//! then a row of buttons back to the list and resetting it. The page edits
//! the config in place, so every change is saved as it is made.

use std::collections::HashMap;

use termide_config::DebuggerSettings;
use termide_i18n as i18n;

use super::connection::{BACK, DELETE};
use super::fields::{ContentRow, FieldDescriptor, FieldType};
use super::SettingsModal;

/// The debuggers termide knows, as `commands.toml` and `[debug.debuggers]`
/// name them: the adapters first, then the command-line debuggers.
pub(super) const DEBUGGERS: [&str; 8] = [
    "lldb-dap", "gdb-dap", "debugpy", "dlv-dap", "lldb", "gdb", "dlv", "pdb",
];

/// The Debug tab's fields, as indices into its descriptors.
pub(super) const NATIVE: usize = 0;
pub(super) const CONSOLE: usize = 1;

/// The debugger page's fields.
pub(super) const COMMAND: usize = 0;
pub(super) const ARGS: usize = 1;

/// Where a debug adapter can run the program.
pub(super) const CONSOLES: [&str; 2] = ["terminal", "panel"];

/// The debugger page, while one is open.
#[derive(Debug, Clone)]
pub(super) struct DebuggerEdit {
    /// Its name, one of [`DEBUGGERS`].
    pub name: String,
    /// Why the value typed last was refused, and the field it was typed in.
    pub error: Option<(usize, String)>,
    /// The button chosen on the buttons row: [`BACK`] or [`DELETE`], which
    /// here resets the debugger to its defaults.
    pub button: usize,
}

/// The Debug tab's fields.
pub(super) fn debug_fields() -> Vec<FieldDescriptor> {
    let t = i18n::t();
    vec![
        FieldDescriptor {
            label: t.settings_debug_native(),
            field_type: FieldType::OptionalText,
        },
        FieldDescriptor {
            label: t.settings_debug_console(),
            field_type: FieldType::Enum,
        },
    ]
}

/// The debugger page's fields.
pub(super) fn debugger_fields() -> Vec<FieldDescriptor> {
    let t = i18n::t();
    [t.settings_debugger_command(), t.settings_debugger_args()]
        .into_iter()
        .map(|label| FieldDescriptor {
            label,
            field_type: FieldType::OptionalText,
        })
        .collect()
}

/// The page's buttons as they are drawn, `[ label ]`, left to right.
pub(super) fn debugger_buttons() -> [String; 2] {
    let t = i18n::t();
    [
        format!("[ {} ]", t.settings_ai_connection_back()),
        format!("[ {} ]", t.settings_debugger_reset()),
    ]
}

/// A console value as the user reads it.
pub(super) fn console_label(value: &str) -> String {
    let t = i18n::t();
    match value {
        "panel" => t.settings_debug_console_panel().to_string(),
        _ => t.settings_debug_console_terminal().to_string(),
    }
}

/// Flags joined back into the text [`split_words`] reads.
///
/// [`split_words`]: termide_config::split_words
pub(super) fn join_flags(flags: &[String]) -> String {
    flags
        .iter()
        .map(|flag| {
            let plain = !flag.is_empty()
                && flag
                    .chars()
                    .all(|c| c.is_alphanumeric() || "-_=./:,@%+".contains(c));
            if plain {
                flag.clone()
            } else {
                format!("'{}'", flag.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl SettingsModal {
    /// The flags each debugger gets on this system when the settings name
    /// none, to show beside its flags field (Delve's, in Termux). The app
    /// knows them; the modal only shows them.
    pub fn with_debugger_defaults(mut self, defaults: HashMap<String, Vec<String>>) -> Self {
        self.debugger_defaults = defaults;
        self
    }

    /// The name of the debugger the page has open.
    pub(super) fn open_debugger_name(&self) -> Option<&str> {
        self.debugger_edit.as_ref().map(|e| e.name.as_str())
    }

    /// The settings of debugger `name`, empty when it has none.
    pub(super) fn debugger_settings(&self, name: &str) -> DebuggerSettings {
        self.config.debug.debugger(name)
    }

    fn default_flags(&self, name: &str) -> &[String] {
        self.debugger_defaults
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Change the open debugger's settings, dropping its entry when nothing
    /// is left in it, so the config file keeps only what was set.
    fn update_debugger(&mut self, change: impl FnOnce(&mut DebuggerSettings)) {
        let Some(name) = self.open_debugger_name().map(str::to_string) else {
            return;
        };
        let mut settings = self.debugger_settings(&name);
        change(&mut settings);
        if settings == DebuggerSettings::default() {
            self.config.debug.debuggers.remove(&name);
        } else {
            self.config.debug.debuggers.insert(name, settings);
        }
    }

    /// The rows of the Debug tab.
    pub(super) fn debug_tab_rows(&self) -> Vec<ContentRow> {
        use ContentRow::{Debugger, Field, Header, Spacer};
        let t = i18n::t();
        let mut rows = vec![
            Header(t.settings_header_general()),
            Field(NATIVE),
            Field(CONSOLE),
            Spacer,
            Header(t.settings_header_debuggers()),
        ];
        rows.extend((0..DEBUGGERS.len()).map(Debugger));
        rows
    }

    /// The rows of the debugger page.
    pub(super) fn debugger_page_rows(&self) -> Vec<ContentRow> {
        use ContentRow::{DebuggerButtons, Field, Header, Spacer};
        vec![
            Header(i18n::t().settings_header_debugger()),
            Field(COMMAND),
            Field(ARGS),
            Spacer,
            DebuggerButtons,
        ]
    }

    /// What a debugger's row shows beside its name: its program and flags,
    /// the defaults' marked as such.
    pub(super) fn debugger_summary(&self, name: &str) -> String {
        let t = i18n::t();
        let settings = self.debugger_settings(name);
        let program = settings
            .command
            .unwrap_or_else(|| t.settings_value_auto().to_string());
        match settings.args {
            Some(args) if args.is_empty() => program,
            Some(args) => format!("{program} {}", join_flags(&args)),
            None => match self.default_flags(name) {
                [] => program,
                flags => format!(
                    "{program} {}",
                    t.settings_debugger_default_fmt(&join_flags(flags))
                ),
            },
        }
    }

    /// Open debugger `name` on its page.
    pub(super) fn open_debugger(&mut self, name: &str) {
        if !DEBUGGERS.contains(&name) {
            return;
        }
        self.debugger_edit = Some(DebuggerEdit {
            name: name.to_string(),
            error: None,
            button: BACK,
        });
        self.enum_picker = None;
        self.editing = false;
        self.content_scroll = 0;
        self.field_cursor = self.first_selectable_row();
    }

    /// Back from the page to the list, on the row of the debugger it had
    /// open.
    pub(super) fn close_debugger(&mut self) {
        let Some(edit) = self.debugger_edit.take() else {
            return;
        };
        self.enum_picker = None;
        self.editing = false;
        let rows = self.content_rows();
        self.field_cursor = DEBUGGERS
            .iter()
            .position(|name| *name == edit.name)
            .and_then(|index| {
                rows.iter()
                    .position(|row| *row == ContentRow::Debugger(index))
            })
            .unwrap_or_else(|| self.first_selectable_row());
    }

    /// Choose the button to the left or right on the buttons row.
    pub(super) fn step_debugger_button(&mut self, forward: bool) {
        if let Some(edit) = self.debugger_edit.as_mut() {
            edit.button = if forward { DELETE } else { BACK };
        }
    }

    /// Press the chosen button: back to the list, or reset the debugger —
    /// as the settings' Reset does, to the global settings in a project
    /// (a project file cannot unset what the global one sets) and to the
    /// defaults in the global file.
    pub(super) fn press_debugger_button(&mut self) {
        let Some(edit) = self.debugger_edit.as_mut() else {
            return;
        };
        if edit.button == DELETE {
            edit.error = None;
            let name = edit.name.clone();
            let target = self
                .in_project
                .then(|| self.global_layer.debug.debuggers.get(&name).cloned())
                .flatten();
            let changed = match target {
                Some(global) => {
                    self.config.debug.debuggers.insert(name, global.clone()) != Some(global)
                }
                None => self.config.debug.debuggers.remove(&name).is_some(),
            };
            if changed {
                self.mark_dirty();
            }
        } else {
            self.close_debugger();
        }
    }

    /// A page field's value as the row shows it.
    pub(super) fn debugger_value(&self, index: usize) -> String {
        let t = i18n::t();
        let Some(name) = self.open_debugger_name() else {
            return String::new();
        };
        let settings = self.debugger_settings(name);
        match index {
            COMMAND => settings
                .command
                .unwrap_or_else(|| t.settings_value_auto().to_string()),
            ARGS => match settings.args {
                Some(args) if args.is_empty() => t.settings_value_none().to_string(),
                Some(args) => join_flags(&args),
                None => match self.default_flags(name) {
                    [] => t.settings_debugger_no_defaults().to_string(),
                    flags => t.settings_debugger_default_fmt(&join_flags(flags)),
                },
            },
            _ => String::new(),
        }
    }

    /// A field's value as it is edited: the stored text, or for flags left
    /// at their defaults, the defaults, to edit from.
    pub(super) fn debugger_edit_text(&self, index: usize) -> String {
        let Some(name) = self.open_debugger_name() else {
            return String::new();
        };
        let settings = self.debugger_settings(name);
        match index {
            COMMAND => settings.command.unwrap_or_default(),
            ARGS => match settings.args {
                Some(args) => join_flags(&args),
                None => join_flags(self.default_flags(name)),
            },
            _ => String::new(),
        }
    }

    /// Store a text field typed on the page. An empty command goes back to
    /// the one found on `PATH`; empty flags mean none (the reset button
    /// brings the defaults back).
    pub(super) fn apply_debugger_text(&mut self, index: usize, text: &str) {
        let text = text.trim();
        match index {
            COMMAND => {
                let command = (!text.is_empty()).then(|| text.to_string());
                self.update_debugger(|s| s.command = command);
            }
            ARGS => match termide_config::split_words(text) {
                Some(flags) => {
                    if let Some(edit) = self.debugger_edit.as_mut() {
                        edit.error = None;
                    }
                    self.update_debugger(|s| s.args = Some(flags));
                }
                None => {
                    if let Some(edit) = self.debugger_edit.as_mut() {
                        edit.error =
                            Some((ARGS, i18n::t().settings_debugger_bad_args().to_string()));
                    }
                }
            },
            _ => {}
        }
    }

    /// Store the native debuggers typed on the Debug tab; an empty list
    /// brings the default order back.
    pub(super) fn apply_debug_native(&mut self, text: &str) {
        let list = super::fields::split_list(text);
        self.config.debug.native = if list.is_empty() {
            termide_config::DebugSettings::default().native
        } else {
            list
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FocusArea, SettingsTab};
    use crate::Modal;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{buffer::Buffer, layout::Rect};
    use termide_config::Config;

    fn debug_modal() -> SettingsModal {
        let config = Config::default();
        let mut modal = SettingsModal::new(config.clone(), config, false).with_debugger_defaults(
            HashMap::from([(
                "dlv-dap".to_string(),
                vec!["--only-same-user=false".to_string()],
            )]),
        );
        modal.active_tab = SettingsTab::Debug;
        modal.focus = FocusArea::Content;
        modal
    }

    fn focus(modal: &mut SettingsModal, row: ContentRow) {
        modal.field_cursor = modal
            .content_rows()
            .iter()
            .position(|r| *r == row)
            .unwrap_or_else(|| panic!("{row:?} is not listed"));
    }

    fn press(modal: &mut SettingsModal, code: KeyCode) {
        let chord = termide_core::KeyChord::identity(KeyEvent::new(code, KeyModifiers::NONE));
        modal.handle_key(chord).unwrap();
    }

    fn edit(modal: &mut SettingsModal, field: usize, text: &str) {
        focus(modal, ContentRow::Field(field));
        press(modal, KeyCode::Enter);
        modal.edit_input.set_text("");
        for c in text.chars() {
            press(modal, KeyCode::Char(c));
        }
        press(modal, KeyCode::Enter);
    }

    fn screen(modal: &mut SettingsModal) -> String {
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        modal.render(area, &mut buf, &termide_theme::Theme::default());
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn dlv_row() -> ContentRow {
        ContentRow::Debugger(DEBUGGERS.iter().position(|d| *d == "dlv-dap").unwrap())
    }

    #[test]
    fn flags_split_as_a_shell_would_and_join_back() {
        assert_eq!(
            termide_config::split_words("--log --log-output=dap,rpc 'a b' \"c\\\"d\""),
            Some(vec![
                "--log".to_string(),
                "--log-output=dap,rpc".to_string(),
                "a b".to_string(),
                "c\"d".to_string(),
            ])
        );
        assert_eq!(termide_config::split_words("'open"), None);
        assert_eq!(termide_config::split_words("  "), Some(Vec::new()));
        let flags = termide_config::split_words("--x 'a b' it\\'s").unwrap();
        assert_eq!(
            termide_config::split_words(&join_flags(&flags)),
            Some(flags)
        );
    }

    /// Stored in the project, the program and flags are the global
    /// settings' to set: the page shows them and says so, and leaves them be.
    #[test]
    fn a_project_cannot_set_a_debugger_program() {
        let mut modal = debug_modal();
        modal.in_project = true;
        focus(&mut modal, dlv_row());
        press(&mut modal, KeyCode::Enter);
        edit(&mut modal, COMMAND, "/tmp/evil");
        assert!(!modal.editing);
        assert_eq!(modal.debugger_settings("dlv-dap").command, None);
        let shown = screen(&mut modal);
        assert!(shown.contains(i18n::t().settings_global_only()), "{shown}");
    }

    #[test]
    fn the_list_shows_every_debugger_and_the_defaults_it_gets() {
        let mut modal = debug_modal();
        let shown = screen(&mut modal);
        for name in DEBUGGERS {
            assert!(shown.contains(name), "{name} missing:\n{shown}");
        }
        assert!(shown.contains("--only-same-user=false"), "{shown}");
    }

    #[test]
    fn a_debugger_page_sets_its_program_and_flags_and_resets_them() {
        let mut modal = debug_modal();
        focus(&mut modal, dlv_row());
        press(&mut modal, KeyCode::Enter);
        assert_eq!(modal.open_debugger_name(), Some("dlv-dap"));

        // The flags field starts from the defaults when none are set.
        focus(&mut modal, ContentRow::Field(ARGS));
        press(&mut modal, KeyCode::Enter);
        assert_eq!(modal.edit_input.text(), "--only-same-user=false");
        press(&mut modal, KeyCode::Esc);

        edit(&mut modal, COMMAND, "/opt/go/bin/dlv");
        edit(&mut modal, ARGS, "--only-same-user=false --log");
        let settings = modal.config.debug.debugger("dlv-dap");
        assert_eq!(settings.command.as_deref(), Some("/opt/go/bin/dlv"));
        assert_eq!(
            settings.args,
            Some(vec!["--only-same-user=false".into(), "--log".into()])
        );
        assert!(modal.save_pending);

        // Emptied flags mean none, not the defaults.
        edit(&mut modal, ARGS, "");
        assert_eq!(
            modal.config.debug.debugger("dlv-dap").args,
            Some(Vec::new())
        );

        // An open quote is refused and said so.
        edit(&mut modal, ARGS, "'--log");
        assert_eq!(
            modal.config.debug.debugger("dlv-dap").args,
            Some(Vec::new())
        );
        assert!(screen(&mut modal).contains(i18n::t().settings_debugger_bad_args()));

        // Reset drops the entry: back to the program on PATH and the
        // defaults.
        focus(&mut modal, ContentRow::DebuggerButtons);
        modal.step_debugger_button(true);
        press(&mut modal, KeyCode::Enter);
        assert!(!modal.config.debug.debuggers.contains_key("dlv-dap"));

        press(&mut modal, KeyCode::Esc);
        assert!(modal.debugger_edit.is_none());
        assert_eq!(modal.current_row(), Some(dlv_row()));
    }

    /// A project file cannot unset what the global one sets, so Reset in a
    /// project goes back to the global entry, as the reload would.
    #[test]
    fn reset_in_a_project_restores_the_global_entry() {
        let global_entry = DebuggerSettings {
            command: Some("/opt/llvm/bin/lldb-dap".into()),
            args: None,
        };
        let mut global = Config::default();
        global
            .debug
            .debuggers
            .insert("lldb-dap".into(), global_entry.clone());
        let mut modal = SettingsModal::new(global.clone(), global, true);
        modal.active_tab = SettingsTab::Debug;
        modal.focus = FocusArea::Content;
        modal.open_debugger("lldb-dap");
        edit(&mut modal, COMMAND, "/usr/bin/lldb-dap");

        focus(&mut modal, ContentRow::DebuggerButtons);
        modal.step_debugger_button(true);
        press(&mut modal, KeyCode::Enter);
        assert_eq!(modal.config.debug.debugger("lldb-dap"), global_entry);
    }

    #[test]
    fn emptying_the_command_keeps_no_empty_entry() {
        let mut modal = debug_modal();
        modal.open_debugger("lldb-dap");
        edit(&mut modal, COMMAND, "/opt/llvm/bin/lldb-dap");
        assert!(modal.config.debug.debuggers.contains_key("lldb-dap"));
        edit(&mut modal, COMMAND, "");
        assert!(!modal.config.debug.debuggers.contains_key("lldb-dap"));
    }

    #[test]
    fn the_tab_sets_the_native_order_and_the_console() {
        let mut modal = debug_modal();
        edit(&mut modal, NATIVE, "gdb-dap, lldb");
        assert_eq!(modal.config.debug.native, ["gdb-dap", "lldb"]);
        edit(&mut modal, NATIVE, "");
        assert_eq!(modal.config.debug.native, ["lldb-dap", "gdb-dap", "lldb"]);
        focus(&mut modal, ContentRow::Field(CONSOLE));
        press(&mut modal, KeyCode::Right);
        assert_eq!(modal.config.debug.console, "panel");
    }
}
