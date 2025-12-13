//! Background task coordination and message polling for termide.
//!
//! This crate provides:
//! - `WatcherRegistry` trait for registering/unregistering watches
//! - `MessageCollector` for aggregating messages from multiple sources
//! - `UpdateThrottler` for rate-limiting updates
//!
//! # Architecture
//!
//! Background watchers produce messages that are collected and processed
//! by the main event loop:
//!
//! ```text
//! GitWatcher ─┐
//! FSWatcher  ─┼─→ MessageCollector → Messages → Event Loop
//! DirSize    ─┘
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use termide_app_core::{FileChange, GitStatus, Message};

// ============================================================================
// Watcher Registry Trait
// ============================================================================

/// Trait for registering and unregistering filesystem watches.
pub trait WatcherRegistry {
    /// Register a git repository for watching.
    fn register_git_repo(&mut self, path: &Path) -> anyhow::Result<()>;

    /// Unregister a git repository from watching.
    fn unregister_git_repo(&mut self, path: &Path);

    /// Check if a git repository is being watched.
    fn is_watching_git_repo(&self, path: &Path) -> bool;

    /// Register a directory for watching (non-recursive).
    fn register_directory(&mut self, path: &Path) -> anyhow::Result<()>;

    /// Unregister a directory from watching.
    fn unregister_directory(&mut self, path: &Path);

    /// Check if a directory is being watched.
    fn is_watching_directory(&self, path: &Path) -> bool;
}

// ============================================================================
// Message Collector
// ============================================================================

/// Collected update from a watcher.
#[derive(Debug, Clone)]
pub enum WatcherUpdate {
    /// Git status changed
    GitStatus {
        /// Repository root path
        repo_path: PathBuf,
        /// Updated status
        status: GitStatus,
    },
    /// Filesystem changed
    FileSystem {
        /// Changed path
        path: PathBuf,
        /// Type of change
        change: FileChange,
    },
    /// Directory size calculated
    DirSize {
        /// Directory path
        path: PathBuf,
        /// Size in bytes
        size: u64,
    },
}

impl WatcherUpdate {
    /// Convert to Message.
    pub fn to_message(self) -> Message {
        match self {
            WatcherUpdate::GitStatus { repo_path, status } => Message::GitStatusUpdate {
                path: repo_path,
                status,
            },
            WatcherUpdate::FileSystem { path, change } => Message::FsUpdate { path, change },
            WatcherUpdate::DirSize { path, size } => Message::DirSizeResult { path, size },
        }
    }
}

/// Trait for receiving updates from watchers.
pub trait UpdateReceiver {
    /// Poll for available updates (non-blocking).
    fn poll(&mut self) -> Vec<WatcherUpdate>;

    /// Check if receiver has pending updates.
    fn has_pending(&self) -> bool;
}

/// Collects and deduplicates updates from multiple sources.
#[derive(Debug, Default)]
pub struct MessageCollector {
    /// Pending messages
    messages: Vec<Message>,
    /// Paths that have pending git updates (for deduplication)
    pending_git_paths: HashSet<PathBuf>,
    /// Paths that have pending fs updates (for deduplication)
    pending_fs_paths: HashSet<PathBuf>,
}

impl MessageCollector {
    /// Create a new message collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an update, deduplicating if needed.
    pub fn add_update(&mut self, update: WatcherUpdate) {
        match &update {
            WatcherUpdate::GitStatus { repo_path, .. } => {
                // Deduplicate git updates by repo path
                if self.pending_git_paths.insert(repo_path.clone()) {
                    self.messages.push(update.to_message());
                }
            }
            WatcherUpdate::FileSystem { path, .. } => {
                // Deduplicate fs updates by path
                if self.pending_fs_paths.insert(path.clone()) {
                    self.messages.push(update.to_message());
                }
            }
            WatcherUpdate::DirSize { .. } => {
                // Don't deduplicate dir size results
                self.messages.push(update.to_message());
            }
        }
    }

    /// Take all collected messages.
    pub fn take_messages(&mut self) -> Vec<Message> {
        self.pending_git_paths.clear();
        self.pending_fs_paths.clear();
        std::mem::take(&mut self.messages)
    }

