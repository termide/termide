//! Async git-status refresh, state recomputation, and file-tree rebuilding.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use termide_git::{self as git, StagedFile, UnstagedFile};

use crate::tree;
use crate::GitStatusPanel;

/// Snapshot returned by the background refresh worker. All the git
/// commands the panel needs for a render run on the worker thread; the
/// UI thread just swaps these fields into place when the result is
/// ready, so the panel never blocks on a slow `git status --porcelain`
/// over a large repository.
pub(crate) struct GitStatusRefreshResult {
    /// The `viewed` branch the worker was started for; a result for another
    /// one is stale.
    pub(crate) requested: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) branches: Vec<String>,
    /// `requested`, dropped when it is gone or is the main copy's branch now.
    pub(crate) viewed: Option<String>,
    pub(crate) worktrees: HashMap<String, PathBuf>,
    pub(crate) ahead: usize,
    pub(crate) behind: usize,
    pub(crate) unstaged_files: Vec<UnstagedFile>,
    pub(crate) staged_files: Vec<StagedFile>,
    pub(crate) stash_count: usize,
}

/// The working copy that shows `viewed`: its linked worktree, or the main
/// copy `repo` when no branch is viewed. `None` when `viewed` is checked out
/// nowhere.
pub(crate) fn resolve_work_dir(
    repo: &Path,
    viewed: Option<&str>,
    worktrees: &HashMap<String, PathBuf>,
) -> Option<PathBuf> {
    match viewed {
        None => Some(repo.to_path_buf()),
        Some(branch) => worktrees.get(branch).cloned(),
    }
}

impl GitStatusPanel {
    /// Trigger a refresh of git status.
    ///
    /// Returns immediately; the heavy `git status` / `git branch` /
    /// `git rev-list` commands run on a worker thread and the result is
    /// folded in by `tick()` via [`Self::poll_refresh`]. The panel
    /// stays in `is_loading` state until then.
    /// Reset all displayed git state to empty. Used when no repository is
    /// selected — e.g. the current repo's `.git` was just deleted — so stale
    /// files, branch and counts don't linger in the panel.
    pub(crate) fn clear_git_state(&mut self) {
        self.branch = None;
        self.branches.clear();
        self.viewed = None;
        self.worktrees.clear();
        self.ahead = 0;
        self.behind = 0;
        self.unstaged_files.clear();
        self.staged_files.clear();
        self.stash_count = 0;
        self.rebuild_trees();
        self.cursor = 0;
    }

