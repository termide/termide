//! Filesystem and git watcher event handling.
//!
//! Handles unified watcher events for filesystem changes and git operations.

use std::collections::HashSet;

use termide_core::{CommandResult, PanelCommand};
use termide_git::find_repo_root;
use termide_watcher::WatchEvent;

use super::App;

impl App {
    /// Register panel directories with the watcher (lazy registration).
    /// Called when panels are added or navigated, not on every tick.
    pub(super) fn register_panel_watchers(&mut self) {
        let Some(watcher) = &mut self.state.watcher else {
            return;
        };

        for panel in self.layout_manager.iter_all_panels_mut() {
            // Use GetFsWatchInfo to check watch state
            if let CommandResult::FsWatchInfo {
                watched_root,
                current_path,
                is_git_repo: _,
            } = panel.handle_command(PanelCommand::GetFsWatchInfo)
            {
                if watched_root.is_none() {
                    // Determine the new watched root
                    let repo_root = find_repo_root(&current_path);
                    let is_git_repo = repo_root.is_some();
                    let new_root = repo_root.unwrap_or_else(|| current_path.clone());

                    // Watch new root (now fast - respects .gitignore)
                    if is_git_repo {
                        if !watcher.is_watching_repo(&new_root) {
                            let _ = watcher.watch_repository(new_root.clone());
                        }
                    } else if !watcher.is_watching_dir(&new_root) {
                        let _ = watcher.watch_directory(new_root.clone());
                    }

                    // Update panel's watched root
                    panel.handle_command(PanelCommand::SetFsWatchRoot {
                        root: Some(new_root),
                        is_git_repo,
                    });
                }
            }

            // Also handle Editor panels via GetRepoRoot
            if let CommandResult::RepoRoot(Some(repo_root)) =
                panel.handle_command(PanelCommand::GetRepoRoot)
            {
                if !watcher.is_watching_repo(&repo_root) {
                    let _ = watcher.watch_repository(repo_root);
                }
            }
        }

        // Sync git panel repo lists with current panel paths so that git-status
        // and git-log panels update when the user navigates to a new directory.
        let paths = self.collect_repo_search_paths();
        for panel in self.layout_manager.iter_all_panels_mut() {
            panel.handle_command(PanelCommand::UpdateRepoPaths {
                paths: paths.clone(),
            });
        }
    }

    /// Poll watcher for filesystem and git events (no registration).
    /// Called on every tick to process pending watcher events.
    pub(super) fn poll_watcher_events(&mut self) {
        let Some(watcher) = &mut self.state.watcher else {
            return;
        };

        // Poll events from unified watcher
        let events = watcher.poll_events();
        if events.is_empty() {
            return;
        }

        // Separate git and fs events
        let mut git_repos: HashSet<std::path::PathBuf> = HashSet::new();
        let mut fs_paths: HashSet<std::path::PathBuf> = HashSet::new();

        let mut gitignore_changed_repos: Vec<std::path::PathBuf> = Vec::new();

        for event in events {
            match event {
                WatchEvent::GitCommit(repo_root) => {
                    git_repos.insert(repo_root);
                }
                WatchEvent::DirectoryChanged { changed, .. } => {
                    fs_paths.insert(changed);
                }
                WatchEvent::FileChanged(path) => {
                    fs_paths.insert(path);
                }
                WatchEvent::GitignoreChanged(repo_root) => {
                    gitignore_changed_repos.push(repo_root);
                }
            }
        }

        // Handle .gitignore changes - reinitialize watcher
        for repo_root in gitignore_changed_repos {
            watcher.unwatch_repository(&repo_root);
            let _ = watcher.watch_repository(repo_root);
        }

        // Invalidate FM directory-size cache for any ancestor containing
        // a changed path. Panels keep stale totals until this event lands,
        // which is why navigating away and back does NOT recompute sizes —
        // only real FS activity does.
        {
            let cache = termide_panel_file_manager::shared_dir_size_cache();
            for path in &fs_paths {
                cache.invalidate_ancestors(path);
            }
        }

        // Invalidate cached commands registry on any filesystem change involving commands.toml.
        // Commands live in ~/.config/termide/commands.toml or .termide/commands.toml.
        if self.state.cache.commands_registry.is_some() {
            let commands_toml = std::path::Path::new("commands.toml");
            let invalidate = fs_paths
                .iter()
                .any(|p| p.file_name() == Some(commands_toml.as_os_str()));
            if invalidate {
                self.state.cache.commands_registry = None;
                self.state.cache.hotkey_table = None;
            }
        }

        // Process git events — expanded panels get the update, collapsed panels get marked stale
        if !git_repos.is_empty() {
            let repo_paths: Vec<&std::path::Path> = git_repos.iter().map(|p| p.as_path()).collect();

            for (panel, is_expanded) in self
                .layout_manager
                .iter_all_panels_with_expanded_state_mut()
            {
                if is_expanded {
                    if panel
                        .handle_command(PanelCommand::OnGitUpdate {
                            repo_paths: &repo_paths,
                        })
                        .needs_redraw()
                    {
                        self.state.needs_redraw = true;
                    }
                } else if panel.handle_command(PanelCommand::MarkStale).needs_redraw() {
                    self.state.needs_redraw = true;
                }
            }
        }

        // Process filesystem events — expanded panels get the update, collapsed panels get marked stale
        for (panel, is_expanded) in self
            .layout_manager
            .iter_all_panels_with_expanded_state_mut()
        {
            if is_expanded {
                for path in &fs_paths {
                    if panel
                        .handle_command(PanelCommand::OnFsUpdate { changed_path: path })
                        .needs_redraw()
                    {
                        self.state.needs_redraw = true;
                        break;
                    }
                }
            } else if !fs_paths.is_empty()
                && panel.handle_command(PanelCommand::MarkStale).needs_redraw()
            {
                self.state.needs_redraw = true;
            }
        }

        // Update outline panel if tracked file changed on disk
        self.notify_outline_on_fs_change(&fs_paths);
    }
}
