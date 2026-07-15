//! Layout geometry, cursor movement, scrolling, and click hit-testing.

use crossterm::event::MouseEvent;

use crate::{BinaryPanel, ViewMode, Zone};

/// Bytes per section; rows are laid out in whole sections (16, 32, 48, …).
const SECTION: u64 = 16;

/// Bytes shown per row for the given inner width: as many whole 16-byte
/// sections as fit, at least one. Row layout is
/// `8 offset + 2 + n*3 hex + 1 + n ascii` ≈ `11 + 4n` columns.
fn bytes_per_row(width: u16) -> u64 {
    let usable = (width as i64 - 11).max(0);
    let fit = (usable / 4) as u64;
    (fit / SECTION).max(1) * SECTION
}

impl BinaryPanel {
    /// Bytes per row for the current width and mode.
    pub(crate) fn cols(&self) -> u64 {
        match self.mode {
            ViewMode::Hex => bytes_per_row(self.last_area.width),
            ViewMode::Text => (self.last_area.width as u64).max(1),
        }
    }

    /// Largest valid `top_byte`, aligned to `bpr`.
    fn max_top(&self, bpr: u64) -> u64 {
        if self.len == 0 {
            return 0;
        }
        (self.len.saturating_sub(1) / bpr) * bpr
    }

    /// Clamp `top_byte` to an aligned, in-range value.
    pub(crate) fn clamp_top(&mut self) {
        let bpr = self.cols();
        self.top_byte -= self.top_byte % bpr;
        let max_top = self.max_top(bpr);
        if self.top_byte > max_top {
            self.top_byte = max_top;
        }
    }

    /// Scroll the view (not the cursor) by whole rows.
    pub(crate) fn scroll_rows(&mut self, rows: i64) {
        let bpr = self.cols() as i64;
        self.top_byte = (self.top_byte as i64 + rows.saturating_mul(bpr)).max(0) as u64;
        self.clamp_top();
    }

    /// Move the cursor by `delta` bytes, optionally extending the selection.
    pub(crate) fn move_cursor(&mut self, delta: i64, extend: bool) {
        if self.len == 0 {
            return;
        }
        self.pending_nibble = None;
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        let max = (self.len - 1) as i64;
        self.cursor = (self.cursor as i64 + delta).clamp(0, max) as u64;
        self.ensure_cursor_visible();
    }

    /// Jump the cursor to an absolute byte, optionally extending the selection.
    pub(crate) fn set_cursor(&mut self, byte: u64, extend: bool) {
        if self.len == 0 {
            return;
        }
        self.pending_nibble = None;
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = byte.min(self.len - 1);
        self.ensure_cursor_visible();
    }

    /// Scroll so the cursor's row is visible.
    pub(crate) fn ensure_cursor_visible(&mut self) {
        let bpr = self.cols();
        let rows = (self.last_area.height as u64).max(1);
        let cur_row = self.cursor / bpr;
        let top_row = self.top_byte / bpr;
        if cur_row < top_row {
            self.top_byte = cur_row * bpr;
        } else if cur_row >= top_row + rows {
            self.top_byte = (cur_row + 1 - rows) * bpr;
        }
        self.clamp_top();
    }

    /// Inclusive selected byte range (or just the cursor byte).
    pub(crate) fn sel_range(&self) -> (u64, u64) {
        match self.anchor {
            Some(a) => (a.min(self.cursor), a.max(self.cursor)),
            None => (self.cursor, self.cursor),
        }
    }

    /// Map a mouse event's absolute position to a byte + zone in the hex grid.
    pub(crate) fn byte_at_event(&self, event: &MouseEvent) -> Option<(u64, Zone)> {
        if event.column < self.last_area.x || event.row < self.last_area.y {
            return None;
        }
        self.byte_at(
            event.column - self.last_area.x,
            event.row - self.last_area.y,
        )
    }

    /// Map a click at panel-relative `(cx, cy)` to a byte + zone.
    fn byte_at(&self, cx: u16, cy: u16) -> Option<(u64, Zone)> {
        let cols = self.cols();
        let row = self.top_byte / cols + cy as u64;
        let row_start = row * cols;
        let cx = cx as u64;
        if self.mode == ViewMode::Text {
            if cx >= cols {
                return None;
            }
            return Some((
                (row_start + cx).min(self.len.saturating_sub(1)),
                Zone::Ascii,
            ));
        }
        let ascii_start = 11 + cols * 3; // 8 offset + 2 + cols*3 hex + 1 sep
        if cx >= ascii_start && cx < ascii_start + cols {
            let i = cx - ascii_start;
            return Some(((row_start + i).min(self.len.saturating_sub(1)), Zone::Ascii));
        }
        if (10..10 + cols * 3).contains(&cx) {
            let i = (cx - 10) / 3;
            return Some(((row_start + i).min(self.len.saturating_sub(1)), Zone::Hex));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use std::path::PathBuf;

    fn panel_with(len: u64, w: u16, h: u16) -> BinaryPanel {
        let mut p = BinaryPanel::new(PathBuf::from("/dev/null")).unwrap();
        p.len = len;
        p.last_area = Rect::new(0, 0, w, h);
        p
    }

    #[test]
    fn bytes_per_row_rounds_to_16_byte_sections() {
        assert_eq!(bytes_per_row(80), 16);
        assert_eq!(bytes_per_row(140), 32);
        assert_eq!(bytes_per_row(10), 16);
    }

    #[test]
    fn cursor_moves_and_clamps() {
        let mut p = panel_with(100, 80, 10); // 16 cols
        p.move_cursor(1, false);
        assert_eq!(p.cursor, 1);
        p.move_cursor(16, false);
        assert_eq!(p.cursor, 17);
        p.move_cursor(1000, false);
        assert_eq!(p.cursor, 99);
        p.move_cursor(-1000, false);
        assert_eq!(p.cursor, 0);
    }

    #[test]
    fn shift_movement_builds_selection_range() {
        let mut p = panel_with(100, 80, 10);
        p.set_cursor(10, false);
        assert_eq!(p.anchor, None);
        p.move_cursor(3, true); // extend to 13
        assert_eq!(p.sel_range(), (10, 13));
        p.move_cursor(-1, false); // plain move clears selection
        assert_eq!(p.anchor, None);
    }

    #[test]
    fn click_maps_to_hex_and_ascii_zones() {
        let p = panel_with(100, 80, 10); // 16 cols, ascii_start = 11+48 = 59
                                         // hex byte 2 at col 10 + 2*3 = 16
        assert_eq!(p.byte_at(16, 0), Some((2, Zone::Hex)));
        // ascii byte 2 at col 59 + 2 = 61
        assert_eq!(p.byte_at(61, 0), Some((2, Zone::Ascii)));
        // second visible row, hex byte 0
        assert_eq!(p.byte_at(10, 1), Some((16, Zone::Hex)));
    }
}
