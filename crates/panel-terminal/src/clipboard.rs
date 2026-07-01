//! Clipboard operations for the terminal.
//!
//! This module provides clipboard-related functionality including:
//! - Getting selected text from the terminal buffer
//! - Copying selection to system clipboard
//! - Pasting from clipboard to PTY

use anyhow::Result;
use std::io::Write;
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
            if col_idx < row.len() {
                let ch = row[col_idx].ch;
                if ch != '\0' {
                    result.push(ch);
                }
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
/// Returns the text to paste, or None if clipboard is empty.
pub fn get_clipboard_text() -> Option<String> {
    termide_ui::clipboard::paste()
}

/// Send paste data as a single atomic write with optional bracketed paste.
pub fn paste_atomic<W: Write>(writer: &mut W, text: &str, bracketed: bool) -> Result<()> {
    let mut buffer = Vec::with_capacity(text.len() + 14);

    if bracketed {
        buffer.extend_from_slice(b"\x1b[200~");
    }
    buffer.extend_from_slice(text.as_bytes());
    if bracketed {
        buffer.extend_from_slice(b"\x1b[201~");
    }

    writer.write_all(&buffer)?;
    writer.flush()?;

    Ok(())
}
