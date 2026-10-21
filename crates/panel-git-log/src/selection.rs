//! Section navigation, cursor movement, scrolling, and selected-commit access.

use termide_git::CommitInfo;

use crate::{GitLogPanel, Section};

impl GitLogPanel {
    /// Move to next section (cycles: RepoSelector → BranchSelector → Commits → RepoSelector)
    pub(crate) fn next_section(&mut self) {
        self.current_section = match self.current_section {
            Section::RepoSelector => Section::BranchSelector,
            Section::BranchSelector => Section::Commits,
            Section::Commits => Section::RepoSelector,
        };
    }

    /// Move to previous section
    pub(crate) fn prev_section(&mut self) {
        self.current_section = match self.current_section {
            Section::RepoSelector => Section::Commits,
            Section::BranchSelector => Section::RepoSelector,
            Section::Commits => Section::BranchSelector,
        };
    }

    /// The project's repositories as the tree the dropdown draws, the closed
    /// folders' contents left out. Recomputed rather than stored, so the renderer
    /// and the mouse hit-test can never disagree about a row.
    pub(crate) fn repo_rows(&self) -> Vec<termide_git::RepoRow> {
        self.repo_folds
            .rows(self.repo_manager.repos(), self.repo_manager.project_root())
    }

    /// Open the repository dropdown on the panel's repository: the folders above
    /// it open, the cursor on its row, the window from the top.
    pub(crate) fn open_repo_dropdown(&mut self) {
        self.dropdown_cursor = self.repo_folds.reveal(
            self.repo_manager.repos(),
            self.repo_manager.project_root(),
            self.repo_manager.selected_index(),
        );
        self.dropdown_scroll = 0;
        self.repo_dropdown_open = true;
    }

    /// Open or close the folder at `row` of the repository tree, leaving the
    /// cursor on it; `open` says which, `None` turns it over. `false` when the
    /// row is not a folder, or already as asked.
    pub(crate) fn set_repo_dir_open(&mut self, row: usize, open: Option<bool>) -> bool {
        let rows = self.repo_rows();
        let changed = self.repo_folds.set_open(&rows, row, open);
        if changed {
            self.dropdown_cursor = row;
        }
        changed
    }

    /// Move selection up
    /// Item count of the currently open selector dropdown (repo or branch).
    pub(crate) fn open_dropdown_len(&self) -> usize {
        if self.repo_dropdown_open {
            self.repo_rows().len()
        } else if self.branch_dropdown_open {
            self.branches.len()
        } else {
            0
        }
    }

    pub(crate) fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.ensure_visible();
        }
    }

    /// Move selection down
    pub(crate) fn move_down(&mut self) {
        if self.selected + 1 < self.commits.len() {
            self.selected += 1;
            self.ensure_visible();
        }
    }

    /// Page up
    pub(crate) fn page_up(&mut self, page_size: usize) {
        if self.selected > page_size {
            self.selected -= page_size;
        } else {
            self.selected = 0;
        }
        self.ensure_visible();
    }

    /// Page down
    pub(crate) fn page_down(&mut self, page_size: usize) {
        let max = self.commits.len().saturating_sub(1);
        if self.selected + page_size < max {
            self.selected += page_size;
        } else {
            self.selected = max;
        }
        self.ensure_visible();
    }

    /// Go to first commit
    pub(crate) fn go_to_start(&mut self) {
        self.selected = 0;
        self.scroll = 0;
    }

    /// Go to last commit; while history remains unread, keep reading and
    /// keep the selection on the last row until it ends.
    pub(crate) fn go_to_end(&mut self) {
        self.follow_end = self.has_more();
        if !self.commits.is_empty() {
            self.selected = self.commits.len() - 1;
            self.ensure_visible();
        }
    }

    /// Rows of commits the panel shows, below its selectors.
    pub(crate) fn visible_rows(&self) -> usize {
        (self.last_area.height.saturating_sub(2) as usize).max(1)
    }

    /// Mouse wheel: scroll the commit list by `rows` and pull the selection
    /// along, rather than move the selection and wait for the view to follow.
    pub(crate) fn wheel(&mut self, rows: isize) {
        (self.scroll, self.selected) = termide_ui::wheel_scroll(
            self.scroll,
            self.selected,
            self.commits.len(),
            self.visible_rows(),
            rows,
        );
    }

    /// Ensure selected item is visible
    pub(crate) fn ensure_visible(&mut self) {
        let visible_height = self.last_area.height.saturating_sub(2) as usize;
        self.scroll = termide_ui::ensure_offset_visible(self.scroll, self.selected, visible_height);
    }

    /// Get selected commit
    pub(crate) fn selected_commit(&self) -> Option<&CommitInfo> {
        self.commits.get(self.selected)
    }
}
