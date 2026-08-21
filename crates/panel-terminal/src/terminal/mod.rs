//! Terminal panel module.
//!
//! This module provides a full-featured terminal emulator with PTY support.

mod csi_handlers;
pub mod vt100_parser;

use ratatui::style::Color;
use std::collections::VecDeque;
use std::sync::LazyLock;

pub use vt100_parser::VtPerformer;

/// Static lookup table for 256-color palette.
///
/// Computing 256-color values on every call was causing overhead during
/// high-frequency SGR sequences (htop generates ~500 color changes per frame).
/// This table is computed once at startup.
static COLOR_256_TABLE: LazyLock<[Color; 256]> = LazyLock::new(|| {
    let mut table = [Color::Reset; 256];
    for i in 0..256u16 {
        table[i as usize] = compute_ansi_256_color(i);
    }
    table
});

/// Compute a single 256-color value (used for table initialization).
fn compute_ansi_256_color(code: u16) -> Color {
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

/// Mouse tracking mode for terminal
#[derive(Clone, Copy, PartialEq)]
pub enum MouseTrackingMode {
    None,
    Normal,      // ?1000 - clicks only
    ButtonEvent, // ?1002 - clicks + drag
    AnyEvent,    // ?1003 - all movements
}

/// Keyboard protocol mode negotiated by the inner application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardProtocolMode {
    /// Traditional xterm-compatible key encoding.
    Legacy,
    /// Kitty/CSI-u compatibility mode enabled via `CSI > 1 u`.
    CsiUCompat,
    /// xterm modifyOtherKeys mode 2 enabled via `CSI > 4 ; 2 m`.
    ModifyOtherKeys2,
}

/// Terminal cell containing a character and its style
#[derive(Clone, Debug, Copy)]
pub struct Cell {
    pub ch: char,
    pub style: CellStyle,
}

