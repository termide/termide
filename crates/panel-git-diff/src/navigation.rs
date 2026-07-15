//! Selection, collapse, and scroll navigation for the Git Diff Panel.

use termide_core::PanelEvent;

use crate::GitDiffPanel;

impl GitDiffPanel {
    /// Toggle collapse state for selected file
    pub(crate) fn toggle_collapse(&mut self) {
        if self.selected_file < self.diffs.len() {
            if self.collapsed.contains(&self.selected_file) {
                self.collapsed.remove(&self.selected_file);
            } else {
                self.collapsed.insert(self.selected_file);
            }
            self.calculate_total_lines();
        }
    }

    /// Collapse selected file
    pub(crate) fn collapse_current(&mut self) {
        if self.selected_file < self.diffs.len() {
            self.collapsed.insert(self.selected_file);
            self.calculate_total_lines();
        }
    }

    /// Expand selected file
    pub(crate) fn expand_current(&mut self) {
        if self.selected_file < self.diffs.len() {
            self.collapsed.remove(&self.selected_file);
            self.calculate_total_lines();
        }
    }

    /// Move selection up
    pub(crate) fn move_up(&mut self) {
        if self.selected_file > 0 {
            self.selected_file -= 1;
            self.ensure_file_visible();
        }
    }

    /// Move selection down
    pub(crate) fn move_down(&mut self) {
        if self.selected_file + 1 < self.diffs.len() {
            self.selected_file += 1;
            self.ensure_file_visible();
        }
    }

    /// Scroll up
    pub(crate) fn scroll_up(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_sub(amount);
    }

    /// Scroll down
    pub(crate) fn scroll_down(&mut self, amount: usize) {
        let max_scroll = self.total_lines.saturating_sub(self.visible_height);
        self.scroll = (self.scroll + amount).min(max_scroll);
    }

    /// Page up
    pub(crate) fn page_up(&mut self) {
        self.scroll_up(self.visible_height.saturating_sub(2));
    }

    /// Page down
    pub(crate) fn page_down(&mut self) {
        self.scroll_down(self.visible_height.saturating_sub(2));
    }

    /// Go to start
    pub(crate) fn go_to_start(&mut self) {
        self.scroll = 0;
        self.selected_file = 0;
    }

    /// Go to end
    pub(crate) fn go_to_end(&mut self) {
        if !self.diffs.is_empty() {
            self.selected_file = self.diffs.len() - 1;
            let max_scroll = self.total_lines.saturating_sub(self.visible_height);
            self.scroll = max_scroll;
        }
    }

    /// Ensure selected file is visible
    fn ensure_file_visible(&mut self) {
        // Calculate line number where selected file starts
        let mut line = 0;
        for i in 0..self.selected_file {
            line += 1; // File header
            if !self.collapsed.contains(&i) {
                for hunk in &self.diffs[i].hunks {
                    line += 1 + hunk.lines.len();
                }
            }
        }

        // Adjust scroll to make file visible
        if line < self.scroll {
            self.scroll = line;
        } else if line >= self.scroll + self.visible_height {
            self.scroll = line.saturating_sub(self.visible_height) + 1;
        }
    }

    /// Open selected file in editor
    pub(crate) fn open_file(&self) -> Vec<PanelEvent> {
        if let Some(diff) = self.diffs.get(self.selected_file) {
            let file_path = self.repo_path.join(&diff.path);
            if file_path.exists() {
                return vec![PanelEvent::OpenFile(file_path)];
            }
        }
        vec![]
    }
}
