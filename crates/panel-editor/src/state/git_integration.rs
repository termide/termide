//! Git integration state for the editor.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use termide_git::{GitDiffAsyncResult, GitDiffCache};

/// Git-related state for the editor.
pub(crate) struct GitIntegration {
    /// Git diff cache for this file (if in git repo).
    pub diff_cache: Option<GitDiffCache>,
    /// Pending git diff update timestamp (for debounce).
    pub update_pending: Option<Instant>,
    /// Receiver for async git diff result (non-blocking load from HEAD).
    pub diff_receiver: Option<Receiver<GitDiffAsyncResult>>,
    /// Cached git repository root for this file (to avoid repeated filesystem lookups).
    /// None = not cached, Some(None) = no repo, Some(Some(path)) = repo found.
    pub cached_repo_root: Option<Option<PathBuf>>,
}

impl Default for GitIntegration {
    fn default() -> Self {
        Self::new()
    }
}

impl GitIntegration {
    /// Create new empty GitIntegration.
    pub fn new() -> Self {
        Self {
            diff_cache: None,
            update_pending: None,
            diff_receiver: None,
            cached_repo_root: None,
        }
    }

    /// Check if git diff is available.
    pub fn has_diff(&self) -> bool {
        self.diff_cache.is_some()
    }

    /// Check if there's a pending async diff load.
    pub fn has_pending_load(&self) -> bool {
        self.diff_receiver.is_some()
    }

    /// Set update pending with current timestamp.
    pub fn mark_update_pending(&mut self) {
        self.update_pending = Some(Instant::now());
    }

    /// Clear update pending.
    pub fn clear_update_pending(&mut self) {
        self.update_pending = None;
    }

    /// Check and clear update pending if debounce time has passed.
    /// Returns true if update should proceed.
    pub fn check_debounce(&mut self, debounce_ms: u128) -> bool {
        if let Some(pending_time) = self.update_pending {
            if pending_time.elapsed().as_millis() >= debounce_ms {
                self.update_pending = None;
                return true;
            }
        }
        false
    }
}
