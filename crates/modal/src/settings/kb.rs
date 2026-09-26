//! Keybindings tab: static binding tables and per-section get/set.
//!
//! All data here is pure config access — no UI state. The rendering and
//! key handling for this tab still live in the parent `settings` module
//! because they touch `SettingsModal` state.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termide_config::{Config, KeyBinding};

/// Keybinding section names shown in the sidebar.
pub(super) const KB_SECTIONS: [&str; 9] = [
    "Global",
    "Editor",
    "FileManager",
    "GitStatus",
    "GitDiff",
    "GitLog",
    "Terminal",
    "Database",
    "Viewer",
];

macro_rules! kb_get {
    ($kb:expr, $name:expr, $($field:ident),* $(,)?) => {{
        let kb = &$kb;
        let name = $name;
        $(if name == stringify!($field) {
            kb.$field.as_ref().map(|b: &termide_config::KeyBinding| b.display_all()).unwrap_or_default()
        } else)* { String::new() }
    }};
}

macro_rules! kb_set {
    ($kb:expr, $name:expr, $value:expr, $($field:ident),* $(,)?) => {{
        let kb = &mut $kb;
        let name = $name;
        let v = $value;
        $(if name == stringify!($field) { kb.$field = Some(v); return; })*
    }};
}

/// Get binding names for a section.
pub(super) fn kb_binding_names(section: usize) -> &'static [&'static str] {
    match section {
        0 => &[
            "toggle_menu",
            "new_file_manager",
            "new_terminal",
            "new_editor",
            "new_journal",
            "open_help",
            "open_preferences",
            "open_projects",
            "new_project",
            "open_git_status",
            "open_bookmark_add",
            "open_outline",
            "open_diagnostics",
            "open_git_log",
            "close_panel",
            "toggle_stack",
            "swap_left",
            "swap_right",
            "move_first",
            "move_last",
            "resize_smaller",
            "resize_larger",
            "toggle_fullscreen_panel",
            "panel_grow_vertical",
            "panel_shrink_vertical",
            "panel_action_menu",
            "prev_group",
            "next_group",
            "prev_panel",
            "next_panel",
            "goto_panel_1",
            "goto_panel_2",
            "goto_panel_3",
            "goto_panel_4",
            "goto_panel_5",
            "goto_panel_6",
            "goto_panel_7",
            "goto_panel_8",
            "goto_panel_9",
            "quit",
            "detach_instance",
            "open_command_palette",
            "open_path",
            "copy",
            "cut",
            "paste",
        ],
        1 => &[
            "save",
            "save_as",
            "reload",
            "undo",
            "redo",
            "duplicate_line",
            "delete_line",
            "toggle_comment",
            "search",
            "search_next",
            "search_prev",
            "replace",
            "replace_current",
            "replace_all",
            "select_all",
            "trigger_completion",
            "show_hover",
            "goto_definition",
            "find_references",
            "rename_symbol",
        ],
        2 => &[
            "rename",
            "view",
            "edit",
            "copy",
            "move_item",
            "create_dir",
            "create_file",
            "delete",
            "info",
            "search",
            "search_content",
            "search_replace",
            "refresh",
            "go_parent",
            "go_home",
            "switch_directory",
            "go_to_path",
            "toggle_selection",
            "select_all",
            "open_external",
            "toggle_hidden",
        ],
        3 => &[
            "stage", "unstage", "view", "edit", "info", "revert", "refresh",
        ],
        4 => &[
            "toggle_collapse",
            "edit",
            "refresh",
            "scroll_half_up",
            "scroll_half_down",
        ],
        5 => &["info", "view_diff", "checkout"],
        6 => &[
            "scroll_up",
            "scroll_down",
            "scroll_top",
            "scroll_bottom",
            "search",
            "switch_directory",
        ],
        7 => &[
            "sort",
            "filter",
            "clear_filter",
            "detail",
            "copy_row",
            "refresh",
        ],
        8 => &["toggle_hex", "toggle_view"],
        _ => &[],
    }
}

