//! Lazy tree expansion/collapse: the pending-listing state machine and
//! the expand/collapse commands that drive it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use termide_git::GitStatus;
use termide_vfs::{VfsEntry, VfsError, VfsOperation, VfsResult};

use super::dir_load::read_dir_entries_standalone;
use super::{tree, FileEntry, FileManager};

/// In-flight directory listing for a tree expansion, regardless of
/// whether the source is a remote VFS operation or a local worker
/// thread.
pub(crate) enum PendingExpand {
    Remote(VfsOperation<Vec<VfsEntry>>),
    Local(mpsc::Receiver<Vec<FileEntry>>),
}

impl PendingExpand {
    /// Drain a result if one is ready. Converts both sources into a
    /// uniform `VfsResult<Vec<FileEntry>>` so `finish_expand` does not
    /// have to care which side produced it. The "." / ".." filter for
    /// VFS is applied here; the panel-state `show_hidden` filter
    /// happens later in `finish_expand`.
    fn try_recv(&self) -> Option<VfsResult<Vec<FileEntry>>> {
        match self {
            Self::Remote(op) => op.try_recv().map(|res| {
                res.map(|entries| {
                    entries
                        .into_iter()
                        .filter(|e| e.name != "." && e.name != "..")
                        .map(FileEntry::from_vfs_entry)
                        .collect()
                })
            }),
            Self::Local(rx) => match rx.try_recv() {
                Ok(entries) => Some(Ok(entries)),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(VfsError::Io(
                    std::io::Error::other("local expand worker disconnected"),
                ))),
            },
        }
    }
}

impl FileManager {
    /// Expand a directory at the given visible index, loading children lazily.
    pub(crate) fn expand_dir(&mut self, vis_idx: usize) {
        let tree_idx = match self.visible_indices.get(vis_idx) {
            Some(&idx) => idx,
            None => return,
        };
        if self.tree_entries[tree_idx].expanded != Some(false) {
            return; // not a collapsed dir
        }

        // Both local and remote expansions go through the same async
        // pipeline now — `begin_expand` picks the right listing source
        // (VFS for remote, a worker thread for local) and inserts the
        // loading placeholder. Real children land via `tick()` once
        // the listing resolves.
        self.begin_expand(tree_idx, vis_idx);
    }

    /// Translate a tree entry's local-style `full_path` into a real
    /// VfsPath rooted on the current remote connection.
    pub(crate) fn remote_vfs_path_for(&self, dir_path: &Path) -> Option<termide_vfs::VfsPath> {
        let base = self.vfs.current_path().clone();
        if let Ok(rel) = dir_path.strip_prefix(&self.current_path) {
            if rel.as_os_str().is_empty() {
                Some(base)
            } else {
                Some(base.join(rel))
            }
        } else {
            // Fallback: treat dir_path itself as the absolute remote path
            // (e.g. for ".." or odd entries). Reuse host/port/user.
            Some(termide_vfs::VfsPath::remote(
                base.protocol,
                base.host.clone().unwrap_or_default(),
                dir_path,
            ))
        }
    }