    /// Check if there are any pending messages.
    pub fn has_messages(&self) -> bool {
        !self.messages.is_empty()
    }

    /// Get count of pending messages.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Clear all pending messages.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.pending_git_paths.clear();
        self.pending_fs_paths.clear();
    }
}

// ============================================================================
// Update Throttler
// ============================================================================

/// Throttles updates to prevent excessive processing.
#[derive(Debug)]
pub struct UpdateThrottler {
    /// Minimum interval between updates
    interval: Duration,
    /// Last update time per path
    last_updates: std::collections::HashMap<PathBuf, Instant>,
    /// Global last update time
    last_global_update: Option<Instant>,
}

impl UpdateThrottler {
    /// Create a new throttler with specified interval.
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_updates: std::collections::HashMap::new(),
            last_global_update: None,
        }
    }

    /// Create a throttler with 100ms interval (default).
    pub fn default_interval() -> Self {
        Self::new(Duration::from_millis(100))
    }

    /// Check if update for path should be processed.
    pub fn should_update(&mut self, path: &Path) -> bool {
        let now = Instant::now();

        if let Some(last) = self.last_updates.get(path) {
            if now.duration_since(*last) < self.interval {
                return false;
            }
        }

        self.last_updates.insert(path.to_path_buf(), now);
        true
    }

    /// Check if global update should be processed.
    pub fn should_update_global(&mut self) -> bool {
        let now = Instant::now();

        if let Some(last) = self.last_global_update {
            if now.duration_since(last) < self.interval {
                return false;
            }
        }

        self.last_global_update = Some(now);
        true
    }

    /// Reset throttler state.
    pub fn reset(&mut self) {
        self.last_updates.clear();
        self.last_global_update = None;
    }

    /// Remove old entries (cleanup).
    pub fn cleanup(&mut self, max_age: Duration) {
        let now = Instant::now();
        self.last_updates
            .retain(|_, last| now.duration_since(*last) < max_age);
    }
}

impl Default for UpdateThrottler {
    fn default() -> Self {
        Self::default_interval()
    }
}

// ============================================================================
// Debounced Update Manager
// ============================================================================

/// Manages debounced updates for paths.
///
/// Used for operations that should be batched over a time window,
/// such as git diff updates after rapid buffer changes.
#[derive(Debug)]
pub struct DebouncedUpdateManager {
    /// Pending paths with their scheduled update time
    pending: std::collections::HashMap<PathBuf, Instant>,
    /// Debounce duration
    debounce: Duration,
}

impl DebouncedUpdateManager {
    /// Create a new debounced update manager.
    pub fn new(debounce: Duration) -> Self {
        Self {
            pending: std::collections::HashMap::new(),
            debounce,
        }
    }

    /// Schedule an update for a path.
    pub fn schedule(&mut self, path: PathBuf) {
        let due = Instant::now() + self.debounce;
        self.pending.insert(path, due);
    }

    /// Get paths that are due for update.
    pub fn get_due(&mut self) -> Vec<PathBuf> {
        let now = Instant::now();
        let due: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, time)| **time <= now)
            .map(|(path, _)| path.clone())
            .collect();

        for path in &due {
            self.pending.remove(path);
        }

        due
    }

    /// Check if there are any pending updates.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Cancel pending update for a path.
    pub fn cancel(&mut self, path: &Path) {
        self.pending.remove(path);
    }

    /// Clear all pending updates.
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

