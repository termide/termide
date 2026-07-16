//! Keybindings tab: static binding tables and per-section get/set.
//!
//! All data here is pure config access — no UI state. The rendering and
//! key handling for this tab still live in the parent `settings` module
//! because they touch `SettingsModal` state.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termide_config::{Config, KeyBinding};

/// Keybinding section names shown in the sidebar.
pub(super) const KB_SECTIONS: [&str; 7] = [
    "Global",
    "Editor",
    "FileManager",
    "GitStatus",
    "GitDiff",
    "GitLog",
    "Terminal",
];

macro_rules! kb_get {
    ($kb:expr, $name:expr, $($field:ident),* $(,)?) => {{
        let kb = &$kb;
        let name = $name;
        $(if name == stringify!($field) {
            kb.$field.as_ref().map(|b: &termide_config::KeyBinding| b.display().to_string()).unwrap_or_default()
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
            "open_sessions",
            "new_session",
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
            "open_command_palette",
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
            open_sessions,
            new_session,
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
            open_command_palette,
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
        _ => String::new(),
    }
}

/// Set a binding.
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
            open_sessions,
            new_session,
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
            open_command_palette,
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
        _ => {}
    }
}

/// Format a `KeyEvent` into a keybinding string like "Ctrl+S".
///
/// The event is canonicalized first (Cyrillic→Latin, shifted-glyph
/// punctuation → `Shift+<unshifted>`, VTE Ctrl+7→Ctrl+/, caps-lock
/// strip when reported). That keeps the string the picker stores in
/// the same canonical form as defaults — picker on a Russian layout
/// records `"Alt+M"`, not `"Alt+Ь"`.
pub(super) fn format_key_event(raw: &KeyEvent) -> String {
    let normalizer = termide_keyboard::KeyNormalizer::default();
    let canon = normalizer.canonicalize(*raw);
    let key = &canon;
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
            let s = c.to_uppercase().to_string();
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
