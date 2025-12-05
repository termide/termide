//! Word wrap calculations for editor content.
//!
//! This module provides utilities for calculating line wrapping in the editor,
//! including smart wrapping (breaking at word boundaries) and hard wrapping
//! (breaking at fixed column width).

use crate::editor::TextBuffer;

/// Calculate wrap points for a single line of text.
///
/// Returns (visual_row_count, wrap_points) where wrap_points contains
/// the character indices where the line should wrap.
pub fn get_line_wrap_points(
    line_text: &str,
    content_width: usize,
    use_smart_wrap: bool,
) -> (usize, Vec<usize>) {
    if content_width == 0 {
        return (1, Vec::new());
    }

    let chars: Vec<char> = line_text.chars().collect();
    let line_len = chars.len();

    if line_len == 0 {
        return (1, Vec::new());
    }

    if line_len <= content_width {
        return (1, Vec::new()); // No wrapping needed
    }

    if use_smart_wrap {
        // Use smart wrapping from wrap.rs module
        let wrap_points = crate::editor::calculate_wrap_points_for_line(line_text, content_width);
        let visual_rows = wrap_points.len() + 1; // +1 for the first line
        (visual_rows, wrap_points)
    } else {
        // Use simple wrapping (hard break at content_width)
        let visual_rows = line_len.div_ceil(content_width);
        let mut wrap_points = Vec::new();
        for i in 1..visual_rows {
            wrap_points.push(i * content_width);
        }
        (visual_rows, wrap_points)
    }
}

/// Calculate the visual row index for a cursor position.
///
/// Returns the visual row index from viewport.top_line.
/// This accounts for word wrapping - a single buffer line may span multiple visual rows.
///
/// # Parameters
/// - `buffer`: The text buffer
/// - `cursor_line`: Current cursor line (buffer coordinates)
/// - `cursor_col`: Current cursor column (buffer coordinates)
/// - `viewport_top`: Top line of viewport (buffer coordinates)
/// - `content_width`: Width of content area for wrapping
/// - `word_wrap_enabled`: Whether word wrap is enabled
/// - `use_smart_wrap`: Whether to use smart wrapping
#[allow(dead_code)] // May be used in future phases
pub fn calculate_visual_row_for_cursor(
    buffer: &TextBuffer,
    cursor_line: usize,
    cursor_col: usize,
    viewport_top: usize,
    content_width: usize,
    word_wrap_enabled: bool,
    use_smart_wrap: bool,
) -> usize {
    if content_width == 0 || !word_wrap_enabled {
        // No word wrap - visual row is just buffer line offset from top
        return cursor_line.saturating_sub(viewport_top);
    }

    let mut visual_row = 0;
    let mut line_idx = viewport_top;

    // Count visual rows from viewport top to cursor line
    while line_idx < cursor_line && line_idx < buffer.line_count() {
        if let Some(line_text) = buffer.line(line_idx) {
            let line_text = line_text.trim_end_matches('\n');
            let (line_visual_rows, _) =
                get_line_wrap_points(line_text, content_width, use_smart_wrap);
            visual_row += line_visual_rows;
        } else {
            visual_row += 1; // Empty line = 1 visual row
        }
        line_idx += 1;
    }

    // Now add the visual row within the cursor's line
    if let Some(line_text) = buffer.line(cursor_line) {
        let line_text = line_text.trim_end_matches('\n');
        let (_line_visual_rows, wrap_points) =
            get_line_wrap_points(line_text, content_width, use_smart_wrap);

        // Find which visual row within this line the cursor is on
        let cursor_col_clamped = cursor_col.min(line_text.chars().count());
        let row_within_line = wrap_points
            .iter()
            .filter(|&&wp| wp <= cursor_col_clamped)
            .count();
        visual_row += row_within_line;
    }

    visual_row
}

/// Calculate total number of visual rows in the entire buffer.
///
/// This accounts for word wrapping - returns total visual rows across all lines.
pub fn calculate_total_visual_rows(
    buffer: &TextBuffer,
    content_width: usize,
    word_wrap_enabled: bool,
    use_smart_wrap: bool,
) -> usize {
    if content_width == 0 || !word_wrap_enabled {
        // No word wrap - just return buffer line count
        return buffer.line_count();
    }

    let mut total_visual_rows = 0;

    for line_idx in 0..buffer.line_count() {
        if let Some(line_text) = buffer.line(line_idx) {
            let line_text = line_text.trim_end_matches('\n');
            let (line_visual_rows, _) =
                get_line_wrap_points(line_text, content_width, use_smart_wrap);
            total_visual_rows += line_visual_rows;
        } else {
            total_visual_rows += 1; // Empty line = 1 visual row
        }
    }

    total_visual_rows
}

/// Convert visual row to buffer position accounting for word wrap.
///
/// Returns (buffer_line, column_offset) for the given visual row.
///
/// # Parameters
/// - `buffer`: The text buffer
/// - `visual_row`: Visual row index relative to viewport
/// - `viewport_top`: Top line of viewport (buffer coordinates)
/// - `content_width`: Width of content area for wrapping
/// - `use_smart_wrap`: Whether to use smart wrapping
pub fn visual_row_to_buffer_position(
    buffer: &TextBuffer,
    visual_row: usize,
    viewport_top: usize,
    content_width: usize,
    use_smart_wrap: bool,
) -> (usize, usize) {
    if content_width == 0 {
        return (viewport_top + visual_row, 0);
    }

    let mut current_visual_row = 0;
    let mut line_idx = viewport_top;

    while line_idx < buffer.line_count() {
        if let Some(line_text) = buffer.line(line_idx) {
            let line_text = line_text.trim_end_matches('\n');

            // Calculate how many visual rows this line occupies using actual wrap points
            let (visual_rows_for_line, wrap_points) =
                get_line_wrap_points(line_text, content_width, use_smart_wrap);

            // Check if target visual row is in this buffer line
            if current_visual_row + visual_rows_for_line > visual_row {
                // Found the buffer line containing the target visual row
                let row_within_line = visual_row - current_visual_row;

                // Calculate column offset using actual wrap points
                let column_offset = if row_within_line == 0 {
                    0 // First visual line starts at column 0
                } else if row_within_line - 1 < wrap_points.len() {
                    wrap_points[row_within_line - 1] // Use actual wrap point
                } else {
                    0 // Shouldn't happen, but safe fallback
                };

                return (line_idx, column_offset);
            }

            current_visual_row += visual_rows_for_line;
        } else {
            // If line doesn't exist, treat as empty (1 visual row)
            if current_visual_row >= visual_row {
                return (line_idx, 0);
            }
            current_visual_row += 1;
        }

        line_idx += 1;
    }

    // If we've exhausted all lines, return the last line
    (buffer.line_count().saturating_sub(1), 0)
}

// WrapContext helper struct removed - not currently used
// Can be re-added in future phases if needed for cleaner APIs
