// Allow some clippy lints for VT100 implementation
#![allow(clippy::needless_range_loop)]

mod clipboard;
mod disk_space;
mod link_detection;
pub mod shell_utils;
mod terminal;
mod terminal_info;

pub use terminal::vt100_parser::VtPerformer;
pub use terminal::{Cell, CellStyle, MouseTrackingMode, TerminalScreen};
pub use terminal_info::TerminalInfo;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use link_detection::{HighlightSegment, LinkType};
#[cfg(unix)]
use nix::sys::signal::{self, Signal};
#[cfg(unix)]
use nix::unistd::Pid;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::Widget,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use std::any::Any;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use vte::Parser;

use termide_config::{matches_binding_or_default, Config, TerminalKeybindings};
use termide_core::{
    get_terminal_caps, CommandResult, Panel, PanelCommand, PanelEvent, RenderContext, Searchable,
    SessionPanel, WidthPreference,
};
use termide_theme::Theme;
use termide_ui::ScrollBar;

/// State for terminal text search across scrollback and visible buffer.
struct TerminalSearchState {
    /// Search query (kept for re-search when scrollback changes).
    #[allow(dead_code)]
    query: String,
    /// Case sensitivity flag (kept for re-search).
    #[allow(dead_code)]
    case_sensitive: bool,
    /// Matches: (absolute_row, column_start, match_length)
    matches: Vec<(usize, usize, usize)>,
    current_match: Option<usize>,
}

/// Full-featured terminal with PTY
pub struct Terminal {
    /// PTY master (wrapped in Arc<Mutex<>> for shared access)
    pty: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    /// Writer for writing to PTY
    writer: Box<dyn Write + Send>,
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
    /// Search state for Ctrl+Shift+F text search
    search_state: Option<TerminalSearchState>,
    /// Selection drag is active (left button held during selection).
    selection_drag_active: bool,
    /// Last mouse position in screen coordinates for auto-scroll.
    last_mouse_position: Option<(u16, u16)>,
    /// Panel bounds for auto-scroll calculations.
    panel_bounds: Option<Rect>,
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
        writer: Box<dyn std::io::Write + Send>,
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
            selection_drag_active: false,
            last_mouse_position: None,
            panel_bounds: None,
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
        let writer = pair.master.take_writer()?;
        let pty = Arc::new(Mutex::new(pair.master));
        let is_alive = Arc::new(Mutex::new(true));
        let has_new_data = Arc::new(AtomicBool::new(false));

