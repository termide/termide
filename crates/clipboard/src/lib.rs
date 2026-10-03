//! Clipboard operations for termide.
//!
//! Provides cross-platform clipboard access using arboard with OSC 52
//! fallback for remote/SSH sessions where no display server is available.
//!
//! Two flavors are supported: plain text, and a native file list that other
//! applications paste as actual files (`NSPasteboardTypeFileURL` on macOS,
//! `CF_HDROP` on Windows, `text/uri-list` on Linux).

use arboard::Clipboard;
use base64::{engine::general_purpose::STANDARD, Engine};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[cfg(target_os = "linux")]
use arboard::{GetExtLinux, LinuxClipboardKind, SetExtLinux};

#[cfg(target_os = "macos")]
use arboard::SetExtApple;

#[cfg(windows)]
use arboard::SetExtWindows;

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

/// Copy a list of files to the system clipboard as a native file list, so
/// other applications paste them as files rather than as a path string.
///
/// The caller is responsible for filtering: only absolute paths that exist on
/// the local filesystem can be represented. Remote (SFTP/FTP) and in-archive
/// paths must stay on the text path — see `FileManager::clipboard_copy_selection`.
///
/// Platform notes, all of which are why this cannot be a plain
/// `set_text` followed by `file_list` on every OS:
///
/// - macOS/Linux: `file_list` owns the clipboard and clears it, so no text
///   flavor is left behind. Text consumers recover the paths via
///   [`paste_files`].
/// - Windows: `SetClipboardData` is only allowed for the current clipboard
///   owner, and `file_list` does not clear. Writing the text first calls
///   `EmptyClipboard`, which takes ownership and makes the file write legal;
///   as a side effect both flavors end up present.
///
/// Returns `Err` when nothing could be written, so the caller can fall back
/// to [`copy`] and report honestly instead of failing silently.
pub fn copy_files(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("Cannot copy empty file list".to_string());
    }

    if !has_display_server() {
        // OSC 52 carries text only; a headless session has no way to put a
        // native file list on the clipboard.
        return Err("File clipboard unavailable (no display server?)".to_string());
    }

    copy_files_arboard(paths).map_err(|e| {
        log::warn!("arboard file_list failed: {}", e);
        e
    })
}

/// Write the native file list. Split out so [`copy_files`] keeps a single
/// place for the display-server check and the logging.
fn copy_files_arboard(paths: &[PathBuf]) -> Result<(), String> {
    let mut clipboard = get_clipboard()?
        .lock()
        .map_err(|e| format!("Failed to lock clipboard: {}", e))?;

    #[cfg(windows)]
    {
        // Take ownership first: `file_list` calls `SetClipboardData` without
        // clearing, which the OS rejects unless we already own the clipboard.
        let text = paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        clipboard
            .set_text(&text)
            .map_err(|e| format!("Failed to set clipboard text: {}", e))?;
        clipboard
            .set()
            .exclude_from_history()
            .file_list(paths)
            .map_err(|e| format!("Failed to set clipboard files: {}", e))?;
    }

    #[cfg(not(windows))]
    {
        #[cfg(target_os = "linux")]
        let set = clipboard.set().clipboard(LinuxClipboardKind::Clipboard);
        #[cfg(not(target_os = "linux"))]
        let set = clipboard.set();

        set.exclude_from_history()
            .file_list(paths)
            .map_err(|e| format!("Failed to set clipboard files: {}", e))?;

        // macOS: `file_list` cleared the pasteboard to own it, so the text
        // flavor is gone. Add it back — `setString_forType` does not clear,
        // so both flavors survive and a plain-text consumer (a browser address
        // bar, the terminal emulator's own Cmd+V, which never reaches termide)
        // gets the paths instead of nothing.
        #[cfg(target_os = "macos")]
        add_text_flavor(&paths_to_text(paths))?;

        // Keep the PRIMARY selection in step with CLIPBOARD, matching what
        // `copy_arboard` does for text.
        #[cfg(target_os = "linux")]
        if let Err(e) = clipboard
            .set()
            .clipboard(LinuxClipboardKind::Primary)
            .file_list(paths)
        {
            log::debug!("Failed to set PRIMARY file list: {}", e);
        }
    }

    Ok(())
}

/// Put `text` on the general pasteboard without clearing what is there.
///
/// arboard's builder clears before every write, so it cannot leave a file
/// list and a text flavor side by side; this is the one place that has to.
#[cfg(target_os = "macos")]
fn add_text_flavor(text: &str) -> Result<(), String> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::NSString;

    let pasteboard = NSPasteboard::generalPasteboard();
    let string = NSString::from_str(text);

    // The only unsafe here: `NSPasteboardTypeString` is an `extern static`,
    // which Rust cannot check. arboard references its pasteboard constants the
    // same way.
    let ok = unsafe { pasteboard.setString_forType(&string, NSPasteboardTypeString) };

    if ok {
        Ok(())
    } else {
        Err("Pasteboard refused the text flavor".to_string())
    }
}

