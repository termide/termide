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

#[cfg(test)]
impl AsyncDirReloadResult {
    pub(crate) fn new(path: PathBuf, entries: Vec<FileEntry>) -> Self {
        Self { path, entries }
    }
}

/// Cursor / selection restoration state to apply once an async directory
/// load completes. Stored on `FileManager` between the moment the load
/// is kicked off and the moment `tick()` sees the worker's result.
#[derive(Default)]
pub(crate) struct PendingDirLoad {
    /// Name to put the cursor on when the load resolves.
    previous_name: Option<String>,
    /// Full path of the entry the cursor should return to; preferred over
    /// the name, which cannot tell a nested entry from a top-level one.
    previous_path: Option<PathBuf>,
    previous_index: usize,
    previous_scroll_offset: usize,
    /// Full paths of the entries that were marked before the load. Empty
    /// when the caller did not request selection preservation.
    selected_paths: HashSet<PathBuf>,
}

/// Cursor and marks waiting for their rows: a reload rebuilds the top level
/// at once but lists expanded folders afterwards, so an entry inside one is
/// found only once its folder's listing lands. Applied by
/// `poll_pending_expansions`; dropped when no listing is left in flight.
#[derive(Default)]
pub(crate) struct PathRestore {
    /// Where the cursor belongs, not shown yet.
    cursor: Option<PathBuf>,
    /// The entry the cursor was left on meanwhile. When the cursor is on
    /// another entry by the time `cursor` shows up, the user moved it and it
    /// stays where it is.
    parked_on: Option<PathBuf>,
    /// Marked entries not shown yet.
    marks: HashSet<PathBuf>,
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
                let file_name = entry.file_name();
                let name = file_name.to_string_lossy().into_owned();
                // Keep the real name for paths when display had to replace bytes.
                let os_name = file_name.to_str().is_none().then_some(file_name);

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
                    os_name,
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
        // The worker reads the local disk: a remote panel's path names a
        // directory on the server, and the local one of that name is not it.
        if self.vfs.is_remote() {
            self.reload_dirty = false;
            return;
        }
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
        self.spawn_passive_read();
    }

    /// Re-read the current directory while its listing stays on screen: the
    /// cursor and the marks keep to their entries once the new one lands
    /// (`check_async_reload`), and keys pressed meanwhile act on the old
    /// rows instead of being lost on an empty list. For an explicit reload
    /// (Ctrl+R, a rescan, the end of an operation); `keep_marks` false drops
    /// the marks at once.
    pub(crate) fn reload_in_place(&mut self, keep_marks: bool) {
        if !keep_marks {
            self.selection.clear();
            self.selection.end_drag();
        }
        // A read already under way may predate the change: this one replaces
        // it, and a navigation's cursor waiting on it waits on this one.
        self.reload_dirty = false;
        self.spawn_passive_read();
        self.refresh_git_status();
    }

    /// Read the current directory on a worker for `check_async_reload`.
    fn spawn_passive_read(&mut self) {
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

        // Stale result: the user navigated away, or onto a server or into an
        // archive, where a path equal to the local one names other files.
        if result.path != self.current_path || self.vfs.is_remote() {
            self.pending_dir_load = None;
            return false;
        }

        // Build entries with ".." prefix. A drive root keeps it too: from
        // there ".." offers the other drives.
        let mut entries = Vec::new();
        if self.current_path.parent().is_some() || termide_vfs::is_drive_root(&self.current_path) {
            entries.push(FileEntry {
                name: "..".to_string(),
                os_name: None,
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
        // resolving a passive watcher refresh, so hold the current cursor
        // and marks on their entries: rows shift when entries come and go.
        let pending = self.pending_dir_load.take();
        let carried = self.path_restore.take();
        let (current_name, current_path, previous_index, previous_scroll_offset, mut marks) =
            if let Some(p) = pending {
                (
                    p.previous_name,
                    p.previous_path,
                    p.previous_index,
                    p.previous_scroll_offset,
                    p.selected_paths,
                )
            } else {
                let mut marks = self.save_selection_paths();
                let mut cursor = self.path_at(self.selected).cloned();
                // An earlier reload's targets whose folders were still being
                // listed carry over.
                if let Some(carried) = carried {
                    marks.extend(carried.marks);
                    if carried.parked_on == cursor && carried.cursor.is_some() {
                        cursor = carried.cursor;
                    }
                }
                (
                    self.entry_at(self.selected).map(|e| e.name.clone()),
                    cursor,
                    self.selected,
                    self.scroll_offset,
                    marks,
                )
            };
        self.selection.clear();
        self.selection.end_drag();

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

        if !marks.is_empty() {
            for (vis_idx, &tree_idx) in self.visible_indices.iter().enumerate() {
                if marks.remove(&self.tree_entries[tree_idx].full_path) {
                    self.selection.select(vis_idx);
                }
            }
        }

        let cursor_target = self.restore_cursor(
            current_name,
            current_path,
            previous_index,
            previous_scroll_offset,
        );

        // Entries inside expanded folders show up once their listings land.
        marks.retain(|p| self.is_nested_path(p));
        if !self.pending_expansions.is_empty() && (cursor_target.is_some() || !marks.is_empty()) {
            self.path_restore = Some(PathRestore {
                cursor: cursor_target,
                parked_on: self.path_at(self.selected).cloned(),
                marks,
            });
        }

        true
    }

    /// Whether `path` lies inside a folder of the current directory rather
    /// than directly in it: only such an entry can appear later, when an
    /// expanded folder's listing lands.
    fn is_nested_path(&self, path: &std::path::Path) -> bool {
        path.starts_with(&self.current_path) && path.parent() != Some(self.current_path.as_path())
    }

    /// Visible row of the entry at `path`.
    pub(crate) fn visible_index_of_path(&self, path: &std::path::Path) -> Option<usize> {
        self.visible_indices
            .iter()
            .position(|&ti| self.tree_entries[ti].full_path == path)
    }

    /// Put back the cursor and marks a reload could not place yet, those
    /// whose rows have appeared since. Called after expansions land, with
    /// the entry the cursor was on before they did.
    pub(crate) fn apply_path_restore(&mut self, cursor_before: Option<&PathBuf>) {
        let Some(mut restore) = self.path_restore.take() else {
            return;
        };
        let found: Vec<(PathBuf, usize)> = restore
            .marks
            .iter()
            .filter_map(|p| Some((p.clone(), self.visible_index_of_path(p)?)))
            .collect();
        for (path, idx) in found {
            restore.marks.remove(&path);
            self.selection.select(idx);
        }
        if let Some(target) = restore.cursor.take() {
            if cursor_before != restore.parked_on.as_ref() {
                // The user moved the cursor meanwhile: leave it be.
            } else if let Some(idx) = self.visible_index_of_path(&target) {
                self.selected = idx;
                if self.visible_height > 0 {
                    self.adjust_scroll_offset(self.visible_height);
                }
            } else {
                restore.cursor = Some(target);
                restore.parked_on = self.path_at(self.selected).cloned();
            }
        }
        if !self.pending_expansions.is_empty()
            && (restore.cursor.is_some() || !restore.marks.is_empty())
        {
            self.path_restore = Some(restore);
        }
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
                    self.current_path.join(fe.path_name())
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
    /// Priority: newly created item → navigating down → restore by path,
    /// then name → fallback to index.
    ///
    /// Returns the entry the cursor belongs on when it lies inside an
    /// expanded folder not listed yet, for `apply_path_restore` to finish.
    fn restore_cursor(
        &mut self,
        current_name: Option<String>,
        current_path: Option<PathBuf>,
        previous_index: usize,
        previous_scroll_offset: usize,
    ) -> Option<PathBuf> {
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
            } else {
                if count > 0 {
                    self.selected = previous_index.min(count - 1);
                }
                return created_path.filter(|p| self.is_nested_path(p));
            }
        } else if self.navigation.check_and_reset_navigating_down() {
            self.selected = 0;
            self.scroll_offset = 0;
        } else if current_name.is_some() || current_path.is_some() {
            // A nested entry is found by its path only: its name may also be
            // a top-level entry's.
            let nested = current_path.as_ref().filter(|p| self.is_nested_path(p));
            let found = current_path
                .as_deref()
                .and_then(|p| self.visible_index_of_path(p))
                .or_else(|| {
                    if nested.is_some() {
                        None
                    } else {
                        current_name
                            .as_deref()
                            .and_then(|n| self.find_entry_index(n))
                    }
                });
            let mut unresolved = None;
            if let Some(pos) = found {
                self.selected = pos;
            } else {
                if count > 0 {
                    self.selected = previous_index.min(count - 1);
                }
                unresolved = nested.cloned();
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
            return unresolved;
        }
        None
    }

    /// Update entries from VFS directory listing (for remote directories).
    pub(crate) fn update_entries_from_vfs(&mut self, vfs_entries: Vec<VfsEntry>) {
        let previous_index = self.selected;
        let previous_scroll_offset = self.scroll_offset;
        // After going up, the cursor lands on the directory (or archive) just
        // left, as for local listings.
        let current_name = self
            .navigation
            .take_previous_dir_name()
            .or_else(|| self.entry_at(self.selected).map(|e| e.name.clone()));

        self.tree_entries.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        self.selection.clear();

        let mut entries = Vec::new();

        // Add ".." entry for parent directory navigation (unless at root).
        // An archive's root has one too: it leads out of the archive.
        if self.vfs.current_path().parent().is_some() || self.vfs.at_archive_root() {
            entries.push(FileEntry {
                name: "..".to_string(),
                os_name: None,
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

        self.path_restore = None;
        self.restore_cursor(current_name, None, previous_index, previous_scroll_offset);
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
        let (previous_name, previous_path) = match self.navigation.take_previous_dir_name() {
            Some(name) => {
                let path = self.current_path.join(&name);
                (Some(name), Some(path))
            }
            None => (
                self.entry_at(self.selected).map(|e| e.name.clone()),
                self.path_at(self.selected).cloned(),
            ),
        };
        let previous_index = self.selected;
        let previous_scroll_offset = self.scroll_offset;

        let selected_paths: HashSet<PathBuf> = if preserve_selection {
            self.save_selection_paths()
        } else {
            HashSet::new()
        };
        self.path_restore = None;

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
            previous_path,
            previous_index,
            previous_scroll_offset,
            selected_paths,
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
