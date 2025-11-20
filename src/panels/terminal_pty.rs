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
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use vte::{Params, Parser, Perform};

use super::Panel;
use crate::state::AppState;
use crate::panels::file_manager::DiskSpaceInfo;

/// Terminal information for status bar
pub struct TerminalInfo {
    pub user_host: String,   // user@host
    pub cwd: String,         // current directory
    pub disk_space: Option<DiskSpaceInfo>, // disk information
}

/// Mouse tracking mode
#[derive(Clone, Copy, PartialEq)]
enum MouseTrackingMode {
    None,
    Normal,      // ?1000 - clicks only
    ButtonEvent, // ?1002 - clicks + drag
    AnyEvent,    // ?1003 - all movements
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
    /// Virtual terminal screen
    screen: Arc<Mutex<TerminalScreen>>,
    /// Terminal size
    size: PtySize,
    /// Process activity flag
    is_alive: Arc<Mutex<bool>>,
    /// Terminal title (user@host:dir)
    terminal_title: String,
}

/// Terminal screen state
#[derive(Clone)]
struct TerminalScreen {
    /// Main line buffer
    lines: Vec<Vec<Cell>>,
    /// Alternate screen buffer (for TUI applications)
    alt_lines: Vec<Vec<Cell>>,
    /// Alternate screen usage flag
    use_alt_screen: bool,
    /// Cursor position (row, col)
    cursor: (usize, usize),
    /// Saved cursor position
    saved_cursor: Option<(usize, usize)>,
    /// Cursor visibility
    cursor_visible: bool,
    /// Screen dimensions
    rows: usize,
    cols: usize,
    /// Current style
    current_style: CellStyle,
    /// Insert mode
    insert_mode: bool,
    /// Application Cursor Keys Mode (DECCKM)
    application_cursor_keys: bool,
    /// Mouse tracking mode
    mouse_tracking: MouseTrackingMode,
    /// SGR extended mouse mode (?1006)
    sgr_mouse_mode: bool,
    /// Bracketed paste mode (?2004)
    bracketed_paste_mode: bool,
    /// Text selection start (row, col)
    selection_start: Option<(usize, usize)>,
    /// Text selection end (row, col)
    selection_end: Option<(usize, usize)>,
    /// History buffer (scrollback)
    scrollback: Vec<Vec<Cell>>,
    /// View offset (0 = current screen, >0 = viewing history)
    scroll_offset: usize,
    /// Maximum history size (number of lines)
    max_scrollback: usize,
    /// Deferred line wrap
    wrap_pending: bool,
}

/// Screen cell
#[derive(Clone, Debug)]
struct Cell {
    ch: char,
    style: CellStyle,
}

/// Cell style
#[derive(Clone, Debug, Copy)]
struct CellStyle {
    fg: Color,
    bg: Color,
    bold: bool,
    italic: bool,
    underline: bool,
    reverse: bool,
}

impl Default for CellStyle {
    fn default() -> Self {
        Self {
            fg: Color::White,
            bg: Color::Reset,  // Использовать фон темы по умолчанию
            bold: false,
            italic: false,
            underline: false,
            reverse: false,
        }
    }
}

impl TerminalScreen {
    fn new(rows: usize, cols: usize) -> Self {
        let empty_cell = Cell {
            ch: ' ',
            style: CellStyle::default(),
        };

        Self {
            lines: vec![vec![empty_cell.clone(); cols]; rows],
            alt_lines: vec![vec![empty_cell; cols]; rows],
            use_alt_screen: false,
            cursor: (0, 0),
            saved_cursor: None,
            cursor_visible: true,
            rows,
            cols,
            current_style: CellStyle::default(),
            insert_mode: false,
            application_cursor_keys: false,
            mouse_tracking: MouseTrackingMode::None,
            sgr_mouse_mode: false,
            bracketed_paste_mode: false,
            selection_start: None,
            selection_end: None,
            scrollback: Vec::new(),
            scroll_offset: 0,
            max_scrollback: 10000,
            wrap_pending: false,
        }
    }

    /// Get mutable reference to active buffer
    fn active_buffer_mut(&mut self) -> &mut Vec<Vec<Cell>> {
        if self.use_alt_screen {
            &mut self.alt_lines
        } else {
            &mut self.lines
        }
    }

    /// Get reference to active buffer
    fn active_buffer(&self) -> &Vec<Vec<Cell>> {
        if self.use_alt_screen {
            &self.alt_lines
        } else {
            &self.lines
        }
    }

    /// Switch to alternate screen
    fn switch_to_alt_screen(&mut self) {
        if !self.use_alt_screen {
            self.use_alt_screen = true;
            self.wrap_pending = false;
            // Clear alt buffer
            let empty_cell = Cell {
                ch: ' ',
                style: CellStyle::default(),
            };
            self.alt_lines = vec![vec![empty_cell; self.cols]; self.rows];
            self.cursor = (0, 0);
        }
    }

    /// Return to main screen
    fn switch_to_main_screen(&mut self) {
        if self.use_alt_screen {
            self.use_alt_screen = false;
            self.wrap_pending = false;
        }
    }

    /// Write character at current cursor position
    fn put_char(&mut self, ch: char) {
        // If there was a deferred wrap - execute it now
        if self.wrap_pending {
            self.wrap_pending = false;
            self.cursor.1 = 0;
            if self.cursor.0 + 1 >= self.rows {
                self.scroll_up();
            } else {
                self.cursor.0 += 1;
            }
        }

        let (row, col) = self.cursor;
        let cols = self.cols;
        let rows = self.rows;
        let style = self.current_style;

        if row < rows && col < cols {
            let buffer = self.active_buffer_mut();
            buffer[row][col] = Cell {
                ch,
                style,
            };
            // Move cursor right
            if col + 1 >= cols {
                // Reached last column - defer wrap
                self.wrap_pending = true;
            } else {
                self.cursor.1 = col + 1;
            }
        }
    }

    /// Newline
    fn newline(&mut self) {
        self.wrap_pending = false;
        self.cursor.1 = 0;
        if self.cursor.0 < self.rows - 1 {
            self.cursor.0 += 1;
        } else {
            // Scroll up
            self.scroll_up();
        }
    }

    /// Carriage return
    fn carriage_return(&mut self) {
        self.wrap_pending = false;
        self.cursor.1 = 0;
    }

