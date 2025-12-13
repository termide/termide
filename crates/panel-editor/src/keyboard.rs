//! Keyboard command handling for the editor.
//!
//! This module implements the Command Pattern for keyboard input, separating
//! key parsing from command execution for better testability and maintainability.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Editor command representing a user action.
///
/// This enum captures all possible commands that can be triggered by keyboard input,
/// separating the concern of "what key was pressed" from "what action to perform".
#[derive(Debug, Clone, PartialEq)]
pub enum EditorCommand {
    // Navigation commands (clear selection and close search)
    MoveCursorUp,
    MoveCursorDown,
    MoveCursorLeft,
    MoveCursorRight,
    #[allow(dead_code)] // Mapped to MoveToVisualLineStart in from_key_event
    MoveToLineStart,
    #[allow(dead_code)] // Mapped to MoveToVisualLineEnd in from_key_event
    MoveToLineEnd,
    MoveToVisualLineStart,
    MoveToVisualLineEnd,
    PageUp,
    PageDown,
    MoveToDocumentStart,
    MoveToDocumentEnd,

    // Navigation with selection (Shift modifier, closes search)
    MoveCursorUpWithSelection,
    MoveCursorDownWithSelection,
    MoveCursorLeftWithSelection,
    MoveCursorRightWithSelection,
    #[allow(dead_code)] // Mapped to MoveToVisualLineStartWithSelection in from_key_event
    MoveToLineStartWithSelection,
    #[allow(dead_code)] // Mapped to MoveToVisualLineEndWithSelection in from_key_event
    MoveToLineEndWithSelection,
    MoveToVisualLineStartWithSelection,
    MoveToVisualLineEndWithSelection,
    PageUpWithSelection,
    PageDownWithSelection,
    MoveToDocumentStartWithSelection,
    MoveToDocumentEndWithSelection,

    // Text editing
    InsertChar(char),
    InsertTab,
    IndentLines,
    UnindentLines,
    InsertNewline,
    Backspace,
    Delete,

    // Undo/Redo
    Undo,
    Redo,

    // File operations
    Save,
    #[allow(dead_code)] // Included for completeness, triggered by Save when no file path exists
    SaveAs,
    /// Force save (ignore external changes)
    ForceSave,
    /// Reload file from disk (discard local changes)
    ReloadFromDisk,

    // Selection
    SelectAll,

    // Clipboard
    Copy,
    Cut,
    Paste,

    // Advanced editing
    DuplicateLine,

    // Search
    StartSearch,
    SearchNext,
    SearchPrev,
    CloseSearch,
    SearchNextOrOpen,
    SearchPrevOrOpen,

    // Replace
    StartReplace,
    ReplaceNext,
    ReplaceAll,

    // No operation (for unhandled keys)
    None,
}

