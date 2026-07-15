//! Keyboard command handling for the file manager.
//!
//! This module implements the Command Pattern for keyboard input, separating
//! key parsing from command execution for better testability and maintainability.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termide_config::{is_go_end, is_go_home, is_move_down, is_move_up};
use termide_core::HotkeyTable;

/// File manager command representing a user action.
///
/// This enum captures all possible commands that can be triggered by keyboard input,
/// separating the concern of "what key was pressed" from "what action to perform".
#[derive(Debug, Clone, PartialEq)]
pub enum FmCommand {
    // Navigation
    MoveUp,
    MoveDown,
    PageUp,
    PageDown,
    GoHome,
    GoEnd,
    Enter,
    GoParent,
    GoHomeDir,

    // Selection
    ToggleSelection,
    SelectAll,
    ClearSelection,
    MoveUpWithSelection,
    MoveDownWithSelection,
    PageUpWithSelection,
    PageDownWithSelection,
    SelectToHome,
    SelectToEnd,
    MoveUpWithToggle,
    MoveDownWithToggle,
    PageUpWithToggle,
    PageDownWithToggle,

    // File operations
    NewFile,
    NewDirectory,
    DeleteFiles,
    CopyFiles,
    MoveFiles,
    RenameFile,
    EditFile,
    ViewFile,
    OpenExternal,

    // Search
    Search,
    SearchContent,
    SearchReplace,

    // Clipboard
    ClipboardCopy,
    ClipboardCut,
    ClipboardPaste,

    // Misc
    ShowFileInfo,
    Refresh,
    ToggleHidden,
    NextPanel,
    PrevPanel,
    /// Go to path/URL (Ctrl+G) - opens modal to enter path directly
    GoToPath,
    /// Open directory switcher modal (Ctrl+/)
    SwitchDirectory,
    /// Cancel pending VFS operation (Escape during connection)
    /// Note: Not yet mapped to any key, reserved for future use
    #[allow(dead_code)]
    CancelOperation,

    // Tree expand/collapse
    ExpandDir,
    CollapseDir,

    // No operation
    None,
}