    /// Scroll screen up one line
    fn scroll_up(&mut self) {
        let cols = self.cols;

        // For main buffer, save line to scrollback
        if !self.use_alt_screen {
            let top_line = self.lines[0].clone();
            self.scrollback.push(top_line);

            // Limit scrollback size
            if self.scrollback.len() > self.max_scrollback {
                self.scrollback.remove(0);
            }
        }

        let buffer = self.active_buffer_mut();
        buffer.remove(0);
        let empty_cell = Cell {
            ch: ' ',
            style: CellStyle::default(),
        };
        buffer.push(vec![empty_cell; cols]);
    }

    /// Scroll view up (into history)
    fn scroll_view_up(&mut self, lines: usize) {
        let max_offset = self.scrollback.len();
        self.scroll_offset = (self.scroll_offset + lines).min(max_offset);
    }

    /// Scroll view down (to current)
    fn scroll_view_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    /// Reset scroll to current screen
    fn reset_scroll(&mut self) {
        self.scroll_offset = 0;
    }

    /// Check if cell (row, col) is in current selection
    fn is_in_selection(&self, row: usize, col: usize) -> bool {
        let (start, end) = match (self.selection_start, self.selection_end) {
            (Some(s), Some(e)) => (s, e),
            _ => return false,
        };

        // Normalize: start should be before end
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };

        // Simple rectangular selection by lines
        // More correct: linear selection like in regular terminals
        if row < start.0 || row > end.0 {
            return false;
        }

        if row == start.0 && row == end.0 {
            // Single line
            col >= start.1 && col <= end.1
        } else if row == start.0 {
            // First line
            col >= start.1
        } else if row == end.0 {
            // Last line
            col <= end.1
        } else {
            // Middle lines - all selected
            true
        }
    }

    /// Clear screen (doesn't move cursor)
    fn clear_screen(&mut self) {
        let rows = self.rows;
        let cols = self.cols;
        let empty_cell = Cell {
            ch: ' ',
            style: CellStyle::default(),
        };
        let buffer = self.active_buffer_mut();
        *buffer = vec![vec![empty_cell; cols]; rows];
        // Cursor stays in place (standard ED 2 behavior)
    }

    /// Move cursor
    fn move_cursor(&mut self, row: usize, col: usize) {
        self.wrap_pending = false;
        self.cursor.0 = row.min(self.rows - 1);
        self.cursor.1 = col.min(self.cols - 1);
    }

    /// Backspace
    fn backspace(&mut self) {
        self.wrap_pending = false;
        if self.cursor.1 > 0 {
            self.cursor.1 -= 1;
        }
    }

    /// Tab
    fn tab(&mut self) {
        // Move cursor to next position divisible by 8
        let next_tab = ((self.cursor.1 / 8) + 1) * 8;
        self.cursor.1 = next_tab.min(self.cols - 1);
    }

    /// Save cursor position
    fn save_cursor(&mut self) {
        self.saved_cursor = Some(self.cursor);
    }

    /// Restore cursor position
    fn restore_cursor(&mut self) {
        if let Some(saved) = self.saved_cursor {
            self.cursor = saved;
            self.wrap_pending = false;
        }
    }
}

/// VT Performer for handling escape sequences
struct VtPerformer {
    screen: Arc<Mutex<TerminalScreen>>,
    pending_backslash: bool,
}

impl Perform for VtPerformer {
    fn print(&mut self, ch: char) {
        // Filter control characters that shouldn't be displayed
        // (except printable characters)
        if ch.is_control() && ch != '\t' && ch != '\n' && ch != '\r' {
            return;
        }

        // Handle bash readline markers \[ and \]
        if self.pending_backslash {
            self.pending_backslash = false;
            // If backslash is followed by [ or ], skip both characters
            if ch == '[' || ch == ']' {
                return;
            }
            // Otherwise print deferred backslash and current character
            if let Ok(mut screen) = self.screen.lock() {
                screen.put_char('\\');
                screen.put_char(ch);
            }
            return;
        }

        // If we encounter backslash, defer it
        if ch == '\\' {
            self.pending_backslash = true;
            return;
        }

        if let Ok(mut screen) = self.screen.lock() {
            screen.put_char(ch);
        }
    }

