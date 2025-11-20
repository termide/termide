use super::Cursor;

/// Viewport for virtual scrolling
/// Tracks which part of document is visible on screen
#[derive(Debug, Clone)]
pub struct Viewport {
    /// First visible line (0-based)
    pub top_line: usize,
    /// Number of visible lines
    pub height: usize,
    /// Horizontal scroll (left column)
    pub left_column: usize,
    /// Width of visible area
    pub width: usize,
}

impl Viewport {
    /// Create a new viewport
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            top_line: 0,
            height,
            left_column: 0,
            width,
        }
    }

    /// Update viewport dimensions
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
    }

    /// Get index of last visible line (exclusive)
    pub fn bottom_line(&self) -> usize {
        self.top_line + self.height
    }

    /// Get index of last visible column (exclusive)
    pub fn right_column(&self) -> usize {
        self.left_column + self.width
    }

    /// Check if line is visible
    pub fn is_line_visible(&self, line: usize) -> bool {
        line >= self.top_line && line < self.bottom_line()
    }

    /// Check if column is visible
    pub fn is_column_visible(&self, column: usize) -> bool {
        column >= self.left_column && column < self.right_column()
    }

    /// Check if cursor is visible
    pub fn is_cursor_visible(&self, cursor: &Cursor) -> bool {
        self.is_line_visible(cursor.line) && self.is_column_visible(cursor.column)
    }

    /// Scroll viewport to make cursor visible
    /// Returns true if viewport was changed
    pub fn ensure_cursor_visible(&mut self, cursor: &Cursor, total_lines: usize) -> bool {
        let mut changed = false;

        // Vertical scroll
        if cursor.line < self.top_line {
            // Cursor above viewport - scroll up
            self.top_line = cursor.line;
            changed = true;
        } else if cursor.line >= self.bottom_line() {
            // Cursor below viewport - scroll down
            self.top_line = cursor.line.saturating_sub(self.height - 1);
            changed = true;
        }

        // Limit top_line to avoid empty space at bottom
        let max_top = total_lines.saturating_sub(self.height);
        if self.top_line > max_top {
            self.top_line = max_top;
            changed = true;
        }

        // Horizontal scroll
        if cursor.column < self.left_column {
            // Cursor left of viewport
            self.left_column = cursor.column;
            changed = true;
        } else if cursor.column >= self.right_column() {
            // Cursor right of viewport
            self.left_column = cursor.column.saturating_sub(self.width - 1);
            changed = true;
        }

        changed
    }

    /// Scroll up by N lines
    pub fn scroll_up(&mut self, lines: usize) -> bool {
        if self.top_line > 0 {
            self.top_line = self.top_line.saturating_sub(lines);
            true
        } else {
            false
        }
    }

    /// Scroll down by N lines
    pub fn scroll_down(&mut self, lines: usize, total_lines: usize) -> bool {
        let max_top = total_lines.saturating_sub(self.height);
        if self.top_line < max_top {
            self.top_line = (self.top_line + lines).min(max_top);
            true
        } else {
            false
        }
    }

    /// Scroll left by N columns
    pub fn scroll_left(&mut self, columns: usize) -> bool {
        if self.left_column > 0 {
            self.left_column = self.left_column.saturating_sub(columns);
            true
        } else {
            false
        }
    }

    /// Scroll right by N columns
    pub fn scroll_right(&mut self, columns: usize) -> bool {
        self.left_column += columns;
        true
    }

    /// Scroll to document start
    pub fn scroll_to_top(&mut self) -> bool {
        if self.top_line != 0 {
            self.top_line = 0;
            true
        } else {
            false
        }
    }

    /// Scroll to document end
    pub fn scroll_to_bottom(&mut self, total_lines: usize) -> bool {
        let max_top = total_lines.saturating_sub(self.height);
        if self.top_line != max_top {
            self.top_line = max_top;
            true
        } else {
            false
        }
    }

    /// Center viewport on cursor
    pub fn center_on_cursor(&mut self, cursor: &Cursor, total_lines: usize) -> bool {
        let target_top = cursor.line.saturating_sub(self.height / 2);
        let max_top = total_lines.saturating_sub(self.height);
        let new_top = target_top.min(max_top);

        if self.top_line != new_top {
            self.top_line = new_top;
            true
        } else {
            false
        }
    }

    /// Get relative cursor position in viewport
    /// Returns (row, col) relative to viewport start
    pub fn cursor_to_viewport_pos(&self, cursor: &Cursor) -> Option<(usize, usize)> {
        if !self.is_cursor_visible(cursor) {
            return None;
        }

        let row = cursor.line - self.top_line;
        let col = cursor.column - self.left_column;
        Some((row, col))
    }

    /// Convert viewport position to absolute cursor position
    pub fn viewport_pos_to_cursor(&self, row: usize, col: usize) -> Cursor {
        Cursor::at(self.top_line + row, self.left_column + col)
    }
}

