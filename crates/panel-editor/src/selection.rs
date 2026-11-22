//! Text selection operations for the editor.
//!
//! This module provides utilities for managing text selections, including
//! selecting all text, getting selected text, and deleting selections.

use anyhow::Result;

use termide_buffer::{columns, is_word_boundary, Cursor, Selection, TextBuffer};
use unicode_segmentation::UnicodeSegmentation;

/// Select all text in the buffer.
///
/// Returns (new_selection, new_cursor_position).
pub(crate) fn select_all(buffer: &TextBuffer) -> (Selection, Cursor) {
    let start = Cursor::at(0, 0);
    let max_line = buffer.line_count().saturating_sub(1);
    let line_len = buffer.line_len_graphemes(max_line);
    let end = Cursor::at(max_line, line_len);
    (Selection::new(start, end), end)
}

/// Start new selection or continue existing.
///
/// Returns Some(selection) if a new selection should be started, None if selection exists.
pub fn start_or_extend_selection(
    current_selection: Option<&Selection>,
    cursor: Cursor,
) -> Option<Selection> {
    if current_selection.is_none() {
        Some(Selection::new(cursor, cursor))
    } else {
        None
    }
}

/// Update active point of selection after cursor movement.
pub fn update_selection_active(selection: &mut Option<Selection>, new_cursor: Cursor) {
    if let Some(ref mut sel) = selection {
        sel.active = new_cursor;
    }
}

/// Get selected text from buffer: exactly the text [`delete_selection`]
/// removes, sliced by the same grapheme columns.
pub fn get_selected_text(buffer: &TextBuffer, selection: Option<&Selection>) -> Option<String> {
    let selection = selection?;
    if selection.is_empty() {
        return None;
    }
    Some(buffer.text_range(&selection.start(), &selection.end()))
}

/// Delete selected text from buffer.
///
/// Returns (new_cursor_position, should_invalidate_cache) on success.
/// Caller is responsible for invalidating highlight cache and scheduling git diff update.
pub fn delete_selection(
    buffer: &mut TextBuffer,
    selection: Option<&Selection>,
) -> Result<Option<Cursor>> {
    if let Some(selection) = selection {
        if !selection.is_empty() {
            let start = selection.start();
            let end = selection.end();
            buffer.delete_range(&start, &end)?;
            return Ok(Some(start));
        }
    }
    Ok(None)
}

/// Select word at cursor position.
///
/// Finds word boundaries around the cursor and returns selection covering the word.
/// Word boundaries are non-alphanumeric characters (spaces, punctuation, etc.).
/// Returns (new_selection, new_cursor_position) where cursor is at the end of the word.
pub fn select_word(buffer: &TextBuffer, cursor: &Cursor) -> Option<(Selection, Cursor)> {
    let line_text = buffer.line(cursor.line)?;
    if line_text.is_empty() {
        return None;
    }

    // Columns are graphemes; a cluster is a boundary when its base char is.
    let line_text = columns::trim_line_ending(&line_text);
    let bounds: Vec<bool> = line_text
        .graphemes(true)
        .map(|g| g.chars().next().is_none_or(is_word_boundary))
        .collect();
    let count = bounds.len();
    if count == 0 {
        return None;
    }

    // Clamp column to valid range
    let col = cursor.column.min(count - 1);

    // If cursor is at a word boundary character, don't select anything
    if bounds[col] {
        return None;
    }

    // Find word start (go backwards until word boundary)
    let mut start_col = col;
    while start_col > 0 && !bounds[start_col - 1] {
        start_col -= 1;
    }

    // Find word end (go forwards until word boundary or end of line)
    let mut end_col = col;
    while end_col < count && !bounds[end_col] {
        end_col += 1;
    }

    // Create selection
    let start = Cursor::at(cursor.line, start_col);
    let end = Cursor::at(cursor.line, end_col);

    Some((Selection::new(start, end), end))
}

