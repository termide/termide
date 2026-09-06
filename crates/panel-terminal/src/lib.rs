// Allow some clippy lints for VT100 implementation
#![allow(clippy::needless_range_loop)]

mod clipboard;
mod input_encoding;
mod link_detection;
#[cfg(target_os = "macos")]
mod macos_proc;
mod mouse;
mod render;
mod search;
mod selection;
pub mod shell_utils;
mod terminal;
mod terminal_info;

pub use terminal_info::TerminalInfo;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use input_encoding::{arrow_modifier_param, modern_key_bytes};
use link_detection::{HighlightSegment, LinkType};
#[cfg(unix)]
use nix::sys::signal::{self, Signal};
#[cfg(unix)]
use nix::unistd::Pid;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use ratatui::{buffer::Buffer, layout::Rect, prelude::Widget, style::Style, text::Line};
use std::any::Any;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use terminal::{Cell, CellStyle, KeyboardProtocolMode, MouseTrackingMode, TerminalScreen};
use vte::Parser;

use termide_config::{Config, TerminalKeybindings};
use termide_core::{
    get_terminal_caps, CommandResult, HotkeyTable, Panel, PanelCommand, PanelEvent, RenderContext,
    SessionPanel, WidthPreference,
};
use termide_modal::FindBar;
use termide_theme::Theme;
use termide_ui::{ColorPreview, ScrollBar};

/// State for terminal text search across scrollback and visible buffer.
struct TerminalSearchState {
    /// Matches: (absolute_row, column_start, match_length)
    matches: Vec<(usize, usize, usize)>,
    current_match: Option<usize>,
}

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Full-featured terminal with PTY
pub struct Terminal {
    /// Scrollbar drawn by the last render, for mouse thumb dragging.
    scrollbars: termide_core::ScrollBars,
    /// PTY master (wrapped in Arc<Mutex<>> for shared access)
    pty: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    /// Writer for writing to PTY
    writer: SharedWriter,
    /// Shell process
    child: Box<dyn Child + Send>,
    /// Shell process PID
    shell_pid: Option<u32>,
    /// Virtual terminal screen (RwLock allows concurrent reads during render)
    screen: Arc<RwLock<TerminalScreen>>,
    /// Terminal size
    size: PtySize,
    /// Process activity flag
    is_alive: Arc<Mutex<bool>>,
    /// Where the panel title text comes from.
    title: TitleSource,
    /// Initial working directory (set when terminal was created)
    initial_cwd: std::path::PathBuf,
    /// Cached theme for rendering
    cached_theme: Theme,
    /// Cached keybindings for keyboard handling
    keybindings: TerminalKeybindings,
    /// Flag set by PTY thread when new data arrives (triggers redraw)
    has_new_data: Arc<AtomicBool>,
    /// Cached rendered lines to avoid re-rendering when nothing changed
    /// Wrapped in Arc for O(1) clone on cache hit
    cached_lines: Option<Arc<Vec<Line<'static>>>>,
    /// Cached cursor position
    cached_cursor: (usize, usize),
    /// Cached cursor visibility state
    cached_cursor_shown: bool,
    /// Last focus state (for cache invalidation)
    cached_focus: bool,
    /// Cached active buffer state (main vs alt screen) for cache invalidation
    cached_use_alt_screen: bool,
    /// Currently hovered link (type, segments for multi-line highlighting)
    hovered_link: Option<(LinkType, Vec<HighlightSegment>)>,
    /// Whether Ctrl key is pressed (tracked for link highlighting)
    ctrl_pressed: bool,
    /// Search state for text search (matches across scrollback + visible grid)
    search_state: Option<TerminalSearchState>,
    /// Inline find bar docked at the top of the panel (Ctrl+F), mirroring the
    /// editor / file-manager UX. `None` when closed.
    find_bar: Option<FindBar>,
    /// When the bar is open, whether focus is on the terminal grid ("buffer"
    /// zone) rather than the bar. Toggled with Tab, like the editor.
    find_bar_focus_buffer: bool,
    /// Selection drag is active (left button held during selection).
    selection_drag_active: bool,
    /// Multi-click tracking (double = word, triple = line) keyed by the
    /// absolute (row, column) clicked.
    click_tracker: termide_ui::ClickTracker<(usize, usize)>,
    /// Last mouse position in screen coordinates for auto-scroll.
    last_mouse_position: Option<(u16, u16)>,
    /// Panel bounds for auto-scroll calculations.
    panel_bounds: Option<Rect>,
    /// Active color preview popup (shown while Ctrl+click is held on a hex color)
    color_preview: Option<ColorPreview>,
    /// Hotkey table for configurable keyboard shortcuts
    hotkeys: HotkeyTable,
    /// Pointer of the last Arc<Config> used to build hotkeys (skip rebuild when unchanged)
    last_config_ptr: usize,
    /// Cached foreground command name (avoids /proc reads on every render frame).
    /// Mutex for interior mutability from `&self` in `title()`.
    cached_fg_command: std::sync::Mutex<(String, std::time::Instant)>,
    /// Cached shell working directory, for the same reason as
    /// `cached_fg_command`: `title()` runs per frame and takes `&self`.
    cached_cwd: std::sync::Mutex<Option<(std::path::PathBuf, std::time::Instant)>>,
    /// Last working directory reported to the app, so a `cd` inside the shell
    /// is announced once instead of on every tick.
    last_reported_cwd: Option<std::path::PathBuf>,
}

/// Where a terminal panel's title text comes from.
#[derive(Debug, Clone)]
enum TitleSource {
    /// A shell: `user@host/<current directory>`, where the directory tracks
    /// the shell's own `cd`.
    Shell { user_host: String },
    /// A fixed command (ssh, a tool run in a panel): the command line itself,
    /// which has no directory to track.
    Command(String),
}

/// Build HotkeyTable for the terminal panel from config.
fn build_terminal_hotkey_table(config: &Config) -> HotkeyTable {
    let mut t = HotkeyTable::new();
    let kb = &config.terminal.keybindings;

    t.insert("search", &kb.search);
    t.insert("switch_directory", &kb.switch_directory);
    t.insert("scroll_up", &kb.scroll_up);
    t.insert("scroll_down", &kb.scroll_down);
    t.insert("scroll_top", &kb.scroll_top);
    t.insert("scroll_bottom", &kb.scroll_bottom);
    t
}

