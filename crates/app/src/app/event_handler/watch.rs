//! Filesystem watcher registration event handlers.

use std::path::PathBuf;

use crate::app::App;

impl App {
    /// Handle WatchPath event - register path with file watcher
    pub(super) fn event_watch_path(&mut self, path: PathBuf) {
        if let Some(watcher) = &mut self.state.watcher {
            if path.is_dir() {
                // Check if it's a git repo
                if termide_git::find_repo_root(&path).is_some() {
                    if let Err(e) = watcher.watch_repository(path.clone()) {
                        log::error!("Failed to watch repository {}: {}", path.display(), e);
                    }
                } else if let Err(e) = watcher.watch_directory(path.clone()) {
                    log::error!("Failed to watch directory {}: {}", path.display(), e);
                }
            }
        }
    }

    /// Handle UnwatchPath event - unregister path from file watcher
    pub(super) fn event_unwatch_path(&mut self, path: PathBuf) {
        if let Some(watcher) = &mut self.state.watcher {
            if termide_git::find_repo_root(&path).is_some() {
                watcher.unwatch_repository(&path);
            } else {
                watcher.unwatch_directory(&path);
            }
        }
    }
}
