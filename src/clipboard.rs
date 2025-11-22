use std::io::Write;
use std::sync::{Mutex, OnceLock};

/// Clipboard operation mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardMode {
    Copy,
    Cut,
}

/// Backend for working with system clipboard
#[derive(Debug, Clone, Copy)]
enum ClipboardBackend {
    WlCopy,    // Wayland
    Xclip,     // X11
    Xsel,      // X11 alternative
    Osc52Only, // OSC 52 only + internal buffer
}

/// Global clipboard
static CLIPBOARD: OnceLock<Mutex<Clipboard>> = OnceLock::new();

/// Detected backend
static BACKEND: OnceLock<ClipboardBackend> = OnceLock::new();

/// Universal clipboard for text
#[derive(Debug, Clone)]
struct Clipboard {
    text: String,
    mode: ClipboardMode,
}

impl Clipboard {
    fn new() -> Self {
        Self {
            text: String::new(),
            mode: ClipboardMode::Copy,
        }
    }

    fn set(&mut self, text: String, mode: ClipboardMode) {
        self.text = text;
        self.mode = mode;
    }

    fn get(&self) -> (String, ClipboardMode) {
        (self.text.clone(), self.mode)
    }

    fn clear(&mut self) {
        self.text.clear();
        self.mode = ClipboardMode::Copy;
    }
}

/// Get reference to global clipboard
fn get_clipboard() -> &'static Mutex<Clipboard> {
    CLIPBOARD.get_or_init(|| Mutex::new(Clipboard::new()))
}

/// Check if command exists in the system
fn command_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Detect available backend for clipboard
fn detect_backend() -> ClipboardBackend {
    // Check Wayland
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        if command_exists("wl-copy") {
            return ClipboardBackend::WlCopy;
        }
    }

    // Check X11
    if std::env::var("DISPLAY").is_ok() {
        if command_exists("xclip") {
            return ClipboardBackend::Xclip;
        }
        if command_exists("xsel") {
            return ClipboardBackend::Xsel;
        }
    }

    // Fallback - OSC 52 only
    ClipboardBackend::Osc52Only
}

/// Get detected backend
fn get_backend() -> ClipboardBackend {
    *BACKEND.get_or_init(detect_backend)
}

/// Send text to system clipboard via OSC 52
fn send_osc52(text: &str) {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use crossterm::{execute, style::Print};

    let encoded = STANDARD.encode(text.as_bytes());
    let osc52 = format!("\x1b]52;c;{}\x07", encoded);

    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, Print(osc52));
}

/// Read text from system clipboard
fn read_from_system() -> Option<String> {
    match get_backend() {
        ClipboardBackend::WlCopy => {
            // wl-paste for Wayland
            if let Ok(output) = std::process::Command::new("wl-paste")
                .args(["--no-newline"])
                .output()
            {
                if output.status.success() {
                    return String::from_utf8(output.stdout).ok();
                }
            }
        }
        ClipboardBackend::Xclip => {
            if let Ok(output) = std::process::Command::new("xclip")
                .args(["-selection", "clipboard", "-o"])
                .output()
            {
                if output.status.success() {
                    return String::from_utf8(output.stdout).ok();
                }
            }
        }
        ClipboardBackend::Xsel => {
            if let Ok(output) = std::process::Command::new("xsel")
                .args(["--clipboard", "--output"])
                .output()
            {
                if output.status.success() {
                    return String::from_utf8(output.stdout).ok();
                }
            }
        }
        ClipboardBackend::Osc52Only => {
            // OSC 52 doesn't support reading, use internal buffer
        }
    }
    None
}

/// Copy text to system clipboard via detected backend
fn copy_to_system(text: &str) {
    match get_backend() {
        ClipboardBackend::WlCopy => {
            if let Ok(mut child) = std::process::Command::new("wl-copy")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                if let Some(stdin) = child.stdin.as_mut() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
            }
        }
        ClipboardBackend::Xclip => {
            if let Ok(mut child) = std::process::Command::new("xclip")
                .args(["-selection", "clipboard"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                if let Some(stdin) = child.stdin.as_mut() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
            }
        }
        ClipboardBackend::Xsel => {
            if let Ok(mut child) = std::process::Command::new("xsel")
                .args(["--clipboard", "--input"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                if let Some(stdin) = child.stdin.as_mut() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
            }
        }
        ClipboardBackend::Osc52Only => {
            send_osc52(text);
        }
    }
}

/// Copy text to clipboard
pub fn copy(text: String) {
    if text.is_empty() {
        return;
    }

    // Save to internal buffer
    if let Ok(mut clipboard) = get_clipboard().lock() {
        clipboard.set(text.clone(), ClipboardMode::Copy);
    }

    // Copy to system clipboard
    copy_to_system(&text);
}

/// Cut text to clipboard (for FM - delete sources after paste)
pub fn cut(text: String) {
    if text.is_empty() {
        return;
    }

    // Save to internal buffer
    if let Ok(mut clipboard) = get_clipboard().lock() {
        clipboard.set(text.clone(), ClipboardMode::Cut);
    }

    // Copy to system clipboard
    copy_to_system(&text);
}

/// Get text from clipboard
/// First tries to read from system clipboard, then from internal buffer
/// Returns (text, mode)
pub fn paste() -> (String, ClipboardMode) {
    // Try to read from system clipboard
    if let Some(system_text) = read_from_system() {
        if !system_text.is_empty() {
            // Check if it matches internal buffer to determine mode
            if let Ok(clipboard) = get_clipboard().lock() {
                let (internal_text, mode) = clipboard.get();
                if internal_text == system_text {
                    return (system_text, mode);
                }
            }
            // System buffer differs - this is external copy
            return (system_text, ClipboardMode::Copy);
        }
    }

    // Fallback to internal buffer
    if let Ok(clipboard) = get_clipboard().lock() {
        clipboard.get()
    } else {
        (String::new(), ClipboardMode::Copy)
    }
}

/// Clear clipboard
pub fn clear() {
    if let Ok(mut clipboard) = get_clipboard().lock() {
        clipboard.clear();
    }
}
