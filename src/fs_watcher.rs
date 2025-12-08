// Future API methods
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use notify_debouncer_mini::{new_debouncer, notify::*, Debouncer};

/// Event sent when a file or directory changes
#[derive(Debug, Clone)]
pub struct DirectoryUpdate {
    /// Root path being watched (repo_root or directory)
    pub watched_root: PathBuf,
    /// Actual path that changed
    pub changed_path: PathBuf,
}

/// Watches filesystem directories for changes and sends update events
/// Supports two modes:
/// - Git repositories: watched recursively from repo root
/// - Non-git directories: watched non-recursively (direct children only)
#[derive(Debug)]
pub struct FileSystemWatcher {
    debouncer: Debouncer<RecommendedWatcher>,
    /// Git repos: repo_root -> reference count (Recursive mode)
    watched_repos: HashMap<PathBuf, usize>,
    /// Non-git dirs: dir_path -> reference count (NonRecursive mode)
    watched_dirs: HashMap<PathBuf, usize>,
}

impl FileSystemWatcher {
    /// Create a new FileSystemWatcher that sends events through the provided channel
    /// Debounces events to 1 second intervals
    pub fn new(tx: Sender<DirectoryUpdate>) -> anyhow::Result<Self> {
        let debouncer = new_debouncer(
            Duration::from_secs(1),
            move |result: notify_debouncer_mini::DebounceEventResult| {
                if let Ok(events) = result {
                    for event in events {
                        // Send the changed path directly
                        // The watched_root will be determined by the receiver
                        // based on which watcher triggered the event
                        let changed_path = event.path.clone();

                        // Try to find the watched root by walking up the path
                        // This is a simple heuristic - the actual root tracking
                        // is done in check_fs_update()
                        let watched_root = changed_path
                            .parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_else(|| changed_path.clone());

                        let _ = tx.send(DirectoryUpdate {
                            watched_root,
                            changed_path,
                        });
                    }
                }
            },
        )?;

        Ok(Self {
            debouncer,
            watched_repos: HashMap::new(),
            watched_dirs: HashMap::new(),
        })
    }

    /// Start watching a git repository root recursively
    /// Increments reference count if already watching
    pub fn watch_repository(&mut self, repo_root: PathBuf) -> anyhow::Result<()> {
        // Increment reference count if already watching
        if let Some(count) = self.watched_repos.get_mut(&repo_root) {
            *count += 1;
            return Ok(());
        }

        let watcher = self.debouncer.watcher();

        // Watch repository root recursively
        watcher.watch(&repo_root, RecursiveMode::Recursive)?;

        self.watched_repos.insert(repo_root, 1);
        Ok(())
    }

    /// Stop watching a git repository (decrement reference count)
    /// Only unwatches when count reaches 0
    pub fn unwatch_repository(&mut self, repo_root: &Path) {
        if let Some(count) = self.watched_repos.get_mut(repo_root) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.watched_repos.remove(repo_root);
                let watcher = self.debouncer.watcher();
                let _ = watcher.unwatch(repo_root);
            }
        }
    }

    /// Check if repository root is being watched
    pub fn is_watching_repo(&self, repo_root: &Path) -> bool {
        self.watched_repos.contains_key(repo_root)
    }

    /// Start watching a non-git directory (non-recursive, direct children only)
    /// Increments reference count if already watching
    pub fn watch_directory(&mut self, dir_path: PathBuf) -> anyhow::Result<()> {
        // Increment reference count if already watching
        if let Some(count) = self.watched_dirs.get_mut(&dir_path) {
            *count += 1;
            return Ok(());
        }

        let watcher = self.debouncer.watcher();

        // Watch the directory non-recursively (only direct children, not subdirectories)
        watcher.watch(&dir_path, RecursiveMode::NonRecursive)?;

        self.watched_dirs.insert(dir_path, 1);
        Ok(())
    }

    /// Stop watching a non-git directory (decrement reference count)
    /// Only unwatches when count reaches 0
    pub fn unwatch_directory(&mut self, dir_path: &Path) {
        if let Some(count) = self.watched_dirs.get_mut(dir_path) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.watched_dirs.remove(dir_path);
                let watcher = self.debouncer.watcher();
                let _ = watcher.unwatch(dir_path);
            }
        }
    }

    /// Check if non-git directory is being watched
    pub fn is_watching_dir(&self, dir_path: &Path) -> bool {
        self.watched_dirs.contains_key(dir_path)
    }
}

/// Global filesystem watcher instance
/// This is created once at application startup
pub fn create_fs_watcher() -> anyhow::Result<(
    FileSystemWatcher,
    std::sync::mpsc::Receiver<DirectoryUpdate>,
)> {
    let (tx, rx) = channel();
    let watcher = FileSystemWatcher::new(tx)?;
    Ok((watcher, rx))
}
