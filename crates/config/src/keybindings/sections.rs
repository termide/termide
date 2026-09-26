//! Per-section keybinding tables (global, editor, file-manager, git, viewer,
//! terminal, database) with their TOML defaults and key-matching logic.

use serde::{Deserialize, Serialize};

use super::KeyBinding;

/// Global keybindings (general.keybindings section).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalKeybindings {
    // Menu & UI
    pub toggle_menu: Option<KeyBinding>,

    // Panel creation
    pub new_file_manager: Option<KeyBinding>,
    pub new_terminal: Option<KeyBinding>,
    pub new_editor: Option<KeyBinding>,
    pub new_journal: Option<KeyBinding>,
    pub open_help: Option<KeyBinding>,
    pub open_preferences: Option<KeyBinding>,
    /// Open the project switcher. The old `open_sessions` spelling is still
    /// accepted so keybindings written before the rename keep working.
    #[serde(alias = "open_sessions")]
    pub open_projects: Option<KeyBinding>,
    /// Start a new project. Old spelling accepted, as above.
    #[serde(alias = "new_session")]
    pub new_project: Option<KeyBinding>,
    pub open_git_status: Option<KeyBinding>,
    pub open_bookmark_add: Option<KeyBinding>,
    pub open_outline: Option<KeyBinding>,
    pub open_agent: Option<KeyBinding>,
    pub open_diagnostics: Option<KeyBinding>,
    pub open_git_log: Option<KeyBinding>,

    // Panel management
    pub close_panel: Option<KeyBinding>,
    pub toggle_stack: Option<KeyBinding>,
    pub swap_left: Option<KeyBinding>,
    pub swap_right: Option<KeyBinding>,
    pub move_first: Option<KeyBinding>,
    pub move_last: Option<KeyBinding>,
    pub resize_smaller: Option<KeyBinding>,
    pub resize_larger: Option<KeyBinding>,
    /// Toggle accordion / split layout for the active panel group.
    pub toggle_fullscreen_panel: Option<KeyBinding>,
    /// Grow the focused panel's height in split mode.
    pub panel_grow_vertical: Option<KeyBinding>,
    /// Shrink the focused panel's height in split mode.
    pub panel_shrink_vertical: Option<KeyBinding>,
    /// Open the active panel's action context menu (the `[≡]` button dropdown).
    pub panel_action_menu: Option<KeyBinding>,

    // Navigation
    pub prev_group: Option<KeyBinding>,
    pub next_group: Option<KeyBinding>,
    pub prev_panel: Option<KeyBinding>,
    pub next_panel: Option<KeyBinding>,
    pub goto_panel_1: Option<KeyBinding>,
    pub goto_panel_2: Option<KeyBinding>,
    pub goto_panel_3: Option<KeyBinding>,
    pub goto_panel_4: Option<KeyBinding>,
    pub goto_panel_5: Option<KeyBinding>,
    pub goto_panel_6: Option<KeyBinding>,
    pub goto_panel_7: Option<KeyBinding>,
    pub goto_panel_8: Option<KeyBinding>,
    pub goto_panel_9: Option<KeyBinding>,

    // Application
    pub quit: Option<KeyBinding>,
    /// Detach from the attached client, leaving the instance running.
    /// Only meaningful when termide is hosted in a detached instance.
    /// The old `detach_session` spelling is still accepted so keybindings
    /// written before the rename keep working.
    #[serde(alias = "detach_session")]
    pub detach_instance: Option<KeyBinding>,
    pub open_command_palette: Option<KeyBinding>,
    /// Open a file, directory or URL through the prompt with suggestions.
    /// The file manager keeps the key for its own "go to path", and a
    /// terminal passes it on to the program running in it.
    pub open_path: Option<KeyBinding>,

    // Clipboard (routed to the focused panel, which copies/cuts/pastes
    // according to its own capabilities).
    pub copy: Option<KeyBinding>,
    pub cut: Option<KeyBinding>,
    pub paste: Option<KeyBinding>,
}

