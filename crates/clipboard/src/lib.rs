//! Clipboard operations for termide.
//!
//! Provides cross-platform clipboard access using arboard.
//! On Linux, supports both CLIPBOARD and PRIMARY selections.

use arboard::Clipboard;
use std::sync::{Mutex, OnceLock};

#[cfg(target_os = "linux")]
use arboard::{GetExtLinux, LinuxClipboardKind, SetExtLinux};

/// Global clipboard instance that persists for the application lifetime.
static CLIPBOARD: OnceLock<Mutex<Clipboard>> = OnceLock::new();

/// Get or initialize the global clipboard instance.
fn get_clipboard() -> &'static Mutex<Clipboard> {
    CLIPBOARD.get_or_init(|| Mutex::new(Clipboard::new().expect("Failed to initialize clipboard")))
}

/// Copy text to system clipboard.
///
/// Uses arboard for cross-platform clipboard access.
/// On Linux, copies to BOTH CLIPBOARD and PRIMARY selections.
///
/// Returns Ok(()) on success, or Err with detailed error message.
pub fn copy(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Err("Cannot copy empty text".to_string());
    }

    #[cfg(target_os = "linux")]
    {
        let mut clipboard = get_clipboard()
            .lock()
            .map_err(|e| format!("Failed to lock clipboard: {}", e))?;

        // Copy to CLIPBOARD selection (Ctrl+C/V)
        clipboard
            .set()
            .clipboard(LinuxClipboardKind::Clipboard)
            .text(text.to_string())
            .map_err(|e| format!("Failed to set clipboard text: {}", e))?;

        // Copy to PRIMARY selection (middle-click/Shift+Insert)
        let _ = clipboard
            .set()
            .clipboard(LinuxClipboardKind::Primary)
            .text(text.to_string());
    }

    #[cfg(not(target_os = "linux"))]
    {
        let mut clipboard = get_clipboard()
            .lock()
            .map_err(|e| format!("Failed to lock clipboard: {}", e))?;
        clipboard
            .set_text(text)
            .map_err(|e| format!("Failed to set clipboard text: {}", e))?;
    }

    Ok(())
}

/// Paste text from system clipboard.
///
/// On Linux, tries CLIPBOARD selection first, then falls back to PRIMARY.
/// Returns None if clipboard is empty or inaccessible.
pub fn paste() -> Option<String> {
    let mut clipboard = get_clipboard().lock().ok()?;

    #[cfg(target_os = "linux")]
    {
        // Try CLIPBOARD selection first
        if let Ok(text) = clipboard
            .get()
            .clipboard(LinuxClipboardKind::Clipboard)
            .text()
        {
            if !text.is_empty() {
                return Some(text);
            }
        }

        // Fall back to PRIMARY selection
        clipboard
            .get()
            .clipboard(LinuxClipboardKind::Primary)
            .text()
            .ok()
    }

    #[cfg(not(target_os = "linux"))]
    clipboard.get_text().ok()
}

/// Cut text to clipboard.
///
/// Same as copy - actual deletion is handled by the caller.
pub fn cut(text: &str) -> Result<(), String> {
    copy(text)
}

/// Check if clipboard contains text.
pub fn has_text() -> bool {
    paste().map(|t| !t.is_empty()).unwrap_or(false)
}
