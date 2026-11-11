//! Clipboard operations for the terminal.
//!
//! This module provides clipboard-related functionality including:
//! - Getting selected text from the terminal buffer
//! - Copying selection to system clipboard
//! - Pasting from clipboard to PTY

use anyhow::Result;
use std::sync::RwLock;

use crate::terminal::TerminalScreen;

/// Get selected text from terminal screen using absolute buffer coordinates.
pub fn get_selected_text(screen: &RwLock<TerminalScreen>) -> String {
    let screen = screen.read().unwrap_or_else(|e| {
        log::warn!("Terminal screen RwLock poisoned (read), recovering");
        e.into_inner()
    });
    let (start, end) = match (screen.selection_start, screen.selection_end) {
        (Some(s), Some(e)) => (s, e),
        _ => return String::new(),
    };

    // Normalize: start should be before end
    let (start, end) = if start <= end {
        (start, end)
    } else {
        (end, start)
    };

    let mut result = String::new();

    // Selection coordinates are absolute buffer positions
    for abs_row in start.0..=end.0 {
        // Get row from scrollback or active buffer
        let Some(row) = screen.get_line_by_absolute(abs_row) else {
            continue;
        };

        let col_start = if abs_row == start.0 { start.1 } else { 0 };
        let col_end = if abs_row == end.0 {
            end.1.min(row.len().saturating_sub(1))
        } else {
            row.len().saturating_sub(1)
        };

        let line_start = result.len();

        for col_idx in col_start..=col_end {
            if let Some(cell) = row.get(col_idx) {
                cell.push_text(&mut result);
            }
        }

        // Trim trailing spaces from this line
        let trimmed_len = result[line_start..].trim_end_matches(' ').len();
        result.truncate(line_start + trimmed_len);

        // Add line break between lines, but skip for soft-wrapped lines
        if abs_row < end.0 && !screen.get_wrapped_by_absolute(abs_row) {
            result.push('\n');
        }
    }

    result
}

/// The last `max` lines the terminal holds — its screen and, outside a
/// full-screen program, the scrollback above it — as text, with the number
/// of lines. Soft-wrapped rows join into one line; trailing spaces and the
/// blank lines below the last output are dropped.
pub fn tail_text(screen: &RwLock<TerminalScreen>, max: usize) -> (String, usize) {
    let screen = screen.read().unwrap_or_else(|e| e.into_inner());
    let rows = if screen.use_alt_screen {
        screen.alt_lines.len()
    } else {
        screen.scrollback.len() + screen.lines.len()
    };
    let row_text = |abs_row: usize| {
        let mut text = String::new();
        for cell in screen.get_line_by_absolute(abs_row).unwrap_or_default() {
            cell.push_text(&mut text);
        }
        text
    };
    // Walk up from the bottom a logical line at a time: one ends at `end`
    // and starts below the last row above it that was not soft-wrapped.
    let mut lines: Vec<String> = Vec::new();
    let mut end = rows;
    while end > 0 && lines.len() < max {
        let mut start = end - 1;
        while start > 0 && screen.get_wrapped_by_absolute(start - 1) {
            start -= 1;
        }
        let line: String = (start..end).map(row_text).collect();
        let line = line.trim_end().to_string();
        if !(lines.is_empty() && line.is_empty()) {
            lines.push(line);
        }
        end = start;
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.reverse();
    let count = lines.len();
    (lines.join("\n"), count)
}

/// Copy selected text to clipboard.
pub fn copy_selection_to_clipboard(screen: &RwLock<TerminalScreen>) -> Result<()> {
    let text = get_selected_text(screen);
    if text.is_empty() {
        return Ok(());
    }

    termide_ui::clipboard::copy(&text).map_err(|e| anyhow::anyhow!("{}", e))
}

/// Paste text from system clipboard.
///
/// Returns the text to paste, or None if clipboard is empty. A clipboard
/// holding only a native file list yields its paths, so a file copied from
/// the file manager still types a usable path into the shell.
pub fn get_clipboard_text() -> Option<String> {
    termide_ui::clipboard::paste_text_or_paths()
}

/// Paste data as one chunk, so nothing interleaves with it, optionally
/// wrapped in bracketed-paste markers.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(text.len() + 14);

    if bracketed {
        buffer.extend_from_slice(b"\x1b[200~");
    }
    buffer.extend_from_slice(text.as_bytes());
    if bracketed {
        buffer.extend_from_slice(b"\x1b[201~");
    }

    buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn print(screen: &mut TerminalScreen, text: &str) {
        for ch in text.chars() {
            match ch {
                '\n' => {
                    screen.carriage_return();
                    screen.newline();
                }
                ch => screen.put_char(ch),
            }
        }
    }

    /// The tail reaches up into the scrollback, joins a soft-wrapped line
    /// and leaves out the empty rows below the prompt.
    #[test]
    fn the_tail_is_the_last_lines_with_wrapped_ones_joined() {
        let mut screen = TerminalScreen::new(4, 10);
        print(&mut screen, "one\ntwo\nthree\n0123456789abc\n$ ");
        let screen = RwLock::new(screen);
        assert_eq!(
            tail_text(&screen, 10),
            ("one\ntwo\nthree\n0123456789abc\n$".to_string(), 5)
        );
        assert_eq!(tail_text(&screen, 2), ("0123456789abc\n$".to_string(), 2));
        let empty = RwLock::new(TerminalScreen::new(4, 10));
        assert_eq!(tail_text(&empty, 10), (String::new(), 0));
    }
}