/// Get a binding's display string.
pub(super) fn get_kb_value(config: &Config, section: usize, name: &str) -> String {
    match section {
        0 => kb_get!(
            config.general.keybindings,
            name,
            toggle_menu,
            new_file_manager,
            new_terminal,
            new_editor,
            new_journal,
            open_help,
            open_preferences,
            open_projects,
            new_project,
            open_git_status,
            open_bookmark_add,
            open_outline,
            open_diagnostics,
            open_git_log,
            close_panel,
            toggle_stack,
            swap_left,
            swap_right,
            move_first,
            move_last,
            resize_smaller,
            resize_larger,
            toggle_fullscreen_panel,
            panel_grow_vertical,
            panel_shrink_vertical,
            panel_action_menu,
            prev_group,
            next_group,
            prev_panel,
            next_panel,
            goto_panel_1,
            goto_panel_2,
            goto_panel_3,
            goto_panel_4,
            goto_panel_5,
            goto_panel_6,
            goto_panel_7,
            goto_panel_8,
            goto_panel_9,
            quit,
            detach_instance,
            open_command_palette,
            open_path,
            copy,
            cut,
            paste
        ),
        1 => kb_get!(
            config.editor.keybindings,
            name,
            save,
            save_as,
            reload,
            undo,
            redo,
            duplicate_line,
            delete_line,
            toggle_comment,
            search,
            search_next,
            search_prev,
            replace,
            replace_current,
            replace_all,
            select_all,
            trigger_completion,
            show_hover,
            goto_definition,
            find_references,
            rename_symbol
        ),
        2 => kb_get!(
            config.file_manager.keybindings,
            name,
            rename,
            view,
            edit,
            copy,
            move_item,
            create_dir,
            create_file,
            delete,
            info,
            search,
            search_content,
            search_replace,
            refresh,
            go_parent,
            go_home,
            switch_directory,
            go_to_path,
            toggle_selection,
            select_all,
            open_external,
            toggle_hidden
        ),
        3 => kb_get!(
            config.git_status.keybindings,
            name,
            stage,
            unstage,
            view,
            edit,
            info,
            revert,
            refresh
        ),
        4 => kb_get!(
            config.git_diff.keybindings,
            name,
            toggle_collapse,
            edit,
            refresh,
            scroll_half_up,
            scroll_half_down
        ),
        5 => kb_get!(config.git_log.keybindings, name, info, view_diff, checkout),
        6 => kb_get!(
            config.terminal.keybindings,
            name,
            scroll_up,
            scroll_down,
            scroll_top,
            scroll_bottom,
            search,
            switch_directory
        ),
        7 => kb_get!(
            config.database.keybindings,
            name,
            sort,
            filter,
            clear_filter,
            detail,
            copy_row,
            refresh
        ),
        8 => kb_get!(config.viewer.keybindings, name, toggle_hex, toggle_view),
        _ => String::new(),
    }
}

/// Set a binding.
/// Read a binding as a value, not as text — Delete needs to remove one key
/// from it and put the rest back.
pub(super) fn get_kb_binding(config: &Config, section: usize, name: &str) -> Option<KeyBinding> {
    let text = get_kb_value(config, section, name);
    if text.is_empty() {
        return None;
    }
    let keys: Vec<String> = text.split(", ").map(|s| s.to_string()).collect();
    Some(if keys.len() == 1 {
        KeyBinding::Single(keys.into_iter().next().unwrap_or_default())
    } else {
        KeyBinding::Multiple(keys)
    })
}

pub(super) fn set_kb_value(config: &mut Config, section: usize, name: &str, value: KeyBinding) {
    match section {
        0 => kb_set!(
            config.general.keybindings,
            name,
            value,
            toggle_menu,
            new_file_manager,
            new_terminal,
            new_editor,
            new_journal,
            open_help,
            open_preferences,
            open_projects,
            new_project,
            open_git_status,
            open_bookmark_add,
            open_outline,
            open_diagnostics,
            open_git_log,
            close_panel,
            toggle_stack,
            swap_left,
            swap_right,
            move_first,
            move_last,
            resize_smaller,
            resize_larger,
            toggle_fullscreen_panel,
            panel_grow_vertical,
            panel_shrink_vertical,
            panel_action_menu,
            prev_group,
            next_group,
            prev_panel,
            next_panel,
            goto_panel_1,
            goto_panel_2,
            goto_panel_3,
            goto_panel_4,
            goto_panel_5,
            goto_panel_6,
            goto_panel_7,
            goto_panel_8,
            goto_panel_9,
            quit,
            detach_instance,
            open_command_palette,
            open_path,
            copy,
            cut,
            paste
        ),
        1 => kb_set!(
            config.editor.keybindings,
            name,
            value,
            save,
            save_as,
            reload,
            undo,
            redo,
            duplicate_line,
            delete_line,
            toggle_comment,
            search,
            search_next,
            search_prev,
            replace,
            replace_current,
            replace_all,
            select_all,
            trigger_completion,
            show_hover,
            goto_definition,
            find_references,
            rename_symbol
        ),
        2 => kb_set!(
            config.file_manager.keybindings,
            name,
            value,
            rename,
            view,
            edit,
            copy,
            move_item,
            create_dir,
            create_file,
            delete,
            info,
            search,
            search_content,
            search_replace,
            refresh,
            go_parent,
            go_home,
            switch_directory,
            go_to_path,
            toggle_selection,
            select_all,
            open_external,
            toggle_hidden
        ),
        3 => kb_set!(
            config.git_status.keybindings,
            name,
            value,
            stage,
            unstage,
            view,
            edit,
            info,
            revert,
            refresh
        ),
        4 => kb_set!(
            config.git_diff.keybindings,
            name,
            value,
            toggle_collapse,
            edit,
            refresh,
            scroll_half_up,
            scroll_half_down
        ),
        5 => kb_set!(
            config.git_log.keybindings,
            name,
            value,
            info,
            view_diff,
            checkout
        ),
        6 => kb_set!(
            config.terminal.keybindings,
            name,
            value,
            scroll_up,
            scroll_down,
            scroll_top,
            scroll_bottom,
            search,
            switch_directory
        ),
        7 => kb_set!(
            config.database.keybindings,
            name,
            value,
            sort,
            filter,
            clear_filter,
            detail,
            copy_row,
            refresh
        ),
        8 => kb_set!(
            config.viewer.keybindings,
            name,
            value,
            toggle_hex,
            toggle_view
        ),
        _ => {}
    }
}