/// Cell style with colors and text attributes
#[derive(Clone, Debug, Copy)]
pub struct CellStyle {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
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
pub fn ansi_to_color(code: u16) -> Color {
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
pub fn ansi_to_bright_color(code: u16) -> Color {
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

/// Convert 256-color index to ratatui Color using cached lookup table.
///
/// O(1) table lookup instead of computing on every call.
#[inline]
pub fn ansi_256_to_color(code: u16) -> Color {
    COLOR_256_TABLE
        .get(code as usize)
        .copied()
        .unwrap_or(Color::Reset)
}

/// Terminal screen state
#[derive(Clone)]
pub struct TerminalScreen {
    /// Main line buffer - VecDeque for O(1) scroll operations
    pub lines: VecDeque<Vec<Cell>>,
    /// Alternate screen buffer (for TUI applications)
    pub alt_lines: VecDeque<Vec<Cell>>,
    /// Alternate screen usage flag
    pub use_alt_screen: bool,
    /// Cursor position (row, col)
    pub cursor: (usize, usize),
    /// Saved cursor position
    pub saved_cursor: Option<(usize, usize)>,
    /// Cursor visibility
    pub cursor_visible: bool,
    /// Screen dimensions
    pub rows: usize,
    pub cols: usize,
    /// Current style
    pub current_style: CellStyle,
    /// Application Cursor Keys Mode (DECCKM)
    pub application_cursor_keys: bool,
    /// Mouse tracking mode
    pub mouse_tracking: MouseTrackingMode,
    /// SGR extended mouse mode (?1006)
    pub sgr_mouse_mode: bool,
    /// Bracketed paste mode (?2004)
    pub bracketed_paste_mode: bool,
    /// Focus event reporting (?1004)
    pub focus_reporting: bool,
    /// Negotiated keyboard protocol mode for inner apps
    pub keyboard_protocol: KeyboardProtocolMode,
    /// Text selection start (row, col)
    pub selection_start: Option<(usize, usize)>,
    /// Text selection end (row, col)
    pub selection_end: Option<(usize, usize)>,
    /// History buffer (scrollback) - VecDeque for O(1) push/pop at both ends
    pub scrollback: VecDeque<Vec<Cell>>,
    /// Soft-wrap flags for main lines (true = line wrapped due to terminal width)
    pub lines_wrapped: VecDeque<bool>,
    /// Soft-wrap flags for alternate screen lines
    pub alt_lines_wrapped: VecDeque<bool>,
    /// Soft-wrap flags for scrollback lines
    pub scrollback_wrapped: VecDeque<bool>,
    /// View offset (0 = current screen, >0 = viewing history)
    pub scroll_offset: usize,
    /// Maximum scrollback lines
    pub max_scrollback: usize,
    /// Wrap pending flag (for auto-wrap mode)
    pub wrap_pending: bool,
    /// Dirty flag - screen content has changed and needs re-render
    pub dirty: bool,
    /// Scroll region top (0-based, inclusive)
    pub scroll_top: usize,
    /// Scroll region bottom (0-based, inclusive)
    pub scroll_bottom: usize,
    /// Synchronized output mode (CSI ? 2026 h/l)
    /// When enabled, rendering is deferred until mode is disabled
    pub sync_output: bool,
    /// Flag set when sync_output transitions from true to false
    /// Signals that cached content must be invalidated
    pub sync_output_ended: bool,
    /// Flag to force cache invalidation on next render
    /// Set by ED (clear screen) commands to ensure fresh content is shown
    pub force_cache_invalidation: bool,
}

impl TerminalScreen {
    pub fn new(rows: usize, cols: usize) -> Self {
        // Hard-floor the dimensions at 1×1 so the scroll/cursor code (which
        // freely uses `rows - 1`, `cols - 1`, `self.lines[0]`) never hits
        // out-of-bounds or subtract-overflow panics on very small terminals.
        let rows = rows.max(1);
        let cols = cols.max(1);
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
            application_cursor_keys: false,
            mouse_tracking: MouseTrackingMode::None,
            sgr_mouse_mode: false,
            bracketed_paste_mode: false,
            focus_reporting: false,
            keyboard_protocol: KeyboardProtocolMode::Legacy,
            selection_start: None,
            selection_end: None,
            scrollback: std::collections::VecDeque::new(),
            lines_wrapped: std::collections::VecDeque::from(vec![false; rows]),
            alt_lines_wrapped: std::collections::VecDeque::from(vec![false; rows]),
            scrollback_wrapped: std::collections::VecDeque::new(),
            scroll_offset: 0,
            max_scrollback: 10000,
            wrap_pending: false,
            dirty: true,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            sync_output: false,
            sync_output_ended: false,
            force_cache_invalidation: false,
        }
    }

    /// Get mutable reference to active buffer
    pub fn active_buffer_mut(&mut self) -> &mut std::collections::VecDeque<Vec<Cell>> {
        if self.use_alt_screen {
            &mut self.alt_lines
        } else {
            &mut self.lines
        }
    }

    /// Get reference to active buffer
    pub fn active_buffer(&self) -> &std::collections::VecDeque<Vec<Cell>> {
        if self.use_alt_screen {
            &self.alt_lines
        } else {
            &self.lines
        }
    }

    /// Get mutable reference to active wrapped-flags buffer
    pub fn active_wrapped_mut(&mut self) -> &mut std::collections::VecDeque<bool> {
        if self.use_alt_screen {
            &mut self.alt_lines_wrapped
        } else {
            &mut self.lines_wrapped
        }
    }

    /// Check if a line (by absolute row index) was soft-wrapped
    pub fn get_wrapped_by_absolute(&self, abs_row: usize) -> bool {
        if self.use_alt_screen {
            self.alt_lines_wrapped
                .get(abs_row)
                .copied()
                .unwrap_or(false)
        } else {
            let scrollback_len = self.scrollback_wrapped.len();
            if abs_row < scrollback_len {
                self.scrollback_wrapped
                    .get(abs_row)
                    .copied()
                    .unwrap_or(false)
            } else {
                self.lines_wrapped
                    .get(abs_row - scrollback_len)
                    .copied()
                    .unwrap_or(false)
            }
        }
    }

    /// Switch to alternate screen
    pub fn switch_to_alt_screen(&mut self) {
        if !self.use_alt_screen {
            self.use_alt_screen = true;
            self.wrap_pending = false;
            self.reset_scroll_region();
            // Clear alt buffer
            let empty_cell = Cell {
                ch: ' ',
                style: CellStyle::default(),
            };
            self.alt_lines =
                std::collections::VecDeque::from(vec![vec![empty_cell; self.cols]; self.rows]);
            self.alt_lines_wrapped = std::collections::VecDeque::from(vec![false; self.rows]);
            self.cursor = (0, 0);
        }
    }

    /// Return to main screen
    pub fn switch_to_main_screen(&mut self) {
        if self.use_alt_screen {
            self.use_alt_screen = false;
            self.wrap_pending = false;
            self.reset_scroll_region();
        }
    }

    /// Write character at current cursor position (respects scroll region)
    pub fn put_char(&mut self, ch: char) {
        // If there was a deferred wrap - execute it now
        if self.wrap_pending {
            self.wrap_pending = false;
            // Mark current line as soft-wrapped (not a real newline)
            let row = self.cursor.0;
            if let Some(w) = self.active_wrapped_mut().get_mut(row) {
                *w = true;
            }
            self.cursor.1 = 0;
            if self.cursor.0 >= self.scroll_bottom {
                self.scroll_up();
            } else {
                self.cursor.0 += 1;
            }
        }

        let (row, col) = self.cursor;
        let cols = self.cols;
        let rows = self.rows;
        let style = self.current_style;

        let buffer = self.active_buffer_mut();
        if row < rows && col < cols && row < buffer.len() {
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

    /// Line Feed - move cursor down (respects scroll region)
    /// NOTE: LF does NOT reset column position - only CR does that
    pub fn newline(&mut self) {
        self.wrap_pending = false;
        // Explicit newline — ensure current line is NOT marked as soft-wrapped
        let row = self.cursor.0;
        if let Some(w) = self.active_wrapped_mut().get_mut(row) {
            *w = false;
        }
        // Do NOT reset cursor.1 here - LF only moves down, CR resets column
        if self.cursor.0 >= self.scroll_bottom {
            // At or below scroll region bottom - scroll
            self.scroll_up();
        } else {
            self.cursor.0 += 1;
        }
    }

    /// Carriage return
    pub fn carriage_return(&mut self) {
        self.wrap_pending = false;
        self.cursor.1 = 0;
    }

    /// Scroll screen up one line (respects scroll region)
    pub fn scroll_up(&mut self) {
        let cols = self.cols;
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let empty_cell = Cell {
            ch: ' ',
            style: CellStyle::default(),
        };

        // Full-screen scroll (no region set or region covers entire screen)
        if top == 0 && bottom == self.rows.saturating_sub(1) {
            // For main buffer, save line to scrollback
            if !self.use_alt_screen {
                // Guard against a shrunk-to-zero buffer (tiny terminal). Without
                // this, `self.lines[0]` panics with "Out of bounds access".
                let Some(top_line) = self.lines.front().cloned() else {
                    return;
                };
                self.scrollback.push_back(top_line);

                // Save wrapped flag to scrollback
                let top_wrapped = self.lines_wrapped.front().copied().unwrap_or(false);
                self.scrollback_wrapped.push_back(top_wrapped);

                // Limit scrollback size - O(1) with VecDeque
                if self.scrollback.len() > self.max_scrollback {
                    self.scrollback.pop_front();
                }
                if self.scrollback_wrapped.len() > self.max_scrollback {
                    self.scrollback_wrapped.pop_front();
                }

                // Preserve the user's scrollback view when they're scrolled
                // away from the live tail. The renderer derives the visible
                // window from `scrollback.len() + visible_rows - scroll_offset`;
                // pushing a line into scrollback shifts that window forward by
                // one (or, when capped at `max_scrollback`, the absolute index
                // of every kept line drops by one), so the same scroll_offset
                // would point at later content on the next render. Bumping
                // scroll_offset by one keeps the on-screen content stable.
                // At the tail (`scroll_offset == 0`) we leave it alone so the
                // natural follow-tail behaviour stays intact.
                if self.scroll_offset > 0 {
                    self.scroll_offset = (self.scroll_offset + 1).min(self.scrollback.len());
                }
            }

            let buffer = self.active_buffer_mut();
            buffer.pop_front(); // O(1) with VecDeque
            buffer.push_back(vec![empty_cell; cols]);

            let wrapped = self.active_wrapped_mut();
            wrapped.pop_front();
            wrapped.push_back(false);
        } else {
            // Region scroll - remove line at top of region, insert at bottom
            let buffer = self.active_buffer_mut();
            if top < buffer.len() && bottom < buffer.len() {
                buffer.remove(top);
                buffer.insert(bottom, vec![empty_cell; cols]);
            }

            let wrapped = self.active_wrapped_mut();
            if top < wrapped.len() && bottom < wrapped.len() {
                wrapped.remove(top);
                wrapped.insert(bottom, false);
            }
        }
    }

    /// Set scroll region (DECSTBM). top/bottom are 1-based per VT100 spec.
    pub fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        let top_0 = top.saturating_sub(1);
        let bottom_0 = bottom.saturating_sub(1);

        if top_0 < bottom_0 && bottom_0 < self.rows {
            self.scroll_top = top_0;
            self.scroll_bottom = bottom_0;
        } else {
            self.reset_scroll_region();
        }
        self.cursor = (0, 0);
        self.wrap_pending = false;
    }

    /// Reset scroll region to full screen
    pub fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
    }

    /// Scroll down within region (for Reverse Index)
    pub fn scroll_down_region(&mut self) {
        let cols = self.cols;
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let empty_cell = Cell {
            ch: ' ',
            style: CellStyle::default(),
        };

        let buffer = self.active_buffer_mut();
        let is_full_screen = top == 0 && bottom == buffer.len().saturating_sub(1);
        // Full-screen scroll
        if is_full_screen {
            buffer.pop_back();
            buffer.push_front(vec![empty_cell; cols]);
        } else {
            // Region scroll - remove line at bottom, insert at top
            if bottom < buffer.len() {
                buffer.remove(bottom);
            }
            buffer.insert(top, vec![empty_cell; cols]);
        }

        let wrapped = self.active_wrapped_mut();
        if is_full_screen {
            wrapped.pop_back();
            wrapped.push_front(false);
        } else {
            if bottom < wrapped.len() {
                wrapped.remove(bottom);
            }
            wrapped.insert(top, false);
        }
    }

    /// Scroll view up (into history)
    pub fn scroll_view_up(&mut self, lines: usize) {
        let max_offset = self.scrollback.len();
        self.scroll_offset = (self.scroll_offset + lines).min(max_offset);
        self.dirty = true; // Invalidate cache to force re-render
    }

    /// Scroll view down (to current)
    pub fn scroll_view_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        self.dirty = true; // Invalidate cache to force re-render
    }

    /// Set the view scroll offset directly, clamped to the scrollback length.
    ///
    /// Used by the scrollbar thumb drag, which computes an absolute position
    /// rather than a delta. Marks the screen dirty like the relative
    /// `scroll_view_*` helpers do: the render cache is only rebuilt for a dirty
    /// screen, so without this the view would keep showing the cached frame.
    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.scroll_offset = offset.min(self.scrollback.len());
        self.dirty = true;
    }

