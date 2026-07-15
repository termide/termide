//! Async commit-log refresh: worker dispatch, result polling, and state reset.

use termide_git::{self as git, CommitInfo};

use crate::GitLogPanel;

/// Snapshot returned by the background refresh worker.
pub(crate) struct GitLogRefreshResult {
    pub(crate) branch: Option<String>,
    pub(crate) branches: Vec<String>,
    pub(crate) commits: Vec<CommitInfo>,
}

impl GitLogPanel {
    /// Trigger a refresh of the commit log.
    ///
    /// Returns immediately; the `get_all_branches` and
    /// `get_log_with_graph` git commands run on a worker thread and
    /// `tick()` folds the result in via [`Self::poll_refresh`].
    /// Reset displayed log state to empty — used when no repository is selected
    /// (e.g. the current repo's `.git` was deleted) so stale commits/branch
    /// don't linger.
    pub(crate) fn clear_git_state(&mut self) {
        self.branch = None;
        self.branches.clear();
        self.selected_branch = None;
        self.commits.clear();
        self.selected = 0;
        self.scroll = 0;
    }

    pub fn refresh(&mut self) {
        let Some(repo) = self.repo_manager.current() else {
            self.clear_git_state();
            return;
        };
        let repo = repo.to_path_buf();
        let count = self.commit_count;
        let selected_branch = self.selected_branch.clone();
        let unicode_graph = self.unicode_graph;

        let (tx, rx) = std::sync::mpsc::channel();
        self.refresh_rx = Some(rx);
        std::thread::spawn(move || {
            let branch = git::get_current_branch(&repo);
            let branches = git::get_all_branches(&repo);
            let commits = if unicode_graph {
                git::get_log_graph_unicode(&repo, count, selected_branch.as_deref())
            } else {
                git::get_log_with_graph(&repo, count, selected_branch.as_deref())
            };
            let _ = tx.send(GitLogRefreshResult {
                branch,
                branches,
                commits,
            });
        });
    }

    /// Apply an async refresh result if one is ready. Returns `true`
    /// when the panel state changed so the caller can emit
    /// `NeedsRedraw`.
    pub(crate) fn poll_refresh(&mut self) -> bool {
        let Some(rx) = self.refresh_rx.as_ref() else {
            return false;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.refresh_rx = None;
                return true;
            }
        };
        self.refresh_rx = None;

        self.branch = result.branch;
        self.branches = result.branches;

        // If the previously selected branch no longer exists, reset to HEAD
        if let Some(ref b) = self.selected_branch {
            if !self.branches.contains(b) {
                self.selected_branch = None;
            }
        }

        self.commits = result.commits;
        if self.selected >= self.commits.len() && !self.commits.is_empty() {
            self.selected = self.commits.len() - 1;
        }
        self.scroll = 0;
        true
    }
}
