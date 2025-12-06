//! Text selection operations for the editor.
//!
//! This module provides utilities for managing text selections, including
//! selecting all text, getting selected text, and deleting selections.

use anyhow::Result;

use crate::editor::{is_word_boundary, Cursor, HighlightCache, Selection, TextBuffer};

/// Select all text in the buffer.
///
/// Returns (new_selection, new_cursor_position).
pub fn select_all(buffer: &TextBuffer) -> (Selection, Cursor) {
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

/// Get selected text from buffer.
pub fn get_selected_text(buffer: &TextBuffer, selection: Option<&Selection>) -> Option<String> {
    if let Some(selection) = selection {
        if !selection.is_empty() {
            let start = selection.start();
            let end = selection.end();

            // Simple implementation - get all text and cut the needed fragment
            // TODO: optimize for large selections
            let full_text = buffer.text();
            let lines: Vec<&str> = full_text.lines().collect();

            if start.line == end.line {
                // Single line
                if let Some(line) = lines.get(start.line) {
                    // Extract substring by character indices without allocating Vec<char>
                    let selected: String = line
                        .chars()
                        .skip(start.column)
                        .take(end.column.saturating_sub(start.column))
                        .collect();
                    return Some(selected);
                }
            } else {
                // Multiple lines
                let mut result = String::new();
                for (i, line) in lines.iter().enumerate() {
                    if i < start.line || i > end.line {
                        continue;
                    }

                    if i == start.line {
                        // Extract from start.column to end without Vec<char>
                        for ch in line.chars().skip(start.column) {
                            result.push(ch);
                        }
                        result.push('\n');
                    } else if i == end.line {
                        // Extract from beginning to end.column without Vec<char>
                        for ch in line.chars().take(end.column) {
                            result.push(ch);
                        }
                    } else {
                        result.push_str(line);
                        result.push('\n');
                    }
                }
                return Some(result);
            }
        }
    }
    None
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

/// Invalidate highlight cache after selection deletion.
///
/// This is a helper to ensure cache invalidation happens consistently.
pub fn invalidate_cache_after_deletion(
    highlight_cache: &mut HighlightCache,
    deletion_start_line: usize,
    buffer_line_count: usize,
) {
    highlight_cache.invalidate_range(deletion_start_line, buffer_line_count);
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

    let chars: Vec<char> = line_text.chars().collect();
    let char_count = chars.len();

    // Clamp column to valid range
    let col = cursor.column.min(char_count.saturating_sub(1));

    // If cursor is at a word boundary character, don't select anything
    if col < char_count && is_word_boundary(chars[col]) {
        return None;
    }

    // Find word start (go backwards until word boundary)
    let mut start_col = col;
    while start_col > 0 && !is_word_boundary(chars[start_col - 1]) {
        start_col -= 1;
    }

    // Find word end (go forwards until word boundary or end of line)
    let mut end_col = col;
    while end_col < char_count && !is_word_boundary(chars[end_col]) {
        end_col += 1;
    }

    // Create selection
    let start = Cursor::at(cursor.line, start_col);
    let end = Cursor::at(cursor.line, end_col);

    Some((Selection::new(start, end), end))
}
