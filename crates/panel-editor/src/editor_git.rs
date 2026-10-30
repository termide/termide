//! Git diff cache and external-change detection for the Editor.
//!
//! Bundles the methods that read or refresh git state for the open
//! file: repo-root caching, async git-diff updates with debounce, and
//! external-modification detection. The actual git work lives in
//! `crate::git`; this module is the Editor-level glue.

use std::path::{Path, PathBuf};

use crate::{file_io, git};

use super::Editor;

impl Editor {
    /// Get cached git repository root (returns None if not yet cached)
    pub fn cached_repo_root(&self) -> Option<Option<&PathBuf>> {
        self.git.cached_repo_root.as_ref().map(|opt| opt.as_ref())
    }

    /// Get or compute git repository root for this file
    /// Returns Some(path) if in a git repo, None otherwise
    pub fn get_or_compute_repo_root(&mut self) -> Option<&PathBuf> {
        if self.git.cached_repo_root.is_none() {
            // Compute and cache
            let repo_root = self.file_path().and_then(termide_git::find_repo_root);
            self.git.cached_repo_root = Some(repo_root);
        }
        self.git
            .cached_repo_root
            .as_ref()
            .and_then(|opt| opt.as_ref())
    }

    /// Update git diff cache for this file (async - non-blocking)
    ///
    /// Spawns a background thread to load original content from HEAD.
    /// The result will be applied on next tick via check_git_diff_receiver().
    pub fn update_git_diff(&mut self) {
        // Clone file path to avoid borrow conflict with git_diff_cache
        let file_path = self.file_path().map(|p| p.to_path_buf());
        if let Some(rx) = git::update_git_diff_async(&mut self.git.diff_cache, file_path.as_deref())
        {
            self.git.diff_receiver = Some(rx);
        }
    }

    /// Mark the lines changed against `base` — the file as another version
    /// control system has it — while git has no original for the file;
    /// `None` takes it back. Git's markers, where git has the file, win.
    pub fn set_base_text(&mut self, base: Option<String>) {
        let Some(path) = self.file_path().map(Path::to_path_buf) else {
            return;
        };
        let cache = self
            .git
            .diff_cache
            .get_or_insert_with(|| termide_git::GitDiffCache::new(path));
        cache.set_fallback_original(base);
        if let Err(e) = cache.update_from_buffer(&self.buffer.to_string()) {
            log::warn!("diff against the base failed: {e}");
        }
    }

    /// Check and apply async git diff result if ready (called on each tick)
    ///
    /// Returns true if result was applied and needs_redraw should be set.
    pub fn check_git_diff_receiver(&mut self) -> bool {
        git::check_git_diff_receiver(&mut self.git.diff_receiver, &mut self.git.diff_cache)
    }

    /// Schedule git diff update with debounce (300ms delay)
    pub fn schedule_git_diff_update(&mut self) {
        if let Some(instant) = git::schedule_git_diff_update(&self.git.diff_cache) {
            self.git.update_pending = Some(instant);
        }
    }

    /// Check and apply pending git diff update if debounce time has passed
    pub fn check_pending_git_diff_update(&mut self) {
        let (updated, new_pending) = git::check_pending_git_diff_update(
            self.git.update_pending,
            &mut self.git.diff_cache,
            &self.buffer,
        );
        if updated {
            self.git.update_pending = new_pending;
        }
    }

    /// Check if the file was modified externally (outside of this editor)
    pub fn check_external_modification(&mut self) {
        // Skip check for remote files - temp file changes don't indicate external edits
        if self.file_state.is_remote() {
            return;
        }

        let Some(file_path) = self.buffer.file_path().map(Path::to_path_buf) else {
            return;
        };
        if !file_io::was_modified_externally(&file_path, self.file_state.mtime) {
            return;
        }
        // A buffer without unsaved work follows the file, as VS Code, Zed and
        // JetBrains do; one with unsaved work keeps it and shows the conflict
        // until the user reloads or saves elsewhere.
        if self.buffer.is_modified() {
            self.file_state.external_change_detected = true;
        } else if let Err(error) = self.reload_from_disk() {
            log::warn!("cannot reload {}: {error}", file_path.display());
            self.file_state.external_change_detected = true;
        }
    }

    /// Follow a change of the file's write permission: `p4 edit` or a
    /// `chmod +w` makes a read-only editor editable, `p4 revert` or a
    /// `chmod -w` makes it read-only again. Only a change is followed, so
    /// `Ctrl+E` holds until the permission changes next. A buffer with unsaved
    /// work is not made read-only under the user's hands. Returns whether
    /// the editor's mode changed.
    pub fn follow_disk_permissions(&mut self) -> bool {
        if self.file_state.is_remote() {
            return false;
        }
        let Some(last) = self.file_state.disk_read_only else {
            return false;
        };
        let Some(now) = self.buffer.file_path().and_then(file_io::file_readonly) else {
            return false;
        };
        if now == last {
            return false;
        }
        self.file_state.disk_read_only = Some(now);
        if now && self.buffer.is_modified() {
            return false;
        }
        let changed = self.config.read_only != now;
        self.config.read_only = now;
        changed
    }

    /// Check if external modification was detected
    pub fn has_external_change(&self) -> bool {
        self.file_state.external_change_detected
    }
}
