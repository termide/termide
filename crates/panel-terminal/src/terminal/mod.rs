//! Terminal panel module.
//!
//! This module provides a full-featured terminal emulator with PTY support.

mod csi_handlers;
mod osc_cwd;
pub mod vt100_parser;

use ratatui::style::Color;
use std::collections::VecDeque;
use std::sync::LazyLock;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
/// One grid cell.
///
/// A character wider than one column occupies its own cell plus a
/// continuation cell (`ch == CONTINUATION`) to its right, so a cell index is
/// always a screen column. `extra` carries one zero-width mark attached to
/// the base character (combining accent, variation selector, skin-tone
/// modifier); it is emitted right after `ch` when the row is rendered.
#[derive(Clone, Debug, Copy)]
pub struct Cell {
    pub ch: char,
    pub style: CellStyle,
    pub extra: Option<char>,
}

/// Marker stored in the right half of a wide character.
pub const CONTINUATION: char = '\0';

impl Cell {
    pub fn blank(style: CellStyle) -> Self {
        Self {
            ch: ' ',
            style,
            extra: None,
        }
    }

    fn continuation(style: CellStyle) -> Self {
        Self {
            ch: CONTINUATION,
            style,
            extra: None,
        }
    }

    pub fn is_continuation(&self) -> bool {
        self.ch == CONTINUATION
    }

    /// Append the cell's text (base plus attached mark) to `out`.
    /// Continuation cells add nothing: the base already covers that column.
    pub fn push_text(&self, out: &mut String) {
        if self.is_continuation() {
            return;
        }
        out.push(self.ch);
        if let Some(extra) = self.extra {
            out.push(extra);
        }
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::blank(CellStyle::default())
    }
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
    /// Base cell of a pending ZWJ (U+200D): the next emoji joins that
    /// cluster instead of taking cells of its own.
    pub zwj_base: Option<(usize, usize)>,
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
    /// Working directory the shell last reported in-band (OSC 7 / OSC 9;9).
    pub reported_cwd: Option<std::path::PathBuf>,
    /// The program rang the bell (BEL) since the panel was last shown
    /// focused: it waits for the user.
    pub bell: bool,
}

