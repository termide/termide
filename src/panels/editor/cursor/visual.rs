//! Visual cursor movement operations (word wrap aware).
//!
//! This module provides cursor movement that accounts for word wrapping.

use crate::editor::{Cursor, TextBuffer};
use crate::panels::editor::word_wrap;

/// Helper to calculate visual row boundaries from wrap points.
fn get_visual_row_bounds(
    visual_row: usize,
    wrap_points: &[usize],
    line_len: usize,
) -> (usize, usize) {
    let start = if visual_row == 0 {
        0
    } else if visual_row - 1 < wrap_points.len() {
        wrap_points[visual_row - 1]
    } else {
        0
    };

    let end = if visual_row < wrap_points.len() {
        wrap_points[visual_row]
    } else {
        line_len
    };

    (start, end)
}

/// Clamp column to visual row bounds, respecting preferred column.
fn clamp_to_visual_row(
    preferred_col: usize,
    visual_row_start: usize,
    visual_row_end: usize,
) -> usize {
    if preferred_col < visual_row_start {
        visual_row_start
    } else if preferred_col >= visual_row_end {
        visual_row_end.saturating_sub(1).max(visual_row_start)
    } else {
        preferred_col
    }
}

/// Move cursor up by one visual line.
///
/// Returns new cursor position if movement occurred, None otherwise.
pub fn move_up(
    cursor: &Cursor,
    buffer: &TextBuffer,
    preferred_column: Option<usize>,
    content_width: usize,
    use_smart_wrap: bool,
) -> Option<Cursor> {
    let preferred_col = preferred_column.unwrap_or(cursor.column);

    // Try to move within current line first
    if let Some(line_text) = buffer.line(cursor.line) {
        let line_text = line_text.trim_end_matches('\n');
        let line_len = line_text.chars().count();
        let cursor_col = cursor.column.min(line_len);

        let (_visual_rows, wrap_points) =
            word_wrap::get_line_wrap_points(line_text, content_width, use_smart_wrap);

        let current_visual_row = wrap_points.iter().filter(|&&wp| wp <= cursor_col).count();

        if current_visual_row > 0 {
            // Move up within same physical line
            let target_visual_row = current_visual_row - 1;
            let (visual_row_start, visual_row_end) =
                get_visual_row_bounds(target_visual_row, &wrap_points, line_len);
            let new_col = clamp_to_visual_row(preferred_col, visual_row_start, visual_row_end);
            return Some(Cursor::at(cursor.line, new_col));
        }
    }

    // Move to previous physical line
    if cursor.line > 0 {
        let new_line = cursor.line - 1;

        if let Some(line_text) = buffer.line(new_line) {
            let line_text = line_text.trim_end_matches('\n');
            let line_len = line_text.chars().count();

            if line_len == 0 {
                return Some(Cursor::at(new_line, 0));
            }

            let (visual_rows, wrap_points) =
                word_wrap::get_line_wrap_points(line_text, content_width, use_smart_wrap);
            let last_visual_row = visual_rows - 1;

            let (visual_row_start, visual_row_end) =
                get_visual_row_bounds(last_visual_row, &wrap_points, line_len);
            let new_col = clamp_to_visual_row(preferred_col, visual_row_start, visual_row_end);
            return Some(Cursor::at(new_line, new_col));
        }
    }

    None
}

/// Move cursor down by one visual line.
///
/// Returns new cursor position if movement occurred, None otherwise.
pub fn move_down(
    cursor: &Cursor,
    buffer: &TextBuffer,
    preferred_column: Option<usize>,
    content_width: usize,
    use_smart_wrap: bool,
) -> Option<Cursor> {
    let preferred_col = preferred_column.unwrap_or(cursor.column);

    // Try to move within current line first
    if let Some(line_text) = buffer.line(cursor.line) {
        let line_text = line_text.trim_end_matches('\n');
        let line_len = line_text.chars().count();
        let cursor_col = cursor.column.min(line_len);

        let (total_visual_rows, wrap_points) =
            word_wrap::get_line_wrap_points(line_text, content_width, use_smart_wrap);

        let current_visual_row = wrap_points.iter().filter(|&&wp| wp <= cursor_col).count();

        if current_visual_row + 1 < total_visual_rows {
            // Move down within same physical line
            let target_visual_row = current_visual_row + 1;
            let (visual_row_start, visual_row_end) =
                get_visual_row_bounds(target_visual_row, &wrap_points, line_len);
            let new_col = clamp_to_visual_row(preferred_col, visual_row_start, visual_row_end);
            return Some(Cursor::at(cursor.line, new_col));
        }
    }

    // Move to next physical line
    let max_line = buffer.line_count().saturating_sub(1);
    if cursor.line < max_line {
        let new_line = cursor.line + 1;

        if let Some(line_text) = buffer.line(new_line) {
            let line_text = line_text.trim_end_matches('\n');
            let line_len = line_text.chars().count();

            if line_len == 0 {
                return Some(Cursor::at(new_line, 0));
            }

            let (_visual_rows, wrap_points) =
                word_wrap::get_line_wrap_points(line_text, content_width, use_smart_wrap);

            // First visual row
            let visual_row_end = if !wrap_points.is_empty() {
                wrap_points[0]
            } else {
                line_len
            };

            let new_col = clamp_to_visual_row(preferred_col, 0, visual_row_end);
            return Some(Cursor::at(new_line, new_col));
        }
    }

    None
}