    fn execute(&mut self, byte: u8) {
        if let Ok(mut screen) = self.screen.lock() {
            match byte {
                b'\n' => screen.newline(),
                b'\r' => screen.carriage_return(),
                b'\x08' => screen.backspace(),
                b'\t' => screen.tab(),
                _ => {}
            }
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _c: char) {}

    fn put(&mut self, _byte: u8) {}

    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, c: char) {
        // Обработка приватных sequences (начинаются с '?')
        if !intermediates.is_empty() && intermediates[0] == b'?' {
            if let Ok(mut screen) = self.screen.lock() {
                // Получить номер приватной sequence
                let mode = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(0);

                match (mode, c) {
                    (1049, 'h') => {
                        // Переключиться на alternate screen и сохранить курсор
                        screen.saved_cursor = Some(screen.cursor);
                        screen.switch_to_alt_screen();
                    }
                    (1049, 'l') => {
                        // Вернуться к основному экрану и восстановить курсор
                        screen.switch_to_main_screen();
                        if let Some(saved) = screen.saved_cursor {
                            screen.cursor = saved;
                            screen.saved_cursor = None;
                        }
                    }
                    (47, 'h') => {
                        // Переключиться на alternate screen (без сохранения курсора)
                        screen.switch_to_alt_screen();
                    }
                    (47, 'l') => {
                        // Вернуться к основному экрану
                        screen.switch_to_main_screen();
                    }
                    (25, 'h') => {
                        // Показать курсор
                        screen.cursor_visible = true;
                    }
                    (25, 'l') => {
                        // Скрыть курсор
                        screen.cursor_visible = false;
                    }
                    (1, 'h') => {
                        // DECCKM - Application Cursor Keys Mode ON
                        screen.application_cursor_keys = true;
                    }
                    (1, 'l') => {
                        // DECCKM - Application Cursor Keys Mode OFF
                        screen.application_cursor_keys = false;
                    }
                    // Mouse tracking modes
                    (1000, 'h') => {
                        // Normal tracking mode ON
                        screen.mouse_tracking = MouseTrackingMode::Normal;
                    }
                    (1000, 'l') => {
                        // Normal tracking mode OFF
                        screen.mouse_tracking = MouseTrackingMode::None;
                    }
                    (1002, 'h') => {
                        // Button event tracking mode ON
                        screen.mouse_tracking = MouseTrackingMode::ButtonEvent;
                    }
                    (1002, 'l') => {
                        // Button event tracking mode OFF
                        screen.mouse_tracking = MouseTrackingMode::None;
                    }
                    (1003, 'h') => {
                        // Any event tracking mode ON
                        screen.mouse_tracking = MouseTrackingMode::AnyEvent;
                    }
                    (1003, 'l') => {
                        // Any event tracking mode OFF
                        screen.mouse_tracking = MouseTrackingMode::None;
                    }
                    (1006, 'h') => {
                        // SGR extended mouse mode ON
                        screen.sgr_mouse_mode = true;
                    }
                    (1006, 'l') => {
                        // SGR extended mouse mode OFF
                        screen.sgr_mouse_mode = false;
                    }
                    (2004, 'h') => {
                        // Bracketed paste mode ON
                        screen.bracketed_paste_mode = true;
                    }
                    (2004, 'l') => {
                        // Bracketed paste mode OFF
                        screen.bracketed_paste_mode = false;
                    }
                    _ => {
                        // Другие приватные sequences игнорируем
                    }
                }
            }
            return;
        }

        // Игнорировать другие intermediate bytes
        if !intermediates.is_empty() {
            return;
        }

        if let Ok(mut screen) = self.screen.lock() {
            match c {
                'H' | 'f' => {
                    // Переместить курсор
                    let row = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    let col = params.iter().nth(1).and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    screen.move_cursor(row.saturating_sub(1), col.saturating_sub(1));
                }
                'J' => {
                    // ED - Erase in Display
                    let param = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(0);
                    let (row, col) = screen.cursor;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    match param {
                        0 => {
                            // Clear from cursor to end of screen
                            let buffer = screen.active_buffer_mut();
                            let buf_rows = buffer.len();

                            // Clear rest of current line
                            if row < buf_rows {
                                let buf_cols = buffer[row].len();
                                for i in col..buf_cols {
                                    buffer[row][i] = empty_cell.clone();
                                }
                            }
                            // Clear all lines below
                            for r in (row + 1)..buf_rows {
                                let buf_cols = buffer[r].len();
                                for c in 0..buf_cols {
                                    buffer[r][c] = empty_cell.clone();
                                }
                            }
                        }
                        1 => {
                            // Clear from start of screen to cursor
                            let buffer = screen.active_buffer_mut();
                            let buf_rows = buffer.len();

                            // Clear all lines above
                            for r in 0..row.min(buf_rows) {
                                let buf_cols = buffer[r].len();
                                for c in 0..buf_cols {
                                    buffer[r][c] = empty_cell.clone();
                                }
                            }
                            // Clear current line up to and including cursor
                            if row < buf_rows {
                                let buf_cols = buffer[row].len();
                                for i in 0..=col.min(buf_cols.saturating_sub(1)) {
                                    buffer[row][i] = empty_cell.clone();
                                }
                            }
                        }
                        2 => {
                            // Clear entire screen and move cursor to (0,0)
                            let buffer = screen.active_buffer_mut();
                            for r in 0..buffer.len() {
                                for c in 0..buffer[r].len() {
                                    buffer[r][c] = empty_cell.clone();
                                }
                            }
                            // Move cursor to home position (compatibility with old behavior)
                            screen.cursor = (0, 0);
                        }
                        3 => {
                            // Clear entire screen and scrollback
                            let is_alt = screen.use_alt_screen;
                            let buffer = screen.active_buffer_mut();
                            for r in 0..buffer.len() {
                                for c in 0..buffer[r].len() {
                                    buffer[r][c] = empty_cell.clone();
                                }
                            }
                            // Clear scrollback only for main screen
                            if !is_alt {
                                screen.scrollback.clear();
                            }
                            screen.cursor = (0, 0);
                        }
                        _ => {}
                    }
                }
                'K' => {
                    // EL - Erase in Line
                    let param = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(0);
                    let (row, col) = screen.cursor;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    let buffer = screen.active_buffer_mut();
                    if row < buffer.len() {
                        let buf_cols = buffer[row].len();
                        match param {
                            0 => {
                                // От курсора до конца строки
                                for i in col..buf_cols {
                                    buffer[row][i] = empty_cell.clone();
                                }
                            }
                            1 => {
                                // От начала строки до курсора (включительно)
                                for i in 0..=col.min(buf_cols.saturating_sub(1)) {
                                    buffer[row][i] = empty_cell.clone();
                                }
                            }
                            2 => {
                                // Вся строка
                                for i in 0..buf_cols {
                                    buffer[row][i] = empty_cell.clone();
                                }
                            }
                            _ => {}
                        }
                    }
                }
                'P' => {
                    // DCH - Delete Character
                    let n = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    let (row, col) = screen.cursor;
                    let cols = screen.cols;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    let buffer = screen.active_buffer_mut();
                    // Сдвинуть символы слева от удаленного
                    for i in col..(cols - n) {
                        buffer[row][i] = buffer[row][i + n].clone();
                    }

                    // Заполнить освободившееся место пробелами
                    for i in (cols - n)..cols {
                        buffer[row][i] = empty_cell.clone();
                    }
                }
                'X' => {
                    // ECH - Erase Character
                    let n = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    let (row, col) = screen.cursor;
                    let cols = screen.cols;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    let buffer = screen.active_buffer_mut();
                    for i in col..(col + n).min(cols) {
                        buffer[row][i] = empty_cell.clone();
                    }
                }
                '@' => {
                    // ICH - Insert Character (сдвинуть символы вправо)
                    let n = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    let (row, col) = screen.cursor;
                    let cols = screen.cols;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    let buffer = screen.active_buffer_mut();
                    // Сдвинуть символы вправо
                    if col + n < cols {
                        for i in (col + n..cols).rev() {
                            buffer[row][i] = buffer[row][i - n].clone();
                        }
                    }

                    // Вставить пробелы на освободившиеся места
                    for i in col..(col + n).min(cols) {
                        buffer[row][i] = empty_cell.clone();
                    }
                }
                'L' => {
                    // IL - Insert Lines (вставить пустые строки)
                    let n = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    let row = screen.cursor.0;
                    let cols = screen.cols;
                    let rows = screen.rows;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    let buffer = screen.active_buffer_mut();
                    if row < buffer.len() {
                        // Удалить n строк снизу
                        for _ in 0..n.min(rows - row) {
                            if buffer.len() > row {
                                buffer.pop();
                            }
                        }
                        // Вставить n пустых строк на позиции курсора
                        for _ in 0..n.min(rows - row) {
                            buffer.insert(row, vec![empty_cell.clone(); cols]);
                        }
                    }
                }
                'M' => {
                    // DL - Delete Lines (удалить строки)
                    let n = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    let row = screen.cursor.0;
                    let cols = screen.cols;
                    let rows = screen.rows;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    let buffer = screen.active_buffer_mut();
                    if row < buffer.len() {
                        // Удалить n строк на позиции курсора
                        for _ in 0..n.min(buffer.len() - row) {
                            if row < buffer.len() {
                                buffer.remove(row);
                            }
                        }
                        // Добавить n пустых строк внизу
                        while buffer.len() < rows {
                            buffer.push(vec![empty_cell.clone(); cols]);
                        }
                    }
                }
                'S' => {
                    // SU - Scroll Up (прокрутить экран вверх)
                    let n = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    let cols = screen.cols;
                    let rows = screen.rows;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    let buffer = screen.active_buffer_mut();
                    for _ in 0..n.min(rows) {
                        if !buffer.is_empty() {
                            buffer.remove(0);
                        }
                        buffer.push(vec![empty_cell.clone(); cols]);
                    }
                }
                'T' => {
                    // SD - Scroll Down (прокрутить экран вниз)
                    let n = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    let cols = screen.cols;
                    let rows = screen.rows;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    let buffer = screen.active_buffer_mut();
                    for _ in 0..n.min(rows) {
                        if buffer.len() >= rows {
                            buffer.pop();
                        }
                        buffer.insert(0, vec![empty_cell.clone(); cols]);
                    }
                }
                'A' => {
                    // Курсор вверх
                    let n = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.0 = screen.cursor.0.saturating_sub(n);
                }
                'B' => {
                    // Курсор вниз
                    let n = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.0 = (screen.cursor.0 + n).min(screen.rows - 1);
                }
                'C' => {
                    // Курсор вправо
                    let n = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.1 = (screen.cursor.1 + n).min(screen.cols - 1);
                }
                'D' => {
                    // Курсор влево
                    let n = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.1 = screen.cursor.1.saturating_sub(n);
                }
                'E' => {
                    // CNL - Cursor Next Line
                    let n = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.0 = (screen.cursor.0 + n).min(screen.rows - 1);
                    screen.cursor.1 = 0;
                }
                'F' => {
                    // CPL - Cursor Previous Line
                    let n = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.0 = screen.cursor.0.saturating_sub(n);
                    screen.cursor.1 = 0;
                }
                'G' => {
                    // CHA - Cursor Horizontal Absolute
                    let col = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.1 = col.saturating_sub(1).min(screen.cols - 1);
                }
                'd' => {
                    // VPA - Vertical Position Absolute
                    let row = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.0 = row.saturating_sub(1).min(screen.rows - 1);
                }
                'm' => {
                    // SGR - установить стиль
                    // Собрать все параметры в один вектор для обработки 38;5;N и 48;5;N
                    let all_params: Vec<u16> = params.iter().flat_map(|p| p.iter().copied()).collect();
                    let mut i = 0;
                    while i < all_params.len() {
                        let p = all_params[i];
                        match p {
                            0 => screen.current_style = CellStyle::default(),
                            1 => screen.current_style.bold = true,
                            3 => screen.current_style.italic = true,
                            4 => screen.current_style.underline = true,
                            7 => screen.current_style.reverse = true,
                            22 => screen.current_style.bold = false,
                            23 => screen.current_style.italic = false,
                            24 => screen.current_style.underline = false,
                            27 => screen.current_style.reverse = false,
                            30..=37 => {
                                screen.current_style.fg = ansi_to_color(p - 30);
                            }
                            38 => {
                                // 256-цветный или RGB foreground
                                if i + 2 < all_params.len() && all_params[i + 1] == 5 {
                                    // 38;5;N - 256-цветный
                                    let color_idx = all_params[i + 2];
                                    screen.current_style.fg = ansi_256_to_color(color_idx);
                                    i += 2;
                                } else if i + 4 < all_params.len() && all_params[i + 1] == 2 {
                                    // 38;2;R;G;B - True Color (24-bit)
                                    let r = all_params[i + 2] as u8;
                                    let g = all_params[i + 3] as u8;
                                    let b = all_params[i + 4] as u8;
                                    screen.current_style.fg = Color::Rgb(r, g, b);
                                    i += 4;
                                }
                            }
                            39 => {
                                // Сброс foreground на default
                                screen.current_style.fg = Color::Reset;
                            }
                            40..=47 => {
                                screen.current_style.bg = ansi_to_color(p - 40);
                            }
                            48 => {
                                // 256-цветный или RGB background
                                if i + 2 < all_params.len() && all_params[i + 1] == 5 {
                                    // 48;5;N - 256-цветный
                                    let color_idx = all_params[i + 2];
                                    screen.current_style.bg = ansi_256_to_color(color_idx);
                                    i += 2;
                                } else if i + 4 < all_params.len() && all_params[i + 1] == 2 {
                                    // 48;2;R;G;B - True Color (24-bit)
                                    let r = all_params[i + 2] as u8;
                                    let g = all_params[i + 3] as u8;
                                    let b = all_params[i + 4] as u8;
                                    screen.current_style.bg = Color::Rgb(r, g, b);
                                    i += 4;
                                }
                            }
                            49 => {
                                // Сброс background на default
                                screen.current_style.bg = Color::Reset;
                            }
                            90..=97 => {
                                screen.current_style.fg = ansi_to_bright_color(p - 90);
                            }
                            100..=107 => {
                                screen.current_style.bg = ansi_to_bright_color(p - 100);
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                }
                's' => {
                    // Сохранить позицию курсора
                    screen.save_cursor();
                }
                'u' => {
                    // Восстановить позицию курсора
                    screen.restore_cursor();
                }
                'r' => {
                    // DECSTBM - Set scrolling region (игнорируем, но не ломаемся)
                }
                'l' | 'h' => {
                    // Set/Reset Mode (игнорируем, но не ломаемся)
                }
                _ => {}
            }
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {}
}

/// Convert ANSI color to ratatui Color
fn ansi_to_color(code: u16) -> Color {
    match code {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::White,
        _ => Color::White,
    }
}

/// Convert bright ANSI color to ratatui Color
fn ansi_to_bright_color(code: u16) -> Color {
    match code {
        0 => Color::DarkGray,
        1 => Color::LightRed,
        2 => Color::LightGreen,
        3 => Color::LightYellow,
        4 => Color::LightBlue,
        5 => Color::LightMagenta,
        6 => Color::LightCyan,
        7 => Color::White,
        _ => Color::White,
    }
}

/// Convert 256-color index to ratatui Color
fn ansi_256_to_color(code: u16) -> Color {
    match code {
        // Basic 16 colors (0-15)
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::White,
        8 => Color::DarkGray,
        9 => Color::LightRed,
        10 => Color::LightGreen,
        11 => Color::LightYellow,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        14 => Color::LightCyan,
        15 => Color::White,
        // 216 colors (6x6x6 cube) - indices 16-231
        16..=231 => {
            let idx = code - 16;
            let r = (idx / 36) as u8;
            let g = ((idx % 36) / 6) as u8;
            let b = (idx % 6) as u8;
            // Convert 0-5 to 0-255
            let r = if r == 0 { 0 } else { 55 + r * 40 };
            let g = if g == 0 { 0 } else { 55 + g * 40 };
            let b = if b == 0 { 0 } else { 55 + b * 40 };
            Color::Rgb(r, g, b)
        }
        // 24 grayscale gradations (232-255)
        232..=255 => {
            let gray = 8 + (code - 232) as u8 * 10;
            Color::Rgb(gray, gray, gray)
        }
        _ => Color::White,
    }
}

impl Terminal {
    /// Create new terminal with PTY
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
        let working_dir = cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| "/".into()));
        cmd.cwd(&working_dir);

        // Set environment variables for correct readline and escape sequence behavior
        cmd.env("TERM", "xterm-256color");
        cmd.env("SHELL", &shell);
        cmd.env("HOME", std::env::var("HOME").unwrap_or_else(|_| "/".to_string()));
        cmd.env("USER", std::env::var("USER").unwrap_or_else(|_| "user".to_string()));
        cmd.env("LANG", std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".to_string()));
        if let Ok(lc_all) = std::env::var("LC_ALL") {
            cmd.env("LC_ALL", lc_all);
        }
        cmd.env("PWD", working_dir.display().to_string());
        // PATH is critical for NixOS - without it bash-interactive won't be found
        cmd.env("PATH", std::env::var("PATH").unwrap_or_else(|_|
            "/run/current-system/sw/bin:/usr/bin:/bin".to_string()
        ));

        let child = pair.slave.spawn_command(cmd)?;
        let shell_pid = child.process_id();

        let screen = Arc::new(Mutex::new(TerminalScreen::new(
            rows as usize,
            cols as usize,
        )));

        // Create reader and writer BEFORE placing PTY in Arc<Mutex>
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        let pty = Arc::new(Mutex::new(pair.master));
        let is_alive = Arc::new(Mutex::new(true));

        // Start thread for reading from PTY
        let screen_clone = Arc::clone(&screen);
        let is_alive_clone = Arc::clone(&is_alive);
        thread::spawn(move || {
            let mut parser = Parser::new();
            let mut buf = [0u8; 4096];

            // Temporary log for debugging
            let mut log_file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/termide_pty.log")
                .ok();

            loop {
                match reader.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        // Log raw data
                        if let Some(ref mut log) = log_file {
                            use std::io::Write;
                            let _ = writeln!(log, "RAW [{} bytes]: {:?}", n, &buf[..n]);
                            let _ = writeln!(log, "STR: {:?}", String::from_utf8_lossy(&buf[..n]));
                        }

                        let mut performer = VtPerformer {
                            screen: Arc::clone(&screen_clone),
                            pending_backslash: false,
                        };

                        for byte in &buf[..n] {
                            parser.advance(&mut performer, *byte);
                        }
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
        })
    }

    /// Определить доступный shell
    fn detect_shell() -> String {
        // На NixOS сначала проверяем bash-interactive в системном профиле
        // (обычный bash в nix store может быть без readline)
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

        // Затем проверяем $SHELL
        if let Ok(shell) = std::env::var("SHELL") {
            if std::path::Path::new(&shell).exists() {
                return shell;
            }
        }

        // Проверяем популярные шеллы на обычных системах
        let shells = ["/usr/bin/fish", "/usr/bin/zsh", "/bin/bash", "/bin/sh"];
        for shell in shells {
            if std::path::Path::new(shell).exists() {
                return shell.to_string();
            }
        }

        "/bin/sh".to_string()
    }

    /// Получить аргументы для запуска шелла
    fn get_shell_args(shell_path: &str) -> Vec<&'static str> {
        let shell_name = std::path::Path::new(shell_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        match shell_name {
            "fish" => vec!["-l"],       // login shell
            "zsh" => vec!["-l", "-i"],  // login + interactive
            "bash" => vec![],           // PTY сделает интерактивным автоматически
            _ => vec![],                // без аргументов
        }
    }

    /// Изменить размер терминала
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

        // Обновить размер виртуального экрана
        if let Ok(mut screen) = self.screen.lock() {
            let new_rows = rows as usize;
            let new_cols = cols as usize;

            // Если размер изменился, создаем новый экран с сохранением содержимого
            if screen.rows != new_rows || screen.cols != new_cols {
                let old_lines = screen.lines.clone();
                let empty_cell = Cell {
                    ch: ' ',
                    style: CellStyle::default(),
                };

                // Создать новый буфер нужного размера
                let mut new_lines = vec![vec![empty_cell.clone(); new_cols]; new_rows];

                // Скопировать старое содержимое
                for (i, old_line) in old_lines.iter().enumerate() {
                    if i >= new_rows {
                        break;
                    }
                    for (j, cell) in old_line.iter().enumerate() {
                        if j >= new_cols {
                            break;
                        }
                        new_lines[i][j] = cell.clone();
                    }
                }

                screen.lines = new_lines;
                screen.rows = new_rows;
                screen.cols = new_cols;

                // Ограничить позицию курсора новыми размерами
                screen.cursor.0 = screen.cursor.0.min(new_rows.saturating_sub(1));
                screen.cursor.1 = screen.cursor.1.min(new_cols.saturating_sub(1));
            }
        }

        Ok(())
    }

    /// Проверить, жив ли PTY процесс
    pub fn is_alive(&self) -> bool {
        self.is_alive.lock().map(|alive| *alive).unwrap_or(false)
    }

    /// Получить информацию о терминале для статусной строки
    pub fn get_terminal_info(&self) -> TerminalInfo {
        // Получить user@host
        let username = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("HOST"))
            .unwrap_or_else(|_| {
                // Попытаться получить hostname через gethostname
                let mut buf = [0u8; 256];
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

        // Получить текущую директорию (используем переменную окружения)
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "~".to_string());

        // Получить информацию о диске для текущей директории
        let disk_space = self.get_disk_space_for_path(&cwd);

        TerminalInfo {
            user_host,
            cwd,
            disk_space,
        }
    }

    /// Получить информацию о дисковом пространстве для указанного пути
    fn get_disk_space_for_path(&self, path: &str) -> Option<DiskSpaceInfo> {
        use std::ffi::CString;

        let path_cstr = CString::new(path).ok()?;

        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(path_cstr.as_ptr(), &mut stat) == 0 {
                let available = (stat.f_bavail as u64) * (stat.f_bsize as u64);
                let total = (stat.f_blocks as u64) * (stat.f_bsize as u64);

                Some(DiskSpaceInfo {
                    available,
                    total,
                })
            } else {
                None
            }
        }
    }

    /// Отправить ввод в PTY
    fn send_input(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Получить выделенный текст
    fn get_selected_text(&self) -> String {
        let screen = self.screen.lock().unwrap();
        let (start, end) = match (screen.selection_start, screen.selection_end) {
            (Some(s), Some(e)) => (s, e),
            _ => return String::new(),
        };

        // Нормализовать
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
            let col_end = if row_idx == end.0 { end.1.min(row.len().saturating_sub(1)) } else { row.len().saturating_sub(1) };

            for col_idx in col_start..=col_end {
                if col_idx < row.len() {
                    let ch = row[col_idx].ch;
                    if ch != '\0' {
                        result.push(ch);
                    }
                }
            }

            // Добавить перенос строки между строками (но не в конце)
            if row_idx < end.0 {
                result.push('\n');
            }
        }

        // Обрезать trailing whitespace с каждой строки
        result.lines()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Копировать выделенный текст в буфер обмена
    fn copy_selection_to_clipboard(&self) -> Result<()> {
        let text = self.get_selected_text();
        if text.is_empty() {
            return Ok(());
        }

        // Используем универсальный буфер (включает OSC 52)
        crate::clipboard::copy(text);

        Ok(())
    }

    /// Отправить событие мыши в PTY (если mouse tracking включён)
    fn send_mouse_to_pty(&mut self, mouse: &crossterm::event::MouseEvent, panel_area: Rect) -> Result<()> {
        use crossterm::event::{MouseEventKind, MouseButton};

        let (mouse_tracking, sgr_mode) = {
            let screen = self.screen.lock().unwrap();
            (screen.mouse_tracking, screen.sgr_mouse_mode)
        };

        // Если mouse tracking отключён, не отправлять
        if mouse_tracking == MouseTrackingMode::None {
            return Ok(());
        }

        // 1-based координаты для SGR
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
                    let encoded_x = (inner_x as u8).saturating_add(32).min(255);
                    let encoded_y = (inner_y as u8).saturating_add(32).min(255);
                    format!("\x1b[M{}{}{}", encoded_btn as char, encoded_x as char, encoded_y as char)
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
                    let encoded_x = (inner_x as u8).saturating_add(32).min(255);
                    let encoded_y = (inner_y as u8).saturating_add(32).min(255);
                    format!("\x1b[M{}{}{}", (3 + 32) as u8 as char, encoded_x as char, encoded_y as char)
                }
            }
            MouseEventKind::ScrollUp => {
                let btn_code = 64;
                if sgr_mode {
                    format!("\x1b[<{};{};{}M", btn_code, inner_x, inner_y)
                } else {
                    let encoded_x = (inner_x as u8).saturating_add(32).min(255);
                    let encoded_y = (inner_y as u8).saturating_add(32).min(255);
                    format!("\x1b[M{}{}{}", (btn_code + 32) as u8 as char, encoded_x as char, encoded_y as char)
                }
            }
            MouseEventKind::ScrollDown => {
                let btn_code = 65;
                if sgr_mode {
                    format!("\x1b[<{};{};{}M", btn_code, inner_x, inner_y)
                } else {
                    let encoded_x = (inner_x as u8).saturating_add(32).min(255);
                    let encoded_y = (inner_y as u8).saturating_add(32).min(255);
                    format!("\x1b[M{}{}{}", (btn_code + 32) as u8 as char, encoded_x as char, encoded_y as char)
                }
            }
            _ => return Ok(()),
        };

        self.send_input(sequence.as_bytes())?;
        Ok(())
    }