/// Editor keybindings (editor.keybindings section).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EditorKeybindings {
    // File operations
    pub save: Option<KeyBinding>,
    pub save_as: Option<KeyBinding>,
    pub reload: Option<KeyBinding>,

    // Editing
    pub undo: Option<KeyBinding>,
    pub redo: Option<KeyBinding>,
    pub duplicate_line: Option<KeyBinding>,
    pub delete_line: Option<KeyBinding>,
    pub toggle_comment: Option<KeyBinding>,

    // Search & Replace
    pub search: Option<KeyBinding>,
    pub search_next: Option<KeyBinding>,
    pub search_prev: Option<KeyBinding>,
    pub replace: Option<KeyBinding>,
    pub replace_current: Option<KeyBinding>,
    pub replace_all: Option<KeyBinding>,

    // Selection
    pub select_all: Option<KeyBinding>,

    // LSP
    pub trigger_completion: Option<KeyBinding>,
    pub show_hover: Option<KeyBinding>,
    pub goto_definition: Option<KeyBinding>,
    pub find_references: Option<KeyBinding>,
    pub rename_symbol: Option<KeyBinding>,
    pub code_action: Option<KeyBinding>,
}

/// File manager keybindings (file_manager.keybindings section).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileManagerKeybindings {
    // File operations
    pub rename: Option<KeyBinding>,
    pub view: Option<KeyBinding>,
    pub edit: Option<KeyBinding>,
    pub copy: Option<KeyBinding>,
    pub move_item: Option<KeyBinding>,
    pub create_dir: Option<KeyBinding>,
    pub create_file: Option<KeyBinding>,
    pub delete: Option<KeyBinding>,
    pub info: Option<KeyBinding>,

    // Search
    pub search: Option<KeyBinding>,
    pub search_content: Option<KeyBinding>,
    pub search_replace: Option<KeyBinding>,

    // Navigation
    pub refresh: Option<KeyBinding>,
    pub go_parent: Option<KeyBinding>,
    pub go_home: Option<KeyBinding>,
    pub switch_directory: Option<KeyBinding>,
    pub go_to_path: Option<KeyBinding>,

    // Selection
    pub toggle_selection: Option<KeyBinding>,
    pub select_all: Option<KeyBinding>,

    // Other
    pub open_external: Option<KeyBinding>,
    pub toggle_hidden: Option<KeyBinding>,
}

/// Git status panel keybindings (git_status.keybindings section).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitStatusKeybindings {
    /// Stage selected file
    pub stage: Option<KeyBinding>,
    /// Unstage selected file
    pub unstage: Option<KeyBinding>,
    /// View diff for selected file
    pub view: Option<KeyBinding>,
    /// Edit selected file in editor
    pub edit: Option<KeyBinding>,
    /// Show file info / context menu
    pub info: Option<KeyBinding>,
    /// Revert (discard changes) for selected file
    pub revert: Option<KeyBinding>,
    /// Refresh git status
    pub refresh: Option<KeyBinding>,
}

impl GitStatusKeybindings {
    /// Fill None values with default keybindings
    pub fn with_defaults(&mut self) {
        macro_rules! set_default {
            ($field:ident, $default:expr) => {
                if self.$field.is_none() {
                    self.$field = Some(KeyBinding::Single($default.into()));
                }
            };
        }

        set_default!(stage, "S");
        set_default!(unstage, "U");
        set_default!(view, "F3");
        set_default!(edit, "F4");
        if self.info.is_none() {
            self.info = Some(KeyBinding::Multiple(vec!["Space".into(), "F12".into()]));
        }
        if self.revert.is_none() {
            self.revert = Some(KeyBinding::Multiple(vec![
                "Backspace".into(),
                "Delete".into(),
            ]));
        }
        set_default!(refresh, "Ctrl+R");
    }
}

/// Git diff panel keybindings (git_diff.keybindings section).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitDiffKeybindings {
    /// Toggle collapse of file diff section
    pub toggle_collapse: Option<KeyBinding>,
    /// Edit file in editor
    pub edit: Option<KeyBinding>,
    /// Refresh diff
    pub refresh: Option<KeyBinding>,
    /// Scroll half page up
    pub scroll_half_up: Option<KeyBinding>,
    /// Scroll half page down
    pub scroll_half_down: Option<KeyBinding>,
}

