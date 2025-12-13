//! Search and replace operations for the editor.
//!
//! This module provides utilities for searching through text, navigating matches,
//! and performing find-and-replace operations.

use anyhow::Result;

use termide_buffer::{Cursor, SearchState, Selection, TextBuffer};

/// Perform search through the entire buffer.
///
/// Populates the search state with all matching positions.
pub fn perform_search(buffer: &TextBuffer, search: &mut SearchState) {
    search.matches.clear();

    if search.query.is_empty() {
        return;
    }

    let query = if search.case_sensitive {
        search.query.clone()
    } else {
        search.query.to_lowercase()
    };

    // Search through all lines
    for line_idx in 0..buffer.line_count() {
        if let Some(line_text) = buffer.line(line_idx) {
            let search_text = if search.case_sensitive {
                line_text.to_string()
            } else {
                line_text.to_lowercase()
            };

            // Find all occurrences in line
            let mut col = 0;
            while let Some(pos) = search_text[col..].find(&query) {
                let match_col = col + pos;
                search.matches.push(Cursor {
                    line: line_idx,
                    column: match_col,
                });
                col = match_col + 1;
            }
        }
    }
}

/// Get selection for a search match.
///
/// Returns (Selection, end_cursor) for highlighting the match.
pub fn get_match_selection(match_cursor: &Cursor, query_len: usize) -> (Selection, Cursor) {
    let end_cursor = Cursor::at(match_cursor.line, match_cursor.column + query_len);
    (Selection::new(*match_cursor, end_cursor), end_cursor)
}

/// Replace result information.
pub struct ReplaceResult {
    pub new_cursor: Cursor,
    pub start_line: usize,
}

/// Replace text at a specific match position.
///
/// Returns ReplaceResult with new cursor position and affected line.
pub fn replace_at_position(
    buffer: &mut TextBuffer,
    match_cursor: &Cursor,
    query_len: usize,
    replace_with: &str,
) -> Result<ReplaceResult> {
    let end_cursor = Cursor {
        line: match_cursor.line,
        column: match_cursor.column + query_len,
    };

    // Delete old text
    buffer.delete_range(match_cursor, &end_cursor)?;

    // Insert new text
    buffer.insert(match_cursor, replace_with)?;

    let new_cursor = Cursor {
        line: match_cursor.line,
        column: match_cursor.column + replace_with.len(),
    };

    Ok(ReplaceResult {
        new_cursor,
        start_line: match_cursor.line,
    })
}

/// Update match positions after a replacement.
///
/// Adjusts the positions of matches that come after the replacement point
/// on the same line.
pub fn update_match_positions_after_replace(
    matches: &mut [Cursor],
    match_cursor: &Cursor,
    query_len: usize,
    replace_with_len: usize,
) {
    let replacement_offset = replace_with_len as isize - query_len as isize;
    if replacement_offset != 0 {
        for match_pos in matches.iter_mut() {
            // Only update matches on same line that come after the replacement
            if match_pos.line == match_cursor.line && match_pos.column > match_cursor.column {
                // Adjust column position by the length difference
                match_pos.column = (match_pos.column as isize + replacement_offset).max(0) as usize;
            }
        }
    }
}

/// Replace all matches in reverse order.
///
/// Returns the number of replacements made.
pub fn replace_all_matches(
    buffer: &mut TextBuffer,
    matches: &[Cursor],
    query_len: usize,
    replace_with: &str,
) -> Result<usize> {
    let mut count = 0;

    // Replace in reverse order to avoid position shifts
    for match_cursor in matches.iter().rev() {
        let end_cursor = Cursor {
            line: match_cursor.line,
            column: match_cursor.column + query_len,
        };

        // Delete old text and insert new text
        buffer.delete_range(match_cursor, &end_cursor)?;
        buffer.insert(match_cursor, replace_with)?;

        count += 1;
    }

    Ok(count)
}
