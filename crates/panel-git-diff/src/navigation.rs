//! Selection, collapse, and scroll navigation for the Git Diff Panel.

use std::collections::HashSet;

use termide_core::PanelEvent;

use crate::{FileDiff, GitDiffPanel};

/// Which files are folded: each follows `all_folded` but for those the user
/// flipped by hand. Files are known by path and side rather than position,
/// so a fold stays with its file when a refresh adds or drops others, and a
/// file new to the panel follows the last fold-all.
#[derive(Debug, Default)]
pub(crate) struct Folds {
    all_folded: bool,
    flipped: HashSet<(String, bool)>,
}

fn fold_key(diff: &FileDiff) -> (String, bool) {
    (diff.path.clone(), diff.staged)
}

impl Folds {
    fn is_folded(&self, diff: &FileDiff) -> bool {
        self.all_folded != self.flipped.contains(&fold_key(diff))
    }

    fn set_folded(&mut self, diff: &FileDiff, folded: bool) {
        if folded == self.all_folded {
            self.flipped.remove(&fold_key(diff));
        } else {
            self.flipped.insert(fold_key(diff));
        }
    }

    /// Forget the files no longer shown.
    pub(crate) fn retain(&mut self, diffs: &[FileDiff]) {
        let shown: HashSet<_> = diffs.iter().map(fold_key).collect();
        self.flipped.retain(|key| shown.contains(key));
    }
}

impl GitDiffPanel {
    /// Whether file `file` is folded to its header.
    pub(crate) fn is_collapsed(&self, file: usize) -> bool {
        self.diffs
            .get(file)
            .is_some_and(|diff| self.folds.is_folded(diff))
    }

    fn set_collapsed(&mut self, file: usize, folded: bool) {
        if let Some(diff) = self.diffs.get(file) {
            self.folds.set_folded(diff, folded);
            self.after_fold();
        }
    }

    /// Toggle collapse state for selected file
    pub(crate) fn toggle_collapse(&mut self) {
        self.set_collapsed(self.selected_file, !self.is_collapsed(self.selected_file));
    }

    /// Collapse selected file
    pub(crate) fn collapse_current(&mut self) {
        self.set_collapsed(self.selected_file, true);
    }

    /// Expand selected file
    pub(crate) fn expand_current(&mut self) {
        self.set_collapsed(self.selected_file, false);
    }

    /// Fold every file when any is open, otherwise unfold them all. Files
    /// a later refresh brings follow suit.
    pub(crate) fn toggle_all_folds(&mut self) {
        let any_open = (0..self.diffs.len()).any(|i| !self.is_collapsed(i));
        self.folds = Folds {
            all_folded: any_open,
            flipped: HashSet::new(),
        };
        self.after_fold();
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
