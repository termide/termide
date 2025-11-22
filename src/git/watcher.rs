use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use notify_debouncer_mini::{new_debouncer, notify::*, Debouncer};

/// Event sent when git status needs to be updated
#[derive(Debug, Clone)]
pub struct GitStatusUpdate {
    /// Path to the repository root (contains .git directory)
    pub repo_path: PathBuf,
}

/// Watches git repositories for changes and sends update events
#[derive(Debug)]
pub struct GitWatcher {
    debouncer: Debouncer<RecommendedWatcher>,
    watched_repos: HashMap<PathBuf, PathBuf>,  // repo_path -> git_dir_path
}

impl GitWatcher {
    /// Create a new GitWatcher that sends events through the provided channel
    /// Debounces events to minimum 300ms intervals
    pub fn new(tx: Sender<GitStatusUpdate>) -> anyhow::Result<Self> {
        let debouncer = new_debouncer(
            Duration::from_millis(100),
            move |result: notify_debouncer_mini::DebounceEventResult| {
                if let Ok(events) = result {
                    for event in events {
                        // Filter: only process changes to relevant git files
                        // This prevents processing of .git/objects/* and other irrelevant files
                        if let Some(file_name) = event.path.file_name().and_then(|n| n.to_str()) {
                            // Check if this is a file we care about
                            let is_relevant = file_name == "index" ||
                                             file_name == "HEAD" ||
                                             event.path.to_str().map(|s| s.contains("/refs/") || s.contains("/logs/")).unwrap_or(false);

                            if !is_relevant {
                                continue; // Skip irrelevant files
                            }
                        }

                        // Get repository root from the event path
                        if let Some(repo_path) = Self::find_repo_root(&event.path) {
                            eprintln!("[GitWatcher] File change detected: {:?} -> repo: {:?}", event.path, repo_path);
                            let _ = tx.send(GitStatusUpdate { repo_path });
                        }
                    }
                }
            },
        )?;

        Ok(Self {
            debouncer,
            watched_repos: HashMap::new(),
        })
    }

    /// Start watching a git repository
    /// Returns Ok if watching started successfully or repository was already being watched
    pub fn watch_repository(&mut self, repo_path: PathBuf) -> anyhow::Result<()> {
        // Check if already watching
        if self.watched_repos.contains_key(&repo_path) {
            return Ok(());
        }

        let git_dir = repo_path.join(".git");
        if !git_dir.exists() {
            return Ok(()); // Not a git repository, silently skip
        }

        let watcher = self.debouncer.watcher();

        // Watch the entire .git directory recursively
        // This allows us to catch rename/create events when git atomically updates files
        // (e.g., git creates .git/index.lock, writes to it, then renames to .git/index)
        watcher.watch(&git_dir, RecursiveMode::Recursive)?;

        eprintln!("[GitWatcher] Watching repository: {:?}", repo_path);

        self.watched_repos.insert(repo_path, git_dir);
        Ok(())
    }

    /// Stop watching a git repository
    pub fn unwatch_repository(&mut self, repo_path: &Path) {
        if let Some(git_dir) = self.watched_repos.remove(repo_path) {
            let watcher = self.debouncer.watcher();

            // Unwatch the .git directory (errors are ignored as directory may not exist anymore)
            let _ = watcher.unwatch(&git_dir);
        }
    }

    /// Find the git repository root from a path inside .git directory
    /// Returns None if the path is not inside a git directory
    fn find_repo_root(path: &Path) -> Option<PathBuf> {
        // Walk up the path to find .git directory
        let mut current = path;
        while let Some(parent) = current.parent() {
            if parent.file_name()?.to_str()? == ".git" {
                // Found .git directory, return its parent (repo root)
                return parent.parent().map(|p| p.to_path_buf());
            }
            current = parent;
        }
        None
    }

    /// Check if repository is being watched
    pub fn is_watching(&self, repo_path: &Path) -> bool {
        self.watched_repos.contains_key(repo_path)
    }

    /// Get list of currently watched repositories
    pub fn watched_repositories(&self) -> Vec<&PathBuf> {
        self.watched_repos.keys().collect()
    }
}

/// Global git watcher instance
/// This is created once at application startup and runs in a background thread
pub fn create_git_watcher() -> anyhow::Result<(GitWatcher, std::sync::mpsc::Receiver<GitStatusUpdate>)> {
    let (tx, rx) = channel();
    let watcher = GitWatcher::new(tx)?;
    Ok((watcher, rx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_repo_root() {
        let path = PathBuf::from("/home/user/project/.git/refs/heads/main");
        let root = GitWatcher::find_repo_root(&path);
        assert_eq!(root, Some(PathBuf::from("/home/user/project")));

        let path = PathBuf::from("/home/user/project/.git/index");
        let root = GitWatcher::find_repo_root(&path);
        assert_eq!(root, Some(PathBuf::from("/home/user/project")));

        let path = PathBuf::from("/home/user/project/src/main.rs");
        let root = GitWatcher::find_repo_root(&path);
        assert_eq!(root, None);
    }
}
