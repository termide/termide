//! Async git-status refresh, state recomputation, and file-tree rebuilding.

use std::collections::HashSet;
use std::path::PathBuf;

use termide_git::{self as git, StagedFile, UnstagedFile};

use crate::tree;
use crate::GitStatusPanel;

/// Snapshot returned by the background refresh worker. All the git
/// commands the panel needs for a render run on the worker thread; the
/// UI thread just swaps these fields into place when the result is
/// ready, so the panel never blocks on a slow `git status --porcelain`
/// over a large repository.
pub(crate) struct GitStatusRefreshResult {
    pub(crate) branch: Option<String>,
    pub(crate) branches: Vec<String>,
    pub(crate) ahead: usize,
    pub(crate) behind: usize,
    pub(crate) unstaged_files: Vec<UnstagedFile>,
    pub(crate) staged_files: Vec<StagedFile>,
    pub(crate) stash_count: usize,
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
        std::thread::spawn(move || {
            let branch = git::get_current_branch(&repo);
            let branches = git::get_all_branches(&repo);
            let (ahead, behind) = git::get_ahead_behind(&repo);
            let mut unstaged_files = git::get_unstaged_files(&repo);
            let mut staged_files = git::get_staged_files(&repo);
            let stash_count = git::stash_list(&repo).len();
            unstaged_files.sort_by(|a, b| a.path.cmp(&b.path));
            staged_files.sort_by(|a, b| a.path.cmp(&b.path));
            let _ = tx.send(GitStatusRefreshResult {
                branch,
                branches,
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

        self.branch = result.branch;
        self.branches = result.branches;
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
        let repo = match self.repo_manager.current() {
            Some(r) => r.to_path_buf(),
            None => return,
        };

        self.branch = git::get_current_branch(&repo);
        let (ahead, behind) = git::get_ahead_behind(&repo);
        self.ahead = ahead;
        self.behind = behind;
        self.unstaged_files = git::get_unstaged_files(&repo);
        self.staged_files = git::get_staged_files(&repo);
    }

    /// Build a section tree from file entries.
    fn build_section_tree(
        paths: &[(PathBuf, usize, char, bool)],
        collapsed: &HashSet<PathBuf>,
    ) -> (Vec<tree::TreeNode>, Vec<usize>, Vec<String>) {
        let entries: Vec<tree::FileEntry> = paths
            .iter()
            .map(|(path, index, status, untracked)| tree::FileEntry {
                path: path.clone(),
                index: *index,
                status: *status,
                untracked: *untracked,
            })
            .collect();
        let tree_nodes = tree::build_tree(&entries, collapsed);
        let visible = tree::compute_visible_nodes(&tree_nodes);
        let prefixes = tree::compute_tree_prefixes(&tree_nodes, &visible);
        (tree_nodes, visible, prefixes)
    }

    /// Rebuild tree data structures from current file lists.
    fn rebuild_trees(&mut self) {
        let unstaged_data: Vec<_> = self
            .unstaged_files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.path.clone(), i, f.status, f.untracked))
            .collect();
        let (tree, visible, prefixes) =
            Self::build_section_tree(&unstaged_data, &self.unstaged.collapsed);
        self.unstaged.tree = tree;
        self.unstaged.visible = visible;
        self.unstaged.prefixes = prefixes;

        let staged_data: Vec<_> = self
            .staged_files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.path.clone(), i, f.status, false))
            .collect();
        let (tree, visible, prefixes) =
            Self::build_section_tree(&staged_data, &self.staged.collapsed);
        self.staged.tree = tree;
        self.staged.visible = visible;
        self.staged.prefixes = prefixes;
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
