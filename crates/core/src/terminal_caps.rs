//! Terminal capabilities detection for Linux TTY/framebuffer support.
//!
//! This module detects the color depth and other capabilities of the
//! current terminal to adapt theme rendering appropriately.

use std::sync::OnceLock;
use termide_config::IconMode;

/// Global terminal capabilities (detected once at startup).
static TERMINAL_CAPS: OnceLock<TerminalCaps> = OnceLock::new();

/// Resolved "use emoji icons" flag (set after config is loaded).
static USE_EMOJI: OnceLock<bool> = OnceLock::new();

/// Initialize global terminal capabilities.
///
/// Call this once at application startup. Subsequent calls are ignored.
pub fn init_terminal_caps() -> &'static TerminalCaps {
    TERMINAL_CAPS.get_or_init(TerminalCaps::detect)
}

/// Get the global terminal capabilities.
///
/// Returns None if `init_terminal_caps()` hasn't been called yet.
pub fn get_terminal_caps() -> Option<&'static TerminalCaps> {
    TERMINAL_CAPS.get()
}

/// Color depth supported by the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// 8 colors (TTY without framebuffer)
    Basic,
    /// 16 colors (Linux console with framebuffer, old terminals)
    Extended,
    /// 256 colors (xterm-256color and similar)
    Palette256,
    /// 24-bit RGB (modern terminals with true color support)
    TrueColor,
}

/// Terminal capabilities detected at startup.
#[derive(Debug, Clone)]
pub struct TerminalCaps {
    /// Maximum color depth supported
    pub color_depth: ColorDepth,
    /// Whether running in Linux console (TTY)
    pub is_linux_console: bool,
    /// TERM environment variable value
    pub term: String,
}

impl Default for TerminalCaps {
    fn default() -> Self {
        Self::detect()
    }
}

impl TerminalCaps {
    /// Detect terminal capabilities from environment.
    ///
    /// Detection methods:
    /// 1. TERM environment variable (linux, linux-16color)
    /// 2. /proc/self/fd/0 symlink target (/dev/tty* vs /dev/pts/*)
    /// 3. COLORTERM environment variable (truecolor, 24bit)
    pub fn detect() -> Self {
        let term = std::env::var("TERM").unwrap_or_default();
        let colorterm = std::env::var("COLORTERM").ok();

        // Check if running in Linux console via TERM
        let is_linux_term = term == "linux" || term == "linux-16color";

        // Check if running in Linux console via /dev/tty*
        let is_linux_tty = is_linux_tty();

        let is_linux_console = is_linux_term || is_linux_tty;

        // Determine color depth
        let color_depth = if is_linux_console {
            // Linux console: 16 colors max (or 8 without framebuffer)
            if term == "linux-16color" || has_framebuffer() {
                ColorDepth::Extended
            } else {
                ColorDepth::Basic
            }
        } else if colorterm.as_deref() == Some("truecolor") || colorterm.as_deref() == Some("24bit")
        {
            ColorDepth::TrueColor
        } else if term.contains("256color") || term.contains("256") {
            ColorDepth::Palette256
        } else if term.contains("color") || term == "xterm" || term.starts_with("xterm-") {
            // Most modern xterm-compatible terminals support 256 colors
            ColorDepth::Palette256
        } else {
            // Default to 256 colors for unknown terminals
            ColorDepth::Palette256
        };

        Self {
            color_depth,
            is_linux_console,
            term,
        }
    }

    /// Get the appropriate TERM value for child processes.
    ///
    /// Returns "linux" for Linux console, "xterm-256color" otherwise.
    pub fn term_for_child(&self) -> &'static str {
        if self.is_linux_console {
            if self.color_depth == ColorDepth::Extended {
                "linux-16color"
            } else {
                "linux"
            }
        } else {
            "xterm-256color"
        }
    }

    /// Check if theme colors should be adapted to limited palette.
    pub fn needs_color_adaptation(&self) -> bool {
        matches!(self.color_depth, ColorDepth::Basic | ColorDepth::Extended)
    }
}