    /// Получить строки для отображения
    fn get_display_lines(&self, show_cursor: bool) -> Vec<Line> {
        let screen = self.screen.lock().unwrap();
        let mut lines = Vec::new();
        let buffer = screen.active_buffer();
        let cursor_pos = screen.cursor;

        // Если есть смещение просмотра и мы на основном экране, показываем историю
        let (display_buffer, actual_cursor_pos, show_cursor_now) = if screen.scroll_offset > 0 && !screen.use_alt_screen {
            // Собираем виртуальный буфер: scrollback + текущий экран
            let total_scrollback = screen.scrollback.len();
            let visible_rows = screen.rows;

            // Вычисляем начальную позицию в общей истории
            // scroll_offset=1 означает что мы на 1 строку выше текущего экрана
            let total_lines = total_scrollback + visible_rows;
            let view_end = total_lines.saturating_sub(screen.scroll_offset);
            let view_start = view_end.saturating_sub(visible_rows);

            // Создаем временный буфер для отображения
            let mut temp_buffer = Vec::with_capacity(visible_rows);
            for i in view_start..view_end {
                if i < total_scrollback {
                    // Строка из scrollback
                    temp_buffer.push(screen.scrollback[i].clone());
                } else {
                    // Строка из текущего буфера
                    let buf_idx = i - total_scrollback;
                    if buf_idx < buffer.len() {
                        temp_buffer.push(buffer[buf_idx].clone());
                    }
                }
            }

            // Курсор не показываем при просмотре истории
            (temp_buffer, cursor_pos, false)
        } else {
            (buffer.clone(), cursor_pos, show_cursor)
        };

        for (row_idx, row) in display_buffer.iter().enumerate() {
            let mut spans = Vec::new();
            let mut current_text = String::new();
            let mut current_style = None;

            for (col_idx, cell) in row.iter().enumerate() {
                // Применить reverse если установлен
                let (fg, bg) = if cell.style.reverse {
                    (cell.style.bg, cell.style.fg)
                } else {
                    (cell.style.fg, cell.style.bg)
                };
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

                // Проверить, попадает ли ячейка в выделение
                let is_selected = screen.is_in_selection(row_idx, col_idx);
                if is_selected {
                    // Яркий контрастный цвет для выделения
                    style = Style::default()
                        .fg(Color::Black)
                        .bg(Color::LightYellow);
                }

                // Если это позиция курсора и нужно его показывать, инвертировать стиль (реверс видео)
                if show_cursor_now && row_idx == actual_cursor_pos.0 && col_idx == actual_cursor_pos.1 {
                    // Сохранить текущий накопленный текст
                    if !current_text.is_empty() {
                        spans.push(Span::styled(current_text.clone(), current_style.unwrap()));
                        current_text.clear();
                        current_style = None;
                    }

                    // Поменять fg и bg местами для курсора (классическая инверсия)
                    let cursor_fg = match bg {
                        Color::Reset => Color::Black,  // default bg -> black text
                        c => c                         // bg становится fg
                    };
                    let cursor_bg = match fg {
                        Color::Reset => Color::White,  // default fg -> white bg
                        c => c                         // fg становится bg
                    };
                    let cursor_style = Style::default()
                        .bg(cursor_bg)
                        .fg(cursor_fg);
                    let cursor_char = if cell.ch == ' ' || cell.ch == '\0' {
                        ' '
                    } else {
                        cell.ch
                    };
                    spans.push(Span::styled(cursor_char.to_string(), cursor_style));
                    continue;
                }

                // Группировать символы с одинаковым стилем
                if current_style.is_none() || current_style == Some(style) {
                    current_text.push(cell.ch);
                    current_style = Some(style);
                } else {
                    if !current_text.is_empty() {
                        spans.push(Span::styled(current_text.clone(), current_style.unwrap()));
                    }
                    current_text.clear();
                    current_text.push(cell.ch);
                    current_style = Some(style);
                }
            }

            // Добавить последний span
            if !current_text.is_empty() {
                spans.push(Span::styled(current_text, current_style.unwrap()));
            }

            // Если строка пустая, курсор на ней и нужно его показывать, добавить курсор
            if show_cursor_now && spans.is_empty() && row_idx == actual_cursor_pos.0 {
                let cursor_style = Style::default()
                    .bg(Color::White);
                spans.push(Span::styled(" ", cursor_style));
            }

            lines.push(Line::from(spans));
        }

        lines
    }
}