impl Terminal {
    /// Set common environment variables for a terminal command.
    fn set_env(cmd: &mut CommandBuilder, working_dir: &std::path::Path) {
        let term_value = get_terminal_caps()
            .map(|caps| caps.term_for_child())
            .unwrap_or("xterm-256color");
        cmd.env("TERM", term_value);
        cmd.env(
            "HOME",
            std::env::var("HOME").unwrap_or_else(|_| "/".to_string()),
        );
        cmd.env(
            "USER",
            std::env::var("USER").unwrap_or_else(|_| "user".to_string()),
        );
        cmd.env(
            "LANG",
            std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".to_string()),
        );
        if let Ok(lc_all) = std::env::var("LC_ALL") {
            cmd.env("LC_ALL", lc_all);
        }
        cmd.env("PWD", working_dir.display().to_string());
        cmd.env(
            "PATH",
            std::env::var("PATH")
                .unwrap_or_else(|_| "/run/current-system/sw/bin:/usr/bin:/bin".to_string()),
        );
    }

    /// Spawn a PTY reader thread that feeds output into the terminal screen.
    fn spawn_reader(
        mut reader: Box<dyn std::io::Read + Send>,
        writer: SharedWriter,
        screen: &Arc<RwLock<TerminalScreen>>,
        is_alive: &Arc<Mutex<bool>>,
        has_new_data: &Arc<AtomicBool>,
    ) {
        let screen_clone = Arc::clone(screen);
        let is_alive_clone = Arc::clone(is_alive);
        let has_new_data_clone = Arc::clone(has_new_data);
        thread::spawn(move || {
            let mut parser = Parser::new();
            let mut buf = [0u8; 16384];
            let mut performer = terminal::VtPerformer {
                writer,
                screen: Arc::clone(&screen_clone),
                pending_backslash: false,
                pending_ops: Vec::with_capacity(8192),
            };

            loop {
                match reader.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        for byte in &buf[..n] {
                            parser.advance(&mut performer, *byte);
                        }
                        performer.flush();
                        has_new_data_clone.store(true, Ordering::Release);
                    }
                    Ok(_) => break,
                    Err(_) => break,
                }
            }

            if let Ok(mut alive) = is_alive_clone.lock() {
                *alive = false;
            }
        });
    }

    /// Finalize terminal construction from spawned PTY components.
    #[allow(clippy::too_many_arguments)]
    fn build(
        pty: Arc<Mutex<Box<dyn MasterPty + Send>>>,
        writer: SharedWriter,
        child: Box<dyn portable_pty::Child + Send + Sync>,
        shell_pid: Option<u32>,
        screen: Arc<RwLock<TerminalScreen>>,
        size: PtySize,
        is_alive: Arc<Mutex<bool>>,
        has_new_data: Arc<AtomicBool>,
    ) -> Self {
        Self {
            scrollbars: termide_core::ScrollBars::default(),
            pty,
            writer,
            child,
            shell_pid,
            screen,
            size,
            is_alive,
            title: TitleSource::Command(String::new()),
            initial_cwd: std::path::PathBuf::new(),
            cached_theme: Theme::default(),
            keybindings: TerminalKeybindings::default(),
            has_new_data,
            cached_lines: None,
            cached_cursor: (0, 0),
            cached_cursor_shown: false,
            cached_focus: false,
            cached_use_alt_screen: false,
            hovered_link: None,
            ctrl_pressed: false,
            search_state: None,
            find_bar: None,
            find_bar_focus_buffer: false,
            selection_drag_active: false,
            click_tracker: termide_ui::ClickTracker::new(),
            last_mouse_position: None,
            panel_bounds: None,
            color_preview: None,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            cached_fg_command: std::sync::Mutex::new((
                "shell".to_string(),
                std::time::Instant::now(),
            )),
            cached_cwd: std::sync::Mutex::new(None),
            last_reported_cwd: None,
        }
    }

    /// Create new terminal with specified working directory (auto-detects shell)
    pub fn new_with_cwd(rows: u16, cols: u16, cwd: Option<std::path::PathBuf>) -> Result<Self> {
        let shell = shell_utils::detect_shell();
        Self::new_with_shell(rows, cols, &shell, cwd)
    }

    /// Create new terminal with a specific shell and optional working directory.
    pub fn new_with_shell(
        rows: u16,
        cols: u16,
        shell_path: &str,
        cwd: Option<std::path::PathBuf>,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = pty_system.openpty(size)?;

        let shell_args = shell_utils::get_shell_args(shell_path);

        // WSL entries use "wsl -d distro" format
        let mut cmd = if shell_path.starts_with("wsl ") {
            let parts: Vec<&str> = shell_path.split_whitespace().collect();
            if parts.is_empty() {
                anyhow::bail!("empty shell path");
            }
            let mut cmd = CommandBuilder::new(parts[0]);
            for arg in &parts[1..] {
                cmd.arg(arg);
            }
            cmd
        } else {
            let mut cmd = CommandBuilder::new(shell_path);
            for arg in shell_args {
                cmd.arg(arg);
            }
            cmd
        };

        let working_dir =
            cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| "/".into()));
        cmd.cwd(&working_dir);
        Self::set_env(&mut cmd, &working_dir);
        cmd.env("SHELL", shell_path);

        let child = pair.slave.spawn_command(cmd)?;
        let shell_pid = child.process_id();
        let screen = Arc::new(RwLock::new(TerminalScreen::new(
            rows as usize,
            cols as usize,
        )));
        let reader = pair.master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        let pty = Arc::new(Mutex::new(pair.master));
        let is_alive = Arc::new(Mutex::new(true));
        let has_new_data = Arc::new(AtomicBool::new(false));

        Self::spawn_reader(
            reader,
            Arc::clone(&writer),
            &screen,
            &is_alive,
            &has_new_data,
        );

        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "user".to_string());
        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("HOST"))
            .unwrap_or_else(|_| "localhost".to_string());
        let user_host = format!("{}@{}", username, hostname);

        let mut term = Self::build(
            pty,
            writer,
            child,
            shell_pid,
            screen,
            size,
            is_alive,
            has_new_data,
        );
        term.title = TitleSource::Shell { user_host };
        term.initial_cwd = working_dir;
        Ok(term)
    }

    /// Create new terminal that runs a specific command (e.g., ssh user@host)
    pub fn new_with_command(rows: u16, cols: u16, command: &str) -> Result<Self> {
        let pty_system = native_pty_system();
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = pty_system.openpty(size)?;

        let parts: Vec<&str> = command.split_whitespace().collect();
        if parts.is_empty() {
            anyhow::bail!("Empty command");
        }

        let mut cmd = CommandBuilder::new(parts[0]);
        for arg in &parts[1..] {
            cmd.arg(*arg);
        }

        let working_dir = std::env::current_dir().unwrap_or_else(|_| "/".into());
        cmd.cwd(&working_dir);
        Self::set_env(&mut cmd, &working_dir);

        let child = pair.slave.spawn_command(cmd)?;
        let shell_pid = child.process_id();
        let screen = Arc::new(RwLock::new(TerminalScreen::new(
            rows as usize,
            cols as usize,
        )));
        let reader = pair.master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        let pty = Arc::new(Mutex::new(pair.master));
        let is_alive = Arc::new(Mutex::new(true));
        let has_new_data = Arc::new(AtomicBool::new(false));

        Self::spawn_reader(
            reader,
            Arc::clone(&writer),
            &screen,
            &is_alive,
            &has_new_data,
        );

        let mut term = Self::build(
            pty,
            writer,
            child,
            shell_pid,
            screen,
            size,
            is_alive,
            has_new_data,
        );
        term.title = TitleSource::Command(command.to_string());
        term.initial_cwd = working_dir;
        Ok(term)
    }

    /// Resize terminal
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        if let Ok(pty) = self.pty.lock() {
            pty.resize(self.size)?;
        }

        // Update virtual screen size - in-place resize without cloning
        if let Ok(mut screen) = self.screen.write() {
            let new_rows = rows as usize;
            let new_cols = cols as usize;

            // If size changed, resize in-place
            if screen.rows != new_rows || screen.cols != new_cols {
                let empty_cell = Cell {
                    ch: ' ',
                    style: CellStyle::default(),
                };

                // Adjust row count
                while screen.lines.len() > new_rows {
                    screen.lines.pop_back();
                }
                while screen.lines.len() < new_rows {
                    screen.lines.push_back(vec![empty_cell; new_cols]);
                }
                while screen.lines_wrapped.len() > new_rows {
                    screen.lines_wrapped.pop_back();
                }
                while screen.lines_wrapped.len() < new_rows {
                    screen.lines_wrapped.push_back(false);
                }

                // Adjust column count for each existing row
                for row in screen.lines.iter_mut() {
                    row.resize(new_cols, empty_cell);
                }

                // Adjust row count for alternate buffer
                while screen.alt_lines.len() > new_rows {
                    screen.alt_lines.pop_back();
                }
                while screen.alt_lines.len() < new_rows {
                    screen.alt_lines.push_back(vec![empty_cell; new_cols]);
                }
                while screen.alt_lines_wrapped.len() > new_rows {
                    screen.alt_lines_wrapped.pop_back();
                }
                while screen.alt_lines_wrapped.len() < new_rows {
                    screen.alt_lines_wrapped.push_back(false);
                }

                // Adjust column count for each existing row in alternate buffer
                for row in screen.alt_lines.iter_mut() {
                    row.resize(new_cols, empty_cell);
                }

                screen.rows = new_rows;
                screen.cols = new_cols;

                // Reset scroll region to match new dimensions
                screen.reset_scroll_region();

                // Limit cursor position to new dimensions
                screen.cursor.0 = screen.cursor.0.min(new_rows.saturating_sub(1));
                screen.cursor.1 = screen.cursor.1.min(new_cols.saturating_sub(1));

                // Mark dirty to force re-render
                screen.dirty = true;
            }
        }

        // Invalidate render cache on resize
        self.cached_lines = None;

        Ok(())
    }

    /// Check if PTY process is alive
    pub fn is_alive(&self) -> bool {
        self.is_alive.lock().map(|alive| *alive).unwrap_or(false)
    }

    /// Get terminal info for status bar
    pub fn get_terminal_info(&self) -> TerminalInfo {
        // Get user@host
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "user".to_string());
        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("HOST"))
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| {
                #[cfg(unix)]
                {
                    // Try to get hostname via gethostname
                    let mut buf = [0u8; 256];
                    // SAFETY: gethostname is a POSIX function that writes a null-terminated
                    // hostname into the provided buffer. We provide a stack-allocated buffer
                    // of 256 bytes (sufficient for hostnames per POSIX HOST_NAME_MAX).
                    // On success (return 0), the buffer contains a valid C string.
                    // We use CStr::from_ptr which requires a null-terminated string - guaranteed
                    // by gethostname on success. The buffer outlives the CStr usage.
                    unsafe {
                        if libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) == 0
                        {
                            let cstr =
                                std::ffi::CStr::from_ptr(buf.as_ptr() as *const libc::c_char);
                            return cstr.to_string_lossy().into_owned();
                        }
                    }
                }
                "localhost".to_string()
            });
        let user_host = format!("{}@{}", username, hostname);

        // Get current directory (using environment variable)
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "~".to_string());

        TerminalInfo { user_host, cwd }
    }

    /// Acquire a read lock on the terminal screen, recovering from poisoning.
    fn read_screen(&self) -> std::sync::RwLockReadGuard<'_, TerminalScreen> {
        self.screen.read().unwrap_or_else(|e| {
            log::warn!("Terminal screen RwLock poisoned (read), recovering");
            e.into_inner()
        })
    }

    /// Acquire a write lock on the terminal screen, recovering from poisoning.
    fn write_screen(&self) -> std::sync::RwLockWriteGuard<'_, TerminalScreen> {
        self.screen.write().unwrap_or_else(|e| {
            log::warn!("Terminal screen RwLock poisoned (write), recovering");
            e.into_inner()
        })
    }

    /// Send input to PTY
    fn send_input(&mut self, data: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        writer.write_all(data)?;
        writer.flush()?;
        Ok(())
    }

    /// Send a command to the terminal and execute it (adds Enter)
    pub fn send_command(&mut self, command: &str) -> Result<()> {
        self.send_input(command.as_bytes())?;
        self.send_input(b"\r")?;
        Ok(())
    }

    fn send_focus_event(&mut self, focused: bool) -> Result<()> {
        let focus_reporting = self.read_screen().focus_reporting;
        if !focus_reporting {
            return Ok(());
        }
        if focused {
            self.send_input(b"\x1b[I")
        } else {
            self.send_input(b"\x1b[O")
        }
    }

    /// Copy selected text to clipboard
    fn copy_selection_to_clipboard(&self) -> Result<()> {
        clipboard::copy_selection_to_clipboard(&self.screen)
    }

    /// Paste text from clipboard to PTY.
    pub fn paste_from_clipboard(&mut self) -> Result<()> {
        let Some(text) = clipboard::get_clipboard_text() else {
            return Ok(());
        };

        if text.is_empty() {
            return Ok(());
        }

        self.paste_text(&text)
    }

    /// Paste text directly to PTY (from bracketed paste event or clipboard).
    ///
    /// Uses bracketed paste mode to wrap the text, which tells the shell/application
    /// that this is pasted content and newlines should not trigger command execution.
    pub fn paste_text(&mut self, text: &str) -> Result<()> {
        // Always use bracketed paste - the outer terminal (where termide runs)
        // already stripped the brackets, so we need to re-add them for the
        // inner shell/application running in our PTY
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        clipboard::paste_atomic(&mut *writer, text, true)
    }

    /// Check if PTY has new data that needs rendering
    pub fn has_pending_output(&self) -> bool {
        self.has_new_data.swap(false, Ordering::AcqRel)
    }

    /// Get the name of the currently running foreground command (cached, 500ms TTL).
    fn get_foreground_command(&self) -> String {
        const FG_COMMAND_TTL: std::time::Duration = std::time::Duration::from_millis(500);

        // Recover from a poisoned lock instead of cascading the panic to every
        // subsequent render (this runs on the render/tick path).
        let mut cache = self
            .cached_fg_command
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if cache.1.elapsed() < FG_COMMAND_TTL {
            return cache.0.clone();
        }

        let result = self.read_foreground_command_raw();
        *cache = (result.clone(), std::time::Instant::now());
        result
    }

    /// Name of the directory the shell is currently in, for the panel title.
    ///
    /// Read from the live shell process, so a `cd` inside the shell shows up
    /// in the title — the panel's starting directory only serves as a
    /// fallback. Cached on the same TTL as the foreground command because
    /// both run on the render path.
    fn cwd_label(&self) -> String {
        let cwd = self.shell_cwd();
        cwd.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            // The filesystem root has no last component.
            .unwrap_or_else(|| cwd.to_string_lossy().into_owned())
    }

    /// The shell's current working directory.
    ///
    /// Read from the live process so it follows a `cd`, cached on a short TTL
    /// because both the title (every frame) and the app's panel-path collection
    /// (every tick) ask for it.
    fn shell_cwd(&self) -> std::path::PathBuf {
        const CWD_TTL: std::time::Duration = std::time::Duration::from_millis(500);

        // Recover from a poisoned lock instead of cascading the panic to every
        // subsequent render.
        let mut cache = self.cached_cwd.lock().unwrap_or_else(|e| e.into_inner());
        let fresh = cache.as_ref().is_some_and(|(_, at)| at.elapsed() < CWD_TTL);
        if !fresh {
            *cache = Some((self.read_shell_cwd_raw(), std::time::Instant::now()));
        }
        cache
            .as_ref()
            .map(|(path, _)| path.clone())
            .unwrap_or_else(|| self.initial_cwd.clone())
    }

    /// Read the shell's working directory from the live process, falling back
    /// to the directory the panel was created in.
    fn read_shell_cwd_raw(&self) -> std::path::PathBuf {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if let Some(pid) = self.shell_pid {
            // The shell's own cwd, not the foreground child's: that is what the
            // shell prompt shows and what a `cd` changes.
            #[cfg(target_os = "linux")]
            if let Ok(path) = std::fs::read_link(format!("/proc/{}/cwd", pid)) {
                return path;
            }

            // macOS has no /proc; the same vnode comes from libproc.
            #[cfg(target_os = "macos")]
            if let Some(path) = macos_proc::shell_cwd(pid) {
                return path;
            }
        }
        self.initial_cwd.clone()
    }

    /// Read foreground command from /proc (Linux), libproc (macOS) or a
    /// process snapshot (Windows).
    fn read_foreground_command_raw(&self) -> String {
        if let Some(pid) = self.shell_pid {
            #[cfg(target_os = "macos")]
            if let Some(name) = macos_proc::foreground_command(pid) {
                return name;
            }

            #[cfg(target_os = "linux")]
            {
                // Read children of shell
                let children_path = format!("/proc/{}/task/{}/children", pid, pid);
                if let Ok(children) = std::fs::read_to_string(&children_path) {
                    if let Some(child_pid) = children.split_whitespace().next() {
                        let comm_path = format!("/proc/{}/comm", child_pid);
                        if let Ok(comm) = std::fs::read_to_string(&comm_path) {
                            return comm.trim().to_string();
                        }
                    }
                }
                // No children - return shell name
                let comm_path = format!("/proc/{}/comm", pid);
                if let Ok(comm) = std::fs::read_to_string(&comm_path) {
                    return comm.trim().to_string();
                }
            }

            #[cfg(windows)]
            {
                use windows_sys::Win32::Foundation::CloseHandle;
                use windows_sys::Win32::System::Diagnostics::ToolHelp::*;

                // SAFETY: CreateToolhelp32Snapshot with TH32CS_SNAPPROCESS takes a
                // snapshot of all processes. The returned handle must be closed.
                let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
                if !snapshot.is_null() && snapshot != -1isize as *mut _ {
                    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
                    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

                    // Find child process of our shell
                    let mut found_child = None;
                    let mut shell_name = None;

                    unsafe {
                        if Process32FirstW(snapshot, &mut entry) != 0 {
                            loop {
                                if entry.th32ParentProcessID == pid {
                                    // Found a child of our shell
                                    let name_len = entry
                                        .szExeFile
                                        .iter()
                                        .position(|&c| c == 0)
                                        .unwrap_or(entry.szExeFile.len());
                                    found_child = Some(String::from_utf16_lossy(
                                        &entry.szExeFile[..name_len],
                                    ));
                                }
                                if entry.th32ProcessID == pid {
                                    let name_len = entry
                                        .szExeFile
                                        .iter()
                                        .position(|&c| c == 0)
                                        .unwrap_or(entry.szExeFile.len());
                                    shell_name = Some(String::from_utf16_lossy(
                                        &entry.szExeFile[..name_len],
                                    ));
                                }
                                if Process32NextW(snapshot, &mut entry) == 0 {
                                    break;
                                }
                            }
                        }
                        CloseHandle(snapshot);
                    }

                    if let Some(name) = found_child {
                        return name;
                    }
                    if let Some(name) = shell_name {
                        return name;
                    }
                }
            }
        }
        "shell".to_string()
    }

    /// Scroll the terminal view to center the given absolute row.
    fn scroll_to_abs_row(&mut self, abs_row: usize) {
        let screen = self.read_screen();
        let scrollback_len = screen.scrollback.len();
        let visible_rows = screen.rows;
        let total_lines = scrollback_len + visible_rows;
        drop(screen);

        // Calculate scroll_offset to center abs_row in viewport
        // scroll_offset = distance from bottom of total content to bottom of viewport
        let target_bottom = abs_row + visible_rows / 2 + 1;
        if target_bottom >= total_lines {
            // Row is in or near the active buffer — no scroll needed
            self.write_screen().scroll_offset = 0;
        } else {
            let offset = total_lines - target_bottom;
            let mut screen = self.write_screen();
            screen.scroll_offset = offset.min(scrollback_len);
        }
    }
}

