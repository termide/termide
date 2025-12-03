use arboard::Clipboard;

// Linux-specific imports for PRIMARY selection support
#[cfg(target_os = "linux")]
use arboard::{GetExtLinux, LinuxClipboardKind, SetExtLinux};

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

    let mut clipboard =
        Clipboard::new().map_err(|e| format!("Failed to access clipboard: {}", e))?;

    #[cfg(target_os = "linux")]
    {
        // Copy to CLIPBOARD selection (Ctrl+C/V)
        clipboard
            .set()
            .clipboard(LinuxClipboardKind::Clipboard)
            .text(text.clone())
            .map_err(|e| format!("Failed to set clipboard text: {}", e))?;

        // Copy to PRIMARY selection (middle-click/Shift+Insert)
        // Ignore errors - PRIMARY may not be supported on some Wayland compositors
        let _ = clipboard
            .set()
            .clipboard(LinuxClipboardKind::Primary)
            .text(text);
    }

    #[cfg(not(target_os = "linux"))]
    {
        clipboard
            .set_text(text)
            .map_err(|e| format!("Failed to set clipboard text: {}", e))?;
    }

    Ok(())
}

/// Paste text from system clipboard
///
/// On Linux, tries PRIMARY selection first (for middle-click paste support),
/// then falls back to CLIPBOARD selection.
///
/// Returns None if clipboard is empty or inaccessible.
pub fn paste() -> Option<String> {
    let mut clipboard = Clipboard::new().ok()?;

    #[cfg(target_os = "linux")]
    {
        // Try PRIMARY selection first (from middle-click copy or text selection)
        if let Ok(text) = clipboard
            .get()
            .clipboard(LinuxClipboardKind::Primary)
            .text()
        {
            if !text.is_empty() {
                return Some(text);
            }
        }

        // Fall back to CLIPBOARD selection
        clipboard
            .get()
            .clipboard(LinuxClipboardKind::Clipboard)
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