impl Panel for Terminal {
    fn render(&mut self, area: Rect, buf: &mut Buffer, is_focused: bool, panel_index: usize, state: &AppState) {
        // Обновить размер если изменился
        let new_rows = area.height.saturating_sub(2);
        let new_cols = area.width.saturating_sub(2);

        if new_rows != self.size.rows || new_cols != self.size.cols {
            let _ = self.resize(new_rows, new_cols);
        }

        // Данные читаются в отдельном потоке, просто отрисовываем текущее состояние
        // Курсор показываем только когда панель в фокусе
        let display_lines = self.get_display_lines(is_focused);

        // Создать заголовок панели с учетом состояния процесса
        let panel_title = if self.is_alive() {
            self.terminal_title.clone()
        } else {
            format!("{} [Process Exited - Press Tab to switch panel]", self.terminal_title)
        };

        let block = crate::ui::panel_helpers::create_panel_block(&panel_title, is_focused, panel_index, state);
        let inner = block.inner(area);

        let paragraph = Paragraph::new(display_lines).block(block);
        paragraph.render(area, buf);

        // Заменить Color::Reset и Color::White на цвета темы
        for y in inner.top()..inner.bottom() {
            for x in inner.left()..inner.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    let current_style = cell.style();
                    let mut new_style = current_style;

                    // Заменить фон Reset на фон темы
                    if current_style.bg == Some(Color::Reset) || current_style.bg.is_none() {
                        new_style.bg = Some(state.theme.background);
                    }

                    // Заменить текст White на текст темы
                    if current_style.fg == Some(Color::White) || current_style.fg.is_none() {
                        new_style.fg = Some(state.theme.text_primary);
                    }

                    cell.set_style(new_style);
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Если процесс завершился, не обрабатывать input
        if !self.is_alive() {
            return Ok(());
        }

        // Вставка обрабатывается родительским терминалом (Ctrl+Shift+V, Shift+Insert)

        // Обработать прокрутку истории (Shift+PageUp/PageDown)
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::PageUp => {
                    let rows = self.screen.lock().unwrap().rows;
                    self.screen.lock().unwrap().scroll_view_up(rows.saturating_sub(1));
                    return Ok(());
                }
                KeyCode::PageDown => {
                    let rows = self.screen.lock().unwrap().rows;
                    self.screen.lock().unwrap().scroll_view_down(rows.saturating_sub(1));
                    return Ok(());
                }
                KeyCode::Home => {
                    let scrollback_len = self.screen.lock().unwrap().scrollback.len();
                    self.screen.lock().unwrap().scroll_offset = scrollback_len;
                    return Ok(());
                }
                KeyCode::End => {
                    self.screen.lock().unwrap().reset_scroll();
                    return Ok(());
                }
                _ => {}
            }
        }