impl FmCommand {
    /// Parse a KeyEvent into an FmCommand.
    ///
    /// Configurable actions are resolved via HotkeyTable.
    /// Non-configurable navigation (arrows, shift+arrows, vim mode) remains hardcoded.
    ///
    /// # Arguments
    ///
    /// * `key` - The key event to parse (should already be translated via translate_hotkey)
    /// * `hotkeys` - HotkeyTable built from config (configurable bindings)
    /// * `vim_mode` - Whether vim mode is enabled (adds j/k/g/G navigation)
    pub fn from_key_event(key: KeyEvent, hotkeys: &HotkeyTable, vim_mode: bool) -> Self {
        // =================================================================
        // Configurable actions from HotkeyTable
        // =================================================================

        if hotkeys.matches("rename", &key) {
            return Self::RenameFile;
        }
        if hotkeys.matches("view", &key) {
            return Self::ViewFile;
        }
        if hotkeys.matches("edit", &key) {
            return Self::EditFile;
        }
        if hotkeys.matches("copy", &key) {
            return Self::CopyFiles;
        }
        if hotkeys.matches("move", &key) {
            return Self::MoveFiles;
        }
        if hotkeys.matches("create_dir", &key) {
            return Self::NewDirectory;
        }
        if hotkeys.matches("create_file", &key) {
            return Self::NewFile;
        }
        if hotkeys.matches("delete", &key) {
            return Self::DeleteFiles;
        }
        if hotkeys.matches("info", &key) {
            return Self::ShowFileInfo;
        }
        if hotkeys.matches("search", &key) {
            return Self::Search;
        }
        if hotkeys.matches("search_content", &key) {
            return Self::SearchContent;
        }
        if hotkeys.matches("search_replace", &key) {
            return Self::SearchReplace;
        }
        if hotkeys.matches("refresh", &key) {
            return Self::Refresh;
        }
        if hotkeys.matches("go_parent", &key) {
            return Self::GoParent;
        }
        if hotkeys.matches("go_home", &key) {
            return Self::GoHomeDir;
        }
        if hotkeys.matches("toggle_selection", &key) {
            return Self::ToggleSelection;
        }
        if hotkeys.matches("select_all", &key) {
            return Self::SelectAll;
        }
        if hotkeys.matches("toggle_hidden", &key) {
            return Self::ToggleHidden;
        }
        if hotkeys.matches("open_external", &key) {
            return Self::OpenExternal;
        }
        if hotkeys.matches("switch_directory", &key) {
            return Self::SwitchDirectory;
        }
        if hotkeys.matches("go_to_path", &key) {
            return Self::GoToPath;
        }
        if hotkeys.matches("clipboard_copy", &key) {
            return Self::ClipboardCopy;
        }
        if hotkeys.matches("clipboard_cut", &key) {
            return Self::ClipboardCut;
        }
        if hotkeys.matches("clipboard_paste", &key) {
            return Self::ClipboardPaste;
        }

        // =================================================================
        // Non-configurable bindings (navigation, basic keys)
        // =================================================================

        // Vim-aware navigation (j/k/g/G when vim_mode is enabled)
        if is_move_down(&key, vim_mode) {
            return Self::MoveDown;
        }
        if is_move_up(&key, vim_mode) {
            return Self::MoveUp;
        }
        if is_go_home(&key, vim_mode) {
            return Self::GoHome;
        }
        if is_go_end(&key, vim_mode) {
            return Self::GoEnd;
        }

        match (key.code, key.modifiers) {
            // Selection with Shift
            (KeyCode::Down, KeyModifiers::SHIFT) => Self::MoveDownWithSelection,
            (KeyCode::Up, KeyModifiers::SHIFT) => Self::MoveUpWithSelection,
            (KeyCode::PageDown, KeyModifiers::SHIFT) => Self::PageDownWithSelection,
            (KeyCode::PageUp, KeyModifiers::SHIFT) => Self::PageUpWithSelection,
            (KeyCode::Home, KeyModifiers::SHIFT) => Self::SelectToHome,
            (KeyCode::End, KeyModifiers::SHIFT) => Self::SelectToEnd,

            // Toggle selection with Ctrl
            (KeyCode::Down, KeyModifiers::CONTROL) => Self::MoveDownWithToggle,
            (KeyCode::Up, KeyModifiers::CONTROL) => Self::MoveUpWithToggle,
            (KeyCode::PageDown, KeyModifiers::CONTROL) => Self::PageDownWithToggle,
            (KeyCode::PageUp, KeyModifiers::CONTROL) => Self::PageUpWithToggle,

            // Tree expand/collapse
            (KeyCode::Right, KeyModifiers::NONE) => Self::ExpandDir,
            (KeyCode::Left, KeyModifiers::NONE) => Self::CollapseDir,
            _ if vim_mode && key.modifiers.is_empty() && {
                matches!(
                    key.code,
                    KeyCode::Char(c)
                        if termide_keyboard::cyrillic_to_latin(c) == 'l'
                )
            } =>
            {
                Self::ExpandDir
            }
            _ if vim_mode && key.modifiers.is_empty() && {
                matches!(
                    key.code,
                    KeyCode::Char(c)
                        if termide_keyboard::cyrillic_to_latin(c) == 'h'
                )
            } =>
            {
                Self::CollapseDir
            }

            // Regular navigation (arrows-only, vim handled above)
            (KeyCode::PageUp, KeyModifiers::NONE) => Self::PageUp,
            (KeyCode::PageDown, KeyModifiers::NONE) => Self::PageDown,
            (KeyCode::Enter, KeyModifiers::NONE) => Self::Enter,
            (KeyCode::Esc, KeyModifiers::NONE) => Self::ClearSelection,

            // Backspace with any modifiers (go to parent)
            (KeyCode::Backspace, mods) if mods != KeyModifiers::NONE => Self::GoParent,

            // Panel navigation
            (KeyCode::Tab, KeyModifiers::NONE) => Self::NextPanel,
            (KeyCode::BackTab, _) => Self::PrevPanel,

            _ => Self::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn default_hotkeys() -> HotkeyTable {
        let mut config = termide_config::Config::default();
        config.normalize();
        crate::command_dispatch::build_fm_hotkey_table(&config)
    }

    #[test]
    fn test_navigation_keys() {
        let hk = default_hotkeys();

        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Up, KeyModifiers::NONE), &hk, false),
            FmCommand::MoveUp
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Down, KeyModifiers::NONE), &hk, false),
            FmCommand::MoveDown
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::PageUp, KeyModifiers::NONE), &hk, false),
            FmCommand::PageUp
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::PageDown, KeyModifiers::NONE), &hk, false),
            FmCommand::PageDown
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Home, KeyModifiers::NONE), &hk, false),
            FmCommand::GoHome
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::End, KeyModifiers::NONE), &hk, false),
            FmCommand::GoEnd
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Enter, KeyModifiers::NONE), &hk, false),
            FmCommand::Enter
        );
    }

    #[test]
    fn test_tree_expand_collapse_keys() {
        let hk = default_hotkeys();

        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Right, KeyModifiers::NONE), &hk, false),
            FmCommand::ExpandDir
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Left, KeyModifiers::NONE), &hk, false),
            FmCommand::CollapseDir
        );

        // Vim mode: l/h for expand/collapse
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Char('l'), KeyModifiers::NONE), &hk, true),
            FmCommand::ExpandDir
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Char('h'), KeyModifiers::NONE), &hk, true),
            FmCommand::CollapseDir
        );
    }

    #[test]
    fn test_vim_navigation_keys() {
        let hk = default_hotkeys();

        // Vim keys should not work when vim_mode is false
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Char('j'), KeyModifiers::NONE), &hk, false),
            FmCommand::None
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Char('k'), KeyModifiers::NONE), &hk, false),
            FmCommand::None
        );

        // Vim keys should work when vim_mode is true
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Char('j'), KeyModifiers::NONE), &hk, true),
            FmCommand::MoveDown
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Char('k'), KeyModifiers::NONE), &hk, true),
            FmCommand::MoveUp
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Char('g'), KeyModifiers::NONE), &hk, true),
            FmCommand::GoHome
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Char('G'), KeyModifiers::SHIFT), &hk, true),
            FmCommand::GoEnd
        );
    }

    #[test]
    fn test_selection_keys() {
        let hk = default_hotkeys();

        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Down, KeyModifiers::SHIFT), &hk, false),
            FmCommand::MoveDownWithSelection
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Up, KeyModifiers::SHIFT), &hk, false),
            FmCommand::MoveUpWithSelection
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Esc, KeyModifiers::NONE), &hk, false),
            FmCommand::ClearSelection
        );
    }

    #[test]
    fn test_file_operations() {
        let hk = default_hotkeys();

        // Letter shortcuts
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Char('v'), KeyModifiers::NONE), &hk, false),
            FmCommand::ViewFile
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Char('e'), KeyModifiers::NONE), &hk, false),
            FmCommand::EditFile
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Char('r'), KeyModifiers::NONE), &hk, false),
            FmCommand::RenameFile
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Char('c'), KeyModifiers::NONE), &hk, false),
            FmCommand::CopyFiles
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Char('m'), KeyModifiers::NONE), &hk, false),
            FmCommand::MoveFiles
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Char('d'), KeyModifiers::NONE), &hk, false),
            FmCommand::NewDirectory
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Delete, KeyModifiers::NONE), &hk, false),
            FmCommand::DeleteFiles
        );

        // F-keys
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::F(2), KeyModifiers::NONE), &hk, false),
            FmCommand::RenameFile
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::F(4), KeyModifiers::NONE), &hk, false),
            FmCommand::EditFile
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::F(5), KeyModifiers::NONE), &hk, false),
            FmCommand::CopyFiles
        );
    }

    #[test]
    fn test_panel_navigation() {
        let hk = default_hotkeys();

        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Tab, KeyModifiers::NONE), &hk, false),
            FmCommand::NextPanel
        );
        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::BackTab, KeyModifiers::SHIFT), &hk, false),
            FmCommand::PrevPanel
        );
    }

    #[test]
    fn test_toggle_hidden() {
        let hk = default_hotkeys();

        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::Char('.'), KeyModifiers::NONE), &hk, false),
            FmCommand::ToggleHidden
        );
    }

    #[test]
    fn test_search_keys() {
        let hk = default_hotkeys();

        // Ctrl+Shift+F → content search
        assert_eq!(
            FmCommand::from_key_event(
                key(
                    KeyCode::Char('F'),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT
                ),
                &hk,
                false
            ),
            FmCommand::SearchContent
        );
    }

    #[test]
    fn test_f12_returns_show_file_info() {
        let hk = default_hotkeys();

        assert_eq!(
            FmCommand::from_key_event(key(KeyCode::F(12), KeyModifiers::NONE), &hk, false),
            FmCommand::ShowFileInfo
        );
    }
}
