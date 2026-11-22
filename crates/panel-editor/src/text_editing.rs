//! Text editing operations for the editor.
//!
//! This module provides core text editing functionality including character
//! insertion, deletion, and line duplication.

use anyhow::Result;

use termide_buffer::{columns, Cursor, Selection, TextBuffer};
use unicode_segmentation::UnicodeSegmentation;

use crate::auto_pairs;

/// Result of a text editing operation.
///
/// Contains information about what changed so the caller can update
/// highlight cache and schedule git diff updates appropriately.
pub struct EditResult {
    pub new_cursor: Cursor,
    pub start_line: usize,
    pub is_multiline: bool,
}

/// Insert a character at the cursor position.
///
/// Returns EditResult with new cursor position and cache invalidation info.
pub fn insert_char(buffer: &mut TextBuffer, cursor: &Cursor, ch: char) -> Result<EditResult> {
    let text = ch.to_string();
    let new_cursor = buffer.insert(cursor, &text)?;

    Ok(EditResult {
        new_cursor,
        start_line: cursor.line,
        is_multiline: false,
    })
}

/// Insert a newline at the cursor position.
///
/// Returns EditResult with new cursor position and cache invalidation info.
pub fn insert_newline(buffer: &mut TextBuffer, cursor: &Cursor) -> Result<EditResult> {
    let old_line = cursor.line;
    let new_cursor = buffer.insert(cursor, "\n")?;

    Ok(EditResult {
        new_cursor,
        start_line: old_line,
        is_multiline: true,
    })
}

/// Insert a newline with auto-indentation at the cursor position.
///
/// The new line always inherits the indentation of the current line. When
/// `smart_indent` is on, an additional level of indentation is added if the
/// text before the cursor ends with `{`, `(`, `[`, or `:`. Callers pass
/// `smart_indent = false` for buffers without a recognized syntax so plain
/// prose (e.g. notes with trailing colons) doesn't accidentally accumulate
/// indentation.
///
/// Returns EditResult with new cursor position and cache invalidation info.
pub fn insert_newline_with_indent(
    buffer: &mut TextBuffer,
    cursor: &Cursor,
    tab_size: usize,
    smart_indent: bool,
) -> Result<EditResult> {
    let old_line = cursor.line;
    let line_cow = buffer.line_cow(cursor.line).unwrap_or_default();
    let line_content = columns::trim_line_ending(&line_cow);

    // Collect indentation from the current line
    let indent: String = line_content
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();

    // Smart indent: check if text before cursor ends with an opener. The
    // cursor column counts graphemes, so slice by graphemes too.
    let before_cursor = columns::grapheme_slice(line_content, 0, cursor.column);
    let trimmed = before_cursor.trim_end();
    let extra_indent = if smart_indent
        && (trimmed.ends_with('{')
            || trimmed.ends_with('(')
            || trimmed.ends_with('[')
            || trimmed.ends_with(':'))
    {
        " ".repeat(tab_size)
    } else {
        String::new()
    };

    // Split brackets: if cursor is between matching pair like {|}, insert
    // an extra line for the closing bracket with base indentation.
    let char_after_cursor =
        columns::grapheme_at(line_content, cursor.column).and_then(|g| g.chars().next());
    let last_before = trimmed.chars().last();
    let split_brackets = match (last_before, char_after_cursor) {
        (Some(open), Some(close)) => auto_pairs::is_matching_pair(open, close),
        _ => false,
    };

    let insert_text = if split_brackets {
        format!("\n{}{}\n{}", indent, extra_indent, indent)
    } else {
        format!("\n{}{}", indent, extra_indent)
    };
    let new_cursor = buffer.insert(cursor, &insert_text)?;

    // When splitting brackets, place cursor on the middle line (already done
    // by buffer.insert returning position after first \n + indent + extra_indent).
    // We just need to make sure we don't advance past the middle line.
    let final_cursor = if split_brackets {
        Cursor {
            line: old_line + 1,
            column: indent.graphemes(true).count() + extra_indent.len(),
        }
    } else {
        new_cursor
    };

    Ok(EditResult {
        new_cursor: final_cursor,
        start_line: old_line,
        is_multiline: true,
    })
}

/// Delete character before cursor (backspace).
///
/// Returns Some(EditResult) if deletion occurred, None if nothing to delete.
pub fn backspace(buffer: &mut TextBuffer, cursor: &Cursor) -> Result<Option<EditResult>> {
    let old_line = cursor.line;
    let was_at_line_start = cursor.column == 0;

    if let Some(new_cursor) = buffer.backspace(cursor)? {
        Ok(Some(EditResult {
            new_cursor,
            start_line: new_cursor.line,
            is_multiline: was_at_line_start && old_line > 0,
        }))
    } else {
        Ok(None)
    }
}

