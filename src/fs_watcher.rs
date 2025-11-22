// Future API methods
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use notify_debouncer_mini::{new_debouncer, notify::*, Debouncer};

/// Event sent when a directory needs to be reloaded
#[derive(Debug, Clone)]
pub struct DirectoryUpdate {
    /// Path to the directory that changed
    pub dir_path: PathBuf,
}

/// Watches filesystem directories for changes and sends update events
#[derive(Debug)]
pub struct FileSystemWatcher {
    debouncer: Debouncer<RecommendedWatcher>,
    watched_dirs: HashMap<PathBuf, ()>,
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
                        // Process all debounced events
                        // The debouncer already filters and groups events (create, delete, rename, modify)
                        // We watch the directory, so any change in direct children will trigger this

                        // Get parent directory from the event path
                        if let Some(dir_path) = event.path.parent() {
                            let _ = tx.send(DirectoryUpdate {
                                dir_path: dir_path.to_path_buf(),
                            });
                        }
                    }
                }
            },
        )?;

        Ok(Self {
            debouncer,
            watched_dirs: HashMap::new(),
        })
    }

    /// Start watching a directory
    /// Returns Ok if watching started successfully or directory was already being watched
    pub fn watch_directory(&mut self, dir_path: PathBuf) -> anyhow::Result<()> {
        // Check if already watching
        if self.watched_dirs.contains_key(&dir_path) {
            return Ok(());
        }

        let watcher = self.debouncer.watcher();

        // Watch the directory non-recursively (only direct children, not subdirectories)
        watcher.watch(&dir_path, RecursiveMode::NonRecursive)?;

        self.watched_dirs.insert(dir_path, ());
        Ok(())
    }

    /// Stop watching a directory
    pub fn unwatch_directory(&mut self, dir_path: &Path) {
        if self.watched_dirs.remove(dir_path).is_some() {
            let watcher = self.debouncer.watcher();
            let _ = watcher.unwatch(dir_path);
        }
    }

    /// Check if directory is being watched
    pub fn is_watching(&self, dir_path: &Path) -> bool {
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
