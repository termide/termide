//! Vim motion types and execution.

use termide_buffer::{Cursor, TextBuffer};
use unicode_segmentation::UnicodeSegmentation;

use crate::word_boundary::{char_type, find_next_word_start, find_prev_word_start, CharType};
use crate::word_wrap::WrapLayout;

/// Vim motion types for cursor movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMotion {
    /// Move left (h)
    Left,
    /// Move down (j) - logical line movement
    Down,
    /// Move up (k) - logical line movement
    Up,
    /// Move right (l)
    Right,
    /// Word forward (w)
    WordForward,
    /// Word backward (b)
    WordBackward,
    /// Word end (e)
    WordEnd,
    /// Line start (0)
    LineStart,
    /// First non-blank character (^)
    FirstNonBlank,
    /// Line end ($)
    LineEnd,
    /// Document start (gg)
    DocumentStart,
    /// Document end (G)
    DocumentEnd,
    /// Go to specific line ({n}G)
    GoToLine(usize),
    /// Half page up (Ctrl+u)
    HalfPageUp,
    /// Half page down (Ctrl+d)
    HalfPageDown,
    /// Move up by visual line (gk, ↑) - respects word wrap
    VisualUp,
    /// Move down by visual line (gj, ↓) - respects word wrap
    VisualDown,
}

/// Execute a motion and return the new cursor position.
///
/// Returns the new cursor position after applying the motion `count` times.
///
/// # Arguments
/// * `motion` - The motion to execute
/// * `cursor` - Current cursor position
/// * `buffer` - Text buffer
/// * `count` - Number of times to repeat the motion
/// * `viewport_height` - Height of the viewport (for page motions)
/// * `content_width` - Width of the content area (for visual line motions)
/// * `use_smart_wrap` - Whether to use smart word wrapping
pub fn execute_motion(
    motion: VimMotion,
    cursor: &Cursor,
    buffer: &TextBuffer,
    count: usize,
    viewport_height: usize,
    layout: WrapLayout,
) -> Cursor {
    let mut new_cursor = *cursor;
    let count = count.max(1);

    match motion {
        VimMotion::Left => {
            for _ in 0..count {
                if new_cursor.column > 0 {
                    new_cursor.column -= 1;
                }
            }
        }
        VimMotion::Down => {
            // A fold counts as one line, as in Vim.
            for _ in 0..count {
                let next = buffer.visible_at_or_after(new_cursor.line + 1);
                if next < buffer.line_count() {
                    new_cursor.line = next;
                }
            }
            clamp_column(&mut new_cursor, buffer);
        }
        VimMotion::Up => {
            for _ in 0..count {
                if new_cursor.line > 0 {
                    new_cursor.line = buffer.visible_at_or_before(new_cursor.line - 1);
                }
            }
            clamp_column(&mut new_cursor, buffer);
        }
        VimMotion::Right => {
            let line_len = buffer.line_len_graphemes(new_cursor.line);
            for _ in 0..count {
                // In normal mode, cursor shouldn't go past last character
                if new_cursor.column < line_len.saturating_sub(1) {
                    new_cursor.column += 1;
                }
            }
        }
        VimMotion::WordForward => {
            for _ in 0..count {
                new_cursor = move_word_forward(&new_cursor, buffer);
            }
        }
        VimMotion::WordBackward => {
            for _ in 0..count {
                new_cursor = move_word_backward(&new_cursor, buffer);
            }
        }
        VimMotion::WordEnd => {
            for _ in 0..count {
                new_cursor = move_word_end(&new_cursor, buffer);
            }
        }
        VimMotion::LineStart => {
            new_cursor.column = 0;
        }
        VimMotion::FirstNonBlank => {
            if let Some(line) = buffer.line(new_cursor.line) {
                let line = line.trim_end_matches('\n');
                let first_non_blank = line
                    .graphemes(true)
                    .position(|g| !g.chars().all(|c| c.is_whitespace()))
                    .unwrap_or(0);
                new_cursor.column = first_non_blank;
            }
        }
        VimMotion::LineEnd => {
            let line_len = buffer.line_len_graphemes(new_cursor.line);
            // In normal mode, cursor sits on last character, not past it
            new_cursor.column = line_len.saturating_sub(1);
        }
        VimMotion::DocumentStart => {
            new_cursor.line = 0;
            new_cursor.column = 0;
        }
        VimMotion::DocumentEnd => {
            new_cursor.line = buffer.line_count().saturating_sub(1);
            // With count, G goes to line number (1-indexed)
            clamp_column(&mut new_cursor, buffer);
        }
        VimMotion::GoToLine(line_num) => {
            // Line numbers are 1-indexed in Vim
            let target_line =
                (line_num.saturating_sub(1)).min(buffer.line_count().saturating_sub(1));
            new_cursor.line = target_line;
            new_cursor.column = 0;
            // Move to first non-blank
            if let Some(line) = buffer.line(new_cursor.line) {
                let line = line.trim_end_matches('\n');
                let first_non_blank = line
                    .graphemes(true)
                    .position(|g| !g.chars().all(|c| c.is_whitespace()))
                    .unwrap_or(0);
                new_cursor.column = first_non_blank;
            }
        }
        VimMotion::HalfPageUp => {
            let half_page = (viewport_height / 2).max(1);
            let move_amount = (half_page * count).min(new_cursor.line);
            new_cursor.line -= move_amount;
            clamp_column(&mut new_cursor, buffer);
        }
        VimMotion::HalfPageDown => {
            let half_page = (viewport_height / 2).max(1);
            let max_line = buffer.line_count().saturating_sub(1);
            let move_amount = (half_page * count).min(max_line - new_cursor.line);
            new_cursor.line += move_amount;
            clamp_column(&mut new_cursor, buffer);
        }
        VimMotion::VisualUp => {
            for _ in 0..count {
                if let Some(new) = crate::cursor::visual::move_up(&new_cursor, buffer, None, layout)
                {
                    new_cursor = new;
                }
            }
            // Clamp column to stay on last character in normal mode
            clamp_column(&mut new_cursor, buffer);
        }
        VimMotion::VisualDown => {
            for _ in 0..count {
                if let Some(new) =
                    crate::cursor::visual::move_down(&new_cursor, buffer, None, layout)
                {
                    new_cursor = new;
                }
            }
            // Clamp column to stay on last character in normal mode
            clamp_column(&mut new_cursor, buffer);
        }
    }

    new_cursor
}