impl GitDiffKeybindings {
    /// Fill None values with default keybindings
    pub fn with_defaults(&mut self) {
        macro_rules! set_default {
            ($field:ident, $default:expr) => {
                if self.$field.is_none() {
                    self.$field = Some(KeyBinding::Single($default.into()));
                }
            };
        }

        if self.toggle_collapse.is_none() {
            self.toggle_collapse = Some(KeyBinding::Multiple(vec!["Enter".into(), "Space".into()]));
        }
        if self.edit.is_none() {
            self.edit = Some(KeyBinding::Multiple(vec!["F4".into(), "E".into()]));
        }
        if self.refresh.is_none() {
            self.refresh = Some(KeyBinding::Multiple(vec!["F5".into(), "Ctrl+R".into()]));
        }
        set_default!(scroll_half_up, "Ctrl+U");
        set_default!(scroll_half_down, "Ctrl+D");
    }
}

/// Git log panel keybindings (git_log.keybindings section).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitLogKeybindings {
    /// Show commit info
    pub info: Option<KeyBinding>,
    /// View commit diff
    pub view_diff: Option<KeyBinding>,
    /// Checkout commit/branch
    pub checkout: Option<KeyBinding>,
}

impl GitLogKeybindings {
    /// Fill None values with default keybindings
    pub fn with_defaults(&mut self) {
        macro_rules! set_default {
            ($field:ident, $default:expr) => {
                if self.$field.is_none() {
                    self.$field = Some(KeyBinding::Single($default.into()));
                }
            };
        }

        set_default!(info, "Space");
        set_default!(view_diff, "D");
        set_default!(checkout, "C");
    }
}

/// Database viewer panel keybindings (database.keybindings section).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DatabaseKeybindings {
    /// Sort by the current column (cycles ascending → descending → unsorted)
    pub sort: Option<KeyBinding>,
    /// Filter the current column
    pub filter: Option<KeyBinding>,
    /// Clear all filters
    pub clear_filter: Option<KeyBinding>,
    /// Show the full current row (detail dialog)
    pub detail: Option<KeyBinding>,
    /// Copy the current row (tab-separated)
    pub copy_row: Option<KeyBinding>,
    /// Reload tables and the current view
    pub refresh: Option<KeyBinding>,
}

impl DatabaseKeybindings {
    /// Fill None values with default keybindings
    pub fn with_defaults(&mut self) {
        macro_rules! set_default {
            ($field:ident, $default:expr) => {
                if self.$field.is_none() {
                    self.$field = Some(KeyBinding::Single($default.into()));
                }
            };
        }

        set_default!(sort, "S");
        // Filter is the table's "search" — mirror the editor/FM find binding.
        if self.filter.is_none() {
            self.filter = Some(KeyBinding::Multiple(vec!["Ctrl+F".into(), "F3".into()]));
        }
        set_default!(clear_filter, "Alt+F");
        if self.detail.is_none() {
            self.detail = Some(KeyBinding::Multiple(vec!["Space".into(), "F12".into()]));
        }
        set_default!(copy_row, "Ctrl+Y");
        set_default!(refresh, "Ctrl+R");
    }
}

/// File viewer panel keybindings (viewer.keybindings section).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ViewerKeybindings {
    /// Toggle between hex and plain-text rendering (binary viewer).
    pub toggle_hex: Option<KeyBinding>,
    /// Toggle between rendered preview and source editing (markdown viewer).
    pub toggle_view: Option<KeyBinding>,
}

impl ViewerKeybindings {
    /// Fill None values with default keybindings.
    pub fn with_defaults(&mut self) {
        macro_rules! set_default {
            ($field:ident, $default:expr) => {
                if self.$field.is_none() {
                    self.$field = Some(KeyBinding::Single($default.into()));
                }
            };
        }

        set_default!(toggle_hex, "Ctrl+L");
        set_default!(toggle_view, "Ctrl+E");
    }
}

/// Terminal panel keybindings (terminal.keybindings section).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TerminalKeybindings {
    pub scroll_up: Option<KeyBinding>,
    pub scroll_down: Option<KeyBinding>,
    pub scroll_top: Option<KeyBinding>,
    pub scroll_bottom: Option<KeyBinding>,
    pub search: Option<KeyBinding>,
    pub switch_directory: Option<KeyBinding>,
}

// =============================================================================
// Default value implementations for config normalization
// =============================================================================

