// Allow some clippy lints for VT100 implementation
#![allow(clippy::needless_range_loop)]

mod clipboard;
mod input_encoding;
mod link_detection;
mod render;
mod search;
mod selection;
pub mod shell_utils;
mod terminal;
mod terminal_info;

pub use terminal_info::TerminalInfo;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use input_encoding::{
    arrow_modifier_param, can_send_mouse_event, modern_key_bytes, mouse_modifier_bits, mouse_route,
    MouseRoute,
};
use link_detection::{HighlightSegment, LinkType};
#[cfg(unix)]
use nix::sys::signal::{self, Signal};
#[cfg(unix)]
use nix::unistd::Pid;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use ratatui::{buffer::Buffer, layout::Rect, prelude::Widget, style::Style, text::Line};
use selection::{line_selection, word_selection};
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
use termide_ui::{extract_hex_color_at_col, ColorPreview, ScrollBar};

/// State for terminal text search across scrollback and visible buffer.
struct TerminalSearchState {
    /// Matches: (absolute_row, column_start, match_length)
    matches: Vec<(usize, usize, usize)>,
    current_match: Option<usize>,
}

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Full-featured terminal with PTY
pub struct Terminal {
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
    /// Terminal title prefix (user@host//dir)
    title_prefix: String,
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
            pty,
            writer,
            child,
            shell_pid,
            screen,
            size,
            is_alive,
            title_prefix: String::new(),
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
        let current_dir = std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "~".to_string());
        let title_prefix = format!("{}@{}//{}", username, hostname, current_dir);

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
        term.title_prefix = title_prefix;
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
        term.title_prefix = command.to_string();
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

    /// Send mouse event to PTY (if mouse tracking is enabled)
    fn send_mouse_to_pty(
        &mut self,
        mouse: &crossterm::event::MouseEvent,
        panel_area: Rect,
    ) -> Result<()> {
        use crossterm::event::{MouseButton, MouseEventKind};
        use std::io::Write;

        let (mouse_tracking, sgr_mode) = {
            let screen = self.read_screen();
            (screen.mouse_tracking, screen.sgr_mouse_mode)
        };

        if !can_send_mouse_event(mouse.kind, mouse_tracking) {
            return Ok(());
        }

        // Split-mode panels can collapse to a 1- or 2-row strip; the
        // inner area below would have width/height = 0 and the clamp
        // bounds below would invert (`min > max`), which panics.
        if panel_area.width < 3 || panel_area.height < 3 {
            return Ok(());
        }

        let inner_x_min = panel_area.x + 1;
        let inner_x_max = panel_area.x + panel_area.width.saturating_sub(2);
        let inner_y_min = panel_area.y + 1;
        let inner_y_max = panel_area.y + panel_area.height.saturating_sub(2);

        let clamped_col = mouse.column.clamp(inner_x_min, inner_x_max);
        let clamped_row = mouse.row.clamp(inner_y_min, inner_y_max);

        // 1-based coordinates for xterm mouse reporting
        let inner_x = clamped_col.saturating_sub(inner_x_min) + 1;
        let inner_y = clamped_row.saturating_sub(inner_y_min) + 1;

        // Reusable buffer to avoid allocations (max SGR sequence is ~20 bytes)
        let mut buf = [0u8; 32];

        // Determine button code and whether this is release event
        let modifier_bits = mouse_modifier_bits(mouse.modifiers);
        let (btn_code, is_release): (u8, bool) = match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => (modifier_bits, false),
            MouseEventKind::Down(MouseButton::Middle) => (1 + modifier_bits, false),
            MouseEventKind::Down(MouseButton::Right) => (2 + modifier_bits, false),
            MouseEventKind::Up(MouseButton::Left)
            | MouseEventKind::Up(MouseButton::Middle)
            | MouseEventKind::Up(MouseButton::Right) => (3 + modifier_bits, true),
            MouseEventKind::Drag(MouseButton::Left) => (32 + modifier_bits, false),
            MouseEventKind::Drag(MouseButton::Middle) => (33 + modifier_bits, false),
            MouseEventKind::Drag(MouseButton::Right) => (34 + modifier_bits, false),
            MouseEventKind::Moved => (35 + modifier_bits, false),
            MouseEventKind::ScrollUp => (64 + modifier_bits, false),
            MouseEventKind::ScrollDown => (65 + modifier_bits, false),
            _ => return Ok(()),
        };

        // Build sequence directly into buffer (zero allocation)
        let len = if sgr_mode {
            // SGR format: ESC [ < btn ; x ; y (M for press, m for release)
            let suffix: u8 = if is_release { b'm' } else { b'M' };
            let mut cursor = std::io::Cursor::new(&mut buf[..]);
            write!(cursor, "\x1b[<{};{};{}", btn_code, inner_x, inner_y).ok();
            let pos = cursor.position() as usize;
            buf[pos] = suffix;
            pos + 1
        } else {
            // X10/Normal format: ESC [ M <btn+32> <x+32> <y+32>
            // Release in non-SGR mode always uses button code 3
            let effective_btn = if is_release { 3 } else { btn_code };
            buf[0] = b'\x1b';
            buf[1] = b'[';
            buf[2] = b'M';
            buf[3] = effective_btn.saturating_add(32);
            buf[4] = (inner_x as u8).saturating_add(32);
            buf[5] = (inner_y as u8).saturating_add(32);
            6
        };

        self.send_input(&buf[..len])?;
        Ok(())
    }

    /// Get lines for display with zero-copy rendering under lock.
    ///
    /// Optimization: Renders directly from screen buffer under lock,
    /// eliminating Vec<Vec<Cell>> cloning (~77KB per dirty frame).
    /// Uses dirty flag to skip re-rendering when content hasn't changed.
    ///
    /// Returns: (lines_arc, cursor_position, cursor_shown)
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

    /// Read foreground command from /proc (Unix) or process snapshot (Windows).
    fn read_foreground_command_raw(&self) -> String {
        if let Some(pid) = self.shell_pid {
            #[cfg(unix)]
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
        format!("{} ({})", self.title_prefix, self.get_foreground_command())
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
                ScrollBar::render(
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
        use crossterm::event::{MouseButton, MouseEventKind};

        // If process exited, don't handle mouse
        if !self.is_alive() {
            return vec![];
        }

        // Split-mode panels can collapse to a 1- or 2-row strip; the
        // inner area would have zero width/height and the clamp bounds
        // below would invert (`min > max`), which panics.
        if panel_area.width < 3 || panel_area.height < 3 {
            return vec![];
        }

        // When the find bar is docked at the top, the grid starts below it
        // (bar rows + separator). A click in that region is routed to the bar
        // (toggles / Prev / Next); grid rows below are offset so they map to
        // the correct PTY cell.
        let bar_offset = self.find_bar.as_ref().map(|b| b.height() + 1).unwrap_or(0);
        if bar_offset > 0 {
            let bar_top = panel_area.y + 1;
            let bar_bottom = bar_top + bar_offset; // exclusive (includes separator)
            if mouse.row >= bar_top && mouse.row < bar_bottom {
                if let Some(mut bar) = self.find_bar.take() {
                    let action = bar.handle_mouse(mouse);
                    self.find_bar = Some(bar);
                    return self.apply_find_bar_action(action);
                }
                return vec![];
            }
        }

        // Calculate inner area (without border); the grid starts below the bar.
        let inner_x_min = panel_area.x + 1;
        let inner_x_max = panel_area.x + panel_area.width.saturating_sub(2);
        let inner_y_min = panel_area.y + 1 + bar_offset;
        let inner_y_max = panel_area.y + panel_area.height.saturating_sub(2);
        // A bar on a very short panel can leave no grid rows; avoid an inverted
        // clamp range below.
        if inner_y_min > inner_y_max {
            return vec![];
        }

        // Calculate coordinates relative to terminal inner area (0-based for selection)
        // Clamped to panel boundaries
        let clamped_col = mouse.column.clamp(inner_x_min, inner_x_max);
        let clamped_row = mouse.row.clamp(inner_y_min, inner_y_max);
        let inner_col = clamped_col.saturating_sub(inner_x_min) as usize;
        let inner_row = clamped_row.saturating_sub(inner_y_min) as usize;

        // Check if click is inside terminal area
        let is_inside = mouse.column >= inner_x_min
            && mouse.column <= inner_x_max
            && mouse.row >= inner_y_min
            && mouse.row <= inner_y_max;

        // Save panel bounds and mouse position for auto-scroll in tick()
        self.panel_bounds = Some(panel_area);
        self.last_mouse_position = Some((mouse.column, mouse.row));

        // Track Ctrl key state for URL highlighting
        let ctrl_pressed = mouse.modifiers.contains(KeyModifiers::CONTROL);
        let alt_pressed = mouse.modifiers.contains(KeyModifiers::ALT);
        self.ctrl_pressed = ctrl_pressed;
        let mut needs_redraw = false;

        // Detect link (URL or path) under cursor when Ctrl is pressed
        if ctrl_pressed && is_inside {
            let screen = self.read_screen();
            let abs_row = screen.visual_to_absolute(inner_row);
            let cols = screen.cols;

            if let Some((link_type, link_start_row, link_start_col, display_len)) =
                link_detection::detect_link_at_position(
                    &screen,
                    abs_row,
                    inner_col,
                    &self.initial_cwd,
                )
            {
                // Link found - check if it's new
                let is_new_link = self
                    .hovered_link
                    .as_ref()
                    .map(|(l, _)| l != &link_type)
                    .unwrap_or(true);

                // Build segments for multi-line highlighting
                let segments = link_detection::build_link_segments(
                    display_len,
                    link_start_row,
                    link_start_col,
                    cols,
                );
                drop(screen);

                if is_new_link {
                    // Copy link text to clipboard
                    let _ = termide_ui::clipboard::copy(&link_detection::link_text(&link_type));
                }
                self.hovered_link = Some((link_type, segments));
                self.cached_lines = None; // Force redraw
                needs_redraw = true;
            } else {
                // No link under cursor
                drop(screen);
                if self.hovered_link.is_some() {
                    self.hovered_link = None;
                    self.cached_lines = None; // Force redraw
                    needs_redraw = true;
                }
            }
        } else if !ctrl_pressed && self.hovered_link.is_some() {
            // Ctrl not pressed - clear link highlight
            self.hovered_link = None;
            self.cached_lines = None; // Force redraw
            needs_redraw = true;
        }

        // `selection_active` gates whether an in-progress local selection drag
        // keeps capturing drag/up events even over a mouse-tracking app. It
        // must reflect a drag IN PROGRESS, not a completed selection that still
        // shows a highlight: otherwise, once a tracking app (e.g. an agent)
        // turns mouse reporting on, every click's button-up would be treated as
        // a selection drag and extend the stale selection to the click point —
        // which then can't be cleared.
        let selection_active = self.selection_drag_active;
        let mouse_tracking = self.read_screen().mouse_tracking;

        // If mouse is outside and selection is not active - ignore other events
        if !is_inside && !selection_active {
            return if needs_redraw {
                vec![PanelEvent::NeedsRedraw]
            } else {
                vec![]
            };
        }

        let route = mouse_route(
            mouse.kind,
            is_inside,
            selection_active,
            mouse_tracking,
            alt_pressed,
        );

        // Ctrl+Click local actions override PTY passthrough
        if ctrl_pressed
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && is_inside
        {
            let line_text = {
                let screen = self.read_screen();
                let abs_row = screen.visual_to_absolute(inner_row);
                screen
                    .get_line_by_absolute(abs_row)
                    .map(|cells| cells.iter().map(|c| c.ch).collect::<String>())
                    .unwrap_or_default()
            };
            if let Some((r, g, b, hex)) = extract_hex_color_at_col(&line_text, inner_col) {
                self.color_preview = Some(ColorPreview {
                    r,
                    g,
                    b,
                    hex,
                    screen_row: mouse.row,
                    screen_col: mouse.column,
                });
                return vec![PanelEvent::NeedsRedraw];
            }

            if let Some((ref link_type, _)) = self.hovered_link {
                match link_type {
                    LinkType::Url(url) => {
                        let _ = open::that(url);
                        return if needs_redraw {
                            vec![PanelEvent::NeedsRedraw]
                        } else {
                            vec![]
                        };
                    }
                    LinkType::FilePath(path) => {
                        let (dir, file) = if path.is_dir() {
                            (path.clone(), None)
                        } else {
                            (
                                path.parent()
                                    .map(|p| p.to_path_buf())
                                    .unwrap_or_else(|| path.clone()),
                                path.file_name().map(|n| n.to_os_string()),
                            )
                        };
                        return vec![PanelEvent::OpenPath {
                            path: dir,
                            select_file: file,
                        }];
                    }
                }
            }
        }

        match route {
            MouseRoute::LocalScrollback => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.write_screen().scroll_view_up(3);
                    needs_redraw = true;
                }
                MouseEventKind::ScrollDown => {
                    self.write_screen().scroll_view_down(3);
                    needs_redraw = true;
                }
                _ => {}
            },
            MouseRoute::LocalSelection => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    // Start selection only inside panel
                    if !is_inside {
                        return if needs_redraw {
                            vec![PanelEvent::NeedsRedraw]
                        } else {
                            vec![]
                        };
                    }

                    let abs_row = self.write_screen().visual_to_absolute(inner_row);
                    // 1 = single (start drag select), 2 = word, 3 = line.
                    let clicks = self.click_tracker.click((abs_row, inner_col));
                    let mut screen = self.write_screen();
                    let range = match clicks {
                        2 => word_selection(&screen, abs_row, inner_col),
                        3 => line_selection(&screen, abs_row),
                        _ => None,
                    };
                    if let Some((start, end)) = range {
                        screen.selection_start = Some(start);
                        screen.selection_end = Some(end);
                    } else {
                        screen.selection_start = Some((abs_row, inner_col));
                        screen.selection_end = Some((abs_row, inner_col));
                    }
                    drop(screen);

                    // Only single-click begins a drag selection; word/line
                    // clicks set a fixed selection.
                    self.selection_drag_active = clicks == 1;
                    needs_redraw = true;
                }
                MouseEventKind::Drag(MouseButton::Left) => {
                    let mut screen = self.write_screen();
                    if screen.selection_start.is_some() {
                        // Auto-scroll if mouse is above or below content area
                        let max_scroll = screen.scrollback.len();
                        if mouse.row < inner_y_min && screen.scroll_offset < max_scroll {
                            // Mouse above panel - scroll up into history
                            screen.scroll_view_up(1);
                        } else if mouse.row > inner_y_max && screen.scroll_offset > 0 {
                            // Mouse below panel - scroll down towards current
                            screen.scroll_view_down(1);
                        }

                        // Update selection end with absolute coordinates (using clamped row)
                        let abs_row = screen.visual_to_absolute(inner_row);
                        screen.selection_end = Some((abs_row, inner_col));
                        needs_redraw = true;
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.color_preview = None;

                    self.selection_drag_active = false;
                    self.last_mouse_position = None;

                    let is_single_click = {
                        let mut screen = self.write_screen();
                        let abs_row = screen.visual_to_absolute(inner_row);
                        if let Some(start) = screen.selection_start {
                            screen.selection_end = Some((abs_row, inner_col));
                            start == (abs_row, inner_col)
                        } else {
                            false
                        }
                    };

                    if is_single_click {
                        let mut screen = self.write_screen();
                        screen.clear_selection();
                    }
                    needs_redraw = true;
                }
                _ => {}
            },
            MouseRoute::Pty => {
                self.color_preview = None;
                self.selection_drag_active = false;
                // A click into a mouse-tracking app supersedes any lingering
                // local selection highlight — drop it so it clears instead of
                // staying stuck on screen.
                if matches!(mouse.kind, MouseEventKind::Down(_)) {
                    let mut screen = self.write_screen();
                    if screen.selection_start.is_some() {
                        screen.clear_selection();
                        needs_redraw = true;
                    }
                }
                let _ = self.send_mouse_to_pty(&mouse, panel_area);
            }
            MouseRoute::Ignore => {
                if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
                    self.color_preview = None;
                    needs_redraw = true;
                }
            }
        }

        if needs_redraw {
            vec![PanelEvent::NeedsRedraw]
        } else {
            vec![]
        }
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
        // Handle auto-scroll during selection drag
        if !self.selection_drag_active {
            return vec![];
        }

        let Some((_mouse_col, mouse_row)) = self.last_mouse_position else {
            return vec![];
        };

        let Some(bounds) = self.panel_bounds else {
            return vec![];
        };

        // Calculate inner area (without border)
        let inner_y = bounds.y + 1;
        let inner_height = bounds.height.saturating_sub(2);

        let mut screen = self.write_screen();

        // Skip if no selection
        if screen.selection_start.is_none() {
            return vec![];
        }

        let max_scroll = screen.scrollback.len();

        // Auto-scroll up (mouse above panel)
        if mouse_row < inner_y && screen.scroll_offset < max_scroll {
            screen.scroll_view_up(1);
            // Extend selection to top visible line
            let abs_row = screen.visual_to_absolute(0);
            screen.selection_end = Some((abs_row, 0));
            return vec![PanelEvent::NeedsRedraw];
        }

        // Auto-scroll down (mouse below panel)
        if mouse_row >= inner_y + inner_height && screen.scroll_offset > 0 {
            screen.scroll_view_down(1);
            // Extend selection to bottom visible line
            let last_row = inner_height.saturating_sub(1) as usize;
            let abs_row = screen.visual_to_absolute(last_row);
            let cols = screen.cols.saturating_sub(1);
            screen.selection_end = Some((abs_row, cols));
            return vec![PanelEvent::NeedsRedraw];
        }

        vec![]
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
        // Save terminal with initial working directory
        Some(SessionPanel::Terminal {
            working_dir: self.initial_cwd.clone(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn get_working_directory(&self) -> Option<std::path::PathBuf> {
        Some(self.initial_cwd.clone())
    }

    fn has_running_processes(&self) -> bool {
        if let Some(pid) = self.shell_pid {
            #[cfg(unix)]
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
