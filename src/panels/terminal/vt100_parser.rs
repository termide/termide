//! VT100/ANSI escape sequence parser implementation.
//!
//! This module provides the VtPerformer struct which implements the vte::Perform trait
//! to parse and handle VT100/ANSI escape sequences for terminal emulation.

#![allow(clippy::needless_range_loop)]

use std::io::Write;
use std::sync::{Arc, Mutex};
use vte::{Params, Perform};

use ratatui::style::Color;

use super::{
    ansi_256_to_color, ansi_to_bright_color, ansi_to_color, Cell, CellStyle, MouseTrackingMode,
    TerminalScreen,
};

/// VT100 parser and performer.
///
/// Implements the vte::Perform trait to handle ANSI/VT100 escape sequences
/// and update the terminal screen state accordingly.
#[allow(private_interfaces)]
pub(crate) struct VtPerformer {
    pub(crate) screen: Arc<Mutex<TerminalScreen>>,
    pub(crate) pending_backslash: bool,
}

impl VtPerformer {
    /// Create a new VtPerformer with the given screen.
    #[allow(dead_code)]
    pub(super) fn new(screen: Arc<Mutex<TerminalScreen>>) -> Self {
        Self {
            screen,
            pending_backslash: false,
        }
    }
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
                b'\x07' => {
                    // Bell character - forward to parent terminal
                    print!("\x07");
                    let _ = std::io::stdout().flush();
                }
                _ => {}
            }
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _c: char) {}

    fn put(&mut self, _byte: u8) {}

    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, c: char) {
        // Handle private sequences (start with '?')
        if !intermediates.is_empty() && intermediates[0] == b'?' {
            if let Ok(mut screen) = self.screen.lock() {
                // Get private sequence number
                let mode = params
                    .iter()
                    .next()
                    .and_then(|p| p.first())
                    .copied()
                    .unwrap_or(0);

                match (mode, c) {
                    (1049, 'h') => {
                        // Switch to alternate screen and save cursor
                        screen.saved_cursor = Some(screen.cursor);
                        screen.switch_to_alt_screen();
                    }
                    (1049, 'l') => {
                        // Return to main screen and restore cursor
                        screen.switch_to_main_screen();
                        if let Some(saved) = screen.saved_cursor {
                            screen.cursor = saved;
                            screen.saved_cursor = None;
                        }
                    }
                    (47, 'h') => {
                        // Switch to alternate screen (without saving cursor)
                        screen.switch_to_alt_screen();
                    }
                    (47, 'l') => {
                        // Return to main screen
                        screen.switch_to_main_screen();
                    }
                    (25, 'h') => {
                        // Show cursor
                        screen.cursor_visible = true;
                    }
                    (25, 'l') => {
                        // Hide cursor
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
                        // Ignore other private sequences
                    }
                }
            }
            return;
        }

        // Ignore other intermediate bytes
        if !intermediates.is_empty() {
            return;
        }

        if let Ok(mut screen) = self.screen.lock() {
            match c {
                'H' | 'f' => {
                    // Move cursor
                    let row = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    let col = params
                        .iter()
                        .nth(1)
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    screen.move_cursor(row.saturating_sub(1), col.saturating_sub(1));
                }
                'J' => {
                    // ED - Erase in Display
                    let param = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(0);
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
                                    buffer[row][i] = empty_cell;
                                }
                            }
                            // Clear all lines below
                            for r in (row + 1)..buf_rows {
                                let buf_cols = buffer[r].len();
                                for c in 0..buf_cols {
                                    buffer[r][c] = empty_cell;
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
                                    buffer[r][c] = empty_cell;
                                }
                            }
                            // Clear current line up to and including cursor
                            if row < buf_rows {
                                let buf_cols = buffer[row].len();
                                for i in 0..=col.min(buf_cols.saturating_sub(1)) {
                                    buffer[row][i] = empty_cell;
                                }
                            }
                        }
                        2 => {
                            // Clear entire screen and move cursor to (0,0)
                            let buffer = screen.active_buffer_mut();
                            for row in buffer.iter_mut() {
                                row.fill(empty_cell);
                            }
                            // Move cursor to home position (compatibility with old behavior)
                            screen.cursor = (0, 0);
                        }
                        3 => {
                            // Clear entire screen and scrollback
                            let is_alt = screen.use_alt_screen;
                            let buffer = screen.active_buffer_mut();
                            for row in buffer.iter_mut() {
                                row.fill(empty_cell);
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
                    let param = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(0);
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
                                // From cursor to end of line
                                for i in col..buf_cols {
                                    buffer[row][i] = empty_cell;
                                }
                            }
                            1 => {
                                // From start of line to cursor (inclusive)
                                for i in 0..=col.min(buf_cols.saturating_sub(1)) {
                                    buffer[row][i] = empty_cell;
                                }
                            }
                            2 => {
                                // Entire line
                                for i in 0..buf_cols {
                                    buffer[row][i] = empty_cell;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                'P' => {
                    // DCH - Delete Character
                    let n = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    let (row, col) = screen.cursor;
                    let cols = screen.cols;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    let buffer = screen.active_buffer_mut();
                    // Shift characters left from deleted position
                    for i in col..(cols - n) {
                        buffer[row][i] = buffer[row][i + n];
                    }

                    // Fill freed space with blanks
                    for i in (cols - n)..cols {
                        buffer[row][i] = empty_cell;
                    }
                }
                'X' => {
                    // ECH - Erase Character
                    let n = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    let (row, col) = screen.cursor;
                    let cols = screen.cols;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    let buffer = screen.active_buffer_mut();
                    for i in col..(col + n).min(cols) {
                        buffer[row][i] = empty_cell;
                    }
                }
                '@' => {
                    // ICH - Insert Character (shift characters right)
                    let n = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    let (row, col) = screen.cursor;
                    let cols = screen.cols;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    let buffer = screen.active_buffer_mut();
                    // Shift characters right
                    if col + n < cols {
                        for i in (col + n..cols).rev() {
                            buffer[row][i] = buffer[row][i - n];
                        }
                    }

                    // Insert blanks at freed positions
                    for i in col..(col + n).min(cols) {
                        buffer[row][i] = empty_cell;
                    }
                }
                'L' => {
                    // IL - Insert Lines (insert blank lines)
                    let n = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    let row = screen.cursor.0;
                    let cols = screen.cols;
                    let rows = screen.rows;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    let buffer = screen.active_buffer_mut();
                    if row < buffer.len() {
                        // Delete n lines from bottom
                        for _ in 0..n.min(rows - row) {
                            if buffer.len() > row {
                                buffer.pop_back();
                            }
                        }
                        // Insert n blank lines at cursor position
                        for _ in 0..n.min(rows - row) {
                            buffer.insert(row, vec![empty_cell; cols]);
                        }
                    }
                }
                'M' => {
                    // DL - Delete Lines (delete lines)
                    let n = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    let row = screen.cursor.0;
                    let cols = screen.cols;
                    let rows = screen.rows;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    let buffer = screen.active_buffer_mut();
                    if row < buffer.len() {
                        // Delete n lines at cursor position
                        for _ in 0..n.min(buffer.len() - row) {
                            if row < buffer.len() {
                                buffer.remove(row);
                            }
                        }
                        // Add n blank lines at bottom
                        while buffer.len() < rows {
                            buffer.push_back(vec![empty_cell; cols]);
                        }
                    }
                }
                'S' => {
                    // SU - Scroll Up (scroll screen up)
                    let n = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    let cols = screen.cols;
                    let rows = screen.rows;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    let buffer = screen.active_buffer_mut();
                    for _ in 0..n.min(rows) {
                        if !buffer.is_empty() {
                            buffer.pop_front(); // O(1) with VecDeque
                        }
                        buffer.push_back(vec![empty_cell; cols]);
                    }
                }
                'T' => {
                    // SD - Scroll Down (scroll screen down)
                    let n = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    let cols = screen.cols;
                    let rows = screen.rows;
                    let empty_cell = Cell {
                        ch: ' ',
                        style: screen.current_style,
                    };

                    let buffer = screen.active_buffer_mut();
                    for _ in 0..n.min(rows) {
                        if buffer.len() >= rows {
                            buffer.pop_back();
                        }
                        buffer.push_front(vec![empty_cell; cols]); // O(1) with VecDeque
                    }
                }
                'A' => {
                    // Cursor up
                    let n = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.0 = screen.cursor.0.saturating_sub(n);
                }
                'B' => {
                    // Cursor down
                    let n = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.0 = (screen.cursor.0 + n).min(screen.rows - 1);
                }
                'C' => {
                    // Cursor right
                    let n = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.1 = (screen.cursor.1 + n).min(screen.cols - 1);
                }
                'D' => {
                    // Cursor left
                    let n = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.1 = screen.cursor.1.saturating_sub(n);
                }
                'E' => {
                    // CNL - Cursor Next Line
                    let n = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.0 = (screen.cursor.0 + n).min(screen.rows - 1);
                    screen.cursor.1 = 0;
                }
                'F' => {
                    // CPL - Cursor Previous Line
                    let n = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.0 = screen.cursor.0.saturating_sub(n);
                    screen.cursor.1 = 0;
                }
                'G' => {
                    // CHA - Cursor Horizontal Absolute
                    let col = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.1 = col.saturating_sub(1).min(screen.cols - 1);
                }
                'd' => {
                    // VPA - Vertical Position Absolute
                    let row = params
                        .iter()
                        .next()
                        .and_then(|p| p.first())
                        .copied()
                        .unwrap_or(1) as usize;
                    screen.wrap_pending = false;
                    screen.cursor.0 = row.saturating_sub(1).min(screen.rows - 1);
                }
                'm' => {
                    // SGR - set style
                    // Collect all parameters into one vector to handle 38;5;N and 48;5;N
                    let all_params: Vec<u16> =
                        params.iter().flat_map(|p| p.iter().copied()).collect();
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
                                // 256-color or RGB foreground
                                if i + 2 < all_params.len() && all_params[i + 1] == 5 {
                                    // 38;5;N - 256-color
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
                                // Reset foreground to default
                                screen.current_style.fg = Color::Reset;
                            }
                            40..=47 => {
                                screen.current_style.bg = ansi_to_color(p - 40);
                            }
                            48 => {
                                // 256-color or RGB background
                                if i + 2 < all_params.len() && all_params[i + 1] == 5 {
                                    // 48;5;N - 256-color
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
                                // Reset background to default
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
                    // Save cursor position
                    screen.save_cursor();
                }
                'u' => {
                    // Restore cursor position
                    screen.restore_cursor();
                }
                'r' => {
                    // DECSTBM - Set scrolling region (ignore but don't break)
                }
                'l' | 'h' => {
                    // Set/Reset Mode (ignore but don't break)
                }
                _ => {}
            }
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {}
}
