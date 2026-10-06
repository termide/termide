//! Link detection for terminal (URLs and file paths).
//!
//! Joins the rows a wrapped link spans and finds the link under the pointer
//! with the detection shared with the other panels (`termide_core::links`).

use std::path::Path;

use termide_core::LinkTarget;

use crate::terminal::TerminalScreen;

/// Highlight segment: (abs_row, start_col, end_col)
pub type HighlightSegment = (usize, usize, usize);

/// Detect link (URL or file path) at given position.
/// Returns (target, start_row, start_col, display_len) if found.
/// `display_len` is the length of the matched text on screen (in cells),
/// which may differ from the target's own text for a resolved file path.
pub(crate) fn detect_link_at_position(
    screen: &TerminalScreen,
    abs_row: usize,
    col: usize,
    cwd: &Path,
) -> Option<(LinkTarget, usize, usize, usize)> {
    let cols = screen.cols;

    // Look back up to 5 lines to find where a wrapped link might have started
    let start_row = abs_row.saturating_sub(5);

    // Find the actual start row (first line that doesn't look like a continuation)
    let mut search_start = abs_row;
    for row in (start_row..abs_row).rev() {
        if let Some(line) = screen.get_line_by_absolute(row) {
            // Use cell count (not byte count) to check if line fills terminal width
            let trimmed_cell_len = line
                .iter()
                .rposition(|c| c.ch != ' ' && c.ch != '\0')
                .map_or(0, |p| p + 1);
            if trimmed_cell_len >= cols {
                search_start = row;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    // Concatenate text from search_start through current row and forward.
    // Track char offsets (= cell offsets) for each line: the shared detection
    // reports char ranges, which match cells where bytes would not.
    let mut combined_text = String::new();
    let mut line_starts: Vec<(usize, usize)> = Vec::new(); // (row, char_offset)
    let mut char_count: usize = 0;

    for row in search_start.. {
        if let Some(line) = screen.get_line_by_absolute(row) {
            line_starts.push((row, char_count));
            let line_text: String = line.iter().map(|c| c.ch).collect();
            char_count += line.len(); // cell count = char count (one char per cell)
            let trimmed_cell_len = line
                .iter()
                .rposition(|c| c.ch != ' ' && c.ch != '\0')
                .map_or(0, |p| p + 1);
            combined_text.push_str(&line_text);

            if row >= abs_row && trimmed_cell_len < cols {
                break;
            }
            if row > abs_row + 5 {
                break;
            }
        } else {
            break;
        }
    }

    // Calculate cursor offset in char/cell units
    let cursor_char_offset = line_starts
        .iter()
        .find(|(row, _)| *row == abs_row)
        .map(|(_, char_offset)| char_offset + col)?;

    // Helper to find start row/col from char offset
    let find_start_pos = |char_offset: usize| -> Option<(usize, usize)> {
        for (row, offset) in line_starts.iter().rev() {
            if char_offset >= *offset {
                return Some((*row, char_offset - offset));
            }
        }
        None
    };

    let (range, target) = termide_core::links::link_at(&combined_text, cursor_char_offset, cwd)?;
    let (row, col) = find_start_pos(range.start)?;
    Some((target, row, col, range.len()))
}

/// Build highlight segments for multi-line link.
pub(crate) fn build_link_segments(
    text_len: usize,
    start_row: usize,
    start_col: usize,
    cols: usize,
) -> Vec<HighlightSegment> {
    let mut segments = Vec::new();
    let mut remaining = text_len;
    let mut current_row = start_row;
    let mut current_col = start_col;

    while remaining > 0 {
        let available = cols.saturating_sub(current_col);
        let segment_len = remaining.min(available);

        if segment_len > 0 {
            segments.push((current_row, current_col, current_col + segment_len));
        }

        remaining = remaining.saturating_sub(segment_len);
        current_row += 1;
        current_col = 0;
    }

    segments
}
