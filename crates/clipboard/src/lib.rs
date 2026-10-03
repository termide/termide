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

/// What [`cut_files`] last published, so a later paste can tell a cut from a
/// copy.
///
/// No system clipboard format carries a "cut" flag — neither
/// `NSPasteboardTypeFileURL` nor `CF_HDROP` — so the intent has to live here.
/// It is kept honest against the clipboard by [`is_cut`], which re-reads what
/// the clipboard actually holds (and, on macOS, its `changeCount`) rather than
/// trusting that nothing else has written to it since.
struct CutMarker {
    /// The paths published as the cut list, normalized by [`normalize`].
    paths: Vec<PathBuf>,
    /// macOS pasteboard change counter at publish time, used to notice that
    /// another application has since replaced the contents. `None` elsewhere,
    /// where the path comparison is the only signal available.
    change_count: Option<isize>,
}

impl CutMarker {
    /// Whether this marker still describes `paths`.
    ///
    /// Split out from [`is_cut`] so the rule is testable without a clipboard:
    /// a mismatched list is not a cut, and on macOS a pasteboard that has
    /// moved on belongs to someone else, so the cut is gone.
    fn matches(&self, paths: &[PathBuf], now_change_count: Option<isize>) -> bool {
        if self.paths != normalize(paths) {
            return false;
        }
        match (self.change_count, now_change_count) {
            (Some(was), Some(now)) => was == now,
            _ => true,
        }
    }
}

/// Resolve symlinked ancestors so both sides of the cut comparison agree.
///
/// The native file list comes back canonicalized (arboard on macOS and Linux
/// resolves it), while the panel publishes the path it shows, which may run
/// through a symlinked directory. A path that cannot be resolved stays as is.
fn normalize(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|path| dunce::canonicalize(path).unwrap_or_else(|_| path.clone()))
        .collect()
}

/// The pending cut, if the clipboard still carries one.
static CUT: OnceLock<Mutex<Option<CutMarker>>> = OnceLock::new();

fn cut_state() -> &'static Mutex<Option<CutMarker>> {
    CUT.get_or_init(|| Mutex::new(None))
}

/// Forget any pending cut, so the next paste copies.
///
/// Every write to the clipboard that is not a file cut calls this: once the
/// contents are no longer what was cut, the marker must not survive, because
/// pasting would then delete files the user only meant to copy. Losing the
/// marker is always safe — it degrades a cut into a copy, which leaves the
/// original in place.
pub fn clear_cut() {
    if let Ok(mut guard) = cut_state().lock() {
        *guard = None;
    }
}

/// The pasteboard's change counter, or `None` off macOS.
///
/// `changeCount` is a safe method in objc2; it bumps whenever any application
/// writes the clipboard, which makes it a reliable "someone replaced it"
/// signal without any unsafe.
#[cfg(target_os = "macos")]
fn pasteboard_change_count() -> Option<isize> {
    use objc2_app_kit::NSPasteboard;

    Some(NSPasteboard::generalPasteboard().changeCount())
}

#[cfg(not(target_os = "macos"))]
fn pasteboard_change_count() -> Option<isize> {
    None
}

/// Cut a list of files: publish them exactly as [`copy_files`] does, and
/// remember them so a later paste in the file manager moves instead of copies.
///
/// The clipboard payload is identical to a copy on purpose. A cut has no way
/// to announce itself to other applications, so Finder and Explorer will paste
/// a copy and leave the original — which is the safe direction to be wrong:
/// a stray copy is one `rm` away from fixed, a deleted original is not. The
/// move happens inside termide only, and [`is_cut`] guards it.
pub fn cut_files(paths: &[PathBuf]) -> Result<(), String> {
    copy_files(paths)?;
    // The counter is read here, right after the publish, so it reflects the
    // pasteboard this call just wrote.
    mark_cut_with(paths, pasteboard_change_count());
    Ok(())
}

/// Cut a list of files through the text flavor: publish the paths as text
/// and remember them as the cut list.
///
/// For a selection the native file list cannot carry (a symlink, which the
/// OS would resolve to its target) or a clipboard that refuses it. A paste in
/// the file manager reads the paths back from the text, so it still moves.
pub fn cut_paths_as_text(paths: &[PathBuf]) -> Result<(), String> {
    cut_publishing(paths, copy)
}

/// Publish `paths` as text through `publish`, then mark them as cut. The
/// marker is set only after the write, because [`copy`] clears it.
fn cut_publishing(
    paths: &[PathBuf],
    publish: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    publish(&paths_to_text(paths))?;
    mark_cut_with(paths, pasteboard_change_count());
    Ok(())
}

/// Remember `paths` as the cut list without publishing anything.
///
/// The marker carries no pasteboard counter, so it stays valid until something
/// else overwrites the clipboard through this crate. [`cut_files`] stamps the
/// counter, which is the stronger guard; this entry point exists for a caller
/// that publishes by other means and for tests, which must not depend on what
/// the real pasteboard happens to hold.
pub fn mark_cut(paths: &[PathBuf]) {
    mark_cut_with(paths, None);
}

fn mark_cut_with(paths: &[PathBuf], change_count: Option<isize>) {
    let marker = CutMarker {
        paths: normalize(paths),
        change_count,
    };
    if let Ok(mut guard) = cut_state().lock() {
        *guard = Some(marker);
    }
}

