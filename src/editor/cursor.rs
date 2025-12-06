use std::cmp::{max, min};

/// Cursor position in document
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    /// Line number (0-based)
    pub line: usize,
    /// Position in line in graphemes (0-based)
    pub column: usize,
}

impl Cursor {
    /// Create a new cursor at position (0, 0)
    pub fn new() -> Self {
        Self { line: 0, column: 0 }
    }

    /// Create cursor at specified position
    pub fn at(line: usize, column: usize) -> Self {
        Self { line, column }
    }

    /// Move cursor up
    pub fn move_up(&mut self, lines: usize) {
        self.line = self.line.saturating_sub(lines);
    }

    /// Move cursor down
    pub fn move_down(&mut self, lines: usize, max_line: usize) {
        self.line = min(self.line + lines, max_line);
    }

    /// Move cursor left
    pub fn move_left(&mut self, columns: usize) {
        if self.column >= columns {
            self.column -= columns;
        } else if self.line > 0 {
            // Move to previous line
            self.line -= 1;
            self.column = usize::MAX; // Will be corrected in clamp_column
        } else {
            self.column = 0;
        }
    }

    /// Move cursor right
    pub fn move_right(&mut self, columns: usize, line_len: usize, max_line: usize) {
        self.column += columns;
        if self.column > line_len && self.line < max_line {
            // Move to next line
            self.line += 1;
            self.column = 0;
        }
    }

    /// Clamp column to maximum line length
    pub fn clamp_column(&mut self, max_column: usize) {
        self.column = min(self.column, max_column);
    }
}

impl Default for Cursor {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialOrd for Cursor {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Cursor {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.line.cmp(&other.line) {
            std::cmp::Ordering::Equal => self.column.cmp(&other.column),
            other => other,
        }
    }
}

/// Text selection
#[derive(Debug, Clone)]
pub struct Selection {
    /// Start point of selection (anchor) - doesn't move
    pub anchor: Cursor,
    /// Active point (moves with cursor)
    pub active: Cursor,
}

impl Selection {
    /// Create a new selection
    pub fn new(anchor: Cursor, active: Cursor) -> Self {
        Self { anchor, active }
    }

    /// Start of selection (minimum position)
    pub fn start(&self) -> Cursor {
        min(self.anchor, self.active)
    }

    /// End of selection (maximum position)
    pub fn end(&self) -> Cursor {
        max(self.anchor, self.active)
    }

    /// Selection is empty (start == end)
    pub fn is_empty(&self) -> bool {
        self.anchor == self.active
    }

    /// Check if selection contains given position (test-only)
    #[cfg(test)]
    pub fn contains(&self, cursor: &Cursor) -> bool {
        let start = self.start();
        let end = self.end();
        cursor >= &start && cursor <= &end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_ordering() {
        let c1 = Cursor::at(0, 5);
        let c2 = Cursor::at(1, 0);
        let c3 = Cursor::at(1, 5);

        assert!(c1 < c2);
        assert!(c2 < c3);
        assert!(c1 < c3);
    }

    #[test]
    fn test_selection_range() {
        let sel = Selection::new(Cursor::at(1, 5), Cursor::at(3, 10));
        assert_eq!(sel.start(), Cursor::at(1, 5));
        assert_eq!(sel.end(), Cursor::at(3, 10));

        let sel_rev = Selection::new(Cursor::at(3, 10), Cursor::at(1, 5));
        assert_eq!(sel_rev.start(), Cursor::at(1, 5));
        assert_eq!(sel_rev.end(), Cursor::at(3, 10));
    }

    #[test]
    fn test_selection_contains() {
        let sel = Selection::new(Cursor::at(1, 5), Cursor::at(3, 10));
        assert!(sel.contains(&Cursor::at(2, 0)));
        assert!(sel.contains(&Cursor::at(1, 5)));
        assert!(sel.contains(&Cursor::at(3, 10)));
        assert!(!sel.contains(&Cursor::at(0, 0)));
        assert!(!sel.contains(&Cursor::at(4, 0)));
    }
}
