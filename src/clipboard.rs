use arboard::Clipboard;
use std::sync::{Mutex, OnceLock};

// Linux-specific imports for PRIMARY selection support
#[cfg(target_os = "linux")]
use arboard::{GetExtLinux, LinuxClipboardKind, SetExtLinux};

/// Global clipboard instance that persists for the application lifetime.
/// This ensures clipboard data remains available after write operations.
static CLIPBOARD: OnceLock<Mutex<Clipboard>> = OnceLock::new();

/// Get or initialize the global clipboard instance
fn get_clipboard() -> &'static Mutex<Clipboard> {
    CLIPBOARD.get_or_init(|| Mutex::new(Clipboard::new().expect("Failed to initialize clipboard")))
}

/// Copy text to system clipboard
///
/// Uses arboard for cross-platform clipboard access.
/// Works on Linux (Wayland/X11), Windows, and macOS without external dependencies.
///
/// On Linux, copies to BOTH CLIPBOARD and PRIMARY selections for compatibility
/// with middle-click paste and Shift+Insert.
///
/// Returns Ok(()) on success, or Err with detailed error message.
pub fn copy(text: String) -> Result<(), String> {
    if text.is_empty() {
        return Err("Cannot copy empty text".to_string());
    }

    #[cfg(target_os = "linux")]
    {
        // Use long-lived clipboard object for both CLIPBOARD and PRIMARY
        let mut clipboard = get_clipboard()
            .lock()
            .map_err(|e| format!("Failed to lock clipboard: {}", e))?;

        // Copy to CLIPBOARD selection (Ctrl+C/V)
        clipboard
            .set()
            .clipboard(LinuxClipboardKind::Clipboard)
            .text(text.clone())
            .map_err(|e| format!("Failed to set clipboard text: {}", e))?;

        // Copy to PRIMARY selection (middle-click/Shift+Insert) using same object
        // Ignore errors - PRIMARY may not be supported on some Wayland compositors
        let _ = clipboard
            .set()
            .clipboard(LinuxClipboardKind::Primary)
            .text(text);
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

/// Paste text from system clipboard
///
/// On Linux, tries CLIPBOARD selection first (more reliable for Ctrl+C/X/V),
/// then falls back to PRIMARY selection (for middle-click paste support).
///
/// Returns None if clipboard is empty or inaccessible.
pub fn paste() -> Option<String> {
    let mut clipboard = get_clipboard().lock().ok()?;

    #[cfg(target_os = "linux")]
    {
        // Try CLIPBOARD selection first (more reliable for Ctrl+C/X/V)
        // PRIMARY selection can be cleared when text selection disappears after cut
        if let Ok(text) = clipboard
            .get()
            .clipboard(LinuxClipboardKind::Clipboard)
            .text()
        {
            if !text.is_empty() {
                return Some(text);
            }
        }

        // Fall back to PRIMARY selection (for middle-click paste)
        clipboard
            .get()
            .clipboard(LinuxClipboardKind::Primary)
            .text()
            .ok()
    }

    #[cfg(not(target_os = "linux"))]
    clipboard.get_text().ok()
}

/// Cut text to clipboard (for file manager - marks for move operation)
///
/// Note: The actual move/delete logic is handled by the file manager.
/// This function just copies the text to clipboard like copy().
pub fn cut(text: String) -> Result<(), String> {
    copy(text)
}
