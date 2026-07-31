//! Git integration state for the editor.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use termide_git::{BlameEntry, GitDiffAsyncResult, GitDiffCache};

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
    /// Whether blame annotation is currently enabled.
    pub blame_enabled: bool,
    /// Blame data indexed by 0-based line (index 0 = line 1).
    pub blame_data: Vec<BlameEntry>,
    /// Receiver for async blame load.
    blame_rx: Option<Receiver<Vec<BlameEntry>>>,
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
            blame_enabled: true,
            blame_data: Vec::new(),
            blame_rx: None,
        }
    }

    /// Start async blame load for the given file (called on open when blame is enabled by default).
    pub fn start_blame(&mut self, repo: &Path, file: &Path) {
        self.blame_rx = Some(termide_git::get_blame_async(
            repo.to_path_buf(),
            file.to_path_buf(),
        ));
    }

    /// Poll the background blame thread.  Returns `true` if new data arrived (triggers redraw).
    pub fn poll_blame(&mut self) -> bool {
        let rx = match &self.blame_rx {
            Some(r) => r,
            None => return false,
        };
        match rx.try_recv() {
            Ok(data) => {
                self.blame_data = data;
                self.blame_rx = None;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.blame_rx = None;
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
        }
    }

    /// Get the blame entry for a 0-based line index (returns `None` if disabled or no data).
    pub fn blame_for_line(&self, line_idx: usize) -> Option<&BlameEntry> {
        if self.blame_enabled {
            self.blame_data.get(line_idx)
        } else {
            None
        }
    }
}