    /// Reset scroll to current screen
    pub fn reset_scroll(&mut self) {
        self.scroll_offset = 0;
    }

    /// Move cursor
    pub fn move_cursor(&mut self, row: usize, col: usize) {
        self.wrap_pending = false;
        self.cursor.0 = row.min(self.rows.saturating_sub(1));
        self.cursor.1 = col.min(self.cols.saturating_sub(1));
    }

    /// Backspace
    pub fn backspace(&mut self) {
        self.wrap_pending = false;
        if self.cursor.1 > 0 {
            self.cursor.1 -= 1;
        }
    }

    /// Tab
    pub fn tab(&mut self) {
        // Move cursor to next position divisible by 8
        let next_tab = ((self.cursor.1 / 8) + 1) * 8;
        self.cursor.1 = next_tab.min(self.cols.saturating_sub(1));
    }

    /// Save cursor position
    pub fn save_cursor(&mut self) {
        self.saved_cursor = Some(self.cursor);
    }

    /// Restore cursor position
    pub fn restore_cursor(&mut self) {
        if let Some(saved) = self.saved_cursor {
            self.cursor = saved;
            self.wrap_pending = false;
        }
    }

    /// Convert visual row (0-based on screen) to absolute buffer index
    /// Absolute index: 0..scrollback.len() = scrollback, scrollback.len()..scrollback.len()+rows = active buffer
    pub fn visual_to_absolute(&self, visual_row: usize) -> usize {
        if self.use_alt_screen {
            // Alt screen has no scrollback
            visual_row
        } else {
            // view_start is the absolute index of visual row 0
            let view_start = self.scrollback.len().saturating_sub(self.scroll_offset);
            view_start + visual_row
        }
    }