/// Check if stdin is connected to a Linux TTY (/dev/tty*) rather than a PTY.
///
/// Both probes below are Linux-console concepts backed by `/proc` and
/// `/dev/fb0`. Off Linux they can only ever answer "no", so they are compiled
/// out rather than paying a failing syscall on every capability detection.
#[cfg(target_os = "linux")]
fn is_linux_tty() -> bool {
    std::fs::read_link("/proc/self/fd/0")
        .map(|p| {
            let path = p.to_string_lossy();
            // /dev/tty1, /dev/tty2, etc. are Linux console
            // /dev/pts/0, /dev/pts/1, etc. are pseudo-terminals
            path.starts_with("/dev/tty") && !path.starts_with("/dev/tty/")
        })
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
fn is_linux_tty() -> bool {
    false
}

/// Check if framebuffer is available (indicates 16-color support in Linux console).
#[cfg(target_os = "linux")]
fn has_framebuffer() -> bool {
    // Check if any framebuffer device exists
    std::path::Path::new("/dev/fb0").exists()
}

#[cfg(not(target_os = "linux"))]
fn has_framebuffer() -> bool {
    false
}

/// Check if the locale suggests UTF-8 support.
fn has_utf8_locale() -> bool {
    for var in &["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(val) = std::env::var(var) {
            let upper = val.to_uppercase();
            if upper.contains("UTF-8") || upper.contains("UTF8") {
                return true;
            }
        }
    }
    false
}

/// Check if running inside a terminal multiplexer (tmux/screen).
fn is_inside_multiplexer() -> bool {
    std::env::var("TMUX").is_ok() || std::env::var("STY").is_ok()
}

/// Heuristic: can the terminal likely render emoji?
fn emoji_likely() -> bool {
    let caps = get_terminal_caps();
    let is_linux_console = caps.is_some_and(|c| c.is_linux_console);
    !is_linux_console && has_utf8_locale() && !is_inside_multiplexer()
}

/// Initialize icon mode after config is loaded.
///
/// Resolves the final "use emoji" decision based on config + terminal heuristics.
pub fn init_icon_mode(mode: IconMode) {
    let use_emoji = match mode {
        IconMode::Emoji => true,
        IconMode::Unicode => false,
        IconMode::Auto => emoji_likely(),
    };
    let _ = USE_EMOJI.set(use_emoji);
}

/// Check if emoji icons should be used in panel titles.
pub fn use_emoji_icons() -> bool {
    USE_EMOJI.get().copied().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_does_not_panic() {
        // Just ensure detection doesn't panic and returns valid data
        let caps = TerminalCaps::detect();
        // Verify we got a valid struct (term may be empty in some CI environments)
        let _ = caps.term_for_child();
    }

    #[test]
    fn test_term_for_child() {
        let caps = TerminalCaps {
            color_depth: ColorDepth::Extended,
            is_linux_console: true,
            term: "linux".to_string(),
        };
        assert_eq!(caps.term_for_child(), "linux-16color");

        let caps = TerminalCaps {
            color_depth: ColorDepth::TrueColor,
            is_linux_console: false,
            term: "xterm-256color".to_string(),
        };
        assert_eq!(caps.term_for_child(), "xterm-256color");
    }

    #[test]
    fn test_needs_color_adaptation() {
        assert!(TerminalCaps {
            color_depth: ColorDepth::Basic,
            is_linux_console: true,
            term: "linux".to_string(),
        }
        .needs_color_adaptation());

        assert!(TerminalCaps {
            color_depth: ColorDepth::Extended,
            is_linux_console: true,
            term: "linux-16color".to_string(),
        }
        .needs_color_adaptation());

        assert!(!TerminalCaps {
            color_depth: ColorDepth::Palette256,
            is_linux_console: false,
            term: "xterm-256color".to_string(),
        }
        .needs_color_adaptation());

        assert!(!TerminalCaps {
            color_depth: ColorDepth::TrueColor,
            is_linux_console: false,
            term: "xterm-256color".to_string(),
        }
        .needs_color_adaptation());
    }
}