impl EditorCommand {
    /// Parse a KeyEvent into an EditorCommand.
    ///
    /// This function encapsulates all keyboard shortcuts and their modifiers,
    /// making it easy to see all bindings in one place and test them independently.
    ///
    /// # Arguments
    ///
    /// * `key` - The key event to parse (should already be translated via translate_hotkey)
    /// * `read_only` - Whether the editor is in read-only mode
    /// * `has_search` - Whether there's an active search
    /// * `has_selection` - Whether there's an active text selection
    pub fn from_key_event(
        key: KeyEvent,
        read_only: bool,
        has_search: bool,
        has_selection: bool,
    ) -> Self {
        match (key.code, key.modifiers) {
            // Navigation (clears selection and closes search)
            (KeyCode::Up, KeyModifiers::NONE) => Self::MoveCursorUp,
            (KeyCode::Down, KeyModifiers::NONE) => Self::MoveCursorDown,
            (KeyCode::Left, KeyModifiers::NONE) => Self::MoveCursorLeft,
            (KeyCode::Right, KeyModifiers::NONE) => Self::MoveCursorRight,
            (KeyCode::Home, KeyModifiers::NONE) => Self::MoveToVisualLineStart,
            (KeyCode::End, KeyModifiers::NONE) => Self::MoveToVisualLineEnd,
            (KeyCode::PageUp, KeyModifiers::NONE) => Self::PageUp,
            (KeyCode::PageDown, KeyModifiers::NONE) => Self::PageDown,
            (KeyCode::Home, KeyModifiers::CONTROL) => Self::MoveToDocumentStart,
            (KeyCode::End, KeyModifiers::CONTROL) => Self::MoveToDocumentEnd,

            // Navigation with selection (Shift) - closes search
            (KeyCode::Up, KeyModifiers::SHIFT) => Self::MoveCursorUpWithSelection,
            (KeyCode::Down, KeyModifiers::SHIFT) => Self::MoveCursorDownWithSelection,
            (KeyCode::Left, KeyModifiers::SHIFT) => Self::MoveCursorLeftWithSelection,
            (KeyCode::Right, KeyModifiers::SHIFT) => Self::MoveCursorRightWithSelection,
            (KeyCode::Home, mods)
                if mods.contains(KeyModifiers::SHIFT) && !mods.contains(KeyModifiers::CONTROL) =>
            {
                Self::MoveToVisualLineStartWithSelection
            }
            (KeyCode::End, mods)
                if mods.contains(KeyModifiers::SHIFT) && !mods.contains(KeyModifiers::CONTROL) =>
            {
                Self::MoveToVisualLineEndWithSelection
            }
            (KeyCode::PageUp, mods)
                if mods.contains(KeyModifiers::SHIFT) && !mods.contains(KeyModifiers::CONTROL) =>
            {
                Self::PageUpWithSelection
            }
            (KeyCode::PageDown, mods)
                if mods.contains(KeyModifiers::SHIFT) && !mods.contains(KeyModifiers::CONTROL) =>
            {
                Self::PageDownWithSelection
            }
            (KeyCode::Home, mods)
                if mods.contains(KeyModifiers::SHIFT) && mods.contains(KeyModifiers::CONTROL) =>
            {
                Self::MoveToDocumentStartWithSelection
            }
            (KeyCode::End, mods)
                if mods.contains(KeyModifiers::SHIFT) && mods.contains(KeyModifiers::CONTROL) =>
            {
                Self::MoveToDocumentEndWithSelection
            }

            // Editing (only if not read-only)
            (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT)
                if !read_only && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                Self::InsertChar(ch)
            }
            (KeyCode::Enter, KeyModifiers::NONE) if !read_only => Self::InsertNewline,
            (KeyCode::Backspace, KeyModifiers::NONE) if !read_only => Self::Backspace,
            (KeyCode::Delete, KeyModifiers::NONE) if !read_only => Self::Delete,

            // Ctrl+S - save (only if not read-only)
            (KeyCode::Char('s'), KeyModifiers::CONTROL) if !read_only => Self::Save,

            // Ctrl+Shift+S - force save (ignore external changes, only if not read-only)
            (KeyCode::Char('S'), mods)
                if !read_only
                    && mods.contains(KeyModifiers::CONTROL)
                    && mods.contains(KeyModifiers::SHIFT) =>
            {
                Self::ForceSave
            }

            // Ctrl+Shift+R - reload from disk
            (KeyCode::Char('R'), mods)
                if mods.contains(KeyModifiers::CONTROL) && mods.contains(KeyModifiers::SHIFT) =>
            {
                Self::ReloadFromDisk
            }

            // Ctrl+Z - undo (only if not read-only)
            (KeyCode::Char('z'), KeyModifiers::CONTROL) if !read_only => Self::Undo,

            // Ctrl+Y - redo (only if not read-only)
            (KeyCode::Char('y'), KeyModifiers::CONTROL) if !read_only => Self::Redo,

            // Ctrl+F - search
            (KeyCode::Char('f'), KeyModifiers::CONTROL) => Self::StartSearch,

            // F3 - next match (or open search if no active search)
            (KeyCode::F(3), KeyModifiers::NONE) => Self::SearchNextOrOpen,

            // Shift+F3 - previous match (or open search if no active search)
            (KeyCode::F(3), KeyModifiers::SHIFT) => Self::SearchPrevOrOpen,

            // Esc - close search
            (KeyCode::Esc, KeyModifiers::NONE) if has_search => Self::CloseSearch,

            // Tab - next match (when search is active), indent lines (with selection), or insert tab
            (KeyCode::Tab, KeyModifiers::NONE) if has_search => Self::SearchNext,
            (KeyCode::Tab, KeyModifiers::NONE) if !read_only && has_selection => Self::IndentLines,
            (KeyCode::Tab, KeyModifiers::NONE) if !read_only => Self::InsertTab,

            // Shift+Tab - previous match (when search is active), or unindent lines
            (KeyCode::BackTab, _) if has_search => Self::SearchPrev,
            (KeyCode::BackTab, _) if !read_only => Self::UnindentLines,

            // Ctrl+H - text replacement (only if not read-only)
            (KeyCode::Char('h'), KeyModifiers::CONTROL) if !read_only => Self::StartReplace,

            // Ctrl+Alt+R - replace all matches (must be before Ctrl+R)
            (KeyCode::Char('r'), mods)
                if !read_only
                    && mods.contains(KeyModifiers::CONTROL)
                    && mods.contains(KeyModifiers::ALT) =>
            {
                Self::ReplaceAll
            }

            // Ctrl+R - replace current match (only if not read-only)
            (KeyCode::Char('r'), KeyModifiers::CONTROL) if !read_only => Self::ReplaceNext,

            // Ctrl+A - select all
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => Self::SelectAll,

            // Ctrl+C - copy
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => Self::Copy,

            // Ctrl+D - duplicate line
            (KeyCode::Char('d'), KeyModifiers::CONTROL) if !read_only => Self::DuplicateLine,

            // Ctrl+Insert - copy
            (KeyCode::Insert, KeyModifiers::CONTROL) => Self::Copy,

            // Ctrl+Shift+C - copy (terminal shortcut)
            (KeyCode::Char('c'), mods)
                if mods.contains(KeyModifiers::CONTROL) && mods.contains(KeyModifiers::SHIFT) =>
            {
                Self::Copy
            }
            (KeyCode::Char('C'), mods)
                if mods.contains(KeyModifiers::CONTROL) && mods.contains(KeyModifiers::SHIFT) =>
            {
                Self::Copy
            }

            // Ctrl+X - cut (only if not read-only)
            (KeyCode::Char('x'), KeyModifiers::CONTROL) if !read_only => Self::Cut,

            // Shift+Delete - cut (only if not read-only)
            (KeyCode::Delete, KeyModifiers::SHIFT) if !read_only => Self::Cut,

            // Ctrl+V - paste (only if not read-only)
            (KeyCode::Char('v'), KeyModifiers::CONTROL) if !read_only => Self::Paste,

            // Shift+Insert - paste (only if not read-only)
            (KeyCode::Insert, KeyModifiers::SHIFT) if !read_only => Self::Paste,

            // Ctrl+Shift+V - paste (terminal shortcut)
            (KeyCode::Char('v'), mods)
                if !read_only
                    && mods.contains(KeyModifiers::CONTROL)
                    && mods.contains(KeyModifiers::SHIFT) =>
            {
                Self::Paste
            }
            (KeyCode::Char('V'), mods)
                if !read_only
                    && mods.contains(KeyModifiers::CONTROL)
                    && mods.contains(KeyModifiers::SHIFT) =>
            {
                Self::Paste
            }

            // Default - no operation
            _ => Self::None,
        }
    }

    /// Execute this command on the given editor.
    ///
    /// This method performs the actual action associated with the command.
    /// Most commands delegate to existing methods on Editor, keeping the
    /// business logic in one place.
    ///
    /// # Arguments
    ///
    /// * `editor` - The editor to execute the command on
    ///
    /// # Returns
    ///
    /// Ok(()) if the command executed successfully, or an error if something went wrong.
    pub fn execute(self, editor: &mut super::Editor) -> Result<()> {
        use super::Editor;

        match self {
            // Navigation (clears selection and closes search)
            Self::MoveCursorUp => {
                editor.navigate(Editor::move_cursor_up_visual, Editor::move_cursor_up);
                Ok(())
            }
            Self::MoveCursorDown => {
                editor.navigate(Editor::move_cursor_down_visual, Editor::move_cursor_down);
                Ok(())
            }
            Self::MoveCursorLeft => {
                editor.navigate_simple(Editor::move_cursor_left);
                Ok(())
            }
            Self::MoveCursorRight => {
                editor.navigate_simple(Editor::move_cursor_right);
                Ok(())
            }
            Self::MoveToLineStart => {
                editor.navigate(
                    Editor::move_to_visual_line_start,
                    Editor::move_to_line_start,
                );
                Ok(())
            }
            Self::MoveToLineEnd => {
                editor.navigate(Editor::move_to_visual_line_end, Editor::move_to_line_end);
                Ok(())
            }
            Self::MoveToVisualLineStart => {
                editor.navigate(
                    Editor::move_to_visual_line_start,
                    Editor::move_to_line_start,
                );
                Ok(())
            }
            Self::MoveToVisualLineEnd => {
                editor.navigate(Editor::move_to_visual_line_end, Editor::move_to_line_end);
                Ok(())
            }
            Self::PageUp => {
                editor.navigate(Editor::page_up_visual, Editor::page_up);
                Ok(())
            }
            Self::PageDown => {
                editor.navigate(Editor::page_down_visual, Editor::page_down);
                Ok(())
            }
            Self::MoveToDocumentStart => {
                editor.navigate_simple(Editor::move_to_document_start);
                Ok(())
            }
            Self::MoveToDocumentEnd => {
                editor.navigate_simple(Editor::move_to_document_end);
                Ok(())
            }

            // Navigation with selection
            Self::MoveCursorUpWithSelection => {
                editor
                    .navigate_with_selection(Editor::move_cursor_up_visual, Editor::move_cursor_up);
                Ok(())
            }
            Self::MoveCursorDownWithSelection => {
                editor.navigate_with_selection(
                    Editor::move_cursor_down_visual,
                    Editor::move_cursor_down,
                );
                Ok(())
            }
            Self::MoveCursorLeftWithSelection => {
                editor.navigate_with_selection_simple(Editor::move_cursor_left);
                Ok(())
            }
            Self::MoveCursorRightWithSelection => {
                editor.navigate_with_selection_simple(Editor::move_cursor_right);
                Ok(())
            }
            Self::MoveToLineStartWithSelection => {
                editor.navigate_with_selection(
                    Editor::move_to_visual_line_start,
                    Editor::move_to_line_start,
                );
                Ok(())
            }
            Self::MoveToLineEndWithSelection => {
                editor.navigate_with_selection(
                    Editor::move_to_visual_line_end,
                    Editor::move_to_line_end,
                );
                Ok(())
            }
            Self::MoveToVisualLineStartWithSelection => {
                editor.navigate_with_selection(
                    Editor::move_to_visual_line_start,
                    Editor::move_to_line_start,
                );
                Ok(())
            }
            Self::MoveToVisualLineEndWithSelection => {
                editor.navigate_with_selection(
                    Editor::move_to_visual_line_end,
                    Editor::move_to_line_end,
                );
                Ok(())
            }
            Self::PageUpWithSelection => {
                editor.navigate_with_selection(Editor::page_up_visual, Editor::page_up);
                Ok(())
            }
            Self::PageDownWithSelection => {
                editor.navigate_with_selection(Editor::page_down_visual, Editor::page_down);
                Ok(())
            }
            Self::MoveToDocumentStartWithSelection => {
                editor.navigate_with_selection_simple(Editor::move_to_document_start);
                Ok(())
            }
            Self::MoveToDocumentEndWithSelection => {
                editor.navigate_with_selection_simple(Editor::move_to_document_end);
                Ok(())
            }

            // Text editing
            Self::InsertChar(ch) => editor.insert_char(ch),
            Self::InsertTab => editor.insert_tab(),
            Self::IndentLines => editor.indent_lines(),
            Self::UnindentLines => editor.unindent_lines(),
            Self::InsertNewline => editor.insert_newline(),
            Self::Backspace => editor.handle_delete_key(|e| e.backspace()),
            Self::Delete => editor.handle_delete_key(|e| e.delete()),

            // Undo/Redo
            Self::Undo => editor.handle_undo_redo(|buf| buf.undo()),
            Self::Redo => editor.handle_undo_redo(|buf| buf.redo()),

            // File operations - Save requires special handling for SaveAs modal
            Self::Save => editor.handle_save(),
            Self::SaveAs => {
                // This shouldn't be reached from key parsing, but included for completeness
                editor.handle_save_as()
            }
            Self::ForceSave => {
                if let Err(e) = editor.force_save() {
                    editor.status_message = Some(format!("Force save failed: {}", e));
                } else {
                    editor.status_message = Some("File force saved".to_string());
                }
                Ok(())
            }
            Self::ReloadFromDisk => {
                if let Err(e) = editor.reload_from_disk() {
                    editor.status_message = Some(format!("Reload failed: {}", e));
                } else {
                    editor.status_message = Some("File reloaded from disk".to_string());
                }
                Ok(())
            }

            // Selection
            Self::SelectAll => {
                editor.select_all();
                Ok(())
            }

            // Clipboard
            Self::Copy => editor.copy_to_clipboard(),
            Self::Cut => editor.cut_to_clipboard(),
            Self::Paste => editor.paste_from_clipboard(),

            // Advanced editing
            Self::DuplicateLine => editor.duplicate_line(),

            // Search
            Self::StartSearch => {
                editor.open_search_modal(true);
                Ok(())
            }
            Self::SearchNext => {
                editor.search_next();
                Ok(())
            }
            Self::SearchPrev => {
                editor.search_prev();
                Ok(())
            }
            Self::CloseSearch => {
                if editor.search.state.is_some() {
                    editor.close_search();
                }
                Ok(())
            }
            Self::SearchNextOrOpen => {
                editor.search_next_or_open();
                Ok(())
            }
            Self::SearchPrevOrOpen => {
                editor.search_prev_or_open();
                Ok(())
            }

            // Replace
            Self::StartReplace => {
                editor.handle_start_replace();
                Ok(())
            }
            Self::ReplaceNext => editor.replace_current(),
            Self::ReplaceAll => match editor.replace_all() {
                Ok(count) => {
                    editor.status_message = Some(format!(
                        "Replaced {} occurrence{}",
                        count,
                        if count == 1 { "" } else { "s" }
                    ));
                    Ok(())
                }
                Err(e) => Err(e),
            },

            // No operation
            Self::None => Ok(()),
        }
    }
}