impl GlobalKeybindings {
    /// Drop bindings that are verbatim copies of defaults this version no
    /// longer ships, so the new default can take their place.
    ///
    /// Needed because older versions wrote the whole binding table into
    /// config.toml on the first save (see `Config::save_global`). Those frozen
    /// copies outrank any new default, so `Alt+D` would keep switching panel
    /// groups instead of detaching, and `Alt+W` would keep moving between
    /// panels instead of closing one.
    ///
    /// Only an exact match is dropped. A user who deliberately bound
    /// `next_group` to something of their own — even something containing the
    /// old keys — keeps it.
    fn drop_superseded_defaults(&mut self) {
        fn drop_if_exactly(field: &mut Option<KeyBinding>, legacy: &[&str]) {
            let matches_legacy = match field {
                Some(KeyBinding::Multiple(keys)) => {
                    keys.len() == legacy.len() && keys.iter().zip(legacy).all(|(k, l)| k == l)
                }
                _ => false,
            };
            if matches_legacy {
                *field = None;
            }
        }

        drop_if_exactly(&mut self.prev_group, &["Alt+Left", "Alt+A"]);
        drop_if_exactly(&mut self.next_group, &["Alt+Right", "Alt+D"]);
        drop_if_exactly(&mut self.prev_panel, &["Alt+Up", "Alt+W"]);
        drop_if_exactly(&mut self.next_panel, &["Alt+Down", "Alt+S"]);
        drop_if_exactly(&mut self.close_panel, &["Alt+X", "F10"]);
    }

    /// Fill None values with default keybindings
    pub fn with_defaults(&mut self) {
        self.drop_superseded_defaults();

        macro_rules! set_default {
            ($field:ident, $default:expr) => {
                if self.$field.is_none() {
                    self.$field = Some(KeyBinding::Single($default.into()));
                }
            };
        }

        macro_rules! set_default_multiple {
            ($field:ident, $($default:expr),+) => {
                if self.$field.is_none() {
                    self.$field = Some(KeyBinding::Multiple(vec![$($default.into()),+]));
                }
            };
        }

        // Menu & UI (toggle_menu uses set_default_multiple, defined below)

        // Panel creation
        set_default!(new_file_manager, "Alt+F");
        set_default!(new_terminal, "Alt+T");
        set_default!(new_editor, "Alt+E");
        set_default!(new_journal, "Alt+L");
        // open_help gets F1 alternative below (needs set_default_multiple)
        set_default!(open_preferences, "Alt+P");
        set_default!(open_projects, "Alt+\\");
        set_default!(new_project, "Alt+N");
        set_default!(open_git_status, "Alt+G");
        set_default!(open_bookmark_add, "Alt+B");
        set_default!(open_outline, "Alt+O");
        set_default!(open_agent, "Alt+A");
        set_default!(open_diagnostics, "Alt+I");
        set_default!(open_git_log, "Alt+C");

        // Panel management (close_panel and toggle_stack get F-key alternatives below)
        set_default!(swap_left, "Alt+PageUp");
        set_default!(swap_right, "Alt+PageDown");
        set_default!(move_first, "Alt+Home");
        set_default!(move_last, "Alt+End");
        set_default!(resize_smaller, "Alt+-");
        set_default!(resize_larger, "Alt+=");
        set_default!(toggle_fullscreen_panel, "Alt+F11");
        // Vertical resize: `Alt+Shift+=` / `Alt+Shift+-`. Parallel to
        // horizontal `Alt+=` / `Alt+-` with Shift as the dimension
        // discriminator.
        //
        // Why these work in VTE despite Phase 12's failed
        // `Alt+Shift+Up/Down` attempt: VTE encodes a Shift+punctuation
        // chord by sending the *shifted glyph* (`+` for `Shift+=`,
        // `_` for `Shift+-`) without the Shift modifier. With Alt
        // prefix it becomes `\e+` / `\e_`, which crossterm parses as
        // `Char('+') + Alt` / `Char('_') + Alt`. `KeyNormalizer`
        // (canonicalize step b) reverses that — `'+' → '=' + Shift`,
        // `'_' → '-' + Shift` — yielding `Char('=') + Alt|Shift` /
        // `Char('-') + Alt|Shift`, which match these bindings strictly.
        //
        // The same reversal covers REPORT_ALTERNATE_KEYS, which substitutes
        // the shifted codepoint and clears Shift. It does NOT cover macOS,
        // where the substitute is the Option-composed glyph — `±` on a Latin
        // layout, `«` on a Russian one — so no table can map it back and these
        // two chords do not fire there. `Alt+F11` and the mouse remain.
        set_default!(panel_grow_vertical, "Alt+Shift+=");
        set_default!(panel_shrink_vertical, "Alt+Shift+-");

        // Navigation (with WASD alternatives)
        set_default_multiple!(toggle_menu, "Alt+M", "F9");
        set_default_multiple!(open_help, "Alt+H", "F1");
        set_default_multiple!(close_panel, "Alt+W", "Alt+X", "F10");
        set_default_multiple!(toggle_stack, "Alt+Backspace", "F11");
        set_default_multiple!(panel_action_menu, "Alt+K", "Shift+F10");

        // Arrows only. The WASD alternatives used to double as a workaround
        // for terminals that swallow `Alt+<arrow>` — Ghostty rebinds
        // `Option+Left`/`Right` to `ESC b`/`ESC f` by default — but they held
        // the two letters users reach for first: `D` for detach, as in tmux
        // and screen, and `W` for closing, as everywhere else. Affected
        // terminals are configured directly instead; see doc/*/keybindings.md.
        set_default!(prev_group, "Alt+Left");
        set_default!(next_group, "Alt+Right");
        set_default!(prev_panel, "Alt+Up");
        set_default!(next_panel, "Alt+Down");
        set_default!(goto_panel_1, "Alt+1");
        set_default!(goto_panel_2, "Alt+2");
        set_default!(goto_panel_3, "Alt+3");
        set_default!(goto_panel_4, "Alt+4");
        set_default!(goto_panel_5, "Alt+5");
        set_default!(goto_panel_6, "Alt+6");
        set_default!(goto_panel_7, "Alt+7");
        set_default!(goto_panel_8, "Alt+8");
        set_default!(goto_panel_9, "Alt+9");

        // Application
        set_default!(quit, "Alt+Q");
        // Universal tier on purpose: detaching matters most over SSH, where
        // the terminal is least likely to speak the Kitty protocol that an
        // `Alt+Shift+<letter>` default would require. `Alt+Z` would be the
        // obvious mnemonic and is unusable on macOS: `Option+Z` is the one
        // combination on the US layout that composes an *uppercase* glyph
        // (`Ω`), reported as `Shift+Ω` with no ALT bit at all.
        set_default!(detach_instance, "Alt+D");
        set_default!(open_command_palette, "Ctrl+P");
        set_default!(open_path, "Ctrl+G");

        // Clipboard (routed to the focused panel)
        set_default!(copy, "Ctrl+C");
        set_default!(cut, "Ctrl+X");
        set_default!(paste, "Ctrl+V");
    }
}

