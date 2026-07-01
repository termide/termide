//! Clipboard operations for termide.
//!
//! Provides cross-platform clipboard access using arboard with OSC 52
//! fallback for remote/SSH sessions where no display server is available.

use arboard::Clipboard;
use base64::{engine::general_purpose::STANDARD, Engine};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

#[cfg(target_os = "linux")]
use arboard::{GetExtLinux, LinuxClipboardKind, SetExtLinux};

/// Global clipboard instance that persists for the application lifetime.
/// `None` when clipboard is unavailable (e.g. headless servers).
static CLIPBOARD: OnceLock<Option<Mutex<Clipboard>>> = OnceLock::new();

/// Get or initialize the global clipboard instance.
///
/// Returns an error on systems without clipboard support (e.g. headless servers).
fn get_clipboard() -> Result<&'static Mutex<Clipboard>, String> {
    CLIPBOARD
        .get_or_init(|| Clipboard::new().ok().map(Mutex::new))
        .as_ref()
        .ok_or_else(|| "Clipboard unavailable (no display server?)".to_string())
}

/// Copy text to the terminal's clipboard via OSC 52 escape sequence.
///
/// This works over SSH when the terminal emulator supports OSC 52
/// (Windows Terminal, iTerm2, kitty, foot, alacritty, etc.).
///
/// Safety: writes directly to stdout. This is called synchronously from key
/// event handlers (between render frames), so there is no race with the
/// ratatui render loop.
fn osc52_copy(text: &str) -> Result<(), String> {
    let encoded = STANDARD.encode(text.as_bytes());
    // OSC 52 ; c ; <base64> ST  ('c' = clipboard selection)
    let sequence = format!("\x1b]52;c;{}\x07", encoded);
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(sequence.as_bytes())
        .and_then(|_| stdout.flush())
        .map_err(|e| format!("Failed to write OSC 52: {}", e))
}

/// Detect whether a display server is available on Linux.
///
/// Returns true when `$DISPLAY` or `$WAYLAND_DISPLAY` is set, indicating
/// an X11 or Wayland session where arboard can reach the clipboard.
/// On non-Linux platforms this always returns true (arboard works natively).
#[cfg(target_os = "linux")]
fn has_display_server() -> bool {
    std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok()
}

#[cfg(not(target_os = "linux"))]
fn has_display_server() -> bool {
    true
}

/// Copy text to system clipboard.
///
/// Uses arboard for local clipboard access. Falls back to OSC 52
/// escape sequence when no display server is available (headless, SSH,
/// Docker, serial console, etc.).
/// On Linux with a display server, copies to BOTH CLIPBOARD and PRIMARY selections.
///
/// Returns Ok(()) on success, or Err with detailed error message.
pub fn copy(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Err("Cannot copy empty text".to_string());
    }

    // Without a display server arboard cannot reach the clipboard — go
    // straight to OSC 52 which the terminal emulator handles locally.
    if !has_display_server() {
        return osc52_copy(text);
    }

    // Try arboard first (works with display server)
    let arboard_result = copy_arboard(text);

    if arboard_result.is_ok() {
        return Ok(());
    }

    // Fall back to OSC 52 for other failures
    log::warn!(
        "arboard failed ({}), falling back to OSC 52",
        arboard_result.unwrap_err()
    );
    osc52_copy(text)
}

/// Copy text using arboard (requires display server).
fn copy_arboard(text: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let mut clipboard = get_clipboard()?
            .lock()
            .map_err(|e| format!("Failed to lock clipboard: {}", e))?;

        // Copy to CLIPBOARD selection (Ctrl+C/V)
        clipboard
            .set()
            .clipboard(LinuxClipboardKind::Clipboard)
            .text(text.to_string())
            .map_err(|e| format!("Failed to set clipboard text: {}", e))?;

        // Copy to PRIMARY selection (middle-click/Shift+Insert)
        if let Err(e) = clipboard
            .set()
            .clipboard(LinuxClipboardKind::Primary)
            .text(text.to_string())
        {
            #[cfg(debug_assertions)]
            log::warn!("Failed to set PRIMARY selection: {}", e);
            let _ = e; // Suppress unused warning in release
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let mut clipboard = get_clipboard()?
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
///
/// Note: OSC 52 paste (reading from terminal) is not supported because it requires
/// async terminal response handling. Paste in SSH sessions relies on the terminal
/// emulator's bracketed paste (Ctrl+V in the terminal sends the text directly).
pub fn paste() -> Option<String> {
    let mut clipboard = get_clipboard().ok()?.lock().ok()?;

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
