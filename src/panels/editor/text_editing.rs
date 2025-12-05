//! Text editing operations for the editor.
//!
//! This module provides core text editing functionality including character
//! insertion, deletion, and line duplication.

use anyhow::Result;

use crate::editor::{Cursor, Selection, TextBuffer};

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

    // Get all text and extract the lines to duplicate
    let full_text = buffer.text();
    let lines: Vec<&str> = full_text.lines().collect();

    // Build text to duplicate
    let mut text_to_duplicate = String::new();
    for line_idx in start_line..=end_line {
        if let Some(line) = lines.get(line_idx) {
            text_to_duplicate.push_str(line);
            if line_idx < end_line {
                text_to_duplicate.push('\n');
            }
        }
    }

    // Insert newline and duplicated text after the last line
    text_to_duplicate.insert(0, '\n');

    // Move cursor to end of last line to duplicate
    let last_line_len = buffer.line_len_graphemes(end_line);
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
