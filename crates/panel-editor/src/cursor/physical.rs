//! Physical cursor movement operations.
//!
//! This module provides basic cursor movement that doesn't account for word wrap.

use termide_buffer::{Cursor, TextBuffer};

/// Move cursor up by one line.
///
/// Returns true if preferred column should be maintained.
pub fn move_up(cursor: &mut Cursor) -> bool {
    cursor.move_up(1);
    true // Maintain preferred column
}

/// Move cursor down by one line.
///
/// Returns true if preferred column should be maintained.
pub fn move_down(cursor: &mut Cursor, buffer: &TextBuffer) -> bool {
    let max_line = buffer.line_count().saturating_sub(1);
    cursor.move_down(1, max_line);
    true // Maintain preferred column
}

/// Move cursor left by one character.
///
/// Returns true to reset preferred column (horizontal movement).
pub fn move_left(cursor: &mut Cursor, buffer: &TextBuffer) -> bool {
    cursor.move_left(1);

    // Clamp cursor to line length
    if cursor.line < buffer.line_count() {
        let line_len = buffer.line_len_graphemes(cursor.line);
        cursor.clamp_column(line_len);
    }

    false // Reset preferred column
}

/// Move cursor right by one character.
///
/// Returns true to reset preferred column (horizontal movement).
pub fn move_right(cursor: &mut Cursor, buffer: &TextBuffer) -> bool {
    let line_len = buffer.line_len_graphemes(cursor.line);
    let max_line = buffer.line_count().saturating_sub(1);
    cursor.move_right(1, line_len, max_line);
    false // Reset preferred column
}

/// Move cursor to start of current line.
///
/// Returns true to reset preferred column.
pub fn move_to_line_start(cursor: &mut Cursor) -> bool {
    cursor.column = 0;
    false // Reset preferred column
}

/// Move cursor to end of current line.
///
/// Returns true to reset preferred column.
pub fn move_to_line_end(cursor: &mut Cursor, buffer: &TextBuffer) -> bool {
    let line_len = buffer.line_len_graphemes(cursor.line);
    cursor.column = line_len;
    false // Reset preferred column
}

/// Move cursor to start of document.
///
/// Returns (new_cursor, should_scroll_viewport_to_top).
pub fn move_to_document_start() -> (Cursor, bool) {
    (Cursor::at(0, 0), true)
}

/// Move cursor to end of document.
///
/// Returns (new_cursor, should_scroll_viewport_to_bottom).
pub fn move_to_document_end(buffer: &TextBuffer) -> (Cursor, bool) {
    let max_line = buffer.line_count().saturating_sub(1);
    let line_len = buffer.line_len_graphemes(max_line);
    (Cursor::at(max_line, line_len), true)
}

/// Clamp cursor position to valid buffer bounds.
pub fn clamp_cursor(cursor: &mut Cursor, buffer: &TextBuffer) {
    let max_line = buffer.line_count().saturating_sub(1);
    if cursor.line > max_line {
        cursor.line = max_line;
    }

    let line_len = buffer.line_len_graphemes(cursor.line);
    cursor.clamp_column(line_len);
}