/// Move cursor to start of current visual line.
///
/// Returns new column position.
pub fn move_to_visual_line_start(
    cursor: &Cursor,
    buffer: &TextBuffer,
    content_width: usize,
    use_smart_wrap: bool,
) -> usize {
    if let Some(line_text) = buffer.line(cursor.line) {
        let line_text = line_text.trim_end_matches('\n');
        let line_len = line_text.chars().count();
        let cursor_col = cursor.column.min(line_len);

        let (_visual_rows, wrap_points) =
            word_wrap::get_line_wrap_points(line_text, content_width, use_smart_wrap);

        // Find which visual row the cursor is on
        let current_visual_row = wrap_points.iter().filter(|&&wp| wp <= cursor_col).count();

        // Get start of this visual row
        let (visual_row_start, _) =
            get_visual_row_bounds(current_visual_row, &wrap_points, line_len);
        return visual_row_start;
    }

    0
}

/// Move cursor to end of current visual line.
///
/// Returns new column position.
pub fn move_to_visual_line_end(
    cursor: &Cursor,
    buffer: &TextBuffer,
    content_width: usize,
    use_smart_wrap: bool,
) -> usize {
    if let Some(line_text) = buffer.line(cursor.line) {
        let line_text = line_text.trim_end_matches('\n');
        let line_len = line_text.chars().count();
        let cursor_col = cursor.column.min(line_len);

        let (_visual_rows, wrap_points) =
            word_wrap::get_line_wrap_points(line_text, content_width, use_smart_wrap);

        // Find which visual row the cursor is on
        let current_visual_row = wrap_points.iter().filter(|&&wp| wp <= cursor_col).count();

        // Get end of this visual row
        let (_, visual_row_end) = get_visual_row_bounds(current_visual_row, &wrap_points, line_len);
        return visual_row_end;
    }

    0
}

/// Move cursor up by page_size visual lines.
///
/// Returns final cursor position after moving up by page_size steps or until top of document.
pub fn page_up(
    cursor: &Cursor,
    buffer: &TextBuffer,
    preferred_column: Option<usize>,
    content_width: usize,
    use_smart_wrap: bool,
    page_size: usize,
) -> Cursor {
    let mut current_cursor = *cursor;

    for _ in 0..page_size {
        let prev_cursor = current_cursor;

        if let Some(new_cursor) = move_up(
            &current_cursor,
            buffer,
            preferred_column,
            content_width,
            use_smart_wrap,
        ) {
            current_cursor = new_cursor;
        }

        // Stop if we haven't moved (at top of document)
        if current_cursor == prev_cursor {
            break;
        }
    }

    current_cursor
}

/// Move cursor down by page_size visual lines.
///
/// Returns final cursor position after moving down by page_size steps or until bottom of document.
pub fn page_down(
    cursor: &Cursor,
    buffer: &TextBuffer,
    preferred_column: Option<usize>,
    content_width: usize,
    use_smart_wrap: bool,
    page_size: usize,
) -> Cursor {
    let mut current_cursor = *cursor;
    let max_line = buffer.line_count().saturating_sub(1);

    for _ in 0..page_size {
        let prev_cursor = current_cursor;

        if let Some(new_cursor) = move_down(
            &current_cursor,
            buffer,
            preferred_column,
            content_width,
            use_smart_wrap,
        ) {
            current_cursor = new_cursor;
        }

        // Stop if we haven't moved (at bottom of document)
        if current_cursor == prev_cursor {
            break;
        }

        // Stop if we reached the last line
        if current_cursor.line >= max_line {
            break;
        }
    }

    current_cursor
}
