//! Filesystem watcher for termide.
//!
//! Provides filesystem change notifications using notify.

// Future API methods
#![allow(dead_code)]

use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebouncedEvent, Debouncer};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, channel, Receiver, Sender};
use std::time::Duration;

/// Filesystem change event.
#[derive(Debug, Clone)]
pub struct FsEvent {
    /// Changed path.
    pub path: PathBuf,
    /// Event kind.
    pub kind: FsEventKind,
}

/// Kind of filesystem event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsEventKind {
    Create,
    Modify,
    Remove,
    Rename,
    Unknown,
}

/// Debounce duration for filesystem events.
pub const DEFAULT_DEBOUNCE_MS: u64 = 100;

/// Create a filesystem watcher with debouncing.
///
/// Returns a receiver for filesystem events and the watcher handle.
pub fn create_watcher(
    debounce_ms: u64,
) -> Result<(Receiver<Vec<FsEvent>>, Debouncer<RecommendedWatcher>)> {
    let (tx, rx) = mpsc::channel();

    let debouncer = new_debouncer(
        Duration::from_millis(debounce_ms),
        move |res: Result<Vec<DebouncedEvent>, _>| {
            if let Ok(events) = res {
                let fs_events: Vec<FsEvent> = events
                    .into_iter()
                    .map(|e| FsEvent {
                        path: e.path,
                        kind: FsEventKind::Modify, // Debouncer doesn't preserve event kind
                    })
                    .collect();
                let _ = tx.send(fs_events);
            }
        },
    )
    .context("Failed to create filesystem watcher")?;

    Ok((rx, debouncer))
}

/// Watch a path for changes.
pub fn watch_path(
    watcher: &mut Debouncer<RecommendedWatcher>,
    path: &Path,
    recursive: bool,
) -> Result<()> {
    let mode = if recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };

    watcher
        .watcher()
        .watch(path, mode)
        .with_context(|| format!("Failed to watch path: {}", path.display()))?;

    Ok(())
}

/// Stop watching a path.
pub fn unwatch_path(watcher: &mut Debouncer<RecommendedWatcher>, path: &Path) -> Result<()> {
    watcher
        .watcher()
        .unwatch(path)
        .with_context(|| format!("Failed to unwatch path: {}", path.display()))?;

    Ok(())
}

/// Simple wrapper for single-path watching.
pub struct PathWatcher {
    debouncer: Debouncer<RecommendedWatcher>,
    receiver: Receiver<Vec<FsEvent>>,
}

impl PathWatcher {
    /// Create a new path watcher.
    pub fn new(debounce_ms: u64) -> Result<Self> {
        let (receiver, debouncer) = create_watcher(debounce_ms)?;
        Ok(Self {
            debouncer,
            receiver,
        })
    }

    /// Watch a path.
    pub fn watch(&mut self, path: &Path, recursive: bool) -> Result<()> {
        watch_path(&mut self.debouncer, path, recursive)
    }

    /// Stop watching a path.
    pub fn unwatch(&mut self, path: &Path) -> Result<()> {
        unwatch_path(&mut self.debouncer, path)
    }

    /// Try to receive pending events (non-blocking).
    pub fn try_recv(&self) -> Option<Vec<FsEvent>> {
        self.receiver.try_recv().ok()
    }

    /// Get the receiver for events.
    pub fn receiver(&self) -> &Receiver<Vec<FsEvent>> {
        &self.receiver
    }
}

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
    /// Debounces events to 300ms intervals
    pub fn new(tx: Sender<DirectoryUpdate>) -> Result<Self> {
        let debouncer = new_debouncer(
            Duration::from_millis(300),
            move |result: notify_debouncer_mini::DebounceEventResult| {
                if let Ok(events) = result {
                    for event in events {
                        // Skip .git directory events to avoid feedback loop
                        // (GitWatcher separately handles .git for git status updates)
                        if event
                            .path
                            .to_str()
                            .map(|s| s.contains("/.git/") || s.ends_with("/.git"))
                            .unwrap_or(false)
                        {
                            continue;
                        }

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
        )
        .context("Failed to create filesystem watcher")?;

        Ok(Self {
            debouncer,
            watched_repos: HashMap::new(),
            watched_dirs: HashMap::new(),
        })
    }

    /// Start watching a git repository root recursively
    /// Increments reference count if already watching
    pub fn watch_repository(&mut self, repo_root: PathBuf) -> Result<()> {
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
    pub fn watch_directory(&mut self, dir_path: PathBuf) -> Result<()> {
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
pub fn create_fs_watcher() -> Result<(FileSystemWatcher, Receiver<DirectoryUpdate>)> {
    let (tx, rx) = channel();
    let watcher = FileSystemWatcher::new(tx)?;
    Ok((watcher, rx))
}
