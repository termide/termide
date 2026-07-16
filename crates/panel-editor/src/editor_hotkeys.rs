//! Builds the editor's `HotkeyTable` from configuration.

use termide_config::Config;
use termide_core::HotkeyTable;

/// Build HotkeyTable for the editor from config.
pub(crate) fn build_editor_hotkey_table(config: &Config) -> HotkeyTable {
    let mut t = HotkeyTable::new();
    let kb = &config.editor.keybindings;

    // File operations
    t.insert("save", &kb.save);
    t.insert("save_as", &kb.save_as);
    t.insert("reload", &kb.reload);

    // Undo/Redo
    t.insert("undo", &kb.undo);
    t.insert("redo", &kb.redo);

    // Search & Replace
    t.insert("search", &kb.search);
    t.insert("search_next", &kb.search_next);
    t.insert("search_prev", &kb.search_prev);
    t.insert("replace", &kb.replace);
    t.insert("replace_current", &kb.replace_current);
    t.insert("replace_all", &kb.replace_all);

    // Selection
    t.insert("select_all", &kb.select_all);

    // Advanced editing
    t.insert("duplicate_line", &kb.duplicate_line);
    t.insert("delete_line", &kb.delete_line);
    t.insert("toggle_comment", &kb.toggle_comment);

    // LSP
    t.insert("trigger_completion", &kb.trigger_completion);
    t.insert("show_hover", &kb.show_hover);
    t.insert("goto_definition", &kb.goto_definition);
    t.insert("find_references", &kb.find_references);
    t.insert("rename_symbol", &kb.rename_symbol);
    t.insert("code_action", &kb.code_action);

    // Viewer: swap this text file to the hex viewer (shared viewer binding).
    t.insert("viewer_toggle_hex", &config.viewer.keybindings.toggle_hex);
    // Viewer: toggle this editor between view (read-only) and edit.
    t.insert("viewer_toggle_view", &config.viewer.keybindings.toggle_view);

    t
}