impl EditorKeybindings {
    /// Fill None values with default keybindings
    pub fn with_defaults(&mut self) {
        macro_rules! set_default {
            ($field:ident, $default:expr) => {
                if self.$field.is_none() {
                    self.$field = Some(KeyBinding::Single($default.into()));
                }
            };
        }

        macro_rules! set_default_multiple {
            ($field:ident, $($default:expr),+) => {
                if self.$field.is_none() {
                    self.$field = Some(KeyBinding::Multiple(vec![$($default.into()),+]));
                }
            };
        }

        // File operations
        set_default_multiple!(save, "F2", "Ctrl+S");
        set_default!(save_as, "Ctrl+Shift+S");
        set_default!(reload, "Ctrl+Shift+R");

        // Editing
        set_default!(undo, "Ctrl+Z");
        set_default_multiple!(redo, "Ctrl+Y", "Ctrl+Shift+Z");
        set_default!(duplicate_line, "Ctrl+D");
        // F8 mirrors the FileManager "delete" binding — both are
        // "delete the thing under the cursor", and the editor and
        // FM hotkey tables are isolated at runtime, so the clash is
        // semantic, not functional.
        set_default!(delete_line, "F8");
        // De-facto editor standards: `Ctrl+/` and `Ctrl+.`. On VTE
        // legacy terminals `Ctrl+/` reaches us via the `Ctrl+7→Ctrl+/`
        // quirk in `KeyNormalizer`. `Ctrl+.` requires Kitty proto.
        set_default_multiple!(toggle_comment, "Ctrl+/", "Ctrl+.");

        // Search & Replace
        set_default!(search, "Ctrl+F");
        set_default!(search_next, "F3");
        set_default!(search_prev, "Shift+F3");
        set_default!(replace, "Ctrl+H");
        set_default!(replace_current, "Ctrl+R");
        // `Ctrl+Alt+R` is the de-facto IDE standard; `Alt+R` is the
        // universal-tier fallback for terminals that drop Ctrl+Alt.
        set_default_multiple!(replace_all, "Ctrl+Alt+R", "Alt+R");

        // Selection
        set_default!(select_all, "Ctrl+A");

        // LSP
        // - `Ctrl+J` (`\x0A`, control char): universal — always reaches
        //   termide, does not collide with the `Enter` byte (`\r`).
        // - `Ctrl+Space`: convenient where IBus / window manager does
        //   not intercept it as the layout-switch shortcut.
        //
        // `Ctrl+.` is intentionally NOT a fallback here: it is bound to
        // `toggle_comment` in the same section.
        set_default_multiple!(trigger_completion, "Ctrl+J", "Ctrl+Space");
        set_default!(show_hover, "Ctrl+K");
        set_default!(goto_definition, "F12");
        set_default!(rename_symbol, "F4");
        set_default_multiple!(find_references, "Shift+F12", "F24");
        // `Ctrl+.` (VS Code's quick-fix) is taken by `toggle_comment` here, so
        // default to `Alt+Enter` — the classic "show intentions" key, which
        // terminals deliver reliably.
        set_default!(code_action, "Alt+Enter");
    }
}