/// Read a native file list from the system clipboard.
///
/// Returns `None` when the clipboard holds no files. Text consumers that want
/// a path string out of a file copy should use [`paste_paths`] instead.
pub fn paste_files() -> Option<Vec<PathBuf>> {
    let mut clipboard = get_clipboard().ok()?.lock().ok()?;

    #[cfg(target_os = "linux")]
    {
        // Try CLIPBOARD selection first, then PRIMARY — the same order as
        // `paste`.
        if let Ok(files) = clipboard
            .get()
            .clipboard(LinuxClipboardKind::Clipboard)
            .file_list()
        {
            if !files.is_empty() {
                return Some(files);
            }
        }

        clipboard
            .get()
            .clipboard(LinuxClipboardKind::Primary)
            .file_list()
            .ok()
            .filter(|list| !list.is_empty())
    }

    #[cfg(not(target_os = "linux"))]
    clipboard
        .get()
        .file_list()
        .ok()
        .filter(|list| !list.is_empty())
}

/// Join paths into the newline-separated text used for the text flavor.
pub fn paths_to_text(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse newline-separated paths, dropping blank lines.
pub fn text_to_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Read paths from the clipboard, accepting either flavor.
///
/// Prefers a native file list, then falls back to newline-separated text.
pub fn paste_paths() -> Vec<PathBuf> {
    if let Some(files) = paste_files() {
        return files;
    }

    paste().map(|text| text_to_paths(&text)).unwrap_or_default()
}

/// Paste text for a text-consuming panel (editor, terminal, agent prompt).
///
/// Identical to [`paste`] except that a clipboard holding only a native file
/// list — which is what [`copy_files`] leaves on macOS and Linux — yields the
/// newline-joined paths instead of nothing. Without this, `Ctrl+V` after a
/// file copy would be a silent no-op outside the file manager.
pub fn paste_text_or_paths() -> Option<String> {
    if let Some(text) = paste() {
        if !text.is_empty() {
            return Some(text);
        }
    }

    let paths = paste_paths();
    if paths.is_empty() {
        return None;
    }

    Some(paths_to_text(&paths))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_round_trip_through_the_text_flavor() {
        let paths = vec![
            PathBuf::from("/tmp/one.txt"),
            PathBuf::from("/tmp/two words.md"),
        ];
        let text = paths_to_text(&paths);
        assert_eq!(text, "/tmp/one.txt\n/tmp/two words.md");
        assert_eq!(text_to_paths(&text), paths);
    }

    #[test]
    fn text_to_paths_drops_blank_and_whitespace_lines() {
        let paths = text_to_paths("/a\n\n  \n/b/c\n");
        assert_eq!(paths, vec![PathBuf::from("/a"), PathBuf::from("/b/c")]);
    }

    #[test]
    fn text_to_paths_handles_crlf_from_foreign_clipboards() {
        let paths = text_to_paths("/a\r\n/b\r\n");
        assert_eq!(paths, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    }

    #[test]
    fn empty_file_list_is_refused_before_touching_the_clipboard() {
        assert!(copy_files(&[]).is_err());
    }

    // The round trip through the real pasteboard is gated because opening the
    // clipboard is slow on some hosts (see the note in panel-agent tests).
    #[test]
    #[ignore = "touches the real system clipboard"]
    fn file_flavor_survives_a_copy_files_round_trip() {
        let file = std::env::temp_dir().join("termide-clipboard-flavor.txt");
        std::fs::write(&file, b"x").unwrap();

        let before = paste();
        let before_files = paste_files();
        copy_files(std::slice::from_ref(&file)).unwrap();

        // arboard canonicalizes on the way in, so assert the invariant rather
        // than the exact spelling: the file manager reads back a native list
        // naming our file...
        let paths = paste_paths();
        assert_eq!(paths.len(), 1, "one file was copied");
        assert_eq!(
            std::fs::read_to_string(&paths[0]).ok().as_deref(),
            Some("x")
        );

        // ...and a text panel gets those same paths instead of nothing.
        let text = paste_text_or_paths().expect("a text panel must still paste");
        assert_eq!(text_to_paths(&text), paths);

        // Put the clipboard back as we found it, file list included.
        if let Some(restored) = before {
            let _ = copy(&restored);
        } else if let Some(restored) = before_files {
            let _ = copy_files(&restored);
        } else if let Ok(clipboard) = get_clipboard() {
            if let Ok(mut clipboard) = clipboard.lock() {
                let _ = clipboard.clear();
            }
        }
        let _ = std::fs::remove_file(&file);
    }
}
