// Allow some clippy lints for VT100 implementation
#![allow(clippy::needless_range_loop)]

mod terminal;
mod terminal_info;

pub use terminal::vt100_parser::VtPerformer;
pub use terminal::{Cell, CellStyle, MouseTrackingMode, TerminalScreen};
pub use terminal_info::TerminalInfo;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nix::sys::signal::{self, Signal};
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
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use vte::Parser;

use termide_config::Config;
use termide_core::{CommandResult, Panel, PanelCommand, PanelEvent, RenderContext, SessionPanel};
use termide_theme::Theme;
use termide_ui::system_monitor::DiskSpaceInfo;

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
    /// Terminal title (user@host:dir)
    terminal_title: String,
    /// Initial working directory (set when terminal was created)
    initial_cwd: std::path::PathBuf,
    /// Cached theme for rendering
    cached_theme: Theme,
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
}

impl Terminal {
    /// Create new terminal with PTY
    #[allow(dead_code)]
    pub fn new(rows: u16, cols: u16) -> Result<Self> {
        Self::new_with_cwd(rows, cols, None)
    }

    /// Create new terminal with specified working directory
    pub fn new_with_cwd(rows: u16, cols: u16, cwd: Option<std::path::PathBuf>) -> Result<Self> {
        let pty_system = native_pty_system();

        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system.openpty(size)?;

        // Detect shell
        let shell = Self::detect_shell();
        let shell_args = Self::get_shell_args(&shell);

        let mut cmd = CommandBuilder::new(&shell);

        // Add arguments for interactive mode
        for arg in shell_args {
            cmd.arg(arg);
        }

        // Set working directory: passed or current
        let working_dir =
            cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| "/".into()));
        cmd.cwd(&working_dir);

        // Set environment variables for correct readline and escape sequence behavior
        cmd.env("TERM", "xterm-256color");
        cmd.env("SHELL", &shell);
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
        // PATH is critical for NixOS - without it bash-interactive won't be found
        cmd.env(
            "PATH",
            std::env::var("PATH")
                .unwrap_or_else(|_| "/run/current-system/sw/bin:/usr/bin:/bin".to_string()),
        );

        let child = pair.slave.spawn_command(cmd)?;
        let shell_pid = child.process_id();

        let screen = Arc::new(RwLock::new(TerminalScreen::new(
            rows as usize,
            cols as usize,
        )));

        // Create reader and writer BEFORE placing PTY in Arc<Mutex>
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        let pty = Arc::new(Mutex::new(pair.master));
        let is_alive = Arc::new(Mutex::new(true));
        let has_new_data = Arc::new(AtomicBool::new(false));

        // Start thread for reading from PTY
        let screen_clone = Arc::clone(&screen);
        let is_alive_clone = Arc::clone(&is_alive);
        let has_new_data_clone = Arc::clone(&has_new_data);
        thread::spawn(move || {
            let mut parser = Parser::new();
            // Increased buffer from 4KB to 16KB for better throughput with intensive output
            let mut buf = [0u8; 16384];
            // Reuse performer across reads to maintain state
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
                        // Flush all batched operations with a single lock
                        // This reduces mutex contention significantly
                        performer.flush();
                        // Signal main thread that new data is available for rendering
                        has_new_data_clone.store(true, Ordering::Release);
                    }
                    Ok(_) => {
                        // EOF - shell terminated
                        break;
                    }
                    Err(_) => {
                        // Read error - exit
                        break;
                    }
                }
            }

            // Set process termination flag
            if let Ok(mut alive) = is_alive_clone.lock() {
                *alive = false;
            }
        });

        // Get information for terminal title
        let username = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("HOST"))
            .unwrap_or_else(|_| "localhost".to_string());
        let current_dir = std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "~".to_string());

        let terminal_title = format!("{}@{}:{}", username, hostname, current_dir);

        Ok(Self {
            pty,
            writer,
            child,
            shell_pid,
            screen,
            size,
            is_alive,
            terminal_title,
            initial_cwd: working_dir,
            cached_theme: Theme::default(),
            has_new_data,
            cached_lines: None,
            cached_cursor: (0, 0),
            cached_cursor_shown: false,
            cached_focus: false,
        })
    }

    /// Detect available shell
    fn detect_shell() -> String {
        // On NixOS first check bash-interactive in system profile
        // (regular bash in nix store might be without readline)
        let nixos_shells = [
            "/run/current-system/sw/bin/fish",
            "/run/current-system/sw/bin/zsh",
            "/run/current-system/sw/bin/bash",
        ];
        for shell in nixos_shells {
            if std::path::Path::new(shell).exists() {
                return shell.to_string();
            }
        }

        // Then check $SHELL
        if let Ok(shell) = std::env::var("SHELL") {
            if std::path::Path::new(&shell).exists() {
                return shell;
            }
        }

        // Check popular shells on regular systems
        let shells = ["/usr/bin/fish", "/usr/bin/zsh", "/bin/bash", "/bin/sh"];
        for shell in shells {
            if std::path::Path::new(shell).exists() {
                return shell.to_string();
            }
        }

        "/bin/sh".to_string()
    }

    /// Get arguments for launching the shell
    fn get_shell_args(shell_path: &str) -> Vec<&'static str> {
        let shell_name = std::path::Path::new(shell_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        match shell_name {
            "fish" => vec!["-l"],      // login shell
            "zsh" => vec!["-l", "-i"], // login + interactive
            "bash" => vec![],          // PTY will make it interactive automatically
            _ => vec![],               // no arguments
        }
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

                screen.rows = new_rows;
                screen.cols = new_cols;

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
        let username = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("HOST"))
            .unwrap_or_else(|_| {
                // Try to get hostname via gethostname
                let mut buf = [0u8; 256];
                // SAFETY: gethostname is a POSIX function that writes a null-terminated
                // hostname into the provided buffer. We provide a stack-allocated buffer
                // of 256 bytes (sufficient for hostnames per POSIX HOST_NAME_MAX).
                // On success (return 0), the buffer contains a valid C string.
                // We use CStr::from_ptr which requires a null-terminated string - guaranteed
                // by gethostname on success. The buffer outlives the CStr usage.
                unsafe {
                    if libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) == 0 {
                        let cstr = std::ffi::CStr::from_ptr(buf.as_ptr() as *const libc::c_char);
                        cstr.to_string_lossy().to_string()
                    } else {
                        "localhost".to_string()
                    }
                }
            });
        let user_host = format!("{}@{}", username, hostname);

        // Get current directory (using environment variable)
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "~".to_string());

        // Get disk info for current directory
        let disk_space = self.get_disk_space_for_path(&cwd);

        TerminalInfo {
            user_host,
            cwd,
            disk_space,
        }
    }

    /// Resolve dm-X device to physical partition
    /// e.g., /dev/dm-0 -> /dev/nvme0n1p2
    fn resolve_dm_device(device: &str) -> Option<String> {
        // Extract dm number (e.g., "dm-0" from "/dev/dm-0")
        let dm_name = device.strip_prefix("/dev/")?;
        if !dm_name.starts_with("dm-") {
            return None;
        }

        // Read /sys/block/dm-X/slaves/ to find physical partition
        let slaves_path = format!("/sys/block/{}/slaves", dm_name);
        let slaves_dir = std::fs::read_dir(&slaves_path).ok()?;

        // Get first slave (physical partition)
        for entry in slaves_dir.flatten() {
            if let Ok(name) = entry.file_name().into_string() {
                return Some(format!("/dev/{}", name));
            }
        }

        None
    }

    /// Get device name from /proc/mounts for a given path
    fn get_device_for_path(path: &str) -> Option<String> {
        let mounts_content = std::fs::read_to_string("/proc/mounts").ok()?;
        let mut best_match: Option<(String, usize)> = None;

        for line in mounts_content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }

            let device = parts[0];
            let mount_point = parts[1];

            // Check if this mount point is a prefix of our path
            if let Ok(canonical_path) = std::path::Path::new(path).canonicalize() {
                if let Ok(canonical_mount) = std::path::Path::new(mount_point).canonicalize() {
                    if canonical_path.starts_with(&canonical_mount) {
                        let mount_len = canonical_mount.as_os_str().len();
                        // Keep track of the longest matching mount point
                        if best_match.is_none() || mount_len > best_match.as_ref().unwrap().1 {
                            best_match = Some((device.to_string(), mount_len));
                        }
                    }
                }
            }
        }

        best_match.and_then(|(device, _)| {
            // First try to resolve symlink (e.g., /dev/disk/by-uuid/... -> /dev/nvme0n1p2)
            let resolved = std::path::Path::new(&device)
                .canonicalize()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| device.clone());

            // If it's a dm device, resolve to physical partition
            if resolved.contains("/dm-") {
                Self::resolve_dm_device(&resolved).or(Some(resolved))
            } else {
                Some(resolved)
            }
        })
    }

    /// Get disk space information for specified path
    fn get_disk_space_for_path(&self, path: &str) -> Option<DiskSpaceInfo> {
        use std::ffi::CString;

        let path_cstr = CString::new(path).ok()?;

        // Get device name for this path
        let device = Self::get_device_for_path(path);

        // SAFETY: statvfs is a POSIX function that fills a statvfs struct with
        // filesystem statistics. We zero-initialize the struct to ensure all fields
        // have defined values. path_cstr is a valid null-terminated CString created
        // above. statvfs returns 0 on success and writes valid data to the struct.
        // We only read the struct fields after confirming success (return == 0).
        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(path_cstr.as_ptr(), &mut stat) == 0 {
                // On macOS, f_bavail and f_blocks are u32, f_bsize is u64
                // On Linux, all are u64
                #[cfg(target_os = "macos")]
                let available = (stat.f_bavail as u64) * stat.f_bsize;
                #[cfg(not(target_os = "macos"))]
                let available = stat.f_bavail * stat.f_bsize;

                #[cfg(target_os = "macos")]
                let total = (stat.f_blocks as u64) * stat.f_bsize;
                #[cfg(not(target_os = "macos"))]
                let total = stat.f_blocks * stat.f_bsize;

                Some(DiskSpaceInfo {
                    device,
                    available,
                    total,
                })
            } else {
                None
            }
        }
    }

    /// Send input to PTY
    fn send_input(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Get selected text
    fn get_selected_text(&self) -> String {
        let screen = self.screen.read().expect("Terminal screen lock poisoned");
        let (start, end) = match (screen.selection_start, screen.selection_end) {
            (Some(s), Some(e)) => (s, e),
            _ => return String::new(),
        };

        // Normalize
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };

        let buffer = screen.active_buffer();
        let mut result = String::new();

        for row_idx in start.0..=end.0 {
            if row_idx >= buffer.len() {
                break;
            }

            let row = &buffer[row_idx];
            let col_start = if row_idx == start.0 { start.1 } else { 0 };
            let col_end = if row_idx == end.0 {
                end.1.min(row.len().saturating_sub(1))
            } else {
                row.len().saturating_sub(1)
            };

            for col_idx in col_start..=col_end {
                if col_idx < row.len() {
                    let ch = row[col_idx].ch;
                    if ch != '\0' {
                        result.push(ch);
                    }
                }
            }

            // Add line break between lines (but not at the end)
            if row_idx < end.0 {
                result.push('\n');
            }
        }

        // Trim trailing whitespace from each line
        result
            .lines()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Copy selected text to clipboard
    fn copy_selection_to_clipboard(&self) -> Result<()> {
        let text = self.get_selected_text();
        if text.is_empty() {
            return Ok(());
        }

        // Use universal buffer (includes OSC 52)
        let _ = termide_ui::clipboard::copy(&text);

        Ok(())
    }

    /// Paste text from clipboard to PTY with bracketed paste mode support
    pub fn paste_from_clipboard(&mut self) -> Result<()> {
        // Get text from clipboard
        let Some(text) = termide_ui::clipboard::paste() else {
            return Ok(());
        };

        // Check if bracketed paste mode is enabled
        let bracketed_paste = self
            .screen
            .read()
            .expect("Terminal screen lock poisoned")
            .bracketed_paste_mode;

        if bracketed_paste {
            // Send bracketed paste start sequence
            self.send_input(b"\x1b[200~")?;
            // Send the actual text
            self.send_input(text.as_bytes())?;
            // Send bracketed paste end sequence
            self.send_input(b"\x1b[201~")?;
        } else {
            // Send text as-is without bracketing
            self.send_input(text.as_bytes())?;
        }

        Ok(())
    }

    /// Send mouse event to PTY (if mouse tracking is enabled)
    fn send_mouse_to_pty(
        &mut self,
        mouse: &crossterm::event::MouseEvent,
        panel_area: Rect,
    ) -> Result<()> {
        use crossterm::event::{MouseButton, MouseEventKind};

        let (mouse_tracking, sgr_mode) = {
            let screen = self.screen.read().expect("Terminal screen lock poisoned");
            (screen.mouse_tracking, screen.sgr_mouse_mode)
        };

        // If mouse tracking is disabled, don't send
        if mouse_tracking == MouseTrackingMode::None {
            return Ok(());
        }

        // 1-based coordinates for SGR
        let inner_x = mouse.column.saturating_sub(panel_area.x + 1) + 1;
        let inner_y = mouse.row.saturating_sub(panel_area.y + 1) + 1;

        let sequence = match mouse.kind {
            MouseEventKind::Down(button) => {
                let btn_code = match button {
                    MouseButton::Left => 0,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                };
                if sgr_mode {
                    format!("\x1b[<{};{};{}M", btn_code, inner_x, inner_y)
                } else {
                    let encoded_btn = (btn_code + 32) as u8;
                    let encoded_x = (inner_x as u8).saturating_add(32);
                    let encoded_y = (inner_y as u8).saturating_add(32);
                    format!(
                        "\x1b[M{}{}{}",
                        encoded_btn as char, encoded_x as char, encoded_y as char
                    )
                }
            }
            MouseEventKind::Up(button) => {
                if sgr_mode {
                    let btn_code = match button {
                        MouseButton::Left => 0,
                        MouseButton::Middle => 1,
                        MouseButton::Right => 2,
                    };
                    format!("\x1b[<{};{};{}m", btn_code, inner_x, inner_y)
                } else {
                    let encoded_x = (inner_x as u8).saturating_add(32);
                    let encoded_y = (inner_y as u8).saturating_add(32);
                    format!(
                        "\x1b[M{}{}{}",
                        (3 + 32) as u8 as char,
                        encoded_x as char,
                        encoded_y as char
                    )
                }
            }
            MouseEventKind::ScrollUp => {
                let btn_code = 64;
                if sgr_mode {
                    format!("\x1b[<{};{};{}M", btn_code, inner_x, inner_y)
                } else {
                    let encoded_x = (inner_x as u8).saturating_add(32);
                    let encoded_y = (inner_y as u8).saturating_add(32);
                    format!(
                        "\x1b[M{}{}{}",
                        (btn_code + 32) as u8 as char,
                        encoded_x as char,
                        encoded_y as char
                    )
                }
            }
            MouseEventKind::ScrollDown => {
                let btn_code = 65;
                if sgr_mode {
                    format!("\x1b[<{};{};{}M", btn_code, inner_x, inner_y)
                } else {
                    let encoded_x = (inner_x as u8).saturating_add(32);
                    let encoded_y = (inner_y as u8).saturating_add(32);
                    format!(
                        "\x1b[M{}{}{}",
                        (btn_code + 32) as u8 as char,
                        encoded_x as char,
                        encoded_y as char
                    )
                }
            }
            _ => return Ok(()),
        };

        self.send_input(sequence.as_bytes())?;
        Ok(())
    }

    /// Get lines for display using copy-out pattern with caching.
    ///
    /// Optimization: Uses dirty flag to skip re-rendering when terminal
    /// content hasn't changed. Returns Arc-wrapped lines for O(1) cache hits.
    ///
    /// Returns: (lines_arc, cursor_position, cursor_shown)
    fn get_display_lines(
        &mut self,
        show_cursor: bool,
        theme: &Theme,
    ) -> (Arc<Vec<Line<'static>>>, (usize, usize), bool) {
        // === PHASE 0: Check if we can return cached result ===
        let (is_dirty, has_selection) = {
            let screen = self.screen.read().expect("Terminal screen lock poisoned");
            (screen.dirty, screen.selection_start.is_some())
        };

        // Return cached if:
        // - Screen is not dirty (no new PTY output)
        // - Focus state hasn't changed (cursor visibility depends on focus)
        // - No active selection (selection changes without dirty flag)
        // - We have cached lines
        if !is_dirty && self.cached_focus == show_cursor && !has_selection {
            if let Some(ref cached) = self.cached_lines {
                // O(1) Arc clone - no data copying!
                return (
                    Arc::clone(cached),
                    self.cached_cursor,
                    self.cached_cursor_shown,
                );
            }
        }

        // === PHASE 1: Quick data copy under lock ===
        let (
            visible_buffer,
            scrollback_slice,
            cursor_pos,
            visible_rows,
            cols,
            cursor_visible,
            has_selection,
            selection_start,
            selection_end,
        ) = {
            let mut screen = self.screen.write().expect("Terminal screen lock poisoned");
            // Clear dirty flag since we're about to render
            screen.dirty = false;

            let visible_rows = screen.rows;
            let scroll_offset = screen.scroll_offset;
            let use_alt_screen = screen.use_alt_screen;

            // Determine what to copy based on scroll state
            let (visible_buffer, scrollback_slice) = if scroll_offset > 0 && !use_alt_screen {
                // Viewing history - need both scrollback and buffer data
                let total_scrollback = screen.scrollback.len();
                let total_lines = total_scrollback + visible_rows;
                let view_end = total_lines.saturating_sub(scroll_offset);
                let view_start = view_end.saturating_sub(visible_rows);

                // Copy only the visible portion
                let mut combined: Vec<Vec<Cell>> = Vec::with_capacity(visible_rows);
                for i in 0..visible_rows {
                    let source_idx = view_start + i;
                    if source_idx < total_scrollback {
                        combined.push(screen.scrollback[source_idx].clone());
                    } else {
                        let buf_idx = source_idx - total_scrollback;
                        if buf_idx < screen.active_buffer().len() {
                            combined.push(screen.active_buffer()[buf_idx].clone());
                        }
                    }
                }
                (combined, true)
            } else {
                // Normal view - copy active buffer
                (screen.active_buffer().iter().cloned().collect(), false)
            };

            (
                visible_buffer,
                scrollback_slice,
                screen.cursor,
                visible_rows,
                screen.cols,
                screen.cursor_visible,
                screen.selection_start.is_some() && screen.selection_end.is_some(),
                screen.selection_start,
                screen.selection_end,
            )
        };
        // Lock released here - PTY writer can proceed

        // === PHASE 2: Expensive rendering without lock ===
        let mut lines = Vec::with_capacity(visible_rows);
        let mut current_text = String::with_capacity(cols);

        // Don't show cursor when viewing history
        let show_cursor_now = if scrollback_slice {
            false
        } else {
            show_cursor && cursor_visible
        };

        // Pre-compute selection bounds if selection exists
        let selection_bounds = if has_selection {
            let (start, end) = (selection_start.unwrap(), selection_end.unwrap());
            let (start, end) = if start <= end {
                (start, end)
            } else {
                (end, start)
            };
            Some((start, end))
        } else {
            None
        };

        // Helper to check selection (inlined for performance)
        let is_in_selection = |row: usize, col: usize| -> bool {
            if let Some((start, end)) = selection_bounds {
                if row < start.0 || row > end.0 {
                    return false;
                }
                if row == start.0 && row == end.0 {
                    col >= start.1 && col <= end.1
                } else if row == start.0 {
                    col >= start.1
                } else if row == end.0 {
                    col <= end.1
                } else {
                    true
                }
            } else {
                false
            }
        };

        for (row_idx, row) in visible_buffer.iter().enumerate() {
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
                    style = Style::default().fg(Color::Black).bg(Color::LightYellow);
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

        // === PHASE 3: Cache the result (no clone - just wrap in Arc) ===
        let arc_lines = Arc::new(lines);
        self.cached_lines = Some(Arc::clone(&arc_lines));
        self.cached_cursor = cursor_pos;
        self.cached_cursor_shown = show_cursor_now;
        self.cached_focus = show_cursor;

        (arc_lines, cursor_pos, show_cursor_now)
    }

    /// Check if PTY has new data that needs rendering
    pub fn has_pending_output(&self) -> bool {
        self.has_new_data.swap(false, Ordering::AcqRel)
    }
}

impl Panel for Terminal {
    fn name(&self) -> &'static str {
        "terminal"
    }

    fn title(&self) -> String {
        self.terminal_title.clone()
    }

    fn prepare_render(&mut self, theme: &Theme, _config: &Config) {
        // Invalidate cache if theme changed
        if self.cached_theme != *theme {
            self.cached_lines = None;
        }
        self.cached_theme = *theme;
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
        let paragraph = Paragraph::new(lines);
        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Vec<PanelEvent> {
        // If process exited, don't handle input
        if !self.is_alive() {
            return vec![];
        }

        // Translate Cyrillic to Latin for hotkeys
        let key = termide_keyboard::translate_hotkey(key);

        // Handle paste from clipboard (Ctrl+Shift+V)
        // When Shift is pressed with a letter, crossterm returns the uppercase character
        // with only CONTROL in modifiers (Shift is "applied" to the character)
        match (key.code, key.modifiers) {
            (KeyCode::Char('V'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                let _ = self.paste_from_clipboard();
                return vec![];
            }
            _ => {}
        }

        // Handle history scrolling (Shift+PageUp/PageDown) - single lock per operation
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::PageUp => {
                    let mut screen = self.screen.write().expect("Terminal screen lock poisoned");
                    let scroll_amount = screen.rows.saturating_sub(1);
                    screen.scroll_view_up(scroll_amount);
                    return vec![];
                }
                KeyCode::PageDown => {
                    let mut screen = self.screen.write().expect("Terminal screen lock poisoned");
                    let scroll_amount = screen.rows.saturating_sub(1);
                    screen.scroll_view_down(scroll_amount);
                    return vec![];
                }
                KeyCode::Home => {
                    let mut screen = self.screen.write().expect("Terminal screen lock poisoned");
                    screen.scroll_offset = screen.scrollback.len();
                    return vec![];
                }
                KeyCode::End => {
                    self.screen
                        .write()
                        .expect("Terminal screen lock poisoned")
                        .reset_scroll();
                    return vec![];
                }
                _ => {}
            }
        }

        // Reset scroll on input and cache application_cursor_keys - single lock
        let application_cursor_keys = {
            let mut screen = self.screen.write().expect("Terminal screen lock poisoned");
            screen.reset_scroll();
            screen.application_cursor_keys
        };

        // Handle special keys
        match key.code {
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    // Ctrl+C, Ctrl+D, etc.
                    if c == 'c' {
                        let _ = self.send_input(&[3]); // Ctrl+C
                    } else if c == 'd' {
                        let _ = self.send_input(&[4]); // Ctrl+D
                    } else if c == 'z' {
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
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    // Shift+Enter sends CSI u sequence
                    let _ = self.send_input(b"\x1b[13;2u");
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

        // Check if selection is active
        let selection_active = {
            let screen = self.screen.read().expect("Terminal screen lock poisoned");
            screen.selection_start.is_some()
        };

        // If mouse is outside and selection is not active - ignore
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
                // Start text selection
                let mut screen = self.screen.write().expect("Terminal screen lock poisoned");
                screen.selection_start = Some((inner_row, inner_col));
                screen.selection_end = Some((inner_row, inner_col)); // Set immediately for visibility
                drop(screen);

                // Also send click to PTY if mouse tracking is enabled
                let _ = self.send_mouse_to_pty(&mouse, panel_area);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Update selection end (using clamped coordinates)
                let mut screen = self.screen.write().expect("Terminal screen lock poisoned");
                if screen.selection_start.is_some() {
                    screen.selection_end = Some((inner_row, inner_col));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // Finalize selection (using clamped coordinates)
                {
                    let mut screen = self.screen.write().expect("Terminal screen lock poisoned");
                    if screen.selection_start.is_some() {
                        screen.selection_end = Some((inner_row, inner_col));
                    }
                }

                // Copy selected text to CLIPBOARD
                let _ = self.copy_selection_to_clipboard();

                // Clear selection after copying
                {
                    let mut screen = self.screen.write().expect("Terminal screen lock poisoned");
                    screen.selection_start = None;
                    screen.selection_end = None;
                }

                // Send release to PTY if mouse tracking is enabled (only if inside)
                if is_inside {
                    let _ = self.send_mouse_to_pty(&mouse, panel_area);
                }
            }
            // Mouse wheel scrolling - for viewing history
            MouseEventKind::ScrollUp => {
                // On scroll up - show history
                self.screen
                    .write()
                    .expect("Terminal screen lock poisoned")
                    .scroll_view_up(3);
            }
            MouseEventKind::ScrollDown => {
                // On scroll down - return to current
                self.screen
                    .write()
                    .expect("Terminal screen lock poisoned")
                    .scroll_view_down(3);
            }
            // Other mouse events send to PTY
            _ => {
                let _ = self.send_mouse_to_pty(&mouse, panel_area);
            }
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
            | PanelCommand::RefreshDirectory => CommandResult::None,
        }
    }

    fn needs_close_confirmation(&self) -> Option<String> {
        // If process is alive and has child processes - request confirmation
        if self.is_alive() && self.has_running_processes() {
            Some("Kill running processes?".to_string())
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
        // Check if shell has child processes
        if let Some(pid) = self.shell_pid {
            // Read /proc/{pid}/task/{pid}/children
            let children_path = format!("/proc/{}/task/{}/children", pid, pid);
            if let Ok(children) = std::fs::read_to_string(&children_path) {
                return !children.trim().is_empty();
            }
        }
        false
    }

    fn kill_processes(&mut self) {
        if let Some(pid) = self.shell_pid {
            let pid = Pid::from_raw(pid as i32);

            // Send SIGTERM to process group
            let _ = signal::killpg(pid, Signal::SIGTERM);

            // Wait a bit
            std::thread::sleep(std::time::Duration::from_millis(100));

            // If process still alive - SIGKILL
            if self.is_alive() {
                let _ = signal::killpg(pid, Signal::SIGKILL);
            }

            // Wait for completion to avoid zombies
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