impl FileManagerKeybindings {
    /// Fill None values with default keybindings
    pub fn with_defaults(&mut self) {
        macro_rules! set_default {
            ($field:ident, $default:expr) => {
                if self.$field.is_none() {
                    self.$field = Some(KeyBinding::Single($default.into()));
                }
            };
        }

        // File operations
        if self.rename.is_none() {
            self.rename = Some(KeyBinding::Multiple(vec!["F2".into(), "R".into()]));
        }
        if self.view.is_none() {
            self.view = Some(KeyBinding::Multiple(vec!["F3".into(), "V".into()]));
        }
        if self.edit.is_none() {
            self.edit = Some(KeyBinding::Multiple(vec!["F4".into(), "E".into()]));
        }
        if self.copy.is_none() {
            self.copy = Some(KeyBinding::Multiple(vec!["F5".into(), "C".into()]));
        }
        if self.move_item.is_none() {
            self.move_item = Some(KeyBinding::Multiple(vec!["F6".into(), "M".into()]));
        }
        if self.create_dir.is_none() {
            self.create_dir = Some(KeyBinding::Multiple(vec!["F7".into(), "D".into()]));
        }
        if self.create_file.is_none() {
            self.create_file = Some(KeyBinding::Multiple(vec!["F".into(), "Ctrl+N".into()]));
        }
        if self.delete.is_none() {
            self.delete = Some(KeyBinding::Multiple(vec!["Delete".into(), "F8".into()]));
        }
        if self.info.is_none() {
            self.info = Some(KeyBinding::Multiple(vec!["F12".into(), "Space".into()]));
        }

        // Search
        set_default!(search, "Ctrl+F");
        set_default!(search_content, "Ctrl+Shift+F");
        set_default!(search_replace, "Ctrl+Shift+H");

        // Navigation
        set_default!(refresh, "Ctrl+R");
        set_default!(go_parent, "Backspace");
        set_default!(go_home, "~");
        // Parallel to global `open_projects = "Alt+\\"`: `Ctrl+\\` for
        // the analogous "switch directory" action. Reaches VTE via
        // the `Ctrl+4→Ctrl+\\` quirk in `KeyNormalizer`.
        set_default!(switch_directory, "Ctrl+\\");
        set_default!(go_to_path, "Ctrl+G");

        // Selection
        set_default!(toggle_selection, "Insert");
        set_default!(select_all, "Ctrl+A");

        // Other
        if self.open_external.is_none() {
            self.open_external = Some(KeyBinding::Multiple(vec!["O".into(), "Alt+Enter".into()]));
        }
        set_default!(toggle_hidden, ".");
    }
}

impl TerminalKeybindings {
    /// Fill None values with default keybindings
    pub fn with_defaults(&mut self) {
        macro_rules! set_default {
            ($field:ident, $default:expr) => {
                if self.$field.is_none() {
                    self.$field = Some(KeyBinding::Single($default.into()));
                }
            };
        }

        set_default!(scroll_up, "Shift+PageUp");
        set_default!(scroll_down, "Shift+PageDown");
        set_default!(scroll_top, "Shift+Home");
        set_default!(scroll_bottom, "Shift+End");
        set_default!(search, "Ctrl+F");
        set_default!(switch_directory, "Ctrl+\\");
    }
}
