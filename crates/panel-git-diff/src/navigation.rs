//! Selection, collapse, and scroll navigation for the Git Diff Panel.

use termide_core::PanelEvent;

use crate::GitDiffPanel;

impl GitDiffPanel {
    /// Toggle collapse state for selected file
    pub(crate) fn toggle_collapse(&mut self) {
        if self.selected_file < self.diffs.len() {
            if !self.collapsed.remove(&self.selected_file) {
                self.collapsed.insert(self.selected_file);
            }
            self.after_fold();
        }
    }

    /// Collapse selected file
    pub(crate) fn collapse_current(&mut self) {
        if self.selected_file < self.diffs.len() {
            self.collapsed.insert(self.selected_file);
            self.after_fold();
        }
    }

    /// Expand selected file
    pub(crate) fn expand_current(&mut self) {
        if self.selected_file < self.diffs.len() {
            self.collapsed.remove(&self.selected_file);
            self.after_fold();
        }
    }

    /// Rows moved: the cursor goes to the folded or unfolded file's header,
    /// and a selection, counted in rows, goes.
    fn after_fold(&mut self) {
        self.calculate_total_lines();
        self.anchor = None;
        self.set_cursor(self.file_row(self.selected_file));
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
        self.move_cursor(-(self.visible_height.saturating_sub(2).max(1) as isize));
    }

    /// Page down
    pub(crate) fn page_down(&mut self) {
        self.move_cursor(self.visible_height.saturating_sub(2).max(1) as isize);
    }

    /// Go to start
    pub(crate) fn go_to_start(&mut self) {
        self.set_cursor(0);
    }

    /// Go to end
    pub(crate) fn go_to_end(&mut self) {
        self.set_cursor(self.total_lines.saturating_sub(1));
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