/// Whether `paths` are still the cut list rather than a copy.
///
/// True only when the marker matches `paths` item for item and, on macOS, the
/// pasteboard has not changed since. A copy of the same selection clears the
/// marker in [`copy_files`], so re-copying what was cut pastes a copy, which
/// is what the last keystroke asked for.
pub fn is_cut(paths: &[PathBuf]) -> bool {
    let Ok(guard) = cut_state().lock() else {
        return false;
    };
    let Some(marker) = guard.as_ref() else {
        return false;
    };

    // If the pasteboard moved on, another application owns the clipboard now
    // and our cut is gone; pasting must copy.
    marker.matches(paths, pasteboard_change_count())
}

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
///
/// A successful write replaces whatever the clipboard held, so it also drops
/// any pending file cut: pasting text that is not the cut list must not
/// delete the files that were cut.
pub fn copy(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Err("Cannot copy empty text".to_string());
    }

    // Without a display server arboard cannot reach the clipboard — go
    // straight to OSC 52 which the terminal emulator handles locally.
    if !has_display_server() {
        let result = osc52_copy(text);
        if result.is_ok() {
            clear_cut();
        }
        return result;
    }

    // Try arboard first (works with display server)
    let arboard_result = copy_arboard(text);

    if arboard_result.is_ok() {
        clear_cut();
        return Ok(());
    }

    // Fall back to OSC 52 for other failures
    log::warn!(
        "arboard failed ({}), falling back to OSC 52",
        arboard_result.unwrap_err()
    );
    let result = osc52_copy(text);
    if result.is_ok() {
        clear_cut();
    }
    result
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
            log::debug!("Failed to set PRIMARY selection: {}", e);
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
/// - macOS: `file_list` owns the clipboard and clears it, so the text
///   flavor is added back afterwards.
/// - Linux: `file_list` owns the clipboard and clears it, so no text flavor
///   is left behind. Text consumers recover the paths via [`paste_files`].
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
    })?;

    // The clipboard now holds this list, so any earlier cut is gone.
    // `cut_files` sets its marker after this returns, so a cut survives.
    clear_cut();
    Ok(())
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
        clipboard
            .set_text(paths_to_text(paths))
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
/// list — which is what [`copy_files`] leaves on Linux — yields the
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
/// Same as copy - actual deletion is handled by the caller. It also drops any
/// pending file cut, since the clipboard no longer holds the cut list; that is
/// what makes [`is_cut`] safe to trust.
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

    /// The cut marker is the only thing that turns a paste into a move, so
    /// every way it can go stale has to fall back to copying.
    #[test]
    fn the_cut_marker_survives_only_while_the_clipboard_still_holds_it() {
        let a = vec![PathBuf::from("/tmp/one.txt")];
        let other = vec![PathBuf::from("/tmp/two.txt")];

        // macOS: the counter is the signal that someone else wrote.
        let marker = CutMarker {
            paths: a.clone(),
            change_count: Some(7),
        };
        assert!(
            marker.matches(&a, Some(7)),
            "the same list on an untouched pasteboard is still a cut"
        );
        assert!(
            !marker.matches(&other, Some(7)),
            "a different list is not the cut"
        );
        assert!(
            !marker.matches(&a, Some(8)),
            "another application replaced the pasteboard, so pasting must copy"
        );

        // Linux/Windows: no counter, so the list comparison is all there is.
        let plain = CutMarker {
            paths: a.clone(),
            change_count: None,
        };
        assert!(plain.matches(&a, None));
        assert!(!plain.matches(&other, None));

        // A marker taken where no counter exists must not be invalidated by a
        // platform that later reports one, and vice versa.
        assert!(plain.matches(&a, Some(9)));
    }

    /// A cut that has to travel as text (a symlink in the selection, or no
    /// file flavor) must still be a cut: the marker is set after the write,
    /// which would otherwise have cleared it.
    #[test]
    fn a_cut_published_as_text_stays_a_cut() {
        let paths = vec![PathBuf::from("/tmp/termide-cut-text/one.txt")];
        let mut published = String::new();

        cut_publishing(&paths, |text| {
            clear_cut();
            published = text.to_string();
            Ok(())
        })
        .unwrap();

        assert_eq!(published, paths_to_text(&paths));
        let guard = cut_state().lock().unwrap();
        let marker = guard.as_ref().expect("the cut is remembered");
        assert_eq!(marker.paths, paths);
    }

    #[test]
    fn a_failed_text_cut_marks_nothing() {
        clear_cut();
        let paths = vec![PathBuf::from("/tmp/termide-cut-text/two.txt")];
        assert!(cut_publishing(&paths, |_| Err("no clipboard".to_string())).is_err());
        assert!(cut_state().lock().unwrap().is_none());
    }

    /// The file list comes back canonicalized, so a cut made under a
    /// symlinked directory must still match it.
    #[cfg(unix)]
    #[test]
    fn the_cut_marker_sees_through_symlinked_directories() {
        let tmp = tempfile::TempDir::new().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("a.txt"), b"a").unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let marker = CutMarker {
            paths: normalize(&[link.join("a.txt")]),
            change_count: None,
        };
        let resolved = dunce::canonicalize(real.join("a.txt")).unwrap();
        assert!(marker.matches(&[resolved], None));
        assert!(marker.matches(&[link.join("a.txt")], None));
        assert!(!marker.matches(&[link.join("b.txt")], None));
    }

    #[test]
    fn clearing_the_cut_makes_every_paste_a_copy() {
        let paths = vec![PathBuf::from("/tmp/one.txt")];
        *cut_state().lock().unwrap() = Some(CutMarker {
            paths: paths.clone(),
            change_count: None,
        });

        clear_cut();
        assert!(cut_state().lock().unwrap().is_none());
        assert!(!is_cut(&paths), "a cleared marker must never move");
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
