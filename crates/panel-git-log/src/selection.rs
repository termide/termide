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

    /// Move selection up
    /// Item count of the currently open selector dropdown (repo or branch).
    pub(crate) fn open_dropdown_len(&self) -> usize {
        if self.repo_dropdown_open {
            self.repo_manager.len()
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

    /// Go to last commit
    pub(crate) fn go_to_end(&mut self) {
        if !self.commits.is_empty() {
            self.selected = self.commits.len() - 1;
            self.ensure_visible();
        }
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