    /// Clear text selection
    pub fn clear_selection(&mut self) {
        self.selection_start = None;
        self.selection_end = None;
    }

    /// Get line by absolute index (from scrollback or active buffer)
    pub fn get_line_by_absolute(&self, abs_row: usize) -> Option<&[Cell]> {
        if self.use_alt_screen {
            self.alt_lines.get(abs_row).map(|v| v.as_slice())
        } else {
            let scrollback_len = self.scrollback.len();
            if abs_row < scrollback_len {
                self.scrollback.get(abs_row).map(|v| v.as_slice())
            } else {
                self.lines
                    .get(abs_row - scrollback_len)
                    .map(|v| v.as_slice())
            }
        }
    }

    /// Ensure buffer has exactly `rows` lines, each with `cols` cells.
    ///
    /// This fixes buffer size invariant violations that can occur after IL/DL
    /// operations when rows are inserted/deleted at boundary positions.
    pub fn ensure_buffer_size(&mut self) {
        let rows = self.rows;
        let cols = self.cols;
        let empty_cell = Cell {
            ch: ' ',
            style: CellStyle::default(),
        };

        let buffer = self.active_buffer_mut();
        while buffer.len() < rows {
            buffer.push_back(vec![empty_cell; cols]);
        }
        while buffer.len() > rows {
            buffer.pop_back();
        }

        let wrapped = self.active_wrapped_mut();
        while wrapped.len() < rows {
            wrapped.push_back(false);
        }
        while wrapped.len() > rows {
            wrapped.pop_back();
        }

        debug_assert_eq!(
            self.active_buffer().len(),
            self.active_wrapped().len(),
            "buffer/wrapped length mismatch after ensure_buffer_size"
        );
    }

