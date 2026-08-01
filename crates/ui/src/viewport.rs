//! Viewport management for scrollable lists.
//!
//! Provides reusable scroll offset and cursor visibility logic
//! for panels that display scrollable lists of items.

/// Adjust a scroll `offset` so item `selected` is visible within a window of
/// `height` rows, returning the new offset.
///
/// This is the free-function form of [`Viewport::ensure_visible`] for the many
/// list panels and modals that track a bare `(offset, selected, height)` triple
/// rather than a [`Viewport`]. It performs only the bring-into-view step (no
/// upper clamp against a total count); callers that need one apply it
/// separately. A `height` of 0 leaves the offset unchanged.
#[must_use]
pub fn ensure_offset_visible(offset: usize, selected: usize, height: usize) -> usize {
    if height == 0 {
        return offset;
    }
    if selected < offset {
        selected
    } else if selected >= offset + height {
        selected + 1 - height
    } else {
        offset
    }
}

/// Viewport state for scrollable content.
///
/// Handles scroll offset calculation to ensure the cursor/selected item
/// remains visible within the viewport.
#[derive(Debug, Clone, Default)]
pub struct Viewport {
    /// Current scroll offset (first visible item index).
    pub offset: usize,
    /// Number of visible items in the viewport.
    pub visible_height: usize,
    /// Total number of items (for bounds checking).
    pub total_items: usize,
}

impl Viewport {
    /// Create a new viewport with given dimensions.
    pub fn new(visible_height: usize, total_items: usize) -> Self {
        Self {
            offset: 0,
            visible_height,
            total_items,
        }
    }

    /// Update visible height (e.g., on resize).
    pub fn set_visible_height(&mut self, height: usize) {
        self.visible_height = height;
        self.clamp_offset();
    }

    /// Update total items count.
    pub fn set_total_items(&mut self, total: usize) {
        self.total_items = total;
        self.clamp_offset();
    }

    /// Ensure scroll offset is within valid bounds.
    pub fn clamp_offset(&mut self) {
        if self.total_items == 0 || self.visible_height == 0 {
            self.offset = 0;
            return;
        }
        let max_offset = self.total_items.saturating_sub(self.visible_height);
        if self.offset > max_offset {
            self.offset = max_offset;
        }
    }

    /// Ensure the given cursor position is visible.
    ///
    /// Adjusts scroll offset if necessary to bring cursor into view.
    /// Returns true if offset was changed.
    pub fn ensure_visible(&mut self, cursor: usize) -> bool {
        if self.visible_height == 0 {
            return false;
        }

        let old_offset = self.offset;

        // Cursor is above visible area
        if cursor < self.offset {
            self.offset = cursor;
        }
        // Cursor is below visible area
        else if cursor >= self.offset + self.visible_height {
            self.offset = cursor.saturating_sub(self.visible_height) + 1;
        }

        self.clamp_offset();
        self.offset != old_offset
    }

    /// Scroll up by one item. Returns true if scroll occurred.
    pub fn scroll_up(&mut self) -> bool {
        if self.offset > 0 {
            self.offset -= 1;
            true
        } else {
            false
        }
    }