/// Clamp cursor column to valid range for current line.
fn clamp_column(cursor: &mut Cursor, buffer: &TextBuffer) {
    let line_len = buffer.line_len_graphemes(cursor.line);
    if line_len == 0 {
        cursor.column = 0;
    } else {
        // In normal mode, cursor sits on last character
        cursor.column = cursor.column.min(line_len.saturating_sub(1));
    }
}

/// Move cursor to next word start.
fn move_word_forward(cursor: &Cursor, buffer: &TextBuffer) -> Cursor {
    let mut new_cursor = *cursor;
    let line_count = buffer.line_count();

    loop {
        if let Some(line) = buffer.line(new_cursor.line) {
            let line = line.trim_end_matches('\n');
            let graphemes: Vec<&str> = line.graphemes(true).collect();

            // Try to find next word start on current line
            if let Some(next_pos) = find_next_word_start(&graphemes, new_cursor.column) {
                new_cursor.column = next_pos;
                return new_cursor;
            }
        }

        // Move to next line
        if new_cursor.line + 1 < line_count {
            new_cursor.line += 1;
            new_cursor.column = 0;

            // Find first non-blank on new line
            if let Some(line) = buffer.line(new_cursor.line) {
                let line = line.trim_end_matches('\n');
                if !line.is_empty() {
                    let graphemes: Vec<&str> = line.graphemes(true).collect();
                    let first_non_blank = graphemes
                        .iter()
                        .position(|g| !g.chars().all(|c| c.is_whitespace()))
                        .unwrap_or(0);
                    new_cursor.column = first_non_blank;
                    return new_cursor;
                }
            }
        } else {
            // End of document
            return new_cursor;
        }
    }
}