/// Delete character at cursor (delete).
///
/// Returns Some(EditResult) if deletion occurred, None if nothing to delete.
pub fn delete_char(buffer: &mut TextBuffer, cursor: &Cursor) -> Result<Option<EditResult>> {
    let line_len = buffer.line_len_graphemes(cursor.line);
    let was_at_line_end = cursor.column >= line_len;

    if buffer.delete_char(cursor)? {
        Ok(Some(EditResult {
            new_cursor: *cursor,
            start_line: cursor.line,
            is_multiline: was_at_line_end,
        }))
    } else {
        Ok(None)
    }
}

/// Duplicate current line or selected lines.
///
/// Returns EditResult with new cursor position and cache invalidation info.
pub fn duplicate_line(
    buffer: &mut TextBuffer,
    cursor: &Cursor,
    selection: Option<&Selection>,
) -> Result<EditResult> {
    // Determine which lines to duplicate
    let (start_line, end_line) = if let Some(selection) = selection {
        let start = selection.start();
        let end = selection.end();
        (start.line, end.line)
    } else {
        (cursor.line, cursor.line)
    };

    // The lines to duplicate, read off the rope by its own line numbers
    // (`str::lines` breaks lines differently and copied the whole buffer).
    let last_line_len = buffer.line_len_graphemes(end_line);
    let mut text_to_duplicate = buffer.text_range(
        &Cursor::at(start_line, 0),
        &Cursor::at(end_line, last_line_len),
    );

    // Insert newline and duplicated text after the last line
    text_to_duplicate.insert(0, '\n');

    // Move cursor to end of last line to duplicate
    let insert_cursor = Cursor {
        line: end_line,
        column: last_line_len,
    };

    buffer.insert(&insert_cursor, &text_to_duplicate)?;

    // Return new cursor at the beginning of the first duplicated line
    let new_cursor = Cursor {
        line: end_line + 1,
        column: 0,
    };

    Ok(EditResult {
        new_cursor,
        start_line,
        is_multiline: true,
    })
}