    /// Start the listing for a directory the user just expanded.
    ///
    /// Inserts a synthetic "Loading…" placeholder under `parent_idx`
    /// and registers the in-flight listing in `pending_expansions`.
    /// The placeholder is replaced with real children once `tick()`
    /// sees the listing resolve, identically for remote (VFS op) and
    /// local (worker thread on `std::fs::read_dir`) panels.
    fn begin_expand(&mut self, parent_idx: usize, vis_idx: usize) {
        let dir_path = self.tree_entries[parent_idx].full_path.clone();
        let depth = self.tree_entries[parent_idx].depth;

        // Mark expanded and remember it for restore-after-reload.
        self.tree_entries[parent_idx].expanded = Some(true);
        self.expanded_dirs.insert(dir_path.clone());

        // Skip if we already have a pending expansion for this directory.
        if self.pending_expansions.contains_key(&dir_path) {
            self.recompute_visible();
            return;
        }

        // If children were already loaded earlier (collapse keeps them
        // in tree_entries to make re-expand instantaneous), don't
        // refetch — otherwise each expand/collapse round-trip would
        // append a duplicate set of children.
        let next_idx = parent_idx + 1;
        let already_loaded = next_idx < self.tree_entries.len()
            && self.tree_entries[next_idx].depth > depth
            && !self.tree_entries[next_idx].is_loading;
        if already_loaded {
            let dir_was_selected = self.selection.items.contains(&vis_idx);
            let saved = self.save_selection_paths();
            self.recompute_visible();
            self.restore_selection_by_paths(&saved);
            if dir_was_selected {
                self.select_descendants(vis_idx);
            }
            return;
        }

        let Some(pending) = self.start_listing(&dir_path) else {
            return;
        };

        let placeholder = tree::TreeEntry {
            file_entry: FileEntry {
                name: "…".to_string(),
                is_dir: false,
                is_symlink: false,
                is_executable: false,
                is_readonly: false,
                git_status: GitStatus::Unmodified,
                size: None,
                modified: None,
            },
            full_path: dir_path.join("__loading__"),
            depth: depth + 1,
            expanded: None,
            is_loading: true,
        };
        let insert_at = parent_idx + 1;
        self.tree_entries.insert(insert_at, placeholder);

        self.pending_expansions.insert(dir_path, pending);

        let dir_was_selected = self.selection.items.contains(&vis_idx);
        let saved = self.save_selection_paths();
        self.recompute_visible();
        self.restore_selection_by_paths(&saved);
        if dir_was_selected {
            self.select_descendants(vis_idx);
        }
    }

    /// Variant of [`Self::begin_expand`] used to restore a previously-
    /// expanded subtree after a reload — there's no `vis_idx` and no
    /// selection to cascade.
    fn kick_off_subtree(&mut self, tree_idx: usize, dir_path: PathBuf, depth: usize) {
        if self.pending_expansions.contains_key(&dir_path) {
            return;
        }
        // Already-loaded guard mirrors `begin_expand`: don't fetch if
        // children are already in the tree.
        let next_idx = tree_idx + 1;
        if next_idx < self.tree_entries.len()
            && self.tree_entries[next_idx].depth > depth
            && !self.tree_entries[next_idx].is_loading
        {
            return;
        }
        let Some(pending) = self.start_listing(&dir_path) else {
            return;
        };
        let placeholder = tree::TreeEntry {
            file_entry: FileEntry {
                name: "…".to_string(),
                is_dir: false,
                is_symlink: false,
                is_executable: false,
                is_readonly: false,
                git_status: GitStatus::Unmodified,
                size: None,
                modified: None,
            },
            full_path: dir_path.join("__loading__"),
            depth: depth + 1,
            expanded: None,
            is_loading: true,
        };
        self.tree_entries.insert(tree_idx + 1, placeholder);
        self.pending_expansions.insert(dir_path, pending);
    }

    /// Pick the right async listing for the current panel mode.
    ///
    /// For remote panels this is the VFS `list_dir`; for local panels
    /// a worker thread runs `read_dir_entries_standalone`. Returning
    /// `None` aborts the expansion — usually because we couldn't
    /// derive a `VfsPath` for a remote tree entry.
    fn start_listing(&self, dir_path: &Path) -> Option<PendingExpand> {
        if self.vfs.is_remote() {
            let vfs_path = self.remote_vfs_path_for(dir_path)?;
            Some(PendingExpand::Remote(
                self.vfs.manager().list_dir(&vfs_path),
            ))
        } else {
            let rel_prefix = dir_path
                .strip_prefix(&self.current_path)
                .ok()
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .to_string();
            let (tx, rx) = mpsc::channel();
            let path_for_worker = dir_path.to_path_buf();
            // Worker uses show_hidden=true so the up-to-date panel
            // state can apply the final filter in `finish_expand`,
            // not whatever snapshot was current at spawn time.
            let git_cache = self.git_status_cache.clone();
            std::thread::spawn(move || {
                let entries = read_dir_entries_standalone(
                    &path_for_worker,
                    &rel_prefix,
                    true,
                    git_cache.as_ref(),
                );
                let _ = tx.send(entries);
            });
            Some(PendingExpand::Local(rx))
        }
    }