    /// Get immutable reference to active wrapped-flags buffer
    fn active_wrapped(&self) -> &std::collections::VecDeque<bool> {
        if self.use_alt_screen {
            &self.alt_lines_wrapped
        } else {
            &self.lines_wrapped
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pump enough lines through `scroll_up` to fill the scrollback to `n`.
    fn fill_scrollback(screen: &mut TerminalScreen, n: usize) {
        for _ in 0..n {
            screen.scroll_up();
        }
    }

    /// The scrollbar thumb drag sets an absolute offset; it has to invalidate
    /// the render cache the same way the relative scroll helpers do, or the
    /// panel keeps drawing the cached frame while the thumb moves.
    #[test]
    fn set_scroll_offset_clamps_and_marks_dirty() {
        let mut screen = TerminalScreen::new(10, 80);
        fill_scrollback(&mut screen, 30);
        screen.dirty = false;

        screen.set_scroll_offset(12);
        assert_eq!(screen.scroll_offset, 12);
        assert!(screen.dirty, "render cache was not invalidated");

        screen.dirty = false;
        screen.set_scroll_offset(usize::MAX);
        assert_eq!(
            screen.scroll_offset,
            screen.scrollback.len(),
            "offset past the oldest line must clamp"
        );
        assert!(screen.dirty);
    }

    #[test]
    fn scroll_up_preserves_user_view_when_in_history() {
        let mut screen = TerminalScreen::new(10, 80);
        // Build some history first.
        fill_scrollback(&mut screen, 30);
        // Pretend the user scrolled up 5 lines from the tail.
        screen.scroll_offset = 5;
        let scrollback_before = screen.scrollback.len();
        let view_top_before = screen.visual_to_absolute(0);

        // Three new output lines push into scrollback.
        for _ in 0..3 {
            screen.scroll_up();
        }

        // scroll_offset must follow so the same content stays under the view.
        assert_eq!(screen.scroll_offset, 8);
        assert_eq!(screen.scrollback.len(), scrollback_before + 3);
        assert_eq!(screen.visual_to_absolute(0), view_top_before);
    }

    #[test]
    fn scroll_up_at_tail_stays_at_tail() {
        let mut screen = TerminalScreen::new(10, 80);
        fill_scrollback(&mut screen, 5);
        assert_eq!(screen.scroll_offset, 0);
        screen.scroll_up();
        assert_eq!(screen.scroll_offset, 0, "follow-tail must be preserved");
    }

    #[test]
    fn scroll_up_caps_at_scrollback_len() {
        let mut screen = TerminalScreen::new(10, 80);
        screen.max_scrollback = 8; // Tight cap to make the test fast.
        fill_scrollback(&mut screen, 8);
        assert_eq!(screen.scrollback.len(), 8);
        // Scroll all the way up — user is at the very top of history.
        screen.scroll_offset = 8;

        // One more push triggers pop_front (we're at the cap). scroll_offset
        // must not exceed scrollback.len(); otherwise visual_to_absolute would
        // saturate-subtract to an invalid index.
        screen.scroll_up();
        assert_eq!(screen.scrollback.len(), 8);
        assert_eq!(screen.scroll_offset, 8);
    }

    #[test]
    fn scroll_up_in_alt_screen_does_not_bump_offset() {
        let mut screen = TerminalScreen::new(10, 80);
        fill_scrollback(&mut screen, 5);
        screen.scroll_offset = 3;
        screen.switch_to_alt_screen();
        let before = screen.scroll_offset;

        // Alt-screen scroll_up does not feed into scrollback.
        screen.scroll_up();
        assert_eq!(screen.scroll_offset, before);
    }
}