impl Default for DebouncedUpdateManager {
    fn default() -> Self {
        Self::new(Duration::from_millis(300))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watcher_update_to_message_git() {
        let update = WatcherUpdate::GitStatus {
            repo_path: PathBuf::from("/repo"),
            status: GitStatus {
                branch: Some("main".to_string()),
                modified_count: 1,
                untracked_count: 0,
                staged_count: 0,
                is_dirty: true,
            },
        };

        let message = update.to_message();
        assert!(matches!(message, Message::GitStatusUpdate { .. }));
    }

    #[test]
    fn test_watcher_update_to_message_fs() {
        let update = WatcherUpdate::FileSystem {
            path: PathBuf::from("/file.txt"),
            change: FileChange::Modified,
        };

        let message = update.to_message();
        assert!(matches!(message, Message::FsUpdate { .. }));
    }

    #[test]
    fn test_watcher_update_to_message_dir_size() {
        let update = WatcherUpdate::DirSize {
            path: PathBuf::from("/dir"),
            size: 1024,
        };

        let message = update.to_message();
        assert!(matches!(message, Message::DirSizeResult { .. }));
    }

    #[test]
    fn test_message_collector_new() {
        let collector = MessageCollector::new();
        assert!(!collector.has_messages());
        assert_eq!(collector.message_count(), 0);
    }

    #[test]
    fn test_message_collector_add_update() {
        let mut collector = MessageCollector::new();

        collector.add_update(WatcherUpdate::FileSystem {
            path: PathBuf::from("/file.txt"),
            change: FileChange::Modified,
        });

        assert!(collector.has_messages());
        assert_eq!(collector.message_count(), 1);
    }

    #[test]
    fn test_message_collector_deduplication() {
        let mut collector = MessageCollector::new();

        // Add same path twice
        collector.add_update(WatcherUpdate::FileSystem {
            path: PathBuf::from("/file.txt"),
            change: FileChange::Modified,
        });
        collector.add_update(WatcherUpdate::FileSystem {
            path: PathBuf::from("/file.txt"),
            change: FileChange::Modified,
        });

        // Should only have one message
        assert_eq!(collector.message_count(), 1);

        // Add different path
        collector.add_update(WatcherUpdate::FileSystem {
            path: PathBuf::from("/other.txt"),
            change: FileChange::Created,
        });

        assert_eq!(collector.message_count(), 2);
    }

    #[test]
    fn test_message_collector_take_messages() {
        let mut collector = MessageCollector::new();

        collector.add_update(WatcherUpdate::FileSystem {
            path: PathBuf::from("/file.txt"),
            change: FileChange::Modified,
        });

        let messages = collector.take_messages();
        assert_eq!(messages.len(), 1);
        assert!(!collector.has_messages());

        // Can add again after clear
        collector.add_update(WatcherUpdate::FileSystem {
            path: PathBuf::from("/file.txt"),
            change: FileChange::Modified,
        });
        assert_eq!(collector.message_count(), 1);
    }

    #[test]
    fn test_update_throttler_should_update() {
        let mut throttler = UpdateThrottler::new(Duration::from_millis(50));
        let path = PathBuf::from("/test");

        // First update should pass
        assert!(throttler.should_update(&path));

        // Immediate second update should be throttled
        assert!(!throttler.should_update(&path));

        // Different path should pass
        let other_path = PathBuf::from("/other");
        assert!(throttler.should_update(&other_path));
    }

    #[test]
    fn test_update_throttler_global() {
        let mut throttler = UpdateThrottler::new(Duration::from_millis(50));

        // First global update should pass
        assert!(throttler.should_update_global());

        // Immediate second should be throttled
        assert!(!throttler.should_update_global());
    }

    #[test]
    fn test_debounced_update_manager_schedule() {
        let mut manager = DebouncedUpdateManager::new(Duration::from_millis(10));

        manager.schedule(PathBuf::from("/file.txt"));
        assert!(manager.has_pending());

        // Not due yet
        let due = manager.get_due();
        assert!(due.is_empty());
    }

    #[test]
    fn test_debounced_update_manager_cancel() {
        let mut manager = DebouncedUpdateManager::new(Duration::from_millis(100));
        let path = PathBuf::from("/file.txt");

        manager.schedule(path.clone());
        assert!(manager.has_pending());

        manager.cancel(&path);
        assert!(!manager.has_pending());
    }

    #[test]
    fn test_debounced_update_manager_clear() {
        let mut manager = DebouncedUpdateManager::new(Duration::from_millis(100));

        manager.schedule(PathBuf::from("/file1.txt"));
        manager.schedule(PathBuf::from("/file2.txt"));
        assert!(manager.has_pending());

        manager.clear();
        assert!(!manager.has_pending());
    }
}