/// Move cursor to previous word start.
fn move_word_backward(cursor: &Cursor, buffer: &TextBuffer) -> Cursor {
    let mut new_cursor = *cursor;

    loop {
        if let Some(line) = buffer.line(new_cursor.line) {
            let line = line.trim_end_matches('\n');
            let graphemes: Vec<&str> = line.graphemes(true).collect();

            if new_cursor.column > 0 {
                // Try to find previous word start on current line
                if let Some(prev_pos) = find_prev_word_start(&graphemes, new_cursor.column) {
                    new_cursor.column = prev_pos;
                    return new_cursor;
                }
            }
        }

        // Move to previous line
        if new_cursor.line > 0 {
            new_cursor.line -= 1;
            if let Some(line) = buffer.line(new_cursor.line) {
                let line = line.trim_end_matches('\n');
                new_cursor.column = line.graphemes(true).count().saturating_sub(1);
            }
        } else {
            new_cursor.column = 0;
            return new_cursor;
        }
    }
}

/// Move cursor to end of current or next word.
fn move_word_end(cursor: &Cursor, buffer: &TextBuffer) -> Cursor {
    let mut new_cursor = *cursor;
    let line_count = buffer.line_count();

    // First, move right one position to avoid staying at current word end
    if let Some(line) = buffer.line(new_cursor.line) {
        let line = line.trim_end_matches('\n');
        let line_len = line.graphemes(true).count();
        if new_cursor.column + 1 < line_len {
            new_cursor.column += 1;
        } else if new_cursor.line + 1 < line_count {
            new_cursor.line += 1;
            new_cursor.column = 0;
        }
    }

    loop {
        if let Some(line) = buffer.line(new_cursor.line) {
            let line = line.trim_end_matches('\n');
            let graphemes: Vec<&str> = line.graphemes(true).collect();

            // Skip leading whitespace
            while new_cursor.column < graphemes.len()
                && char_type(graphemes[new_cursor.column]) == CharType::Space
            {
                new_cursor.column += 1;
            }

            if new_cursor.column < graphemes.len() {
                // Find end of current word
                let word_type = char_type(graphemes[new_cursor.column]);
                while new_cursor.column + 1 < graphemes.len()
                    && char_type(graphemes[new_cursor.column + 1]) == word_type
                {
                    new_cursor.column += 1;
                }
                return new_cursor;
            }
        }

        // Move to next line
        if new_cursor.line + 1 < line_count {
            new_cursor.line += 1;
            new_cursor.column = 0;
        } else {
            return new_cursor;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_buffer(content: &str) -> TextBuffer {
        TextBuffer::from_text(content)
    }

    // Default content width and smart wrap for tests
    const TEST_CONTENT_WIDTH: usize = 80;
    const TEST_SMART_WRAP: bool = true;

    #[test]
    fn test_motion_left_right() {
        let buffer = create_buffer("hello world");
        let cursor = Cursor::at(0, 5);

        let new_cursor = execute_motion(
            VimMotion::Left,
            &cursor,
            &buffer,
            2,
            24,
            WrapLayout {
                width: TEST_CONTENT_WIDTH,
                smart: TEST_SMART_WRAP,
                tab_size: 4,
            },
        );
        assert_eq!(new_cursor.column, 3);

        let new_cursor = execute_motion(
            VimMotion::Right,
            &cursor,
            &buffer,
            2,
            24,
            WrapLayout {
                width: TEST_CONTENT_WIDTH,
                smart: TEST_SMART_WRAP,
                tab_size: 4,
            },
        );
        assert_eq!(new_cursor.column, 7);
    }

    #[test]
    fn test_motion_up_down() {
        let buffer = create_buffer("line 1\nline 2\nline 3");
        let cursor = Cursor::at(1, 2);

        let new_cursor = execute_motion(
            VimMotion::Up,
            &cursor,
            &buffer,
            1,
            24,
            WrapLayout {
                width: TEST_CONTENT_WIDTH,
                smart: TEST_SMART_WRAP,
                tab_size: 4,
            },
        );
        assert_eq!(new_cursor.line, 0);

        let new_cursor = execute_motion(
            VimMotion::Down,
            &cursor,
            &buffer,
            1,
            24,
            WrapLayout {
                width: TEST_CONTENT_WIDTH,
                smart: TEST_SMART_WRAP,
                tab_size: 4,
            },
        );
        assert_eq!(new_cursor.line, 2);
    }

    #[test]
    fn test_motion_line_start_end() {
        let buffer = create_buffer("  hello world");
        let cursor = Cursor::at(0, 5);

        let new_cursor = execute_motion(
            VimMotion::LineStart,
            &cursor,
            &buffer,
            1,
            24,
            WrapLayout {
                width: TEST_CONTENT_WIDTH,
                smart: TEST_SMART_WRAP,
                tab_size: 4,
            },
        );
        assert_eq!(new_cursor.column, 0);

        let new_cursor = execute_motion(
            VimMotion::FirstNonBlank,
            &cursor,
            &buffer,
            1,
            24,
            WrapLayout {
                width: TEST_CONTENT_WIDTH,
                smart: TEST_SMART_WRAP,
                tab_size: 4,
            },
        );
        assert_eq!(new_cursor.column, 2);

        let new_cursor = execute_motion(
            VimMotion::LineEnd,
            &cursor,
            &buffer,
            1,
            24,
            WrapLayout {
                width: TEST_CONTENT_WIDTH,
                smart: TEST_SMART_WRAP,
                tab_size: 4,
            },
        );
        assert_eq!(new_cursor.column, 12); // "  hello world" has 13 chars, last index is 12
    }

    #[test]
    fn test_motion_document_start_end() {
        let buffer = create_buffer("line 1\nline 2\nline 3");
        let cursor = Cursor::at(1, 2);

        let new_cursor = execute_motion(
            VimMotion::DocumentStart,
            &cursor,
            &buffer,
            1,
            24,
            WrapLayout {
                width: TEST_CONTENT_WIDTH,
                smart: TEST_SMART_WRAP,
                tab_size: 4,
            },
        );
        assert_eq!(new_cursor.line, 0);
        assert_eq!(new_cursor.column, 0);

        let new_cursor = execute_motion(
            VimMotion::DocumentEnd,
            &cursor,
            &buffer,
            1,
            24,
            WrapLayout {
                width: TEST_CONTENT_WIDTH,
                smart: TEST_SMART_WRAP,
                tab_size: 4,
            },
        );
        assert_eq!(new_cursor.line, 2);
    }

    #[test]
    fn test_motion_word_forward() {
        let buffer = create_buffer("hello world test");
        let cursor = Cursor::at(0, 0);

        let new_cursor = execute_motion(
            VimMotion::WordForward,
            &cursor,
            &buffer,
            1,
            24,
            WrapLayout {
                width: TEST_CONTENT_WIDTH,
                smart: TEST_SMART_WRAP,
                tab_size: 4,
            },
        );
        assert_eq!(new_cursor.column, 6); // Start of "world"

        let new_cursor = execute_motion(
            VimMotion::WordForward,
            &cursor,
            &buffer,
            2,
            24,
            WrapLayout {
                width: TEST_CONTENT_WIDTH,
                smart: TEST_SMART_WRAP,
                tab_size: 4,
            },
        );
        assert_eq!(new_cursor.column, 12); // Start of "test"
    }

    #[test]
    fn test_motion_goto_line() {
        let buffer = create_buffer("line 1\nline 2\nline 3");
        let cursor = Cursor::at(0, 0);

        let new_cursor = execute_motion(
            VimMotion::GoToLine(2),
            &cursor,
            &buffer,
            1,
            24,
            WrapLayout {
                width: TEST_CONTENT_WIDTH,
                smart: TEST_SMART_WRAP,
                tab_size: 4,
            },
        );
        assert_eq!(new_cursor.line, 1);
    }

    /// `j` keeps the screen column across a tab-indented line, through the
    /// same walk the arrow keys use.
    #[test]
    fn test_motion_down_keeps_the_screen_column_across_tabs() {
        let buffer = create_buffer("        xy\n\t\txy");
        let cursor = Cursor::at(0, 9);

        let new_cursor = execute_motion(
            VimMotion::Down,
            &cursor,
            &buffer,
            1,
            24,
            WrapLayout {
                width: TEST_CONTENT_WIDTH,
                smart: TEST_SMART_WRAP,
                tab_size: 4,
            },
        );
        assert_eq!((new_cursor.line, new_cursor.column), (1, 3));
    }
}
