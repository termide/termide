//! Vim operators (delete, yank, change) and their execution.

use termide_buffer::{Cursor, Selection, TextBuffer};
use unicode_segmentation::UnicodeSegmentation;

use super::state::VimState;

/// Get text in a range from buffer.
fn get_text_in_range(buffer: &TextBuffer, start: &Cursor, end: &Cursor) -> String {
    let mut result = String::new();

    for line_idx in start.line..=end.line {
        if let Some(line) = buffer.line(line_idx) {
            let line_graphemes: Vec<&str> = line.graphemes(true).collect();

            let start_col = if line_idx == start.line {
                start.column
            } else {
                0
            };

            let end_col = if line_idx == end.line {
                end.column.min(line_graphemes.len())
            } else {
                line_graphemes.len()
            };

            for i in start_col..end_col {
                if i < line_graphemes.len() {
                    result.push_str(line_graphemes[i]);
                }
            }
        }
    }

    result
}

/// Vim operator types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimOperator {
    /// Delete operator (d)
    Delete,
    /// Yank operator (y)
    Yank,
    /// Change operator (c) - delete and enter insert mode
    Change,
}

/// Result of operator execution.
#[derive(Debug)]
pub struct OperatorResult {
    /// New cursor position after operation.
    pub cursor: Cursor,
    /// Whether to enter insert mode after operation.
    pub enter_insert: bool,
    /// Text that was yanked/deleted (for register).
    pub yanked_text: Option<String>,
    /// Whether the yanked text is linewise.
    pub linewise: bool,
}

/// Execute an operator with the given range.
///
/// # Arguments
/// * `op` - The operator to execute
/// * `start` - Start cursor position
/// * `end` - End cursor position
/// * `buffer` - The text buffer
/// * `vim_state` - Vim state for register storage
/// * `linewise` - Whether this is a linewise operation (dd, yy, cc)
///
/// # Returns
/// Result containing new cursor position and operation effects
pub fn execute_operator(
    op: VimOperator,
    start: Cursor,
    end: Cursor,
    buffer: &mut TextBuffer,
    vim_state: &mut VimState,
    linewise: bool,
) -> anyhow::Result<OperatorResult> {
    // Normalize start and end so start comes before end
    let (start, end) = normalize_range(start, end);

    if linewise {
        execute_linewise_operator(op, start.line, end.line, buffer, vim_state)
    } else {
        execute_charwise_operator(op, start, end, buffer, vim_state)
    }
}

/// Execute a linewise operator (dd, yy, cc, etc.)
pub fn execute_linewise_operator(
    op: VimOperator,
    start_line: usize,
    end_line: usize,
    buffer: &mut TextBuffer,
    vim_state: &mut VimState,
) -> anyhow::Result<OperatorResult> {
    // Collect the lines to yank
    let mut yanked_lines = String::new();
    for line_idx in start_line..=end_line {
        if let Some(line) = buffer.line(line_idx) {
            yanked_lines.push_str(&line);
            if !line.ends_with('\n') {
                yanked_lines.push('\n');
            }
        }
    }

    // Store in register
    vim_state.yank(yanked_lines.clone(), true);

    let new_cursor = match op {
        VimOperator::Yank => {
            // Just yank, cursor stays at start line
            Cursor::at(start_line, 0)
        }
        VimOperator::Delete | VimOperator::Change => {
            // Delete the lines
            delete_lines(buffer, start_line, end_line)?;

            // Position cursor at start line, first non-blank
            let new_line = start_line.min(buffer.line_count().saturating_sub(1));
            let first_non_blank = find_first_non_blank(buffer, new_line);
            Cursor::at(new_line, first_non_blank)
        }
    };

    Ok(OperatorResult {
        cursor: new_cursor,
        enter_insert: op == VimOperator::Change,
        yanked_text: Some(yanked_lines),
        linewise: true,
    })
}

/// Execute a character-wise operator.
fn execute_charwise_operator(
    op: VimOperator,
    start: Cursor,
    end: Cursor,
    buffer: &mut TextBuffer,
    vim_state: &mut VimState,
) -> anyhow::Result<OperatorResult> {
    // Create selection for the range
    let selection = Selection::new(start, end);
    let sel_start = selection.start();
    let sel_end = selection.end();

    // Get the text in the range
    let yanked_text = get_text_in_range(buffer, &sel_start, &sel_end);

    // Store in register
    vim_state.yank(yanked_text.clone(), false);

    let new_cursor = match op {
        VimOperator::Yank => {
            // Just yank, cursor goes to start
            start
        }
        VimOperator::Delete | VimOperator::Change => {
            // Delete the range
            buffer.delete_range(&sel_start, &sel_end)?;
            sel_start
        }
    };

    Ok(OperatorResult {
        cursor: new_cursor,
        enter_insert: op == VimOperator::Change,
        yanked_text: Some(yanked_text),
        linewise: false,
    })
}

