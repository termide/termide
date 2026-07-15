//! Filesystem watcher registration and git-status refresh event handlers.

#![allow(deprecated)]

use std::path::PathBuf;

use crate::app::App;
use crate::PanelExt;

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

    /// Handle RefreshGitStatus event - refresh git status for panels in path
    pub(super) fn event_refresh_git_status(&mut self, path: PathBuf) {
        // Reload FileManagers whose current path starts with the given path
        for panel in self.layout_manager.iter_all_panels_mut() {
            if let Some(fm) = panel.as_file_manager_mut() {
                if fm.current_path().starts_with(&path) || path.starts_with(fm.current_path()) {
                    let _ = fm.reload_directory();
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
