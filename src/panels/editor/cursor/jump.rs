//! Jump cursor movement operations (page up/down).
//!
//! This module provides page-based cursor movement operations.

use crate::editor::{Cursor, TextBuffer};

/// Move cursor one page up.
///
/// Returns (should_scroll_viewport, scroll_amount).
pub fn page_up(cursor: &mut Cursor, page_size: usize) -> (bool, usize) {
    cursor.move_up(page_size);
    (true, page_size) // Should scroll viewport up
}

/// Move cursor one page down.
///
/// Returns (should_scroll_viewport, scroll_amount).
pub fn page_down(cursor: &mut Cursor, buffer: &TextBuffer, page_size: usize) -> (bool, usize) {
    let max_line = buffer.line_count().saturating_sub(1);
    cursor.move_down(page_size, max_line);
    (true, page_size) // Should scroll viewport down
}