/// Delete the line under the cursor (or every line touched by `selection`).
///
/// Lines are removed whole — both the text and the trailing newline. After
/// deletion the cursor lands at the start of whatever line replaced the
/// deleted region. If the last logical line was part of the deletion, the
/// cursor falls back to the end of the previous line (or `(0, 0)` when the
/// buffer becomes a single empty line).
///
/// Mirrors the line-deletion path of vim's `dd` (`vim/operators.rs`)
/// without touching vim registers — clipboard semantics are deliberately
/// out of scope; users who want cut-line use `Ctrl+X` on a selection.
pub fn delete_line(
    buffer: &mut TextBuffer,
    cursor: &Cursor,
    selection: Option<&Selection>,
) -> Result<EditResult> {
    let line_count = buffer.line_count();
    if line_count == 0 {
        return Ok(EditResult {
            new_cursor: Cursor { line: 0, column: 0 },
            start_line: 0,
            is_multiline: false,
        });
    }

    let (start_line, end_line) = if let Some(sel) = selection {
        let s = sel.start();
        let e = sel.end();
        (s.line.min(line_count - 1), e.line.min(line_count - 1))
    } else {
        let l = cursor.line.min(line_count - 1);
        (l, l)
    };

    let is_multiline = end_line > start_line;
    let last_line_after_region = end_line + 1 < line_count;

    // Build the delete range. When the region ends before the last line we
    // chew through `(end_line+1, 0)` so the trailing newline of `end_line`
    // is consumed too. When it reaches through the last line we either
    // collapse the buffer to one empty line (start_line == 0) or merge
    // with the previous line by starting from its end-of-line position.
    let (range_start, range_end, new_cursor) = if last_line_after_region {
        let start = Cursor::at(start_line, 0);
        let end = Cursor::at(end_line + 1, 0);
        let new_cursor = Cursor::at(start_line, 0);
        (start, end, new_cursor)
    } else if start_line == 0 {
        let last_len = buffer.line_len_graphemes(end_line);
        let start = Cursor::at(0, 0);
        let end = Cursor::at(end_line, last_len);
        let new_cursor = Cursor { line: 0, column: 0 };
        (start, end, new_cursor)
    } else {
        let prev = start_line - 1;
        let prev_len = buffer.line_len_graphemes(prev);
        let last_len = buffer.line_len_graphemes(end_line);
        let start = Cursor::at(prev, prev_len);
        let end = Cursor::at(end_line, last_len);
        let new_cursor = Cursor::at(prev, prev_len);
        (start, end, new_cursor)
    };

    buffer.delete_range(&range_start, &range_end)?;

    Ok(EditResult {
        new_cursor,
        start_line,
        is_multiline,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(content: &str) -> TextBuffer {
        TextBuffer::from_text(content)
    }

    /// Duplicating reads the line by the rope's line numbers: a form feed
    /// on an earlier line used to shift `str::lines` against them.
    #[test]
    fn duplicate_line_takes_the_line_the_cursor_is_on() {
        let mut b = buf("a\u{c}b\nsecond\nthird");
        duplicate_line(&mut b, &Cursor::at(1, 0), None).unwrap();
        assert_eq!(b.text(), "a\u{c}b\nsecond\nsecond\nthird");
    }

    #[test]
    fn smart_indent_reads_the_brackets_around_a_grapheme_column() {
        // The cursor sits between the brackets after a flag: column 3, but
        // char 4.
        let mut b = buf("🇩🇪 {}");
        let r = insert_newline_with_indent(&mut b, &Cursor::at(0, 3), 4, true).unwrap();
        assert_eq!(b.text(), "🇩🇪 {\n    \n}");
        assert_eq!(r.new_cursor, Cursor::at(1, 4));
    }

    #[test]
    fn delete_line_middle_keeps_others() {
        let mut buffer = buf("alpha\nbeta\ngamma");
        let cursor = Cursor::at(1, 2);
        let r = delete_line(&mut buffer, &cursor, None).unwrap();
        assert_eq!(buffer.text(), "alpha\ngamma");
        assert_eq!(r.new_cursor, Cursor::at(1, 0));
        assert_eq!(r.start_line, 1);
        assert!(!r.is_multiline);
    }

    #[test]
    fn delete_line_first_keeps_rest() {
        let mut buffer = buf("alpha\nbeta\ngamma");
        let cursor = Cursor::at(0, 0);
        let r = delete_line(&mut buffer, &cursor, None).unwrap();
        assert_eq!(buffer.text(), "beta\ngamma");
        assert_eq!(r.new_cursor, Cursor::at(0, 0));
    }

    #[test]
    fn delete_line_last_falls_back_to_prev() {
        let mut buffer = buf("alpha\nbeta\ngamma");
        let cursor = Cursor::at(2, 4);
        let r = delete_line(&mut buffer, &cursor, None).unwrap();
        assert_eq!(buffer.text(), "alpha\nbeta");
        // Cursor parks at end of the new last line.
        assert_eq!(r.new_cursor.line, 1);
        assert_eq!(r.new_cursor.column, "beta".chars().count());
    }

    #[test]
    fn delete_line_only_line_clears_buffer() {
        let mut buffer = buf("solo");
        let cursor = Cursor::at(0, 2);
        let r = delete_line(&mut buffer, &cursor, None).unwrap();
        assert_eq!(buffer.text(), "");
        assert_eq!(r.new_cursor, Cursor::at(0, 0));
    }

    #[test]
    fn delete_line_with_multiline_selection() {
        let mut buffer = buf("alpha\nbeta\ngamma\ndelta");
        let cursor = Cursor::at(2, 0);
        let selection = Selection::new(Cursor::at(1, 1), Cursor::at(2, 3));
        let r = delete_line(&mut buffer, &cursor, Some(&selection)).unwrap();
        // Lines 1 and 2 (beta, gamma) gone; alpha and delta survive.
        assert_eq!(buffer.text(), "alpha\ndelta");
        assert_eq!(r.new_cursor, Cursor::at(1, 0));
        assert!(r.is_multiline);
    }

    #[test]
    fn delete_line_selection_through_last_line() {
        let mut buffer = buf("alpha\nbeta\ngamma");
        let cursor = Cursor::at(1, 0);
        let selection = Selection::new(Cursor::at(1, 0), Cursor::at(2, 5));
        let r = delete_line(&mut buffer, &cursor, Some(&selection)).unwrap();
        // Both beta and gamma vanish; cursor parks at end of alpha.
        assert_eq!(buffer.text(), "alpha");
        assert_eq!(r.new_cursor.line, 0);
        assert_eq!(r.new_cursor.column, "alpha".chars().count());
        assert!(r.is_multiline);
    }
}
