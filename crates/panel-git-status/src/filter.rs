//! Repo/branch selector dropdown filtering for the Git Status Panel.

use termide_git::{self as git};

use crate::GitStatusPanel;

impl GitStatusPanel {
    /// Return indices into `self.branches` that match the current filter.
    /// When the filter is empty, returns all indices.
    pub(crate) fn filtered_branch_indices(&self) -> Vec<usize> {
        if self.branch_filter.is_empty() {
            (0..self.branches.len()).collect()
        } else {
            let f = self.branch_filter.to_lowercase();
            self.branches
                .iter()
                .enumerate()
                .filter(|(_, b)| b.to_lowercase().contains(&f))
                .map(|(i, _)| i)
                .collect()
        }
    }

    /// Reset the branch filter state.
    pub(crate) fn reset_branch_filter(&mut self) {
        self.branch_filter.clear();
        self.show_branch_filter = false;
    }

    /// Return indices into repo list that match the current filter.
    pub(crate) fn filtered_repo_indices(&self) -> Vec<usize> {
        let repos = self.repo_manager.repos();
        if self.repo_filter.is_empty() {
            (0..repos.len()).collect()
        } else {
            let f = self.repo_filter.to_lowercase();
            repos
                .iter()
                .enumerate()
                .filter(|(_, p)| git::get_repo_name(p).to_lowercase().contains(&f))
                .map(|(i, _)| i)
                .collect()
        }
    }

    /// Reset the repo filter state.
    pub(crate) fn reset_repo_filter(&mut self) {
        self.repo_filter.clear();
        self.show_repo_filter = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> GitStatusPanel {
        GitStatusPanel::new(&[])
    }

    #[test]
    fn empty_branch_filter_returns_all_indices() {
        let mut p = panel();
        p.branches = vec!["main".into(), "dev".into()];
        assert_eq!(p.filtered_branch_indices(), vec![0, 1]);
    }

    #[test]
    fn branch_filter_matches_case_insensitive_substrings() {
        let mut p = panel();
        p.branches = vec![
            "main".into(),
            "feature/login".into(),
            "Feature/Logout".into(),
        ];
        p.branch_filter = "log".into();
        assert_eq!(p.filtered_branch_indices(), vec![1, 2]);
    }

    #[test]
    fn branch_filter_with_no_match_is_empty() {
        // The Enter handler relies on this: an empty result means `.get(cursor)`
        // yields None, so no branch is checked out (previously it fell back to
        // index 0 and checked out the first branch).
        let mut p = panel();
        p.branches = vec!["main".into(), "dev".into()];
        p.branch_filter = "zzz".into();
        assert!(p.filtered_branch_indices().is_empty());
    }
}
