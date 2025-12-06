//! Terminal panel module.
//!
//! This module provides a full-featured terminal emulator with PTY support.

mod vt100_parser;

use ratatui::style::Color;
use std::collections::VecDeque;

pub(crate) use vt100_parser::VtPerformer;

/// Mouse tracking mode for terminal
#[derive(Clone, Copy, PartialEq)]
pub(super) enum MouseTrackingMode {
    None,
    Normal,      // ?1000 - clicks only
    ButtonEvent, // ?1002 - clicks + drag
    AnyEvent,    // ?1003 - all movements
}

/// Terminal cell containing a character and its style
#[derive(Clone, Debug, Copy)]
pub(super) struct Cell {
    pub(super) ch: char,
    pub(super) style: CellStyle,
}

/// Cell style with colors and text attributes
#[derive(Clone, Debug, Copy)]
pub(super) struct CellStyle {
    pub(super) fg: Color,
    pub(super) bg: Color,
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) underline: bool,
    pub(super) reverse: bool,
}

impl Default for CellStyle {
    fn default() -> Self {
        Self {
            fg: Color::White,
            bg: Color::Reset, // Use theme background by default
            bold: false,
            italic: false,
            underline: false,
            reverse: false,
        }
    }
}

/// Convert ANSI color code to ratatui Color
pub(super) fn ansi_to_color(code: u16) -> Color {
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
pub(super) fn ansi_to_bright_color(code: u16) -> Color {
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
#[allow(clippy::too_many_lines)] // Color table is inherently large
pub(super) fn ansi_256_to_color(code: u16) -> Color {
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
        // Grayscale ramp - indices 232-255 (24 shades of gray)
        232..=255 => {
            let gray = 8 + (code - 232) as u8 * 10;
            Color::Rgb(gray, gray, gray)
        }
        _ => Color::White,
    }
}

/// Terminal screen state
#[derive(Clone)]
pub(super) struct TerminalScreen {
    /// Main line buffer - VecDeque for O(1) scroll operations
    pub(super) lines: VecDeque<Vec<Cell>>,
    /// Alternate screen buffer (for TUI applications)
    pub(super) alt_lines: VecDeque<Vec<Cell>>,
    /// Alternate screen usage flag
    pub(super) use_alt_screen: bool,
    /// Cursor position (row, col)
    pub(super) cursor: (usize, usize),
    /// Saved cursor position
    pub(super) saved_cursor: Option<(usize, usize)>,
    /// Cursor visibility
    pub(super) cursor_visible: bool,
    /// Screen dimensions
    pub(super) rows: usize,
    pub(super) cols: usize,
    /// Current style
    pub(super) current_style: CellStyle,
    /// Insert mode
    #[allow(dead_code)]
    pub(super) insert_mode: bool,
    /// Application Cursor Keys Mode (DECCKM)
    pub(super) application_cursor_keys: bool,
    /// Mouse tracking mode
    pub(super) mouse_tracking: MouseTrackingMode,
    /// SGR extended mouse mode (?1006)
    pub(super) sgr_mouse_mode: bool,
    /// Bracketed paste mode (?2004)
    pub(super) bracketed_paste_mode: bool,
    /// Text selection start (row, col)
    pub(super) selection_start: Option<(usize, usize)>,
    /// Text selection end (row, col)
    pub(super) selection_end: Option<(usize, usize)>,
    /// History buffer (scrollback) - VecDeque for O(1) push/pop at both ends
    pub(super) scrollback: VecDeque<Vec<Cell>>,
    /// View offset (0 = current screen, >0 = viewing history)
    pub(super) scroll_offset: usize,
    /// Maximum scrollback lines
    pub(super) max_scrollback: usize,
    /// Wrap pending flag (for auto-wrap mode)
    pub(super) wrap_pending: bool,
}

impl TerminalScreen {
    pub(super) fn new(rows: usize, cols: usize) -> Self {
        let empty_cell = Cell {
            ch: ' ',
            style: CellStyle::default(),
        };

        Self {
            lines: std::collections::VecDeque::from(vec![vec![empty_cell; cols]; rows]),
            alt_lines: std::collections::VecDeque::from(vec![vec![empty_cell; cols]; rows]),
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
            scrollback: std::collections::VecDeque::new(),
            scroll_offset: 0,
            max_scrollback: 10000,
            wrap_pending: false,
        }
    }

    /// Get mutable reference to active buffer
    pub(super) fn active_buffer_mut(&mut self) -> &mut std::collections::VecDeque<Vec<Cell>> {
        if self.use_alt_screen {
            &mut self.alt_lines
        } else {
            &mut self.lines
        }
    }

    /// Get reference to active buffer
    pub(super) fn active_buffer(&self) -> &std::collections::VecDeque<Vec<Cell>> {
        if self.use_alt_screen {
            &self.alt_lines
        } else {
            &self.lines
        }
    }

    /// Switch to alternate screen
    pub(super) fn switch_to_alt_screen(&mut self) {
        if !self.use_alt_screen {
            self.use_alt_screen = true;
            self.wrap_pending = false;
            // Clear alt buffer
            let empty_cell = Cell {
                ch: ' ',
                style: CellStyle::default(),
            };
            self.alt_lines =
                std::collections::VecDeque::from(vec![vec![empty_cell; self.cols]; self.rows]);
            self.cursor = (0, 0);
        }
    }

    /// Return to main screen
    pub(super) fn switch_to_main_screen(&mut self) {
        if self.use_alt_screen {
            self.use_alt_screen = false;
            self.wrap_pending = false;
        }
    }

    /// Write character at current cursor position
    pub(super) fn put_char(&mut self, ch: char) {
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
            buffer[row][col] = Cell { ch, style };
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
    pub(super) fn newline(&mut self) {
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
    pub(super) fn carriage_return(&mut self) {
        self.wrap_pending = false;
        self.cursor.1 = 0;
    }

    /// Scroll screen up one line
    pub(super) fn scroll_up(&mut self) {
        let cols = self.cols;

        // For main buffer, save line to scrollback
        if !self.use_alt_screen {
            let top_line = self.lines[0].clone();
            self.scrollback.push_back(top_line);

            // Limit scrollback size - O(1) with VecDeque instead of O(n) with Vec::remove(0)
            if self.scrollback.len() > self.max_scrollback {
                self.scrollback.pop_front();
            }
        }

        let buffer = self.active_buffer_mut();
        buffer.pop_front(); // O(1) with VecDeque instead of O(n) with Vec::remove(0)
        let empty_cell = Cell {
            ch: ' ',
            style: CellStyle::default(),
        };
        buffer.push_back(vec![empty_cell; cols]);
    }

    /// Scroll view up (into history)
    pub(super) fn scroll_view_up(&mut self, lines: usize) {
        let max_offset = self.scrollback.len();
        self.scroll_offset = (self.scroll_offset + lines).min(max_offset);
    }

    /// Scroll view down (to current)
    pub(super) fn scroll_view_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    /// Reset scroll to current screen
    pub(super) fn reset_scroll(&mut self) {
        self.scroll_offset = 0;
    }

    /// Check if cell (row, col) is in current selection
    pub(super) fn is_in_selection(&self, row: usize, col: usize) -> bool {
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
    #[allow(dead_code)]
    pub(super) fn clear_screen(&mut self) {
        let rows = self.rows;
        let cols = self.cols;
        let empty_cell = Cell {
            ch: ' ',
            style: CellStyle::default(),
        };
        let buffer = self.active_buffer_mut();
        *buffer = std::collections::VecDeque::from(vec![vec![empty_cell; cols]; rows]);
        // Cursor stays in place (standard ED 2 behavior)
    }

    /// Move cursor
    pub(super) fn move_cursor(&mut self, row: usize, col: usize) {
        self.wrap_pending = false;
        self.cursor.0 = row.min(self.rows - 1);
        self.cursor.1 = col.min(self.cols - 1);
    }

    /// Backspace
    pub(super) fn backspace(&mut self) {
        self.wrap_pending = false;
        if self.cursor.1 > 0 {
            self.cursor.1 -= 1;
        }
    }

    /// Tab
    pub(super) fn tab(&mut self) {
        // Move cursor to next position divisible by 8
        let next_tab = ((self.cursor.1 / 8) + 1) * 8;
        self.cursor.1 = next_tab.min(self.cols - 1);
    }

    /// Save cursor position
    pub(super) fn save_cursor(&mut self) {
        self.saved_cursor = Some(self.cursor);
    }

    /// Restore cursor position
    pub(super) fn restore_cursor(&mut self) {
        if let Some(saved) = self.saved_cursor {
            self.cursor = saved;
            self.wrap_pending = false;
        }
    }
}