/// Format an already-canonical `KeyEvent` into a keybinding string like
/// `"Ctrl+S"`.
///
/// Callers must pass `KeyChord::canonical`, which the dispatch boundary
/// produced with the *live* terminal capabilities (Cyrillic→Latin,
/// shifted-glyph punctuation → `Shift+<unshifted>`, VTE Ctrl+7→Ctrl+/
/// only on non-Kitty terminals, caps-lock strip when reported). That
/// keeps the string the picker stores in the same canonical form as the
/// defaults — the picker on a Russian layout records `"Alt+M"`, not
/// `"Alt+Ь"`. Canonicalizing again here would apply a *different*
/// capability set (`KeyNormalizer::default()` claims no Kitty support)
/// and record chords the matcher can never see, e.g. `Ctrl+7` as
/// `Ctrl+/`.
pub(super) fn format_key_event(key: &KeyEvent) -> String {
    let mut parts = Vec::new();
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        parts.push("Alt");
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("Shift");
    }

    let key_str = match key.code {
        KeyCode::Enter => "Enter",
        KeyCode::Esc => "Esc",
        KeyCode::Tab => "Tab",
        KeyCode::Backspace => "Backspace",
        KeyCode::Delete => "Delete",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::PageUp => "PageUp",
        KeyCode::PageDown => "PageDown",
        KeyCode::Up => "Up",
        KeyCode::Down => "Down",
        KeyCode::Left => "Left",
        KeyCode::Right => "Right",
        KeyCode::F(n) => {
            return format!(
                "{}F{}",
                if parts.is_empty() {
                    String::new()
                } else {
                    parts.join("+") + "+"
                },
                n
            )
        }
        KeyCode::Char(' ') => "Space",
        KeyCode::Char(c) => {
            // ASCII-only upper-casing: `parse_key` is case-insensitive for
            // ASCII, but the matcher compares non-ASCII chars as they came,
            // so upper-casing e.g. the macOS composed glyph `ƒ` to `Ƒ` would
            // store a binding no keypress can ever match.
            let s = if c.is_ascii() {
                c.to_ascii_uppercase().to_string()
            } else {
                c.to_string()
            };
            if parts.is_empty() {
                return s;
            }
            parts.push(&s);
            return parts.join("+");
        }
        _ => return String::new(),
    };
    parts.push(key_str);
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn ev(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    /// Every name the sidebar lists has to be readable and writable. The
    /// `kb_get!` / `kb_set!` macros compare the name against field
    /// identifiers, so a typo in one of these arrays does not fail to
    /// compile — the row just shows an empty binding and silently refuses to
    /// take a new one.
    #[test]
    fn every_listed_binding_is_wired_to_a_field() {
        // `normalize` is what fills in the defaults; a bare `default()` leaves
        // every binding `None` and would make this test pass vacuously.
        let mut config = Config::default();
        config.normalize();
        for (section, label) in KB_SECTIONS.iter().enumerate() {
            let names = kb_binding_names(section);
            assert!(
                !names.is_empty(),
                "section {section} ({label}) lists no bindings"
            );
            for name in names {
                assert!(
                    !get_kb_value(&config, section, name).is_empty(),
                    "{label}.{name} reads back empty — name does not match a field"
                );

                let probe = KeyBinding::Single("Ctrl+Alt+F19".to_string());
                set_kb_value(&mut config, section, name, probe);
                assert_eq!(
                    get_kb_value(&config, section, name),
                    "Ctrl+Alt+F19",
                    "{label}.{name} did not take a new binding"
                );
            }
        }
    }

    /// A section index past the end must not panic; the sidebar clamps, but
    /// the accessors are the ones that would go out of bounds.
    #[test]
    fn an_unknown_section_reads_empty_and_writes_nothing() {
        let mut config = Config::default();
        assert!(get_kb_value(&config, 99, "sort").is_empty());
        set_kb_value(
            &mut config,
            99,
            "sort",
            KeyBinding::Single("Ctrl+X".to_string()),
        );
    }

    #[test]
    fn formats_alt_letter_chord() {
        assert_eq!(
            format_key_event(&ev(KeyCode::Char('f'), KeyModifiers::ALT)),
            "Alt+F"
        );
    }

    /// Regression: whatever the picker stores must be matchable. A macOS
    /// composed glyph (`Option+F` → `ƒ` when the Kitty all-keys flag is
    /// off) used to be upper-cased to `Ƒ`, which no keypress could match.
    #[test]
    fn captured_non_ascii_char_round_trips_to_a_matching_binding() {
        let event = ev(KeyCode::Char('ƒ'), KeyModifiers::empty());
        let binding = format_key_event(&event);
        assert_eq!(binding, "ƒ");
        assert!(
            termide_config::parse_keybinding(&binding)
                .unwrap()
                .matches(&event),
            "binding {binding:?} captured from Char('ƒ') must match it back"
        );
    }
}
