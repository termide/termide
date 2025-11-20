use super::FileManager;

impl FileManager {
    /// Move cursor down
    pub(super) fn move_down(&mut self) {
        if self.selected < self.entries.len().saturating_sub(1) {
            self.selected += 1;
        }
    }

    /// Move cursor up
    pub(super) fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Adjust scroll_offset for current item visibility
    pub(super) fn adjust_scroll_offset(&mut self, visible_height: usize) {
        if self.selected >= self.scroll_offset + visible_height {
            self.scroll_offset = self.selected - visible_height + 1;
        } else if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
    }
}