    /// Scroll down by one item. Returns true if scroll occurred.
    pub fn scroll_down(&mut self) -> bool {
        let max_offset = self.total_items.saturating_sub(self.visible_height);
        if self.offset < max_offset {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    /// Scroll up by a page (visible_height items).
    pub fn page_up(&mut self) -> bool {
        if self.offset > 0 {
            self.offset = self.offset.saturating_sub(self.visible_height);
            true
        } else {
            false
        }
    }

    /// Scroll down by a page (visible_height items).
    pub fn page_down(&mut self) -> bool {
        let max_offset = self.total_items.saturating_sub(self.visible_height);
        if self.offset < max_offset {
            self.offset = (self.offset + self.visible_height).min(max_offset);
            true
        } else {
            false
        }
    }

    /// Scroll to top.
    pub fn scroll_to_top(&mut self) -> bool {
        if self.offset > 0 {
            self.offset = 0;
            true
        } else {
            false
        }
    }

    /// Scroll to bottom.
    pub fn scroll_to_bottom(&mut self) -> bool {
        let max_offset = self.total_items.saturating_sub(self.visible_height);
        if self.offset < max_offset {
            self.offset = max_offset;
            true
        } else {
            false
        }
    }

    /// Check if a given item index is visible.
    pub fn is_visible(&self, index: usize) -> bool {
        index >= self.offset && index < self.offset + self.visible_height
    }

    /// Get the range of visible items.
    pub fn visible_range(&self) -> std::ops::Range<usize> {
        self.offset..(self.offset + self.visible_height).min(self.total_items)
    }

    /// Get maximum scroll offset.
    pub fn max_offset(&self) -> usize {
        self.total_items.saturating_sub(self.visible_height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let vp = Viewport::new(10, 100);
        assert_eq!(vp.offset, 0);
        assert_eq!(vp.visible_height, 10);
        assert_eq!(vp.total_items, 100);
    }

    #[test]
    fn test_ensure_visible_cursor_above() {
        let mut vp = Viewport::new(10, 100);
        vp.offset = 20;
        assert!(vp.ensure_visible(15));
        assert_eq!(vp.offset, 15);
    }

    #[test]
    fn test_ensure_visible_cursor_below() {
        let mut vp = Viewport::new(10, 100);
        vp.offset = 0;
        assert!(vp.ensure_visible(15));
        assert_eq!(vp.offset, 6); // 15 - 10 + 1
    }

    #[test]
    fn test_ensure_visible_already_visible() {
        let mut vp = Viewport::new(10, 100);
        vp.offset = 10;
        assert!(!vp.ensure_visible(15)); // 15 is in [10, 20)
        assert_eq!(vp.offset, 10);
    }

    #[test]
    fn test_clamp_offset() {
        let mut vp = Viewport::new(10, 15);
        vp.offset = 20;
        vp.clamp_offset();
        assert_eq!(vp.offset, 5); // max is 15 - 10 = 5
    }

    #[test]
    fn test_scroll_down() {
        let mut vp = Viewport::new(10, 100);
        assert!(vp.scroll_down());
        assert_eq!(vp.offset, 1);
    }

    #[test]
    fn test_scroll_up() {
        let mut vp = Viewport::new(10, 100);
        vp.offset = 5;
        assert!(vp.scroll_up());
        assert_eq!(vp.offset, 4);
    }

    #[test]
    fn test_page_down() {
        let mut vp = Viewport::new(10, 100);
        assert!(vp.page_down());
        assert_eq!(vp.offset, 10);
    }

    #[test]
    fn test_page_up() {
        let mut vp = Viewport::new(10, 100);
        vp.offset = 25;
        assert!(vp.page_up());
        assert_eq!(vp.offset, 15);
    }

    #[test]
    fn test_visible_range() {
        let mut vp = Viewport::new(10, 100);
        vp.offset = 20;
        assert_eq!(vp.visible_range(), 20..30);
    }

    #[test]
    fn test_visible_range_at_end() {
        let mut vp = Viewport::new(10, 25);
        vp.offset = 20;
        assert_eq!(vp.visible_range(), 20..25);
    }

    #[test]
    fn test_ensure_offset_visible_free_fn() {
        // Above the window: snap to the selected row.
        assert_eq!(ensure_offset_visible(20, 15, 10), 15);
        // Below the window: bring the selected row to the last visible slot.
        assert_eq!(ensure_offset_visible(0, 15, 10), 6); // 15 + 1 - 10
                                                         // Already visible: unchanged.
        assert_eq!(ensure_offset_visible(10, 15, 10), 10);
        // Zero height: never touch the offset (and never underflow).
        assert_eq!(ensure_offset_visible(3, 0, 0), 3);
    }

    #[test]
    fn test_is_visible() {
        let mut vp = Viewport::new(10, 100);
        vp.offset = 20;
        assert!(!vp.is_visible(19));
        assert!(vp.is_visible(20));
        assert!(vp.is_visible(29));
        assert!(!vp.is_visible(30));
    }
}