/// Delete character under cursor (x command).
pub fn delete_char(buffer: &mut TextBuffer, cursor: &Cursor) -> anyhow::Result<Option<String>> {
    let line_len = buffer.line_len_graphemes(cursor.line);
    if cursor.column < line_len {
        // Get the character at cursor
        let end = Cursor::at(cursor.line, cursor.column + 1);
        let deleted = get_text_in_range(buffer, cursor, &end);
        // Delete one character
        buffer.delete_range(cursor, &end)?;
        Ok(Some(deleted))
    } else {
        Ok(None)
    }
}

/// Normalize range so start comes before end.
fn normalize_range(start: Cursor, end: Cursor) -> (Cursor, Cursor) {
    if start.line < end.line || (start.line == end.line && start.column <= end.column) {
        (start, end)
    } else {
        (end, start)
    }
}

/// Delete lines from buffer (inclusive).
fn delete_lines(buffer: &mut TextBuffer, start_line: usize, end_line: usize) -> anyhow::Result<()> {
    // Delete from start of first line to start of line after last (or end of document)
    let (start, end) = if end_line + 1 < buffer.line_count() {
        (Cursor::at(start_line, 0), Cursor::at(end_line + 1, 0))
    } else {
        // Through the last line: take the line break before the lines too,
        // or an empty line would stay in their place.
        let end = Cursor::at(end_line, buffer.line_len_graphemes(end_line));
        let start = match start_line.checked_sub(1) {
            Some(above) => Cursor::at(above, buffer.line_len_graphemes(above)),
            None => Cursor::at(0, 0),
        };
        (start, end)
    };

    buffer.delete_range(&start, &end)?;
    Ok(())
}

/// Find first non-blank column on a line.
fn find_first_non_blank(buffer: &TextBuffer, line: usize) -> usize {
    use unicode_segmentation::UnicodeSegmentation;

    if let Some(line_text) = buffer.line(line) {
        let line_text = line_text.trim_end_matches('\n');
        line_text
            .graphemes(true)
            .position(|g| !g.chars().all(|c| c.is_whitespace()))
            .unwrap_or(0)
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_buffer(content: &str) -> TextBuffer {
        TextBuffer::from_text(content)
    }

    #[test]
    fn test_delete_char() {
        let mut buffer = create_buffer("hello");
        let cursor = Cursor::at(0, 0);

        let deleted = delete_char(&mut buffer, &cursor).unwrap();
        assert_eq!(deleted, Some("h".to_string()));
        assert_eq!(buffer.text(), "ello");
    }

    #[test]
    fn test_linewise_yank() {
        let mut buffer = create_buffer("line 1\nline 2\nline 3");
        let mut vim_state = VimState::new();

        let result =
            execute_linewise_operator(VimOperator::Yank, 0, 0, &mut buffer, &mut vim_state)
                .unwrap();

        assert!(!result.enter_insert);
        assert!(result.linewise);
        assert_eq!(vim_state.get_register(), Some("line 1\n"));
        // Buffer unchanged for yank
        assert!(buffer.text().contains("line 1"));
    }

    #[test]
    fn test_linewise_delete() {
        let mut buffer = create_buffer("line 1\nline 2\nline 3");
        let mut vim_state = VimState::new();

        let result =
            execute_linewise_operator(VimOperator::Delete, 1, 1, &mut buffer, &mut vim_state)
                .unwrap();

        assert!(!result.enter_insert);
        assert!(result.linewise);
        assert_eq!(vim_state.get_register(), Some("line 2\n"));
        assert!(!buffer.text().contains("line 2"));
    }

    #[test]
    fn test_linewise_change() {
        let mut buffer = create_buffer("line 1\nline 2\nline 3");
        let mut vim_state = VimState::new();

        let result =
            execute_linewise_operator(VimOperator::Change, 0, 0, &mut buffer, &mut vim_state)
                .unwrap();

        assert!(result.enter_insert);
        assert!(result.linewise);
    }

    #[test]
    fn test_normalize_range() {
        let start = Cursor::at(5, 10);
        let end = Cursor::at(2, 5);

        let (norm_start, norm_end) = normalize_range(start, end);
        assert_eq!(norm_start, end);
        assert_eq!(norm_end, start);
    }
}