        Self::spawn_reader(reader, &screen, &is_alive, &has_new_data);

        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "user".to_string());
        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("HOST"))
            .unwrap_or_else(|_| "localhost".to_string());
        let current_dir = std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
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
        let writer = pair.master.take_writer()?;
        let pty = Arc::new(Mutex::new(pair.master));
        let is_alive = Arc::new(Mutex::new(true));
        let has_new_data = Arc::new(AtomicBool::new(false));

        Self::spawn_reader(reader, &screen, &is_alive, &has_new_data);

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
                            return cstr.to_string_lossy().to_string();
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

        // Get disk info for current directory
        let disk_space = disk_space::get_disk_space_for_path(&cwd);

        TerminalInfo {
            user_host,
            cwd,
            disk_space,
        }
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
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Send a command to the terminal and execute it (adds Enter)
    pub fn send_command(&mut self, command: &str) -> Result<()> {
        self.send_input(command.as_bytes())?;
        self.send_input(b"\r")?;
        Ok(())
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
        clipboard::paste_atomic(&mut self.writer, text, true)
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

        // If mouse tracking is disabled, don't send
        if mouse_tracking == MouseTrackingMode::None {
            return Ok(());
        }

        // 1-based coordinates for SGR
        let inner_x = mouse.column.saturating_sub(panel_area.x + 1) + 1;
        let inner_y = mouse.row.saturating_sub(panel_area.y + 1) + 1;

        // Reusable buffer to avoid allocations (max SGR sequence is ~20 bytes)
        let mut buf = [0u8; 32];

        // Determine button code and whether this is release event
        let (btn_code, is_release): (u8, bool) = match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => (0, false),
            MouseEventKind::Down(MouseButton::Middle) => (1, false),
            MouseEventKind::Down(MouseButton::Right) => (2, false),
            MouseEventKind::Up(MouseButton::Left) => (0, true),
            MouseEventKind::Up(MouseButton::Middle) => (1, true),
            MouseEventKind::Up(MouseButton::Right) => (2, true),
            MouseEventKind::ScrollUp => (64, false),
            MouseEventKind::ScrollDown => (65, false),
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
    fn get_display_lines(
        &mut self,
        show_cursor: bool,
        theme: &Theme,
    ) -> (Arc<Vec<Line<'static>>>, (usize, usize), bool) {
        // === PHASE 0: Check if we can return cached result ===
        let (
            is_dirty,
            has_selection,
            sync_output,
            sync_output_ended,
            use_alt_screen,
            force_invalidation,
            current_cursor,
        ) = {
            let screen = self.read_screen();
            (
                screen.dirty,
                screen.selection_start.is_some(),
                screen.sync_output,
                screen.sync_output_ended,
                screen.use_alt_screen,
                screen.force_cache_invalidation,
                screen.cursor,
            )
        };

        // During sync_output, return cached content to prevent partial frame rendering
        // Only invalidate cache when sync_output is NOT active
        // IMPORTANT: Only use cache if it's from the same buffer (main vs alt)
        if sync_output && self.cached_use_alt_screen == use_alt_screen {
            // Clear force_invalidation flag but DON'T invalidate cache during batch
            // This defers invalidation until the batch ends
            if force_invalidation {
                if let Ok(mut screen) = self.screen.write() {
                    screen.force_cache_invalidation = false;
                }
            }
            // Return cached content (previous complete frame)
            if let Some(ref cached) = self.cached_lines {
                return (
                    Arc::clone(cached),
                    self.cached_cursor,
                    self.cached_cursor_shown,
                );
            }
            // If no cache exists during sync_output, we must render
            // This happens on first frame - fall through to regenerate
        }

        // Invalidate cache if active buffer changed (main <-> alt screen switch)
        // This prevents showing stale main buffer content over alt screen apps (e.g., Claude Code, htop)
        if self.cached_use_alt_screen != use_alt_screen {
            self.cached_lines = None;
            self.cached_use_alt_screen = use_alt_screen;
        }

        // Force invalidation when NOT in sync_output
        // This handles ED (clear screen) commands that need immediate visual update
        if force_invalidation {
            self.cached_lines = None;
            if let Ok(mut screen) = self.screen.write() {
                screen.force_cache_invalidation = false;
            }
        }

        // Handle sync_output batch end (transition from true to false)
        // IMPORTANT: Don't invalidate cache immediately when sync ends!
        // Between sync blocks, the terminal may be in an intermediate state
        // (e.g., after scroll but before new content is drawn).
        // Instead, return cached content until new dirty content arrives.
        // This prevents rendering artifacts like duplicate prompts.
        if sync_output_ended && !sync_output {
            // Clear the flag but DON'T invalidate cache yet
            if let Ok(mut screen) = self.screen.write() {
                screen.sync_output_ended = false;
            }
            // If we have cached content and screen is not dirty, return cache
            // This prevents showing intermediate state between sync blocks
            if !is_dirty {
                if let Some(ref cached) = self.cached_lines {
                    return (
                        Arc::clone(cached),
                        self.cached_cursor,
                        self.cached_cursor_shown,
                    );
                }
            }
            // Only invalidate cache when there's actual new content (dirty=true)
            self.cached_lines = None;
        }

        // Return cached if:
        // - Screen is not dirty (no new PTY output)
        // - Focus state hasn't changed (cursor visibility depends on focus)
        // - Cursor position hasn't changed (BS/CR move cursor without dirty flag)
        // - No active selection (selection changes without dirty flag)
        // - We have cached lines
        let has_search = self.search_state.is_some();
        if !is_dirty
            && self.cached_focus == show_cursor
            && !has_selection
            && !has_search
            && current_cursor == self.cached_cursor
        {
            if let Some(ref cached) = self.cached_lines {
                // O(1) Arc clone - no data copying!
                return (
                    Arc::clone(cached),
                    self.cached_cursor,
                    self.cached_cursor_shown,
                );
            }
        }

        // === PHASE 1: Render directly under lock (zero-copy) ===
        let mut screen = self.write_screen();
        // Clear dirty flag since we're about to render
        screen.dirty = false;
        // Ensure buffer has correct size before rendering (guards against IL/DL edge cases)
        screen.ensure_buffer_size();

        let visible_rows = screen.rows;
        let cols = screen.cols;
        let cursor_pos = screen.cursor;
        let cursor_visible = screen.cursor_visible;
        let scroll_offset = screen.scroll_offset;
        let use_alt_screen = screen.use_alt_screen;
        let has_selection = screen.selection_start.is_some() && screen.selection_end.is_some();
        let selection_start = screen.selection_start;
        let selection_end = screen.selection_end;

        // Determine view bounds based on scroll state
        let (view_start, total_scrollback, scrollback_slice) =
            if scroll_offset > 0 && !use_alt_screen {
                let total_scrollback = screen.scrollback.len();
                let total_lines = total_scrollback + visible_rows;
                let view_end = total_lines.saturating_sub(scroll_offset);
                let view_start = view_end.saturating_sub(visible_rows);
                (view_start, total_scrollback, true)
            } else {
                (0, 0, false)
            };

        // Pre-allocate output structures
        let mut lines = Vec::with_capacity(visible_rows);
        let mut current_text = String::with_capacity(cols);

        // Don't show cursor when viewing history
        let show_cursor_now = if scrollback_slice {
            false
        } else {
            show_cursor && cursor_visible
        };

        // Pre-compute selection bounds if selection exists (now in absolute coordinates)
        let selection_bounds = match (selection_start, selection_end) {
            (Some(start), Some(end)) if has_selection => {
                if start <= end {
                    Some((start, end))
                } else {
                    Some((end, start))
                }
            }
            _ => None,
        };

        // Calculate base for converting visual row to absolute
        // When scrolled: view_start is already the absolute index
        // When not scrolled: visual row 0 = scrollback.len() (start of active buffer)
        let scrollback_len = screen.scrollback.len();

        // Helper to check selection using absolute coordinates
        let is_in_selection = |visual_row: usize, col: usize| -> bool {
            if let Some((start, end)) = selection_bounds {
                // Convert visual row to absolute
                let abs_row = if scrollback_slice {
                    view_start + visual_row
                } else {
                    scrollback_len + visual_row
                };

                // Compare with absolute selection bounds
                if abs_row < start.0 || abs_row > end.0 {
                    return false;
                }
                if abs_row == start.0 && abs_row == end.0 {
                    col >= start.1 && col <= end.1
                } else if abs_row == start.0 {
                    col >= start.1
                } else if abs_row == end.0 {
                    col <= end.1
                } else {
                    true
                }
            } else {
                false
            }
        };

        // Pre-index URL segments by row for O(1) lookup per row
        // Instead of iterating all segments for each cell, we build a HashMap<row, Vec<(start, end)>>
        let url_segments_by_row: HashMap<usize, Vec<(usize, usize)>> = if self.ctrl_pressed {
            if let Some((_, segments)) = &self.hovered_link {
                let mut map: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
                for &(row, start, end) in segments {
                    map.entry(row).or_default().push((start, end));
                }
                map
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };

        // Helper to check if cell is in hovered URL (O(1) row lookup, then check ranges)
        let is_in_url = |visual_row: usize, col: usize| -> bool {
            if url_segments_by_row.is_empty() {
                return false;
            }
            // Convert visual row to absolute
            let abs_row = if scrollback_slice {
                view_start + visual_row
            } else {
                scrollback_len + visual_row
            };

            // O(1) lookup for the row, then check ranges (typically 1-2 per row)
            if let Some(ranges) = url_segments_by_row.get(&abs_row) {
                ranges.iter().any(|&(start, end)| col >= start && col < end)
            } else {
                false
            }
        };

        // Pre-index search matches by row for O(1) lookup per cell
        // Maps abs_row -> Vec<(col_start, col_end, is_current_match)>
        let search_matches_by_row: HashMap<usize, Vec<(usize, usize, bool)>> =
            if let Some(ref search) = self.search_state {
                let mut map: HashMap<usize, Vec<(usize, usize, bool)>> = HashMap::new();
                for (idx, &(abs_row, col_start, match_len)) in search.matches.iter().enumerate() {
                    let is_current = search.current_match == Some(idx);
                    map.entry(abs_row).or_default().push((
                        col_start,
                        col_start + match_len,
                        is_current,
                    ));
                }
                map
            } else {
                HashMap::new()
            };

        // Helper to check if cell is in a search match
        // Returns: None = not in match, Some(true) = current match, Some(false) = other match
        let is_in_search_match = |visual_row: usize, col: usize| -> Option<bool> {
            if search_matches_by_row.is_empty() {
                return None;
            }
            let abs_row = if scrollback_slice {
                view_start + visual_row
            } else {
                scrollback_len + visual_row
            };
            if let Some(ranges) = search_matches_by_row.get(&abs_row) {
                for &(start, end, is_current) in ranges {
                    if col >= start && col < end {
                        return Some(is_current);
                    }
                }
            }
            None
        };

        // Render directly from screen buffer (zero-copy)
        for row_idx in 0..visible_rows {
            // Get row reference without cloning
            let row: &[Cell] = if scrollback_slice {
                let source_idx = view_start + row_idx;
                if source_idx < total_scrollback {
                    &screen.scrollback[source_idx]
                } else {
                    let buf_idx = source_idx - total_scrollback;
                    if buf_idx < screen.active_buffer().len() {
                        &screen.active_buffer()[buf_idx]
                    } else {
                        lines.push(Line::default());
                        continue;
                    }
                }
            } else if row_idx < screen.active_buffer().len() {
                &screen.active_buffer()[row_idx]
            } else {
                lines.push(Line::default());
                continue;
            };

            let mut spans = Vec::with_capacity(8); // Pre-allocate for typical line
            current_text.clear();
            // Use direct style value instead of Option for faster comparison
            let mut current_style = Style::default();

            for (col_idx, cell) in row.iter().enumerate() {
                // Apply reverse if set
                let (mut fg, mut bg) = if cell.style.reverse {
                    (cell.style.bg, cell.style.fg)
                } else {
                    (cell.style.fg, cell.style.bg)
                };

                // Apply theme colors during rendering (not post-processing)
                if fg == Color::White || fg == Color::Reset {
                    fg = theme.fg;
                }
                if bg == Color::Reset {
                    bg = theme.bg;
                }

                let mut style = Style::default().fg(fg).bg(bg);

                if cell.style.bold {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if cell.style.italic {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if cell.style.underline {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if cell.style.reverse {
                    style = style.add_modifier(Modifier::REVERSED);
                }

                // Check if cell is in selection (optimized - skips if no selection)
                if is_in_selection(row_idx, col_idx) {
                    style = Style::default().fg(theme.bg).bg(theme.accented_fg);
                }

                // Check if cell is in hovered URL (Ctrl+hover) - use warning color
                if is_in_url(row_idx, col_idx) {
                    style = Style::default().fg(theme.bg).bg(theme.warning);
                }

                // Check if cell is in a search match
                if let Some(is_current) = is_in_search_match(row_idx, col_idx) {
                    if is_current {
                        // Current match: accented foreground background
                        style = Style::default().fg(theme.bg).bg(theme.accented_fg);
                    } else {
                        // Other matches: warning background
                        style = Style::default().fg(theme.bg).bg(theme.warning);
                    }
                }

                // If this is cursor position and needs showing, use inverse colors
                if show_cursor_now && row_idx == cursor_pos.0 && col_idx == cursor_pos.1 {
                    // Flush accumulated text
                    if !current_text.is_empty() {
                        spans.push(Span::styled(
                            std::mem::take(&mut current_text),
                            current_style,
                        ));
                    }

                    // Cursor with inverted colors (use original fg/bg for inversion)
                    let cursor_style = Style::default()
                        .bg(
                            if cell.style.fg == Color::White || cell.style.fg == Color::Reset {
                                theme.fg
                            } else {
                                cell.style.fg
                            },
                        )
                        .fg(if cell.style.bg == Color::Reset {
                            theme.bg
                        } else {
                            cell.style.bg
                        })
                        .add_modifier(Modifier::BOLD);

                    let cursor_char = if cell.ch == ' ' || cell.ch == '\0' {
                        ' '
                    } else {
                        cell.ch
                    };
                    let mut cursor_buf = [0u8; 4];
                    let cursor_str = cursor_char.encode_utf8(&mut cursor_buf);
                    spans.push(Span::styled(cursor_str.to_owned(), cursor_style));
                    continue;
                }

                // Group characters with same style (no Option overhead)
                if current_text.is_empty() || current_style == style {
                    current_text.push(cell.ch);
                    current_style = style;
                } else {
                    // Flush accumulated text with previous style
                    spans.push(Span::styled(
                        std::mem::take(&mut current_text),
                        current_style,
                    ));
                    current_text.push(cell.ch);
                    current_style = style;
                }
            }

            // Add last span
            if !current_text.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut current_text),
                    current_style,
                ));
            }

            // If line is empty and cursor is on it, add cursor
            if show_cursor_now && spans.is_empty() && row_idx == cursor_pos.0 {
                let cursor_style = Style::default()
                    .bg(theme.fg)
                    .fg(theme.bg)
                    .add_modifier(Modifier::BOLD);
                spans.push(Span::styled(" ", cursor_style));
            }

            lines.push(Line::from(spans));
        }

        // Release write lock before modifying other self fields
        drop(screen);

        // === PHASE 3: Cache the result (no clone - just wrap in Arc) ===
        let arc_lines = Arc::new(lines);
        self.cached_lines = Some(Arc::clone(&arc_lines));
        self.cached_cursor = cursor_pos;
        self.cached_cursor_shown = show_cursor_now;
        self.cached_focus = show_cursor;
        // Sync cached_use_alt_screen with actual rendered buffer (from write lock)
        self.cached_use_alt_screen = use_alt_screen;

        (arc_lines, cursor_pos, show_cursor_now)
    }

    /// Check if PTY has new data that needs rendering
    pub fn has_pending_output(&self) -> bool {
        self.has_new_data.swap(false, Ordering::AcqRel)
    }

    /// Get the name of the currently running foreground command
    fn get_foreground_command(&self) -> String {
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

    /// Find all search matches in scrollback + visible buffer.
    fn find_matches(
        screen: &TerminalScreen,
        query: &str,
        case_sensitive: bool,
    ) -> Vec<(usize, usize, usize)> {
        let mut matches = Vec::new();
        let scrollback_len = screen.scrollback.len();
        let total_lines = scrollback_len + screen.rows;

        let query_lower = if case_sensitive {
            query.to_string()
        } else {
            query.to_lowercase()
        };

        for abs_row in 0..total_lines {
            let Some(row) = screen.get_line_by_absolute(abs_row) else {
                continue;
            };

            // Extract text from cells
            let line_text: String = row.iter().map(|c| c.ch).collect();
            let search_text = if case_sensitive {
                line_text.clone()
            } else {
                line_text.to_lowercase()
            };

            // Find all occurrences in this line
            let mut start = 0;
            while let Some(pos) = search_text[start..].find(&query_lower) {
                let col = start + pos;
                matches.push((abs_row, col, query_lower.len()));
                start = col + query_lower.len();
                if start >= search_text.len() {
                    break;
                }
            }
        }
        matches
    }
}

impl Searchable for Terminal {
    fn start_search(&mut self, query: String, case_sensitive: bool) {
        if query.is_empty() {
            self.search_state = None;
            self.cached_lines = None;
            return;
        }

        let screen = self.read_screen();
        let matches = Self::find_matches(&screen, &query, case_sensitive);
        let scrollback_len = screen.scrollback.len();
        let visible_rows = screen.rows;
        let scroll_offset = screen.scroll_offset;
        drop(screen);

        // Find nearest match to current viewport
        let current_match = if matches.is_empty() {
            None
        } else {
            // Calculate what absolute row is at the center of the viewport
            let total_lines = scrollback_len + visible_rows;
            let view_bottom = total_lines.saturating_sub(scroll_offset);
            let view_center = view_bottom.saturating_sub(visible_rows / 2);

            // Find the match closest to the center of current viewport
            let idx = matches
                .iter()
                .enumerate()
                .min_by_key(|(_, (row, _, _))| {
                    (*row as isize - view_center as isize).unsigned_abs()
                })
                .map(|(i, _)| i);
            idx
        };

        self.search_state = Some(TerminalSearchState {
            query,
            case_sensitive,
            matches,
            current_match,
        });

        // Scroll to current match
        if let Some(idx) = current_match {
            if let Some(state) = &self.search_state {
                let abs_row = state.matches[idx].0;
                self.scroll_to_abs_row(abs_row);
            }
        }

        self.cached_lines = None;
    }

    fn search_next(&mut self) {
        let should_scroll = if let Some(ref mut state) = self.search_state {
            if state.matches.is_empty() {
                false
            } else {
                let next = match state.current_match {
                    Some(idx) => (idx + 1) % state.matches.len(),
                    None => 0,
                };
                state.current_match = Some(next);
                true
            }
        } else {
            false
        };

        if should_scroll {
            if let Some(ref state) = self.search_state {
                if let Some(idx) = state.current_match {
                    let abs_row = state.matches[idx].0;
                    self.scroll_to_abs_row(abs_row);
                }
            }
            self.cached_lines = None;
        }
    }

    fn search_prev(&mut self) {
        let should_scroll = if let Some(ref mut state) = self.search_state {
            if state.matches.is_empty() {
                false
            } else {
                let prev = match state.current_match {
                    Some(0) => state.matches.len() - 1,
                    Some(idx) => idx - 1,
                    None => state.matches.len() - 1,
                };
                state.current_match = Some(prev);
                true
            }
        } else {
            false
        };

        if should_scroll {
            if let Some(ref state) = self.search_state {
                if let Some(idx) = state.current_match {
                    let abs_row = state.matches[idx].0;
                    self.scroll_to_abs_row(abs_row);
                }
            }
            self.cached_lines = None;
        }
    }

    fn close_search(&mut self) {
        self.search_state = None;
        self.cached_lines = None;
    }

    fn get_search_match_info(&self) -> Option<(usize, usize)> {
        self.search_state
            .as_ref()
            .and_then(|state| state.current_match.map(|idx| (idx, state.matches.len())))
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

    fn prepare_render(&mut self, theme: &Theme, config: &Config) {
        // Invalidate cache if theme changed
        if self.cached_theme != *theme {
            self.cached_lines = None;
        }
        self.cached_theme = *theme;
        if self.keybindings != config.terminal.keybindings {
            self.keybindings = config.terminal.keybindings.clone();
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        // Update size if changed
        // area is already the inner content area (accordion drew outer border)
        let new_rows = area.height;
        let new_cols = area.width;

        if new_rows != self.size.rows || new_cols != self.size.cols {
            let _ = self.resize(new_rows, new_cols);
        }

        // Data is read in a separate thread, just render current state
        // Show cursor only when panel is focused
        // Theme colors are now applied during get_display_lines() - no post-processing needed
        let theme = self.cached_theme;
        let (arc_lines, _cursor_pos, _cursor_shown) =
            self.get_display_lines(ctx.is_focused, &theme);

        // Render terminal content directly (accordion already drew border with title/buttons)
        // Extract Vec from Arc - this is the only clone point now
        // On cache hit: Arc clone was O(1), this clone is the only cost
        // On cache miss: Arc wrap was O(1), this clone is the only cost
        let lines = Arc::try_unwrap(arc_lines).unwrap_or_else(|arc| (*arc).clone());

        // Clear the render area with background color to prevent visual artifacts
        // from previous content (modal borders, old status lines, etc.)
        let bg_style = Style::default().bg(theme.bg);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ');
                    cell.set_style(bg_style);
                }
            }
        }

        let paragraph = Paragraph::new(lines);
        paragraph.render(area, buf);

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

    fn handle_key(&mut self, key: KeyEvent) -> Vec<PanelEvent> {
        // If process exited, don't handle input
        if !self.is_alive() {
            return vec![];
        }

        // Translate Cyrillic to Latin for hotkeys
        let key = termide_keyboard::translate_hotkey(key);

        let kb = &self.keybindings;

        // Configurable clipboard operations
        // Paste (Ctrl+Shift+V)
        if matches_binding_or_default(
            &kb.paste,
            &key,
            KeyCode::Char('V'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ) {
            let _ = self.paste_from_clipboard();
            return vec![];
        }

        // Copy (Ctrl+Shift+C)
        if matches_binding_or_default(
            &kb.copy,
            &key,
            KeyCode::Char('C'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ) {
            let _ = self.copy_selection_to_clipboard();
            return vec![];
        }

        // Search (Ctrl+F)
        if matches_binding_or_default(&kb.search, &key, KeyCode::Char('f'), KeyModifiers::CONTROL) {
            return vec![PanelEvent::ShowSearch {
                mode: termide_core::SearchMode::Text,
                initial_query: None,
            }];
        }

        // Scroll up (Shift+PageUp)
        if matches_binding_or_default(&kb.scroll_up, &key, KeyCode::PageUp, KeyModifiers::SHIFT) {
            let mut screen = self.write_screen();
            let scroll_amount = screen.rows.saturating_sub(1);
            screen.scroll_view_up(scroll_amount);
            return vec![];
        }

        // Scroll down (Shift+PageDown)
        if matches_binding_or_default(
            &kb.scroll_down,
            &key,
            KeyCode::PageDown,
            KeyModifiers::SHIFT,
        ) {
            let mut screen = self.write_screen();
            let scroll_amount = screen.rows.saturating_sub(1);
            screen.scroll_view_down(scroll_amount);
            return vec![];
        }

        // Scroll top (Shift+Home)
        if matches_binding_or_default(&kb.scroll_top, &key, KeyCode::Home, KeyModifiers::SHIFT) {
            let mut screen = self.write_screen();
            screen.scroll_offset = screen.scrollback.len();
            return vec![];
        }

        // Scroll bottom (Shift+End)
        if matches_binding_or_default(&kb.scroll_bottom, &key, KeyCode::End, KeyModifiers::SHIFT) {
            self.write_screen().reset_scroll();
            return vec![];
        }

        // Reset scroll on input, cache application_cursor_keys - single lock
        // Note: selection is NOT cleared on keypress to allow copying from running apps
        let application_cursor_keys = {
            let mut screen = self.write_screen();
            screen.reset_scroll();
            screen.application_cursor_keys
        };

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
                            let _ = self.copy_selection_to_clipboard();
                            // Clear selection after copying
                            self.write_screen().clear_selection();
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
                if key.modifiers.contains(KeyModifiers::SHIFT)
                    || key.modifiers.contains(KeyModifiers::ALT)
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
                // In Application Cursor Keys Mode send \x1bO instead of \x1b[
                if application_cursor_keys {
                    let _ = self.send_input(b"\x1bOD");
                } else {
                    let _ = self.send_input(b"\x1b[D");
                }
            }
            KeyCode::Right => {
                if application_cursor_keys {
                    let _ = self.send_input(b"\x1bOC");
                } else {
                    let _ = self.send_input(b"\x1b[C");
                }
            }
            KeyCode::Up => {
                if application_cursor_keys {
                    let _ = self.send_input(b"\x1bOA");
                } else {
                    let _ = self.send_input(b"\x1b[A");
                }
            }
            KeyCode::Down => {
                if application_cursor_keys {
                    let _ = self.send_input(b"\x1bOB");
                } else {
                    let _ = self.send_input(b"\x1b[B");
                }
            }
            KeyCode::Home => {
                // In Application Cursor Keys Mode send \x1bO instead of \x1b[
                if application_cursor_keys {
                    let _ = self.send_input(b"\x1bOH");
                } else {
                    let _ = self.send_input(b"\x1b[H");
                }
            }
            KeyCode::End => {
                if application_cursor_keys {
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
                // F-keys for xterm-256color
                match n {
                    1 => {
                        let _ = self.send_input(b"\x1bOP");
                    }
                    2 => {
                        let _ = self.send_input(b"\x1bOQ");
                    }
                    3 => {
                        let _ = self.send_input(b"\x1bOR");
                    }
                    4 => {
                        let _ = self.send_input(b"\x1bOS");
                    }
                    5 => {
                        let _ = self.send_input(b"\x1b[15~");
                    }
                    6 => {
                        let _ = self.send_input(b"\x1b[17~");
                    }
                    7 => {
                        let _ = self.send_input(b"\x1b[18~");
                    }
                    8 => {
                        let _ = self.send_input(b"\x1b[19~");
                    }
                    9 => {
                        let _ = self.send_input(b"\x1b[20~");
                    }
                    10 => {
                        let _ = self.send_input(b"\x1b[21~");
                    }
                    11 => {
                        let _ = self.send_input(b"\x1b[23~");
                    }
                    12 => {
                        let _ = self.send_input(b"\x1b[24~");
                    }
                    _ => {}
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

        // Calculate inner area (without border)
        let inner_x_min = panel_area.x + 1;
        let inner_x_max = panel_area.x + panel_area.width.saturating_sub(2);
        let inner_y_min = panel_area.y + 1;
        let inner_y_max = panel_area.y + panel_area.height.saturating_sub(2);

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
        self.ctrl_pressed = ctrl_pressed;

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
            } else {
                // No link under cursor
                drop(screen);
                if self.hovered_link.is_some() {
                    self.hovered_link = None;
                    self.cached_lines = None; // Force redraw
                }
            }
        } else if !ctrl_pressed && self.hovered_link.is_some() {
            // Ctrl not pressed - clear link highlight
            self.hovered_link = None;
            self.cached_lines = None; // Force redraw
        }

        // Handle scroll events first - they should work even when cursor is near border
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.write_screen().scroll_view_up(3);
                return vec![];
            }
            MouseEventKind::ScrollDown => {
                self.write_screen().scroll_view_down(3);
                return vec![];
            }
            _ => {}
        }

        // Check if selection is active
        let selection_active = {
            let screen = self.read_screen();
            screen.selection_start.is_some()
        };

        // If mouse is outside and selection is not active - ignore other events
        if !is_inside && !selection_active {
            return vec![];
        }

        // Handle local text selection (priority over sending to PTY)
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Start selection only inside panel
                if !is_inside {
                    return vec![];
                }

                // Ctrl+Click on link - open URL in browser or path in file manager
                if ctrl_pressed {
                    if let Some((ref link_type, _)) = self.hovered_link {
                        match link_type {
                            LinkType::Url(url) => {
                                let _ = open::that(url);
                            }
                            LinkType::FilePath(path) => {
                                // Open directory in file manager, or file's parent with file selected
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
                        return vec![];
                    }
                }

                // Start text selection with absolute coordinates
                let mut screen = self.write_screen();
                let abs_row = screen.visual_to_absolute(inner_row);
                screen.selection_start = Some((abs_row, inner_col));
                screen.selection_end = Some((abs_row, inner_col)); // Set immediately for visibility
                drop(screen);

                // Start selection drag tracking for auto-scroll
                self.selection_drag_active = true;

                // Also send click to PTY if mouse tracking is enabled
                let _ = self.send_mouse_to_pty(&mouse, panel_area);
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
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // End selection drag tracking
                self.selection_drag_active = false;
                self.last_mouse_position = None;

                // Finalize selection
                let is_single_click = {
                    let mut screen = self.write_screen();
                    let abs_row = screen.visual_to_absolute(inner_row);
                    if let Some(start) = screen.selection_start {
                        screen.selection_end = Some((abs_row, inner_col));
                        // Single click = no drag (start == end)
                        start == (abs_row, inner_col)
                    } else {
                        false
                    }
                };

                // Clear selection on single click (no drag)
                // Copy is done manually via Ctrl+Shift+C
                if is_single_click {
                    let mut screen = self.write_screen();
                    screen.clear_selection();
                }

                // Send release to PTY if mouse tracking is enabled (only if inside)
                if is_inside {
                    let _ = self.send_mouse_to_pty(&mouse, panel_area);
                }
            }
            // Scroll events are handled above (before boundary check)
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {}
            // Other mouse events send to PTY
            _ => {
                let _ = self.send_mouse_to_pty(&mouse, panel_area);
            }
        }

        vec![]
    }

    fn handle_scroll(&mut self, delta: i32, _panel_area: Rect) -> Vec<PanelEvent> {
        let lines = delta.unsigned_abs() as usize * 3; // 3 lines per scroll unit
        let mut screen = self.write_screen();
        if delta < 0 {
            screen.scroll_view_up(lines);
        } else {
            screen.scroll_view_down(lines);
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
            // Terminals always stay active (PTY must be drained), so MarkStale/RefreshIfStale are no-ops
            PanelCommand::MarkStale | PanelCommand::RefreshIfStale => CommandResult::None,
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
            | PanelCommand::SetGitOperationInProgress { .. } => CommandResult::None,
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
        // If there are running processes, Escape is passed to them, not closing the panel
        self.is_alive() && self.has_running_processes()
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
