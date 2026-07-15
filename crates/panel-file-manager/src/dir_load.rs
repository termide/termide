//! Asynchronous directory loading: the standalone reader, background
//! reload plumbing, and cursor/selection restoration after a load.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;

use anyhow::Result;
use termide_git::{get_git_status_async, GitStatus, GitStatusCache};
use termide_vfs::VfsEntry;

use super::{sort_entries, tree, FileEntry, FileManager};

/// Result of a background directory reload.
pub(crate) struct AsyncDirReloadResult {
    path: PathBuf,
    entries: Vec<FileEntry>,
}

/// Cursor / selection restoration state to apply once an async directory
/// load completes. Stored on `FileManager` between the moment the load
/// is kicked off and the moment `tick()` sees the worker's result.
#[derive(Default)]
pub(crate) struct PendingDirLoad {
    /// Name to put the cursor on when the load resolves.
    previous_name: Option<String>,
    previous_index: usize,
    previous_scroll_offset: usize,
    /// Names of files that were selected before the load. Empty when
    /// the caller did not request selection preservation.
    selected_names: HashSet<String>,
}

/// Standalone directory reader that can run in a background thread.
/// Takes all needed parameters by value to avoid borrowing issues.
pub(crate) fn read_dir_entries_standalone(
    dir_path: &std::path::Path,
    rel_prefix: &str,
    show_hidden: bool,
    git_status_cache: Option<&GitStatusCache>,
) -> Vec<FileEntry> {
    let mut entries = Vec::new();

    if let Ok(read_dir) = fs::read_dir(dir_path) {
        for entry in read_dir.flatten() {
            if let Ok(metadata) = entry.metadata() {
                let name = entry.file_name().to_string_lossy().into_owned();

                if !show_hidden && name.starts_with('.') {
                    continue;
                }

                let is_symlink = if let Ok(link_metadata) = fs::symlink_metadata(entry.path()) {
                    link_metadata.is_symlink()
                } else {
                    false
                };

                let is_dir = if is_symlink {
                    fs::metadata(entry.path())
                        .map(|m| m.is_dir())
                        .unwrap_or(false)
                } else {
                    metadata.is_dir()
                };

                let git_name = if rel_prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{rel_prefix}/{name}")
                };

                let git_status = if is_dir {
                    git_status_cache
                        .map(|cache| cache.get_directory_status(&git_name))
                        .unwrap_or(GitStatus::Unmodified)
                } else {
                    git_status_cache
                        .map(|cache| cache.get_status(&git_name))
                        .unwrap_or(GitStatus::Unmodified)
                };

                #[cfg(unix)]
                let is_executable = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let is_executable = false;

                #[cfg(unix)]
                let is_readonly = {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = metadata.permissions().mode();
                    (mode & 0o200) == 0
                };
                #[cfg(not(unix))]
                let is_readonly = metadata.permissions().readonly();

                let size = if metadata.is_file() {
                    Some(metadata.len())
                } else {
                    None
                };
                let modified = metadata.modified().ok();

                entries.push(FileEntry {
                    name,
                    is_dir,
                    is_symlink,
                    is_executable,
                    is_readonly,
                    git_status,
                    size,
                    modified,
                });
            }
        }
    }

    sort_entries(&mut entries);
    entries
}