impl TerminalScreen {
    pub fn new(rows: usize, cols: usize) -> Self {
        // Hard-floor the dimensions at 1×1 so the scroll/cursor code (which
        // freely uses `rows - 1`, `cols - 1`, `self.lines[0]`) never hits
        // out-of-bounds or subtract-overflow panics on very small terminals.
        let rows = rows.max(1);
        let cols = cols.max(1);
        let empty_cell = Cell::blank(CellStyle::default());

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
            zwj_base: None,
            dirty: true,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            sync_output: false,
            sync_output_ended: false,
            force_cache_invalidation: false,
            reported_cwd: None,
            bell: false,
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
            let empty_cell = Cell::blank(CellStyle::default());
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

    /// Write character at current cursor position (respects scroll region).
    ///
    /// Wide characters take two cells (base + continuation) and wrap early
    /// when only one column is left; zero-width marks and emoji modifiers
    /// attach to the preceding character instead of taking a cell. The grid
    /// therefore advances exactly as the application and the host terminal
    /// expect, which keeps relative cursor motion and erasing in sync.
    pub fn put_char(&mut self, ch: char) {
        let width = match ch.width() {
            None => return,
            Some(0) => {
                self.attach_zero_width(ch);
                return;
            }
            Some(w) => w.min(2),
        };

        // Skin-tone modifier after an emoji base: same cluster, no new cells.
        if ('\u{1F3FB}'..='\u{1F3FF}').contains(&ch) && self.attach_modifier(ch) {
            return;
        }

        // Emoji after a ZWJ joins the base emoji; only the base glyph is kept.
        if let Some(base) = self.zwj_base.take() {
            if width == 2 && self.last_printed_cell() == Some(base) {
                return;
            }
        }

        let cols = self.cols;
        let wide_needs_room = width == 2 && cols >= 2 && self.cursor.1 + 1 >= cols;
        if self.wrap_pending || wide_needs_room {
            self.wrap_to_next_line();
        }

        let (row, col) = self.cursor;
        let style = self.current_style;
        if row >= self.rows || col >= cols || row >= self.active_buffer().len() {
            return;
        }
        // A wide character cannot fit a single-column grid; store it narrow.
        let width = width.min(cols - col);

        // Overwriting half of a wide character erases the whole character.
        self.blank_cells(row, col, col + width, style);
        let line = &mut self.active_buffer_mut()[row];
        line[col] = Cell {
            ch,
            style,
            extra: None,
        };
        if width == 2 {
            line[col + 1] = Cell::continuation(style);
        }

        if col + width >= cols {
            // Reached last column - defer wrap
            self.wrap_pending = true;
            self.cursor.1 = cols - 1;
        } else {
            self.cursor.1 = col + width;
        }
    }

    /// Deferred auto-wrap: mark the current line soft-wrapped and move to
    /// the start of the next one (scrolling at the bottom of the region).
    fn wrap_to_next_line(&mut self) {
        self.wrap_pending = false;
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

    /// The cell holding the character printed just before the cursor
    /// (stepping over a continuation cell), if any.
    fn last_printed_cell(&self) -> Option<(usize, usize)> {
        let (row, col) = self.cursor;
        let line = self.active_buffer().get(row)?;
        let mut c = if self.wrap_pending {
            col
        } else {
            col.checked_sub(1)?
        };
        if line.get(c)?.is_continuation() {
            c = c.checked_sub(1)?;
        }
        Some((row, c))
    }

    /// Attach a zero-width code point to the preceding character.
    ///
    /// A variation selector may change the base's width (`✔` + VS16 is two
    /// columns in every terminal); then the continuation cell is added or
    /// dropped and a cursor sitting right after the base follows it.
    fn attach_zero_width(&mut self, ch: char) {
        let Some((row, col)) = self.last_printed_cell() else {
            return;
        };
        if ch == '\u{200D}' {
            self.zwj_base = Some((row, col));
            return;
        }
        let cols = self.cols;
        let base = self.active_buffer()[row][col];
        if base.is_continuation() || base.extra.is_some() {
            return;
        }
        let is_wide = self.active_buffer()[row]
            .get(col + 1)
            .is_some_and(Cell::is_continuation);
        let mut cluster = String::with_capacity(8);
        cluster.push(base.ch);
        cluster.push(ch);
        let new_width = cluster.width().clamp(1, 2);

        if new_width == 2 && !is_wide {
            if col + 1 >= cols {
                // No room to widen at the right edge: keep the base narrow.
                return;
            }
            self.blank_cells(row, col + 1, col + 2, base.style);
            let line = &mut self.active_buffer_mut()[row];
            line[col].extra = Some(ch);
            line[col + 1] = Cell::continuation(base.style);
            if !self.wrap_pending && self.cursor == (row, col + 1) {
                if col + 2 >= cols {
                    self.wrap_pending = true;
                    self.cursor.1 = cols - 1;
                } else {
                    self.cursor.1 = col + 2;
                }
            }
        } else if new_width == 1 && is_wide {
            let line = &mut self.active_buffer_mut()[row];
            line[col].extra = Some(ch);
            line[col + 1] = Cell::blank(base.style);
            let after_base = if self.wrap_pending {
                col + 2 >= cols && self.cursor == (row, cols - 1)
            } else {
                self.cursor == (row, col + 2)
            };
            if after_base {
                self.wrap_pending = false;
                self.cursor.1 = col + 1;
            }
        } else {
            self.active_buffer_mut()[row][col].extra = Some(ch);
        }
    }

    /// Attach an emoji skin-tone modifier to the preceding emoji when the
    /// two form a modifier sequence (one two-column cluster).
    fn attach_modifier(&mut self, ch: char) -> bool {
        let Some((row, col)) = self.last_printed_cell() else {
            return false;
        };
        let line = &self.active_buffer()[row];
        let base = line[col];
        let is_wide = line.get(col + 1).is_some_and(Cell::is_continuation);
        if !is_wide || base.extra.is_some() {
            return false;
        }
        let mut cluster = String::with_capacity(8);
        cluster.push(base.ch);
        cluster.push(ch);
        if cluster.width() != 2 {
            return false;
        }
        self.active_buffer_mut()[row][col].extra = Some(ch);
        true
    }

    /// If `col` holds the right half of a wide character, erase that
    /// character (both cells) so the row can be edited at `col`.
    pub fn split_wide_at(&mut self, row: usize, col: usize, style: CellStyle) {
        let Some(line) = self.active_buffer_mut().get_mut(row) else {
            return;
        };
        if col > 0 && col < line.len() && line[col].is_continuation() {
            let blank = Cell::blank(style);
            line[col - 1] = blank;
            line[col] = blank;
        }
    }

    /// Fill `start..end` of `row` with blanks in `style`, erasing both halves
    /// of any wide character that straddles a boundary of the range.
    pub fn blank_cells(&mut self, row: usize, start: usize, end: usize, style: CellStyle) {
        self.split_wide_at(row, start, style);
        self.split_wide_at(row, end, style);
        let Some(line) = self.active_buffer_mut().get_mut(row) else {
            return;
        };
        let end = end.min(line.len());
        if start < end {
            line[start..end].fill(Cell::blank(style));
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
        let empty_cell = Cell::blank(CellStyle::default());

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
        let empty_cell = Cell::blank(CellStyle::default());

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
        let empty_cell = Cell::blank(CellStyle::default());

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

#[cfg(test)]
mod wide_char_tests {
    use super::*;

    fn text(screen: &TerminalScreen, row: usize) -> String {
        let mut s = String::new();
        for cell in &screen.active_buffer()[row] {
            cell.push_text(&mut s);
        }
        s.trim_end().to_string()
    }

    fn put(screen: &mut TerminalScreen, s: &str) {
        for c in s.chars() {
            screen.put_char(c);
        }
    }

    #[test]
    fn wide_char_takes_two_cells_and_advances_two_columns() {
        let mut s = TerminalScreen::new(2, 10);
        put(&mut s, "a中b");
        let row = &s.active_buffer()[0];
        assert_eq!(row[1].ch, '中');
        assert!(row[2].is_continuation());
        assert_eq!(row[3].ch, 'b');
        assert_eq!(s.cursor, (0, 4));
        assert_eq!(text(&s, 0), "a中b");
    }

    #[test]
    fn wide_char_wraps_when_one_column_is_left() {
        let mut s = TerminalScreen::new(3, 4);
        put(&mut s, "abc中");
        assert_eq!(text(&s, 0), "abc");
        assert_eq!(text(&s, 1), "中");
        assert!(s.lines_wrapped[0]);
        assert_eq!(s.cursor, (1, 2));
    }

    #[test]
    fn full_width_wide_line_defers_wrap_like_narrow_text() {
        let mut s = TerminalScreen::new(3, 4);
        put(&mut s, "ab中");
        assert!(s.wrap_pending);
        assert_eq!(s.cursor, (0, 3));
        s.carriage_return();
        s.newline();
        put(&mut s, "x");
        assert_eq!(text(&s, 1), "x");
        assert!(!s.lines_wrapped[0]);
    }

    #[test]
    fn vs16_widens_the_preceding_symbol() {
        // Only when the host terminal does; the flag is process-global and
        // nothing else in this crate's tests depends on it being off.
        unicode_width::set_variation_selectors_change_width(true);
        let mut s = TerminalScreen::new(2, 10);
        put(&mut s, "\u{2714}\u{FE0F}x");
        let row = &s.active_buffer()[0];
        assert_eq!(row[0].extra, Some('\u{FE0F}'));
        assert!(row[1].is_continuation());
        assert_eq!(row[2].ch, 'x');
        assert_eq!(s.cursor, (0, 3));
    }

    #[test]
    fn vs16_at_the_right_edge_keeps_the_base_narrow() {
        unicode_width::set_variation_selectors_change_width(true);
        let mut s = TerminalScreen::new(2, 3);
        put(&mut s, "ab\u{2714}\u{FE0F}");
        assert_eq!(text(&s, 0), "ab\u{2714}");
        assert!(s.wrap_pending);
        assert_eq!(text(&s, 1), "");
    }

    #[test]
    fn combining_mark_attaches_without_taking_a_cell() {
        let mut s = TerminalScreen::new(2, 4);
        // The second mark finds no free slot and is dropped.
        put(&mut s, "e\u{0301}\u{0301}x");
        assert_eq!(text(&s, 0), "e\u{0301}x");
        assert_eq!(s.cursor, (0, 2));
    }

    #[test]
    fn combining_marks_do_not_cause_spurious_wraps() {
        // Seen with pi: a padded full-width line holding an NFD accent
        // overflowed by one cell and pushed everything below down a row.
        let mut s = TerminalScreen::new(3, 6);
        put(&mut s, "abcde\u{0301}f");
        assert_eq!(text(&s, 0), "abcde\u{0301}f");
        assert!(s.wrap_pending);
        assert_eq!(text(&s, 1), "");
    }

    #[test]
    fn zwj_sequence_collapses_to_one_wide_cell() {
        let mut s = TerminalScreen::new(2, 10);
        put(&mut s, "👨\u{200D}👩\u{200D}👧x");
        assert_eq!(text(&s, 0), "👨x");
        assert_eq!(s.cursor, (0, 3));
    }

    #[test]
    fn skin_tone_modifier_joins_its_base() {
        let mut s = TerminalScreen::new(2, 10);
        put(&mut s, "👍\u{1F3FD}x");
        assert_eq!(text(&s, 0), "👍\u{1F3FD}x");
        assert_eq!(s.cursor, (0, 3));

        // On its own the modifier is an ordinary wide character.
        let mut s = TerminalScreen::new(2, 10);
        put(&mut s, "a\u{1F3FD}x");
        assert_eq!(s.cursor, (0, 4));
    }

    #[test]
    fn overwriting_or_erasing_half_a_wide_char_erases_it() {
        let mut s = TerminalScreen::new(2, 10);
        put(&mut s, "中文");
        s.move_cursor(0, 1);
        put(&mut s, "x");
        assert_eq!(text(&s, 0), " x文");
        s.blank_cells(0, 2, 3, CellStyle::default());
        assert_eq!(text(&s, 0), " x");
    }

    #[test]
    fn vs16_attaches_without_widening_when_the_host_keeps_wcwidth() {
        // The flag is process-global and other tests in this binary turn it
        // on, so decide from its current value rather than assuming.
        let widens = unicode_width::variation_selectors_change_width();
        let mut s = TerminalScreen::new(2, 10);
        put(&mut s, "\u{23F1}\u{FE0F}x");
        let row = &s.active_buffer()[0];
        assert_eq!(row[0].extra, Some('\u{FE0F}'));
        let x_col = if widens { 2 } else { 1 };
        assert_eq!(row[x_col].ch, 'x');
        assert_eq!(s.cursor, (0, x_col + 1));
    }

    #[test]
    fn zero_width_char_with_nothing_before_it_is_dropped() {
        let mut s = TerminalScreen::new(2, 4);
        put(&mut s, "\u{0301}a");
        assert_eq!(text(&s, 0), "a");
        assert_eq!(s.cursor, (0, 1));
    }
}