impl Panel for Terminal {
    fn name(&self) -> &'static str {
        "terminal"
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferWide
    }

    fn title(&self) -> String {
        let foreground = self.get_foreground_command();
        match &self.title {
            TitleSource::Shell { user_host } => {
                format!("{}/{} ({})", user_host, self.cwd_label(), foreground)
            }
            TitleSource::Command(command) => format!("{} ({})", command, foreground),
        }
    }

    fn prepare_render(&mut self, theme: &Theme, config: &std::sync::Arc<Config>) {
        // Invalidate cache if theme changed
        if self.cached_theme != *theme {
            self.cached_lines = None;
        }
        self.cached_theme = *theme;
        if self.keybindings != config.terminal.keybindings {
            self.keybindings = config.terminal.keybindings.clone();
        }
        let config_ptr = std::sync::Arc::as_ptr(config) as usize;
        if self.last_config_ptr != config_ptr {
            self.last_config_ptr = config_ptr;
            self.hotkeys = build_terminal_hotkey_table(config);
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        // area is already the inner content area (accordion drew outer border)
        let theme = self.cached_theme;

        // Dock the inline find bar at the TOP (with a pseudographic separator),
        // shrinking the grid area so the PTY resize / scroll / mouse math see
        // the reduced height — consistent with the editor and file manager.
        let mut area = area;
        if let Some(mut bar) = self.find_bar.take() {
            let bar_h = bar.height().min(area.height);
            let bar_area = Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: bar_h,
            };
            let active = !self.find_bar_focus_buffer;
            bar.render(bar_area, buf, &theme, active);
            self.find_bar = Some(bar);

            let sep_y = area.y + bar_h;
            let mut used = bar_h;
            if sep_y < area.y + area.height {
                let style = Style::default().fg(theme.disabled);
                for dx in 0..area.width {
                    buf[(area.x + dx, sep_y)].set_symbol("─").set_style(style);
                }
                used += 1;
            }
            area = Rect {
                x: area.x,
                y: area.y + used,
                width: area.width,
                height: area.height.saturating_sub(used),
            };
        }

        // Update size if changed (PTY follows the shrunken grid area)
        let new_rows = area.height;
        let new_cols = area.width;

        if new_rows != self.size.rows || new_cols != self.size.cols {
            let _ = self.resize(new_rows, new_cols);
        }

        // Data is read in a separate thread, just render current state
        // Show cursor only when panel is focused
        // Theme colors are now applied during get_display_lines() - no post-processing needed
        let (arc_lines, _cursor_pos, _cursor_shown) =
            self.get_display_lines(ctx.is_focused, &theme);

        // Clear the render area with background color to prevent visual artifacts
        // from previous content (modal borders, old status lines, etc.)
        let bg_style = Style::default().bg(theme.bg);
        buf.set_style(area, bg_style);

        // Render each cached line by reference, one row each — no per-frame
        // clone of the whole `Vec<Line>` (the cache holds an Arc, so consuming
        // it via `Paragraph` always deep-copied). Equivalent to the old
        // top-aligned, no-wrap `Paragraph` over the same `bg`-filled area.
        for (i, line) in arc_lines.iter().enumerate() {
            if i as u16 >= area.height {
                break;
            }
            let row = Rect {
                x: area.x,
                y: area.y + i as u16,
                width: area.width,
                height: 1,
            };
            line.render(row, buf);
        }

        // Render color preview popup if active
        if let Some(ref preview) = self.color_preview {
            preview.render(buf, area);
        }

        // Render scrollbar for scrollback history
        let screen = self.read_screen();
        let scrollback_len = screen.scrollback.len();
        let scroll_offset = screen.scroll_offset;
        let use_alt_screen = screen.use_alt_screen;
        drop(screen);

        // Only show scrollbar when not in alt screen and there's scrollback
        if !use_alt_screen && scrollback_len > 0 {
            if let Some(border_x) = ctx.border_right_x {
                // Terminal scroll is inverted: scroll_offset=0 means at bottom (current),
                // scroll_offset=scrollback_len means at top (oldest history)
                // Convert to standard scrollbar coordinates (0=top, max=bottom)
                let visible_height = area.height as usize;
                let total_lines = scrollback_len + visible_height;
                let scrollbar_offset = scrollback_len.saturating_sub(scroll_offset);

                let theme_colors = termide_core::ThemeColors::from(&self.cached_theme);
                self.scrollbars.vertical = ScrollBar::render_tracked(
                    buf,
                    border_x,
                    area.y,
                    area.height,
                    scrollbar_offset,
                    visible_height,
                    total_lines,
                    &theme_colors,
                    ctx.is_focused,
                );
            }
        }
    }

    fn handle_key(&mut self, chord: termide_core::KeyChord) -> Vec<PanelEvent> {
        let key = chord.raw;
        // If process exited, don't handle input
        if !self.is_alive() {
            return vec![];
        }

        // Ctrl+F opens or refocuses the inline find bar (docked at the top,
        // like the editor / file manager).
        if self.hotkeys.matches("search", &key) {
            self.open_find_bar();
            return vec![PanelEvent::NeedsRedraw];
        }

        // While the bar is open, route keys by zone. Tab toggles between the
        // bar and the terminal grid ("buffer" zone); in the grid zone Esc
        // closes the bar and other keys fall through to normal terminal input.
        if self.find_bar.is_some() {
            let plain = key.modifiers.is_empty();
            if plain && key.code == KeyCode::Tab {
                self.find_bar_focus_buffer = !self.find_bar_focus_buffer;
                return vec![PanelEvent::NeedsRedraw];
            }
            if self.find_bar_focus_buffer {
                if plain && key.code == KeyCode::Esc {
                    self.close_find_bar();
                    return vec![PanelEvent::NeedsRedraw];
                }
            } else {
                return self.handle_find_bar_key(key);
            }
        }

        // Configurable actions via HotkeyTable (key already translated by app).
        // copy/paste are routed from the global keybindings via
        // `PanelCommand::Copy`/`Paste` (see handle_command), not matched here.
        if self.hotkeys.matches("switch_directory", &key) {
            return vec![PanelEvent::OpenDirectorySwitcher];
        }
        if self.hotkeys.matches("scroll_up", &key) {
            let mut screen = self.write_screen();
            let scroll_amount = screen.rows.saturating_sub(1);
            screen.scroll_view_up(scroll_amount);
            return vec![];
        }
        if self.hotkeys.matches("scroll_down", &key) {
            let mut screen = self.write_screen();
            let scroll_amount = screen.rows.saturating_sub(1);
            screen.scroll_view_down(scroll_amount);
            return vec![];
        }
        if self.hotkeys.matches("scroll_top", &key) {
            let mut screen = self.write_screen();
            screen.scroll_offset = screen.scrollback.len();
            return vec![];
        }
        if self.hotkeys.matches("scroll_bottom", &key) {
            self.write_screen().reset_scroll();
            return vec![];
        }

        // Reset scroll on input, cache application_cursor_keys - single lock
        // Note: selection is NOT cleared on keypress to allow copying from running apps
        let (application_cursor_keys, keyboard_protocol) = {
            let mut screen = self.write_screen();
            screen.reset_scroll();
            (screen.application_cursor_keys, screen.keyboard_protocol)
        };

        if keyboard_protocol != KeyboardProtocolMode::Legacy {
            if let Some(bytes) = modern_key_bytes(&key, keyboard_protocol) {
                let _ = self.send_input(&bytes);
                return vec![];
            }
        }

        // Handle special keys
        match key.code {
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    // Ctrl+C, Ctrl+D, etc.
                    if c.eq_ignore_ascii_case(&'c') {
                        // Ctrl+C: copy if there's a selection, otherwise send SIGINT
                        let has_selection = {
                            let screen = self.read_screen();
                            screen.selection_start.is_some() && screen.selection_end.is_some()
                        };
                        if has_selection {
                            let copy_result = self.copy_selection_to_clipboard();
                            // Clear selection after copying
                            self.write_screen().clear_selection();
                            if let Err(e) = copy_result {
                                return vec![PanelEvent::SetStatusMessage {
                                    message: format!("Clipboard error: {}", e),
                                    is_error: true,
                                }];
                            }
                        } else {
                            let _ = self.send_input(&[3]); // Ctrl+C (SIGINT)
                        }
                    } else if c.eq_ignore_ascii_case(&'d') {
                        let _ = self.send_input(&[4]); // Ctrl+D
                    } else if c.eq_ignore_ascii_case(&'z') {
                        let _ = self.send_input(&[26]); // Ctrl+Z
                    } else {
                        // Other Ctrl combinations
                        let ctrl_char = (c as u8) & 0x1f;
                        let _ = self.send_input(&[ctrl_char]);
                    }
                } else {
                    // Regular character
                    let mut buf = [0u8; 4];
                    let s = c.encode_utf8(&mut buf);
                    let _ = self.send_input(s.as_bytes());
                }
            }
            KeyCode::Enter => {
                if keyboard_protocol == KeyboardProtocolMode::Legacy
                    && (key.modifiers.contains(KeyModifiers::SHIFT)
                        || key.modifiers.contains(KeyModifiers::ALT))
                {
                    // Shift+Enter or Alt+Enter sends newline for multi-line input.
                    // Alt+Enter works on VTE terminals (gnome-terminal, etc.) where
                    // Shift+Enter is indistinguishable from Enter without kitty protocol.
                    let _ = self.send_input(b"\n");
                } else {
                    let _ = self.send_input(b"\r");
                }
            }
            KeyCode::Backspace => {
                let _ = self.send_input(&[127]); // DEL
            }
            KeyCode::Delete => {
                let _ = self.send_input(b"\x1b[3~");
            }
            KeyCode::Left => {
                if let Some(m) = arrow_modifier_param(key.modifiers) {
                    let _ = self.send_input(format!("\x1b[1;{m}D").as_bytes());
                } else if application_cursor_keys {
                    // In Application Cursor Keys Mode send \x1bO instead of \x1b[
                    let _ = self.send_input(b"\x1bOD");
                } else {
                    let _ = self.send_input(b"\x1b[D");
                }
            }
            KeyCode::Right => {
                if let Some(m) = arrow_modifier_param(key.modifiers) {
                    let _ = self.send_input(format!("\x1b[1;{m}C").as_bytes());
                } else if application_cursor_keys {
                    let _ = self.send_input(b"\x1bOC");
                } else {
                    let _ = self.send_input(b"\x1b[C");
                }
            }
            KeyCode::Up => {
                if let Some(m) = arrow_modifier_param(key.modifiers) {
                    let _ = self.send_input(format!("\x1b[1;{m}A").as_bytes());
                } else if application_cursor_keys {
                    let _ = self.send_input(b"\x1bOA");
                } else {
                    let _ = self.send_input(b"\x1b[A");
                }
            }
            KeyCode::Down => {
                if let Some(m) = arrow_modifier_param(key.modifiers) {
                    let _ = self.send_input(format!("\x1b[1;{m}B").as_bytes());
                } else if application_cursor_keys {
                    let _ = self.send_input(b"\x1bOB");
                } else {
                    let _ = self.send_input(b"\x1b[B");
                }
            }
            KeyCode::Home => {
                if let Some(m) = arrow_modifier_param(key.modifiers) {
                    let _ = self.send_input(format!("\x1b[1;{m}H").as_bytes());
                } else if application_cursor_keys {
                    // In Application Cursor Keys Mode send \x1bO instead of \x1b[
                    let _ = self.send_input(b"\x1bOH");
                } else {
                    let _ = self.send_input(b"\x1b[H");
                }
            }
            KeyCode::End => {
                if let Some(m) = arrow_modifier_param(key.modifiers) {
                    let _ = self.send_input(format!("\x1b[1;{m}F").as_bytes());
                } else if application_cursor_keys {
                    let _ = self.send_input(b"\x1bOF");
                } else {
                    let _ = self.send_input(b"\x1b[F");
                }
            }
            KeyCode::PageUp => {
                let _ = self.send_input(b"\x1b[5~");
            }
            KeyCode::PageDown => {
                let _ = self.send_input(b"\x1b[6~");
            }
            KeyCode::Tab => {
                let _ = self.send_input(b"\t");
            }
            KeyCode::BackTab => {
                // Shift+Tab sends CSI Z sequence
                let _ = self.send_input(b"\x1b[Z");
            }
            KeyCode::Esc => {
                let _ = self.send_input(b"\x1b");
            }
            KeyCode::F(n) => {
                // F-keys escape sequences for xterm-256color
                const FKEY_SEQS: &[&[u8]] = &[
                    b"\x1bOP",   // F1
                    b"\x1bOQ",   // F2
                    b"\x1bOR",   // F3
                    b"\x1bOS",   // F4
                    b"\x1b[15~", // F5
                    b"\x1b[17~", // F6
                    b"\x1b[18~", // F7
                    b"\x1b[19~", // F8
                    b"\x1b[20~", // F9
                    b"\x1b[21~", // F10
                    b"\x1b[23~", // F11
                    b"\x1b[24~", // F12
                ];
                if let Some(seq) = FKEY_SEQS.get((n as usize).wrapping_sub(1)) {
                    let _ = self.send_input(seq);
                }
            }
            _ => {}
        }

        vec![]
    }

    fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        panel_area: Rect,
    ) -> Vec<PanelEvent> {
        self.on_mouse(mouse, panel_area)
    }

    fn handle_scroll(&mut self, delta: i32, panel_area: Rect) -> Vec<PanelEvent> {
        let tracking = self.read_screen().mouse_tracking;
        if tracking != MouseTrackingMode::None {
            let kind = if delta < 0 {
                crossterm::event::MouseEventKind::ScrollUp
            } else {
                crossterm::event::MouseEventKind::ScrollDown
            };
            let steps = delta.unsigned_abs();
            let (column, row) = self
                .last_mouse_position
                .unwrap_or((panel_area.x + 1, panel_area.y + 1));
            for _ in 0..steps {
                let mouse = crossterm::event::MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers: KeyModifiers::empty(),
                };
                let _ = self.send_mouse_to_pty(&mouse, panel_area);
            }
        } else {
            let lines = delta.unsigned_abs() as usize * 3; // 3 lines per scroll unit
            let mut screen = self.write_screen();
            if delta < 0 {
                screen.scroll_view_up(lines);
            } else {
                screen.scroll_view_down(lines);
            }
        }
        vec![]
    }

    fn tick(&mut self) -> Vec<PanelEvent> {
        let mut events = Vec::new();

        // A `cd` inside the shell changes what this panel's directory means to
        // the rest of the app (watched roots, the git panels' repository list).
        // Nothing else can notice it, so announce it here — once per change.
        let cwd = self.shell_cwd();
        if self.last_reported_cwd.as_deref() != Some(cwd.as_path()) {
            self.last_reported_cwd = Some(cwd);
            events.push(PanelEvent::WorkingDirectoryChanged);
        }

        // Handle auto-scroll during selection drag
        if !self.selection_drag_active {
            return events;
        }

        let Some((_mouse_col, mouse_row)) = self.last_mouse_position else {
            return events;
        };

        let Some(bounds) = self.panel_bounds else {
            return events;
        };

        // Calculate inner area (without border)
        let inner_y = bounds.y + 1;
        let inner_height = bounds.height.saturating_sub(2);

        let mut screen = self.write_screen();

        // Skip if no selection
        if screen.selection_start.is_none() {
            return events;
        }

        let max_scroll = screen.scrollback.len();

        // Auto-scroll up (mouse above panel)
        if mouse_row < inner_y && screen.scroll_offset < max_scroll {
            screen.scroll_view_up(1);
            // Extend selection to top visible line
            let abs_row = screen.visual_to_absolute(0);
            screen.selection_end = Some((abs_row, 0));
            events.push(PanelEvent::NeedsRedraw);
            return events;
        }

        // Auto-scroll down (mouse below panel)
        if mouse_row >= inner_y + inner_height && screen.scroll_offset > 0 {
            screen.scroll_view_down(1);
            // Extend selection to bottom visible line
            let last_row = inner_height.saturating_sub(1) as usize;
            let abs_row = screen.visual_to_absolute(last_row);
            let cols = screen.cols.saturating_sub(1);
            screen.selection_end = Some((abs_row, cols));
            events.push(PanelEvent::NeedsRedraw);
            return events;
        }

        events
    }

    fn should_auto_close(&self) -> bool {
        // Automatically close panel if process exited
        !self.is_alive()
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            PanelCommand::Resize { rows, cols } => {
                if self.resize(rows, cols).is_ok() {
                    CommandResult::NeedsRedraw(true)
                } else {
                    CommandResult::NeedsRedraw(false)
                }
            }
            PanelCommand::SetHostFocus { focused } => {
                if let Err(e) = self.send_focus_event(focused) {
                    log::debug!("Terminal focus event send failed: {}", e);
                }
                CommandResult::None
            }
            // Terminals always stay active (PTY must be drained), so MarkStale/RefreshIfStale are no-ops
            PanelCommand::MarkStale | PanelCommand::RefreshIfStale => CommandResult::None,
            // Smart Ctrl+C: copy the selection if there is one, otherwise let
            // the key fall through so the shell receives it as SIGINT.
            PanelCommand::Copy => {
                let has_selection = self
                    .screen
                    .read()
                    .map(|s| s.selection_start.is_some() && s.selection_end.is_some())
                    .unwrap_or(false);
                if has_selection {
                    if let Err(e) = self.copy_selection_to_clipboard() {
                        log::error!("Terminal copy failed: {}", e);
                    }
                    CommandResult::Handled(true)
                } else {
                    CommandResult::Handled(false)
                }
            }
            // Nothing to cut from terminal output — fall through to the shell.
            PanelCommand::Cut => CommandResult::Handled(false),
            PanelCommand::Paste => {
                if let Err(e) = self.paste_from_clipboard() {
                    log::error!("Terminal paste failed: {}", e);
                }
                CommandResult::Handled(true)
            }
            PanelCommand::PasteText { text } => {
                if let Err(e) = self.paste_text(&text) {
                    log::error!("Terminal paste_text failed: {}", e);
                }
                CommandResult::None
            }
            // Commands not applicable to Terminal
            PanelCommand::GetRepoRoot
            | PanelCommand::OnGitUpdate { .. }
            | PanelCommand::CheckPendingGitDiff
            | PanelCommand::CheckGitDiffReceiver
            | PanelCommand::CheckExternalModification
            | PanelCommand::GetFsWatchInfo
            | PanelCommand::SetFsWatchRoot { .. }
            | PanelCommand::OnFsUpdate { .. }
            | PanelCommand::Reload
            | PanelCommand::GetModificationStatus
            | PanelCommand::Save
            | PanelCommand::CloseWithoutSaving
            | PanelCommand::RefreshDirectory
            | PanelCommand::SetGitOperationInProgress { .. }
            | PanelCommand::UpdateRepoPaths { .. } => CommandResult::None,

            PanelCommand::GetScrollBars => CommandResult::ScrollBars(self.scrollbars),
            PanelCommand::SetScrollOffset { offset, .. } => {
                // The bar counts top-down while the terminal counts rows back
                // from the live edge, so invert: bar offset 0 is the oldest
                // scrollback line, the maximum is the live screen.
                let scrollback_len = self.read_screen().scrollback.len();
                self.write_screen()
                    .set_scroll_offset(scrollback_len.saturating_sub(offset));
                CommandResult::NeedsRedraw(true)
            }
        }
    }

    fn needs_close_confirmation(&self) -> Option<String> {
        // If process is alive and has child processes - request confirmation
        if self.is_alive() && self.has_running_processes() {
            let t = termide_i18n::t();
            Some(t.terminal_kill_confirm().to_string())
        } else {
            None
        }
    }

    fn captures_escape(&self) -> bool {
        // The open find bar consumes Escape (closes the bar). Otherwise, if
        // there are running processes, Escape is passed to them rather than
        // closing the panel.
        self.find_bar.is_some() || (self.is_alive() && self.has_running_processes())
    }

    fn to_session(&self, _session_dir: &std::path::Path) -> Option<SessionPanel> {
        // Save where the shell was last working, not where the panel was
        // created: reopening the session should put the user back in the
        // directory they left off in.
        Some(SessionPanel::Terminal {
            working_dir: self.shell_cwd(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn get_working_directory(&self) -> Option<std::path::PathBuf> {
        // The shell's live directory, not the one the panel started in: opening
        // a panel "here", the directory switcher and the git panels' repository
        // search all mean the directory the user is actually working in.
        Some(self.shell_cwd())
    }

    fn has_running_processes(&self) -> bool {
        if let Some(pid) = self.shell_pid {
            // On the key-input path via `captures_escape`, so every branch
            // below stays at a single kernel query — no scan over all pids.
            // Falls through to `false` rather than returning outright, so the
            // trailing platform blocks stay reachable code.
            #[cfg(target_os = "macos")]
            if macos_proc::has_children(pid) {
                return true;
            }

            #[cfg(target_os = "linux")]
            {
                let children_path = format!("/proc/{}/task/{}/children", pid, pid);
                if let Ok(children) = std::fs::read_to_string(&children_path) {
                    return !children.trim().is_empty();
                }
            }

            #[cfg(windows)]
            {
                use windows_sys::Win32::Foundation::CloseHandle;
                use windows_sys::Win32::System::Diagnostics::ToolHelp::*;

                let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
                if !snapshot.is_null() && snapshot != -1isize as *mut _ {
                    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
                    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

                    unsafe {
                        if Process32FirstW(snapshot, &mut entry) != 0 {
                            loop {
                                if entry.th32ParentProcessID == pid {
                                    CloseHandle(snapshot);
                                    return true;
                                }
                                if Process32NextW(snapshot, &mut entry) == 0 {
                                    break;
                                }
                            }
                        }
                        CloseHandle(snapshot);
                    }
                }
            }
        }
        false
    }

    fn kill_processes(&mut self) {
        if let Some(pid) = self.shell_pid {
            #[cfg(unix)]
            {
                let pid = Pid::from_raw(pid as i32);
                let _ = signal::killpg(pid, Signal::SIGTERM);
                std::thread::sleep(std::time::Duration::from_millis(100));
                if self.is_alive() {
                    let _ = signal::killpg(pid, Signal::SIGKILL);
                }
            }

            #[cfg(windows)]
            {
                // Use taskkill to terminate the process tree
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .output();
            }

            let _ = self.child.wait();
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Properly terminate processes when dropping terminal
        if self.is_alive() {
            self.kill_processes();
        }
    }
}

#[cfg(test)]
mod title_tests {
    use super::*;

    /// The title must name the directory the shell actually runs in — it used
    /// to read the *application's* cwd, so every terminal was labelled with the
    /// session root no matter where it was opened.
    #[test]
    fn title_shows_the_shell_start_directory() {
        let dir = std::env::temp_dir().join(format!("termide-title-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let spawned = Terminal::new_with_cwd(24, 80, Some(dir.clone()));
        let title = spawned.as_ref().ok().map(|t| t.title());
        let _ = std::fs::remove_dir_all(&dir);

        // No PTY available (sandboxed test runner) — nothing to assert.
        let Some(title) = title else {
            return;
        };
        let expected = format!("/{}", dir.file_name().unwrap().to_string_lossy());
        assert!(
            title.contains(&expected),
            "title {title:?} does not name the shell directory as {expected:?}"
        );
    }

    /// Reopening a session must land the shell where the user left off, so the
    /// saved directory is the shell's live one, not the panel's starting one.
    #[cfg(target_os = "linux")]
    #[test]
    fn session_saves_the_directory_the_shell_ended_in() {
        let dir = std::env::temp_dir().join(format!("termide-session-cd-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("subdir")).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let Ok(mut term) = Terminal::new_with_cwd(24, 80, Some(dir.clone())) else {
            let _ = std::fs::remove_dir_all(&dir);
            return; // No PTY available.
        };
        term.send_input(b"cd subdir\n").unwrap();

        let expected = dir.join("subdir");
        let mut saved = None;
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            saved = match term.to_session(std::path::Path::new("/tmp")) {
                Some(SessionPanel::Terminal { working_dir }) => Some(working_dir),
                other => panic!("terminal saved as {other:?}"),
            };
            if saved.as_ref() == Some(&expected) {
                break;
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            saved.as_ref(),
            Some(&expected),
            "session kept the directory the panel was opened in"
        );
    }

    /// A `cd` inside the shell must reach the panel's reported directory too:
    /// the directory switcher, "open a panel here" and the git panels' repo
    /// search all read it, and the app is told to re-sync through
    /// `PanelEvent::WorkingDirectoryChanged`.
    #[cfg(target_os = "linux")]
    #[test]
    fn working_directory_follows_a_cd_inside_the_shell() {
        let dir = std::env::temp_dir().join(format!("termide-cwd-cd-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("subdir")).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let Ok(mut term) = Terminal::new_with_cwd(24, 80, Some(dir.clone())) else {
            let _ = std::fs::remove_dir_all(&dir);
            return; // No PTY available.
        };
        assert_eq!(term.get_working_directory().as_ref(), Some(&dir));
        // The first tick reports the starting directory; drain it so the
        // announcement below can only come from the `cd`.
        term.tick();
        term.send_input(b"cd subdir\n").unwrap();

        let expected = dir.join("subdir");
        let mut announced = false;
        let mut reported = None;
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            announced |= term
                .tick()
                .iter()
                .any(|e| matches!(e, PanelEvent::WorkingDirectoryChanged));
            reported = term.get_working_directory();
            if reported.as_ref() == Some(&expected) {
                break;
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            reported.as_ref(),
            Some(&expected),
            "panel still reports the directory it was opened in"
        );
        assert!(
            announced,
            "the directory change was never announced to the app"
        );
    }

    /// A `cd` inside the shell must reach the title: it is read from the live
    /// process rather than captured once at spawn time.
    #[cfg(target_os = "linux")]
    #[test]
    fn title_follows_a_cd_inside_the_shell() {
        let dir = std::env::temp_dir().join(format!("termide-title-cd-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("subdir")).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let Ok(mut term) = Terminal::new_with_cwd(24, 80, Some(dir.clone())) else {
            let _ = std::fs::remove_dir_all(&dir);
            return; // No PTY available.
        };
        term.send_input(b"cd subdir\n").unwrap();

        // The shell needs a moment to run the builtin, and the title caches the
        // lookup for 500ms; poll instead of guessing a single sleep.
        let mut title = String::new();
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            title = term.title();
            if title.contains("subdir") {
                break;
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            title.contains("subdir"),
            "title {title:?} did not follow the shell into subdir"
        );
    }
}