        // Сбросить прокрутку при вводе
        self.screen.lock().unwrap().reset_scroll();

        // Обработать специальные клавиши
        match key.code {
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    // Ctrl+C, Ctrl+D и т.д.
                    if c == 'c' {
                        self.send_input(&[3])?; // Ctrl+C
                    } else if c == 'd' {
                        self.send_input(&[4])?; // Ctrl+D
                    } else if c == 'z' {
                        self.send_input(&[26])?; // Ctrl+Z
                    } else {
                        // Другие Ctrl комбинации
                        let ctrl_char = (c as u8) & 0x1f;
                        self.send_input(&[ctrl_char])?;
                    }
                } else {
                    // Обычный символ
                    let mut buf = [0u8; 4];
                    let s = c.encode_utf8(&mut buf);
                    self.send_input(s.as_bytes())?;
                }
            }
            KeyCode::Enter => {
                self.send_input(b"\r")?;
            }
            KeyCode::Backspace => {
                self.send_input(&[127])?; // DEL
            }
            KeyCode::Delete => {
                self.send_input(b"\x1b[3~")?;
            }
            KeyCode::Left => {
                // В Application Cursor Keys Mode отправляем \x1bO вместо \x1b[
                if self.screen.lock().unwrap().application_cursor_keys {
                    self.send_input(b"\x1bOD")?;
                } else {
                    self.send_input(b"\x1b[D")?;
                }
            }
            KeyCode::Right => {
                if self.screen.lock().unwrap().application_cursor_keys {
                    self.send_input(b"\x1bOC")?;
                } else {
                    self.send_input(b"\x1b[C")?;
                }
            }
            KeyCode::Up => {
                if self.screen.lock().unwrap().application_cursor_keys {
                    self.send_input(b"\x1bOA")?;
                } else {
                    self.send_input(b"\x1b[A")?;
                }
            }
            KeyCode::Down => {
                if self.screen.lock().unwrap().application_cursor_keys {
                    self.send_input(b"\x1bOB")?;
                } else {
                    self.send_input(b"\x1b[B")?;
                }
            }
            KeyCode::Home => {
                // В Application Cursor Keys Mode отправляем \x1bO вместо \x1b[
                if self.screen.lock().unwrap().application_cursor_keys {
                    self.send_input(b"\x1bOH")?;
                } else {
                    self.send_input(b"\x1b[H")?;
                }
            }
            KeyCode::End => {
                if self.screen.lock().unwrap().application_cursor_keys {
                    self.send_input(b"\x1bOF")?;
                } else {
                    self.send_input(b"\x1b[F")?;
                }
            }
            KeyCode::PageUp => {
                self.send_input(b"\x1b[5~")?;
            }
            KeyCode::PageDown => {
                self.send_input(b"\x1b[6~")?;
            }
            KeyCode::Tab => {
                self.send_input(b"\t")?;
            }
            KeyCode::Esc => {
                self.send_input(b"\x1b")?;
            }
            KeyCode::F(n) => {
                // F-клавиши для xterm-256color
                match n {
                    1 => self.send_input(b"\x1bOP")?,
                    2 => self.send_input(b"\x1bOQ")?,
                    3 => self.send_input(b"\x1bOR")?,
                    4 => self.send_input(b"\x1bOS")?,
                    5 => self.send_input(b"\x1b[15~")?,
                    6 => self.send_input(b"\x1b[17~")?,
                    7 => self.send_input(b"\x1b[18~")?,
                    8 => self.send_input(b"\x1b[19~")?,
                    9 => self.send_input(b"\x1b[20~")?,
                    10 => self.send_input(b"\x1b[21~")?,
                    11 => self.send_input(b"\x1b[23~")?,
                    12 => self.send_input(b"\x1b[24~")?,
                    _ => {}
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn title(&self) -> String {
        self.terminal_title.clone()
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent, panel_area: Rect) -> Result<()> {
        use crossterm::event::{MouseEventKind, MouseButton};

        // Если процесс завершился, не обрабатывать мышь
        if !self.is_alive() {
            return Ok(());
        }

        // Вычислить внутреннюю область (без рамки)
        let inner_x_min = panel_area.x + 1;
        let inner_x_max = panel_area.x + panel_area.width.saturating_sub(2);
        let inner_y_min = panel_area.y + 1;
        let inner_y_max = panel_area.y + panel_area.height.saturating_sub(2);

        // Вычислить координаты относительно внутренней области терминала (0-based для выделения)
        // С ограничением по границам панели
        let clamped_col = mouse.column.clamp(inner_x_min, inner_x_max);
        let clamped_row = mouse.row.clamp(inner_y_min, inner_y_max);
        let inner_col = clamped_col.saturating_sub(inner_x_min) as usize;
        let inner_row = clamped_row.saturating_sub(inner_y_min) as usize;

        // Проверить что клик внутри области терминала
        let is_inside = mouse.column >= inner_x_min && mouse.column <= inner_x_max &&
                        mouse.row >= inner_y_min && mouse.row <= inner_y_max;

        // Проверить идёт ли выделение
        let selection_active = {
            let screen = self.screen.lock().unwrap();
            screen.selection_start.is_some()
        };

        // Если мышь снаружи и выделение не активно - игнорировать
        if !is_inside && !selection_active {
            return Ok(());
        }

        // Обработка локального выделения текста (приоритет над передачей в PTY)
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Начать выделение только внутри панели
                if !is_inside {
                    return Ok(());
                }
                // Начать выделение текста
                let mut screen = self.screen.lock().unwrap();
                screen.selection_start = Some((inner_row, inner_col));
                screen.selection_end = Some((inner_row, inner_col)); // Установить сразу для видимости
                drop(screen);

                // Также передать клик в PTY если mouse tracking включён
                self.send_mouse_to_pty(&mouse, panel_area)?;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Обновить конец выделения (используем ограниченные координаты)
                let mut screen = self.screen.lock().unwrap();
                if screen.selection_start.is_some() {
                    screen.selection_end = Some((inner_row, inner_col));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // Зафиксировать выделение (используем ограниченные координаты)
                {
                    let mut screen = self.screen.lock().unwrap();
                    if screen.selection_start.is_some() {
                        screen.selection_end = Some((inner_row, inner_col));
                    }
                }

                // Копировать выделенный текст в CLIPBOARD
                self.copy_selection_to_clipboard()?;

                // Очистить выделение после копирования
                {
                    let mut screen = self.screen.lock().unwrap();
                    screen.selection_start = None;
                    screen.selection_end = None;
                }

                // Передать отпускание в PTY если mouse tracking включён (только если внутри)
                if is_inside {
                    self.send_mouse_to_pty(&mouse, panel_area)?;
                }
            }
            // Прокрутка колёсиком мыши - для просмотра истории
            MouseEventKind::ScrollUp => {
                // При прокрутке вверх - показать историю
                self.screen.lock().unwrap().scroll_view_up(3);
            }
            MouseEventKind::ScrollDown => {
                // При прокрутке вниз - вернуться к текущему
                self.screen.lock().unwrap().scroll_view_down(3);
            }
            // Остальные события мыши передаём в PTY
            _ => {
                self.send_mouse_to_pty(&mouse, panel_area)?;
            }
        }

        Ok(())
    }

    fn should_auto_close(&self) -> bool {
        // Автоматически закрыть панель если процесс завершился
        !self.is_alive()
    }

    fn needs_close_confirmation(&self) -> Option<String> {
        // Если процесс жив и есть дочерние процессы - запросить подтверждение
        if self.is_alive() && self.has_running_processes() {
            Some("Kill running processes?".to_string())
        } else {
            None
        }
    }

    fn captures_escape(&self) -> bool {
        // Если есть запущенные процессы, Escape передаётся им, а не закрывает панель
        self.is_alive() && self.has_running_processes()
    }

    fn has_running_processes(&self) -> bool {
        // Проверить есть ли дочерние процессы у shell
        if let Some(pid) = self.shell_pid {
            // Читаем /proc/{pid}/task/{pid}/children
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

            // Отправить SIGTERM группе процессов
            let _ = signal::killpg(pid, Signal::SIGTERM);

            // Подождать немного
            std::thread::sleep(std::time::Duration::from_millis(100));

            // Если процесс ещё жив - SIGKILL
            if self.is_alive() {
                let _ = signal::killpg(pid, Signal::SIGKILL);
            }

            // Дождаться завершения чтобы избежать зомби
            let _ = self.child.wait();
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Корректно завершить процессы при удалении терминала
        if self.is_alive() {
            self.kill_processes();
        }
    }
}