impl FileManager {
    /// Start a background directory reload (for watcher-triggered updates).
    /// Reads directory entries in a background thread to avoid blocking the
    /// main tick loop. Call `check_async_reload()` on each tick to apply results.
    pub(crate) fn start_async_reload(&mut self) {
        const RELOAD_DEBOUNCE_MS: u128 = 300;
        if !self.navigation.should_reload(RELOAD_DEBOUNCE_MS) {
            // Too soon after the last reload — remember to retry so a burst's
            // final change isn't lost.
            self.reload_dirty = true;
            return;
        }
        // Don't overlap with an existing async reload
        if self.async_reload_receiver.is_some() {
            self.reload_dirty = true;
            return;
        }
        self.reload_dirty = false;
        let dir_path = self.current_path.clone();
        let show_hidden = self.show_hidden;
        let git_cache = self.git_status_cache.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let entries =
                read_dir_entries_standalone(&dir_path, "", show_hidden, git_cache.as_ref());
            let _ = tx.send(AsyncDirReloadResult {
                path: dir_path,
                entries,
            });
        });
        self.async_reload_receiver = Some(rx);
    }

    /// Check if a background directory reload has completed and apply the result.
    /// Returns `true` if entries were updated.
    pub fn check_async_reload(&mut self) -> bool {
        let rx = match self.async_reload_receiver.take() {
            Some(rx) => rx,
            None => {
                // No reload in flight — retry a burst reload that was coalesced
                // away earlier (start_async_reload re-checks the debounce gate).
                if self.reload_dirty {
                    self.start_async_reload();
                }
                return false;
            }
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // Not ready yet — put receiver back
                self.async_reload_receiver = Some(rx);
                return false;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                // Sender dropped without sending — discard
                return false;
            }
        };

        if result.path != self.current_path {
            self.pending_dir_load = None;
            return false; // Stale result — user navigated away
        }

        // Build entries with ".." prefix
        let mut entries = Vec::new();
        if self.current_path.parent().is_some() {
            entries.push(FileEntry {
                name: "..".to_string(),
                is_dir: true,
                is_symlink: false,
                is_executable: false,
                is_readonly: false,
                git_status: GitStatus::Unmodified,
                size: None,
                modified: None,
            });
        }
        entries.extend(result.entries);

        // If `pending_dir_load` is set, this was a navigation-initiated
        // load — restore the saved cursor/selection. Otherwise we're
        // resolving a passive watcher refresh, so just hold the current
        // cursor by name.
        let pending = self.pending_dir_load.take();
        let (current_name, previous_index, previous_scroll_offset, selected_names) =
            if let Some(p) = pending {
                (
                    p.previous_name,
                    p.previous_index,
                    p.previous_scroll_offset,
                    p.selected_names,
                )
            } else {
                (
                    self.entry_at(self.selected).map(|e| e.name.clone()),
                    self.selected,
                    self.scroll_offset,
                    HashSet::new(),
                )
            };

        self.tree_entries = self.build_top_level_tree(entries);
        self.load_expanded_subtrees();
        self.recompute_visible();

        // If the parallel git-status worker already finished and
        // deposited a cache, its `apply_git_statuses` ran when the
        // tree was still empty. Reapply now that tree_entries is
        // populated so the listing isn't stuck on Unmodified colors.
        if self.git_status_cache.is_some() {
            self.apply_git_statuses();
        }

        if !selected_names.is_empty() {
            for (vis_idx, &tree_idx) in self.visible_indices.iter().enumerate() {
                if selected_names.contains(&self.tree_entries[tree_idx].file_entry.name) {
                    self.selection.select(vis_idx);
                }
            }
        }

        self.restore_cursor(current_name, previous_index, previous_scroll_offset);

        true
    }

    /// Build top-level `tree_entries` from a sorted list of `FileEntry`.
    pub(crate) fn build_top_level_tree(&self, entries: Vec<FileEntry>) -> Vec<tree::TreeEntry> {
        entries
            .into_iter()
            .map(|fe| {
                let full_path = if fe.name == ".." {
                    self.current_path
                        .parent()
                        .unwrap_or(&self.current_path)
                        .to_path_buf()
                } else {
                    self.current_path.join(&fe.name)
                };
                let expanded = if fe.is_dir && fe.name != ".." {
                    let is_expanded = self.expanded_dirs.contains(&full_path);
                    Some(is_expanded)
                } else {
                    None
                };
                tree::TreeEntry {
                    file_entry: fe,
                    full_path,
                    depth: 0,
                    expanded,
                    is_loading: false,
                }
            })
            .collect()
    }

    /// Restore cursor position after entries reload.
    /// Priority: newly created item → navigating down → restore by name → fallback to index.
    fn restore_cursor(
        &mut self,
        current_name: Option<String>,
        previous_index: usize,
        previous_scroll_offset: usize,
    ) {
        let count = self.visible_count();
        // Newly-created cursor restore: prefer matching by full path so
        // an entry nested inside an expanded subdir is found correctly.
        // Fall back to matching by name for older callers that only set
        // the name.
        let created_path = self.navigation.take_newly_created_path();
        let created_name = self.navigation.take_newly_created();
        if created_path.is_some() || created_name.is_some() {
            let mut found: Option<usize> = None;
            if let Some(ref path) = created_path {
                for (vis_idx, &tree_idx) in self.visible_indices.iter().enumerate() {
                    if &self.tree_entries[tree_idx].full_path == path {
                        found = Some(vis_idx);
                        break;
                    }
                }
            }
            if found.is_none() {
                if let Some(ref name) = created_name {
                    found = self.find_entry_index(name);
                }
            }
            if let Some(idx) = found {
                self.selected = idx;
                if self.visible_height > 0 {
                    self.adjust_scroll_offset(self.visible_height);
                }
            } else if count > 0 {
                self.selected = previous_index.min(count - 1);
            }
        } else if self.navigation.check_and_reset_navigating_down() {
            self.selected = 0;
            self.scroll_offset = 0;
        } else if let Some(name) = current_name {
            if let Some(pos) = self.find_entry_index(&name) {
                self.selected = pos;
            } else if count > 0 {
                self.selected = previous_index.min(count - 1);
            }
            if self.visible_height > 0 {
                if count <= self.visible_height {
                    self.scroll_offset = 0;
                } else {
                    let max_scroll = count.saturating_sub(self.visible_height);
                    self.scroll_offset = previous_scroll_offset.min(max_scroll);
                }
                self.adjust_scroll_offset(self.visible_height);
            }
        }
    }

    /// Update entries from VFS directory listing (for remote directories).
    pub(crate) fn update_entries_from_vfs(&mut self, vfs_entries: Vec<VfsEntry>) {
        let previous_index = self.selected;
        let previous_scroll_offset = self.scroll_offset;
        let current_name = self.entry_at(self.selected).map(|e| e.name.clone());

        self.tree_entries.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        self.selection.clear();

        let mut entries = Vec::new();

        // Add ".." entry for parent directory navigation (unless at root)
        if self.vfs.current_path().parent().is_some() {
            entries.push(FileEntry {
                name: "..".to_string(),
                is_dir: true,
                is_symlink: false,
                is_executable: false,
                is_readonly: false,
                git_status: GitStatus::Unmodified,
                size: None,
                modified: None,
            });
        }

        // Convert and add VFS entries
        let mut file_entries: Vec<FileEntry> = vfs_entries
            .into_iter()
            .map(FileEntry::from_vfs_entry)
            .filter(|e| self.show_hidden || !e.name.starts_with('.'))
            .collect();

        sort_entries(&mut file_entries);
        entries.extend(file_entries);

        self.tree_entries = self.build_top_level_tree(entries);
        self.recompute_visible();

        // Clear git status (not applicable for remote files)
        self.git_status_cache = None;
        self.git_root = None;

        self.restore_cursor(current_name, previous_index, previous_scroll_offset);
    }

    /// Internal method to load directory with optional selection preservation.
    ///
    /// Returns immediately. For local paths, the directory read runs on
    /// a worker thread via [`Self::async_reload_receiver`] and the
    /// result is applied by `tick()` through [`Self::check_async_reload`];
    /// the cursor/selection restore info is parked on
    /// [`Self::pending_dir_load`] until then. For remote paths the VFS
    /// list already runs async, so we just kick it off.
    pub(crate) fn load_directory_inner(&mut self, preserve_selection: bool) -> Result<()> {
        // Sync VFS path with current_path for local paths
        if !self.vfs.is_remote() {
            self.vfs
                .set_path(termide_vfs::VfsPath::local(self.current_path.clone()));
        }

        // For remote paths, don't clear entries - keep showing current content while loading
        if self.vfs.is_remote() {
            self.vfs.invalidate_cache();
            self.vfs.start_list_dir();
            return Ok(());
        }

        // Save current file name and index to restore position once the
        // async read completes (see `check_async_reload`).
        let previous_name = self
            .navigation
            .take_previous_dir_name()
            .or_else(|| self.entry_at(self.selected).map(|e| e.name.clone()));
        let previous_index = self.selected;
        let previous_scroll_offset = self.scroll_offset;

        let selected_names: HashSet<String> = if preserve_selection {
            self.selection
                .items
                .iter()
                .filter_map(|&vis_idx| self.entry_at(vis_idx).map(|e| e.name.clone()))
                .collect()
        } else {
            HashSet::new()
        };

        self.tree_entries.clear();
        // Drop the stale `visible_indices` / `tree_prefixes` along with
        // the entries — otherwise the next render would index into an
        // empty `tree_entries` and panic before the worker reports back.
        self.recompute_visible();
        self.selected = 0;
        self.scroll_offset = 0;
        self.selection.clear();
        self.selection.end_drag();
        self.dir_size_queue.clear();

        self.git_status_cache = None;
        self.git_status_receiver = Some(get_git_status_async(self.current_path.clone()));

        self.pending_dir_load = Some(PendingDirLoad {
            previous_name,
            previous_index,
            previous_scroll_offset,
            selected_names,
        });

        // Spawn the read_dir on a worker. `read_dir_entries_standalone`
        // is the same helper the watcher-driven async reload uses; we
        // pass `git_cache = None` here because the git status worker is
        // racing us — the watcher will reapply statuses once the cache
        // is in place.
        let (tx, rx) = mpsc::channel();
        let dir_path = self.current_path.clone();
        let show_hidden = self.show_hidden;
        std::thread::spawn(move || {
            let entries = read_dir_entries_standalone(&dir_path, "", show_hidden, None);
            let _ = tx.send(AsyncDirReloadResult {
                path: dir_path,
                entries,
            });
        });
        self.async_reload_receiver = Some(rx);

        Ok(())
    }
}
