use std::path::PathBuf;

use super::FileManager;

impl FileManager {
    /// Toggle selection of current item
    pub(super) fn toggle_selection(&mut self) {
        if self.selected_items.contains(&self.selected) {
            self.selected_items.remove(&self.selected);
        } else {
            self.selected_items.insert(self.selected);
        }
    }

    /// Select all files
    pub(super) fn select_all(&mut self) {
        self.selected_items.clear();
        for i in 0..self.entries.len() {
            if let Some(entry) = self.entries.get(i) {
                if entry.name != ".." {
                    self.selected_items.insert(i);
                }
            }
        }
    }

    /// Move down with selection
    pub(super) fn move_down_with_selection(&mut self) {
        self.selected_items.insert(self.selected);
        self.move_down();
    }

    /// Move up with selection
    pub(super) fn move_up_with_selection(&mut self) {
        self.selected_items.insert(self.selected);
        self.move_up();
    }

    /// Page down with selection
    pub(super) fn page_down_with_selection(&mut self) {
        let start = self.selected;
        let target = (self.selected + self.visible_height).min(self.entries.len().saturating_sub(1));
        for i in start..=target {
            self.selected_items.insert(i);
        }
        self.selected = target;
        self.adjust_scroll_offset(self.visible_height);
    }

    /// Page up with selection
    pub(super) fn page_up_with_selection(&mut self) {
        let start = self.selected;
        let target = self.selected.saturating_sub(self.visible_height);
        for i in target..=start {
            self.selected_items.insert(i);
        }
        self.selected = target;
        self.scroll_offset = 0;
    }

    /// Select to beginning of list
    pub(super) fn select_to_home(&mut self) {
        for i in 0..=self.selected {
            self.selected_items.insert(i);
        }
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Select to end of list
    pub(super) fn select_to_end(&mut self) {
        let max_index = self.entries.len().saturating_sub(1);
        for i in self.selected..=max_index {
            self.selected_items.insert(i);
        }
        self.selected = max_index;
    }

    /// Get list of selected files/directories
    /// If nothing is selected, return current item under cursor
    pub fn get_selected_paths(&self) -> Vec<PathBuf> {
        if self.selected_items.is_empty() {
            // If no items are selected, return current one
            if let Some(entry) = self.entries.get(self.selected) {
                if entry.name != ".." {
                    return vec![self.current_path.join(&entry.name)];
                }
            }
            return Vec::new();
        }

        // Collect paths of selected items
        let mut paths = Vec::new();
        for &idx in &self.selected_items {
            if let Some(entry) = self.entries.get(idx) {
                if entry.name != ".." {
                    paths.push(self.current_path.join(&entry.name));
                }
            }
        }
        paths
    }

    /// Get count of selected items
    pub fn get_selected_count(&self) -> usize {
        self.selected_items.len()
    }

    /// Clear file selection
    pub fn clear_selection(&mut self) {
        self.selected_items.clear()  ;
    }

    /// Move down with toggle selection
    pub(super) fn move_down_with_toggle(&mut self) {
        // Toggle current
        if self.selected_items.contains(&self.selected) {
            self.selected_items.remove(&self.selected);
        } else {
            self.selected_items.insert(self.selected);
        }
        self.move_down();
    }

    /// Move up with toggle selection
    pub(super) fn move_up_with_toggle(&mut self) {
        // Toggle current
        if self.selected_items.contains(&self.selected) {
            self.selected_items.remove(&self.selected);
        } else {
            self.selected_items.insert(self.selected);
        }
        self.move_up();
    }

    /// Page down with toggle selection
    pub(super) fn page_down_with_toggle(&mut self) {
        let start = self.selected;
        let target = (self.selected + self.visible_height).min(self.entries.len().saturating_sub(1));

        // Toggle all elements from start to target
        for i in start..=target {
            if self.selected_items.contains(&i) {
                self.selected_items.remove(&i);
            } else {
                self.selected_items.insert(i);
            }
        }

        self.selected = target;
        self.adjust_scroll_offset(self.visible_height);
    }

    /// Page up with toggle selection
    pub(super) fn page_up_with_toggle(&mut self) {
        let start = self.selected;
        let target = self.selected.saturating_sub(self.visible_height);

        // Toggle all elements from target to start
        for i in target..=start {
            if self.selected_items.contains(&i) {
                self.selected_items.remove(&i);
            } else {
                self.selected_items.insert(i);
            }
        }

        self.selected = target;
        self.scroll_offset = 0;
    }
}