impl Default for Viewport {
    fn default() -> Self {
        Self::new(80, 24)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_viewport_visibility() {
        let vp = Viewport::new(80, 24);
        assert!(vp.is_line_visible(0));
        assert!(vp.is_line_visible(23));
        assert!(!vp.is_line_visible(24));
    }

    #[test]
    fn test_ensure_cursor_visible_below() {
        let mut vp = Viewport::new(80, 24);
        let cursor = Cursor::at(30, 0);

        let changed = vp.ensure_cursor_visible(&cursor, 100);
        assert!(changed);
        assert!(vp.is_cursor_visible(&cursor));
        assert_eq!(vp.top_line, 7); // 30 - 23
    }

    #[test]
    fn test_ensure_cursor_visible_above() {
        let mut vp = Viewport::new(80, 24);
        vp.top_line = 10;

        let cursor = Cursor::at(5, 0);
        let changed = vp.ensure_cursor_visible(&cursor, 100);

        assert!(changed);
        assert!(vp.is_cursor_visible(&cursor));
        assert_eq!(vp.top_line, 5);
    }

    #[test]
    fn test_scroll_down() {
        let mut vp = Viewport::new(80, 24);
        let changed = vp.scroll_down(5, 100);

        assert!(changed);
        assert_eq!(vp.top_line, 5);
    }

    #[test]
    fn test_scroll_down_limit() {
        let mut vp = Viewport::new(80, 24);
        let changed = vp.scroll_down(100, 30);

        assert!(changed);
        assert_eq!(vp.top_line, 6); // 30 - 24
    }

    #[test]
    fn test_center_on_cursor() {
        let mut vp = Viewport::new(80, 24);
        let cursor = Cursor::at(50, 0);

        let changed = vp.center_on_cursor(&cursor, 100);
        assert!(changed);
        assert_eq!(vp.top_line, 38); // 50 - 12
        assert!(vp.is_cursor_visible(&cursor));
    }

    #[test]
    fn test_cursor_to_viewport_pos() {
        let mut vp = Viewport::new(80, 24);
        vp.top_line = 10;
        vp.left_column = 5;

        let cursor = Cursor::at(15, 10);
        let pos = vp.cursor_to_viewport_pos(&cursor);

        assert_eq!(pos, Some((5, 5)));
    }

    #[test]
    fn test_viewport_pos_to_cursor() {
        let mut vp = Viewport::new(80, 24);
        vp.top_line = 10;
        vp.left_column = 5;

        let cursor = vp.viewport_pos_to_cursor(5, 5);
        assert_eq!(cursor, Cursor::at(15, 10));
    }

    #[test]
    fn test_horizontal_scroll() {
        let mut vp = Viewport::new(80, 24);
        let cursor = Cursor::at(0, 100);

        let changed = vp.ensure_cursor_visible(&cursor, 100);
        assert!(changed);
        assert!(vp.is_cursor_visible(&cursor));
        assert_eq!(vp.left_column, 21); // 100 - 79
    }
}
