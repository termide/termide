//! Jump cursor movement operations (page up/down).
//!
//! This module provides page-based cursor movement operations. A page counts
//! the lines that show: folded lines are passed over.

use termide_buffer::{Cursor, TextBuffer};

/// `line` counted in lines that show.
fn visible_index(buffer: &TextBuffer, line: usize) -> usize {
    line - buffer.hidden_before(line)
}

/// Move cursor one page up.
///
/// If less than a page remains to the start, moves cursor to position (0, 0).
/// Returns (should_scroll_viewport, scroll_amount).
pub fn page_up(cursor: &mut Cursor, buffer: &TextBuffer, page_size: usize) -> (bool, usize) {
    let index = visible_index(buffer, cursor.line);
    if index < page_size {
        // Less than a page to start - move to beginning of file
        cursor.line = 0;
        cursor.column = 0;
    } else {
        cursor.line = buffer.line_at_visible_index(index - page_size);
    }
    (true, page_size)
}

/// Move cursor one page down.
///
/// If less than a page remains to the end, moves cursor to end of file.
/// Returns (should_scroll_viewport, scroll_amount).
pub fn page_down(cursor: &mut Cursor, buffer: &TextBuffer, page_size: usize) -> (bool, usize) {
    let last_line = buffer.visible_at_or_before(buffer.line_count().saturating_sub(1));
    let remaining =
        visible_index(buffer, last_line).saturating_sub(visible_index(buffer, cursor.line));

    if remaining < page_size {
        // Less than a page to end - move to end of file
        cursor.line = last_line;
        cursor.column = buffer.line_len_graphemes(last_line);
    } else {
        cursor.line = buffer.line_at_visible_index(visible_index(buffer, cursor.line) + page_size);
    }
    (true, page_size)
}
