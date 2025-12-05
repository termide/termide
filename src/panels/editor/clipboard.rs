//! Clipboard operations for the editor.
//!
//! This module provides utilities for copying, cutting, and pasting text
//! to/from the system clipboard.

use anyhow::Result;

use crate::editor::{Cursor, TextBuffer};

/// Result of a clipboard operation.
pub struct ClipboardResult {
    pub status_message: String,
}

/// Copy selected text to clipboard.
///
/// Returns status message for display.
pub fn copy_to_clipboard(selected_text: Option<String>) -> ClipboardResult {
    match selected_text {
        Some(text) => {
            // Debug: show what we're trying to copy
            let char_count = text.chars().count();
            let preview = if char_count > 50 {
                let preview_text: String = text.chars().take(50).collect();
                format!("{}...", preview_text)
            } else {
                text.clone()
            };

            match crate::clipboard::copy(text) {
                Ok(()) => ClipboardResult {
                    status_message: format!(
                        "Copied to clipboard: {:?} ({} chars)",
                        preview, char_count
                    ),
                },
                Err(e) => ClipboardResult {
                    status_message: format!("Clipboard error: {}", e),
                },
            }
        }
        None => ClipboardResult {
            status_message: "Nothing selected to copy".to_string(),
        },
    }
}

/// Cut selected text to clipboard.
///
/// Returns status message and whether deletion should proceed.
pub fn cut_to_clipboard(selected_text: Option<String>) -> (ClipboardResult, bool) {
    match selected_text {
        Some(text) => {
            let char_count = text.chars().count();
            let preview = if char_count > 50 {
                let preview_text: String = text.chars().take(50).collect();
                format!("{}...", preview_text)
            } else {
                text.clone()
            };

            match crate::clipboard::copy(text) {
                Ok(()) => (
                    ClipboardResult {
                        status_message: format!(
                            "Cut to clipboard: {:?} ({} chars)",
                            preview, char_count
                        ),
                    },
                    true, // should delete
                ),
                Err(e) => (
                    ClipboardResult {
                        status_message: format!("Clipboard error: {}", e),
                    },
                    false, // should not delete
                ),
            }
        }
        None => (
            ClipboardResult {
                status_message: "Nothing selected to cut".to_string(),
            },
            false,
        ),
    }
}

/// Paste from clipboard into buffer.
///
/// Returns new cursor position and cache invalidation info on success.
pub fn paste_from_clipboard(
    buffer: &mut TextBuffer,
    cursor: &Cursor,
) -> Result<Option<(Cursor, usize, bool)>> {
    // Read from system clipboard via arboard
    if let Some(text) = crate::clipboard::paste() {
        if !text.is_empty() {
            let start_line = cursor.line;
            let new_cursor = buffer.insert(cursor, &text)?;

            let is_multiline = text.contains('\n');
            return Ok(Some((new_cursor, start_line, is_multiline)));
        }
    }
    Ok(None)
}