    /// Drain any completed pending expansions and substitute placeholders
    /// with real children. Returns true if anything changed (caller will
    /// emit NeedsRedraw).
    pub(crate) fn poll_pending_expansions(&mut self) -> bool {
        if self.pending_expansions.is_empty() {
            return false;
        }
        let keys: Vec<PathBuf> = self.pending_expansions.keys().cloned().collect();
        let mut results: Vec<(PathBuf, VfsResult<Vec<FileEntry>>)> = Vec::new();
        for key in keys {
            if let Some(op) = self.pending_expansions.get(&key) {
                if let Some(res) = op.try_recv() {
                    results.push((key, res));
                }
            }
        }
        let changed = !results.is_empty();
        for (dir_path, result) in results {
            self.pending_expansions.remove(&dir_path);
            self.finish_expand(&dir_path, result);
        }
        if changed {
            let saved = self.save_selection_paths();
            self.recompute_visible();
            self.restore_selection_by_paths(&saved);
        }
        changed
    }

    /// Substitute the loading placeholder under `dir_path` with the real
    /// children returned by the directory listing. On error, leave a
    /// "<error>" placeholder so the user sees the failure rather than
    /// silent nothing. Handles both remote (VFS) and local expansions
    /// uniformly — the producer normalised its result to
    /// `Vec<FileEntry>` already.
    fn finish_expand(&mut self, dir_path: &Path, result: VfsResult<Vec<FileEntry>>) {
        // Locate the parent tree index.
        let Some(parent_idx) = self
            .tree_entries
            .iter()
            .position(|te| te.full_path == dir_path)
        else {
            return;
        };
        let parent_depth = self.tree_entries[parent_idx].depth;

        // Remove the placeholder (single entry right after parent that
        // carries `is_loading == true`).
        let placeholder_idx = parent_idx + 1;
        if placeholder_idx < self.tree_entries.len()
            && self.tree_entries[placeholder_idx].is_loading
            && self.tree_entries[placeholder_idx].depth == parent_depth + 1
        {
            self.tree_entries.remove(placeholder_idx);
        }

        match result {
            Ok(entries) => {
                let mut file_entries: Vec<FileEntry> = entries
                    .into_iter()
                    .filter(|e| self.show_hidden || !e.name.starts_with('.'))
                    .collect();
                file_entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                });
                let child_depth = parent_depth + 1;
                let dir_path_owned = dir_path.to_path_buf();
                let children: Vec<tree::TreeEntry> = file_entries
                    .into_iter()
                    .map(|fe| {
                        let full_path = dir_path_owned.join(&fe.name);
                        let expanded = if fe.is_dir {
                            let is_exp = self.expanded_dirs.contains(&full_path);
                            Some(is_exp)
                        } else {
                            None
                        };
                        tree::TreeEntry {
                            file_entry: fe,
                            full_path,
                            depth: child_depth,
                            expanded,
                            is_loading: false,
                        }
                    })
                    .collect();
                let n = children.len();
                let insert_at = parent_idx + 1;
                self.tree_entries.splice(insert_at..insert_at, children);
                // Re-trigger expansion for any newly visible directories
                // the user had previously expanded.
                for offset in 0..n {
                    let idx = insert_at + offset;
                    if idx >= self.tree_entries.len() {
                        break;
                    }
                    if self.tree_entries[idx].expanded == Some(true) {
                        let child_path = self.tree_entries[idx].full_path.clone();
                        let child_depth = self.tree_entries[idx].depth;
                        self.kick_off_subtree(idx, child_path, child_depth);
                    }
                }
            }
            Err(e) => {
                let placeholder = tree::TreeEntry {
                    file_entry: FileEntry {
                        name: format!("<error: {e}>"),
                        is_dir: false,
                        is_symlink: false,
                        is_executable: false,
                        is_readonly: false,
                        git_status: GitStatus::Unmodified,
                        size: None,
                        modified: None,
                    },
                    full_path: dir_path.join("__error__"),
                    depth: parent_depth + 1,
                    expanded: None,
                    is_loading: false,
                };
                self.tree_entries.insert(parent_idx + 1, placeholder);
                // Also clear expanded state so user can retry by clicking again.
                self.tree_entries[parent_idx].expanded = Some(false);
                self.expanded_dirs.remove(dir_path);
            }
        }
    }

    /// Collapse a directory at the given visible index.
    pub(crate) fn collapse_dir(&mut self, vis_idx: usize) {
        let tree_idx = match self.visible_indices.get(vis_idx) {
            Some(&idx) => idx,
            None => return,
        };
        if self.tree_entries[tree_idx].expanded != Some(true) {
            return; // not an expanded dir
        }

        // Mark as collapsed (children stay in tree_entries, just hidden by visibility)
        self.tree_entries[tree_idx].expanded = Some(false);
        self.expanded_dirs
            .remove(&self.tree_entries[tree_idx].full_path);

        let saved = self.save_selection_paths();
        self.recompute_visible();
        self.restore_selection_by_paths(&saved);
    }

    /// Toggle expand/collapse for a directory at the given visible index.
    pub(crate) fn toggle_expand(&mut self, vis_idx: usize) {
        let tree_idx = match self.visible_indices.get(vis_idx) {
            Some(&idx) => idx,
            None => return,
        };
        match self.tree_entries[tree_idx].expanded {
            Some(true) => self.collapse_dir(vis_idx),
            Some(false) => self.expand_dir(vis_idx),
            None => {} // not a directory
        }
    }

    /// Jump cursor to the parent directory node in the tree.
    /// Used when pressing Left on a non-directory or on a child of an expanded dir.
    pub(crate) fn jump_to_parent_dir(&mut self) {
        let tree_idx = match self.visible_indices.get(self.selected) {
            Some(&idx) => idx,
            None => return,
        };
        let current_depth = self.tree_entries[tree_idx].depth;
        if current_depth == 0 {
            return;
        }
        // Walk backwards in visible_indices to find the parent (first entry with depth < current)
        for vis_idx in (0..self.selected).rev() {
            let ti = self.visible_indices[vis_idx];
            if self.tree_entries[ti].depth < current_depth {
                self.selected = vis_idx;
                self.adjust_scroll_offset(self.visible_height);
                return;
            }
        }
    }

    /// Save selection as set of paths (survives tree rebuilds).
    fn save_selection_paths(&self) -> HashSet<PathBuf> {
        self.selection
            .items
            .iter()
            .filter_map(|&vis_idx| self.path_at(vis_idx).cloned())
            .collect()
    }

    /// Restore selection from saved paths after tree rebuild.
    fn restore_selection_by_paths(&mut self, saved: &HashSet<PathBuf>) {
        self.selection.items.clear();
        for (vis_idx, &tree_idx) in self.visible_indices.iter().enumerate() {
            if saved.contains(&self.tree_entries[tree_idx].full_path) {
                self.selection.items.insert(vis_idx);
            }
        }
    }

    /// After building top-level tree, kick off async listings for every
    /// directory that was expanded in the previous session. Same
    /// pipeline as a fresh expand — placeholders inserted here are
    /// replaced by real children in `tick()` once each listing
    /// resolves, then `finish_expand` recursively schedules listings
    /// for any newly visible subdirs that were also expanded.
    pub(crate) fn load_expanded_subtrees(&mut self) {
        let dirs: Vec<(usize, PathBuf, usize)> = self
            .tree_entries
            .iter()
            .enumerate()
            .filter_map(|(idx, te)| {
                if te.expanded == Some(true) && te.file_entry.is_dir {
                    Some((idx, te.full_path.clone(), te.depth))
                } else {
                    None
                }
            })
            .collect();
        // Walk in reverse so insertions don't shift indices we still
        // need to process.
        for (tree_idx, dir_path, depth) in dirs.into_iter().rev() {
            self.kick_off_subtree(tree_idx, dir_path, depth);
        }
    }
}