    pub fn refresh(&mut self) {
        // Coalesce: a worker is already running, so mark that one more pass is
        // needed when it finishes rather than spawning a parallel worker (and a
        // fresh batch of git subprocesses) for every queued event.
        if self.refresh_rx.is_some() {
            self.refresh_pending = true;
            return;
        }
        self.is_loading = true;

        let repo = match self.repo_manager.current() {
            Some(r) => r.to_path_buf(),
            None => {
                // Try to re-discover repos (e.g. after external `git init`)
                if self.repo_manager.update(&self.initial_paths) {
                    if let Some(r) = self.repo_manager.current() {
                        r.to_path_buf()
                    } else {
                        // No repo to show — drop any stale file list.
                        self.clear_git_state();
                        self.is_loading = false;
                        return;
                    }
                } else {
                    self.clear_git_state();
                    self.is_loading = false;
                    return;
                }
            }
        };

        // Replace any in-flight refresh — `try_recv` on the old
        // receiver will start returning Disconnected, which `poll_refresh`
        // treats as "nothing to apply" so the new worker's result wins.
        let (tx, rx) = std::sync::mpsc::channel();
        self.refresh_rx = Some(rx);
        let requested = self.viewed.clone();
        std::thread::spawn(move || {
            let branch = git::get_current_branch(&repo);
            let list = git::get_branch_list(&repo);
            let worktrees = git::linked_worktrees(&repo, &list);
            let viewed = requested.clone().filter(|name| {
                Some(name) != branch.as_ref() && list.iter().any(|b| &b.name == name)
            });
            let branches = list.into_iter().map(|b| b.name).collect();
            let (ahead, behind, mut unstaged_files, mut staged_files) =
                match resolve_work_dir(&repo, viewed.as_deref(), &worktrees) {
                    Some(dir) => {
                        let (ahead, behind) = git::get_ahead_behind(&dir);
                        let unstaged = git::get_unstaged_files(&dir);
                        let staged = git::get_staged_files(&dir);
                        (ahead, behind, unstaged, staged)
                    }
                    None => {
                        let name = viewed.as_deref().unwrap_or("HEAD");
                        let (ahead, behind) = git::get_ahead_behind_of(&repo, name);
                        (ahead, behind, Vec::new(), Vec::new())
                    }
                };
            let stash_count = git::stash_list(&repo).len();
            unstaged_files.sort_by(|a, b| a.path.cmp(&b.path));
            staged_files.sort_by(|a, b| a.path.cmp(&b.path));
            let _ = tx.send(GitStatusRefreshResult {
                requested,
                branch,
                branches,
                viewed,
                worktrees,
                ahead,
                behind,
                unstaged_files,
                staged_files,
                stash_count,
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
                self.is_loading = false;
                self.run_pending_refresh();
                return true;
            }
        };
        self.refresh_rx = None;

        // The view changed while the worker ran: its snapshot shows another
        // working copy.
        if result.requested != self.viewed {
            self.refresh_pending = false;
            self.refresh();
            return false;
        }

        self.branch = result.branch;
        self.branches = result.branches;
        self.viewed = result.viewed;
        self.worktrees = result.worktrees;
        self.ahead = result.ahead;
        self.behind = result.behind;
        self.unstaged_files = result.unstaged_files;
        self.staged_files = result.staged_files;
        self.stash_count = result.stash_count;

        self.rebuild_trees();

        // Adjust cursor to stay within bounds (cursor is virtual line)
        let max_cursor = self.total_virtual_lines().saturating_sub(1);
        if self.cursor > max_cursor {
            self.cursor = max_cursor;
        }
        if !self.is_selectable_line(self.cursor) {
            self.cursor = self.find_nearest_selectable_line(self.cursor);
        }

        self.is_loading = false;
        self.run_pending_refresh();
        true
    }

    /// If a refresh was requested while a worker was in flight, run the single
    /// coalesced follow-up pass now that the receiver is free.
    fn run_pending_refresh(&mut self) {
        if self.refresh_pending {
            self.refresh_pending = false;
            self.refresh();
        }
    }

    /// Lightweight refresh of only the data used by `title()`.
    /// Skips branch listing, sorting, and cursor adjustment.
    pub(crate) fn refresh_title_data(&mut self) {
        let Some(repo) = self.repo_manager.current().map(Path::to_path_buf) else {
            return;
        };
        self.branch = git::get_current_branch(&repo);
        // A branch checked out nowhere has no working copy to count.
        let Some(dir) = self.work_dir() else {
            return;
        };
        let (ahead, behind) = git::get_ahead_behind(&dir);
        self.ahead = ahead;
        self.behind = behind;
        self.unstaged_files = git::get_unstaged_files(&dir);
        self.staged_files = git::get_staged_files(&dir);
    }

    /// Build the node list of a section tree from file entries.
    fn build_section_tree(
        paths: &[(PathBuf, usize, char, bool)],
        collapsed: &HashSet<PathBuf>,
    ) -> Vec<tree::TreeNode> {
        let entries: Vec<tree::FileEntry> = paths
            .iter()
            .map(|(path, index, status, untracked)| tree::FileEntry {
                path: path.clone(),
                index: *index,
                status: *status,
                untracked: *untracked,
            })
            .collect();
        tree::build_tree(&entries, collapsed)
    }

    /// Rebuild tree data structures from current file lists.
    ///
    /// Everything derived from the node list — visible rows, tree prefixes
    /// and the per-directory aggregate status that colours directory rows —
    /// comes from `recompute_visible`, the one place that knows the full set,
    /// so a refresh and a fold/unfold cannot drift apart again.
    pub(crate) fn rebuild_trees(&mut self) {
        let unstaged_data: Vec<_> = self
            .unstaged_files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.path.clone(), i, f.status, f.untracked))
            .collect();
        self.unstaged.tree = Self::build_section_tree(&unstaged_data, &self.unstaged.collapsed);
        self.unstaged.recompute_visible();

        let staged_data: Vec<_> = self
            .staged_files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.path.clone(), i, f.status, false))
            .collect();
        self.staged.tree = Self::build_section_tree(&staged_data, &self.staged.collapsed);
        self.staged.recompute_visible();
    }

    /// Toggle expand/collapse for a directory node.
    pub(crate) fn toggle_dir_expand(&mut self, is_unstaged: bool, tree_idx: usize) {
        let (tree, collapsed) = if is_unstaged {
            (&mut self.unstaged.tree, &mut self.unstaged.collapsed)
        } else {
            (&mut self.staged.tree, &mut self.staged.collapsed)
        };

        if matches!(tree[tree_idx].kind, tree::TreeNodeKind::Directory { .. }) {
            let path = tree[tree_idx].full_path.clone();
            if let tree::TreeNodeKind::Directory { ref mut expanded } = tree[tree_idx].kind {
                *expanded = !*expanded;
                if *expanded {
                    collapsed.remove(&path);
                } else {
                    collapsed.insert(path);
                }
            }
        }

        // Recompute visible nodes and prefixes
        if is_unstaged {
            self.unstaged.recompute_visible();
        } else {
            self.staged.recompute_visible();
        }

        // Clamp cursor
        let max_cursor = self.total_virtual_lines().saturating_sub(1);
        if self.cursor > max_cursor {
            self.cursor = max_cursor;
        }
    }
}