/// Select the whole line at the cursor (triple-click). Covers the line
/// content from column 0 to end of line; the cursor lands at line end.
pub fn select_line(buffer: &TextBuffer, cursor: &Cursor) -> Option<(Selection, Cursor)> {
    if cursor.line >= buffer.line_count() {
        return None;
    }
    // The line's text in graphemes, without its line ending.
    let end_col = buffer.line_len_graphemes(cursor.line);
    let start = Cursor::at(cursor.line, 0);
    let end = Cursor::at(cursor.line, end_col);
    Some((Selection::new(start, end), end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word_at(text: &str, column: usize) -> Option<String> {
        let buffer = TextBuffer::from_text(text);
        let (selection, _) = select_word(&buffer, &Cursor::at(0, column))?;
        Some(get_selected_text(&buffer, Some(&selection)).unwrap_or_default())
    }

    #[test]
    fn a_word_after_a_cluster_is_selected_by_grapheme_columns() {
        assert_eq!(word_at("🇩🇪 foo bar", 2).as_deref(), Some("foo"));
        assert_eq!(word_at("cafe\u{301} x", 1).as_deref(), Some("cafe\u{301}"));
    }

    /// What Copy puts on the clipboard is what Cut deletes, also after
    /// clusters that are one column but several chars.
    #[test]
    fn the_copied_text_is_the_text_a_cut_deletes() {
        let text = "🇩🇪ab e\u{301}cd\nxy🇩🇪z\n";
        for (a, b) in [((0, 1), (0, 3)), ((0, 4), (0, 6)), ((0, 2), (1, 3))] {
            let mut buffer = TextBuffer::from_text(text);
            let selection = Selection::new(Cursor::at(a.0, a.1), Cursor::at(b.0, b.1));
            let copied = get_selected_text(&buffer, Some(&selection)).unwrap();
            delete_selection(&mut buffer, Some(&selection)).unwrap();
            buffer.insert(&selection.start(), &copied).unwrap();
            assert_eq!(buffer.text(), text, "{a:?}..{b:?} copied {copied:?}");
        }
        let buffer = TextBuffer::from_text(text);
        let selection = Selection::new(Cursor::at(0, 1), Cursor::at(0, 3));
        assert_eq!(get_selected_text(&buffer, Some(&selection)).unwrap(), "ab");
    }

    #[test]
    fn a_triple_click_selects_the_line_without_its_newline() {
        let buffer = TextBuffer::from_text("🇩🇪ab\nc");
        let (selection, _) = select_line(&buffer, &Cursor::at(0, 0)).unwrap();
        assert_eq!(selection.end(), Cursor::at(0, 3));
        assert_eq!(
            get_selected_text(&buffer, Some(&selection)).unwrap(),
            "🇩🇪ab"
        );
    }

    /// Double-clicking a `snake_case` identifier must take all of it. The
    /// editor's vim motions and the terminal's own double-click already treat
    /// `_` as part of a word; word selection used to disagree and stop at it.
    #[test]
    fn a_double_click_takes_a_whole_snake_case_identifier() {
        assert_eq!(
            word_at("let some_long_name = 1;", 6).as_deref(),
            Some("some_long_name")
        );
        // From any column inside it, including on an underscore.
        assert_eq!(
            word_at("let some_long_name = 1;", 8).as_deref(),
            Some("some_long_name")
        );
        assert_eq!(
            word_at("let some_long_name = 1;", 17).as_deref(),
            Some("some_long_name")
        );
    }

    #[test]
    fn punctuation_still_bounds_a_word() {
        assert_eq!(word_at("foo.bar", 0).as_deref(), Some("foo"));
        assert_eq!(word_at("foo.bar", 4).as_deref(), Some("bar"));
        assert_eq!(word_at("a(b)", 2).as_deref(), Some("b"));
    }

    #[test]
    fn a_click_on_a_boundary_selects_nothing() {
        assert_eq!(word_at("foo bar", 3), None);
        assert_eq!(word_at("foo.bar", 3), None);
    }

    #[test]
    fn words_are_not_limited_to_ascii() {
        assert_eq!(word_at("let имя_поля = 1;", 5).as_deref(), Some("имя_поля"));
        assert_eq!(word_at("日本語 x", 1).as_deref(), Some("日本語"));
    }

    #[test]
    fn a_leading_or_trailing_underscore_stays_attached() {
        assert_eq!(word_at("let _unused = 1;", 5).as_deref(), Some("_unused"));
        assert_eq!(word_at("call(x_)", 5).as_deref(), Some("x_"));
    }
}
