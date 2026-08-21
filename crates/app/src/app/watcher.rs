//! Filesystem and git watcher event handling.
//!
//! Handles unified watcher events for filesystem changes and git operations.

use std::collections::HashSet;

use termide_core::{CommandResult, PanelCommand};
use termide_git::find_repo_root;
use termide_watcher::WatchEvent;

use super::App;

/// Above this many changed paths in one batch, the fan-out is collapsed to one
/// path per directory. Below it nothing is collapsed: the saving is irrelevant
/// and exact paths keep the file manager's `.gitignore` filtering exact.
const FS_BURST_THRESHOLD: usize = 64;

/// Collapse a filesystem-event batch to the paths worth fanning out.
///
/// Every consumer of `OnFsUpdate` tests the path by prefix or by its parent
/// directory, never by exact file name, so one representative per directory
/// reaches exactly the same panels. That matters for bursts: deleting a
/// 35k-entry tree used to push every path through every panel and through the
/// directory-size cache — 13ms per panel plus 200ms of cache invalidation, all
/// on the main thread. Collapsed, the same burst is ~100 paths.
///
/// The panel-structure notification keeps the full set: it matches its tracked
/// file by exact path.
fn collapse_fs_burst(fs_paths: &HashSet<std::path::PathBuf>) -> Vec<&std::path::Path> {
    if fs_paths.len() <= FS_BURST_THRESHOLD {
        return fs_paths.iter().map(|p| p.as_path()).collect();
    }
    let mut seen_dirs: HashSet<&std::path::Path> = HashSet::new();
    fs_paths
        .iter()
        .filter(|path| seen_dirs.insert(path.parent().unwrap_or(path.as_path())))
        .map(|p| p.as_path())
        .collect()
}

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

        let fanout = collapse_fs_burst(&fs_paths);

        // Invalidate FM directory-size cache for any ancestor containing
        // a changed path. Panels keep stale totals until this event lands,
        // which is why navigating away and back does NOT recompute sizes —
        // only real FS activity does. Driven by the collapsed set: a cached
        // directory contains a file exactly when it contains that file's
        // directory, so one representative per directory gives the same answer
        // for a fraction of the comparisons.
        termide_panel_file_manager::shared_dir_size_cache().invalidate_ancestors_of_all(&fanout);

        // Process filesystem events — expanded panels get the update, collapsed panels get marked stale
        for (panel, is_expanded) in self
            .layout_manager
            .iter_all_panels_with_expanded_state_mut()
        {
            if is_expanded {
                for path in &fanout {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn set(paths: &[&str]) -> HashSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    /// A small batch passes through untouched — exact paths keep the file
    /// manager's per-file `.gitignore` filtering exact.
    #[test]
    fn small_batches_are_not_collapsed() {
        let paths = set(&["/p/a.txt", "/p/b.txt", "/p/sub/c.txt"]);
        let fanout = collapse_fs_burst(&paths);
        assert_eq!(fanout.len(), paths.len());
    }

    /// A burst collapses to one representative per directory, and every
    /// affected directory keeps a representative.
    #[test]
    fn bursts_collapse_to_one_path_per_directory() {
        let mut paths: HashSet<PathBuf> = HashSet::new();
        for dir in 0..5 {
            for file in 0..100 {
                paths.insert(PathBuf::from(format!("/p/d{dir}/f{file}.txt")));
            }
        }
        assert!(paths.len() > FS_BURST_THRESHOLD);

        let fanout = collapse_fs_burst(&paths);

        assert_eq!(fanout.len(), 5, "one path per directory: {fanout:?}");
        let dirs: HashSet<&std::path::Path> = fanout.iter().map(|p| p.parent().unwrap()).collect();
        for dir in 0..5 {
            assert!(
                dirs.contains(PathBuf::from(format!("/p/d{dir}")).as_path()),
                "directory d{dir} lost its representative"
            );
        }
    }

    /// A root-level path has no parent; it must still survive the collapse.
    #[test]
    fn parentless_paths_survive_the_collapse() {
        let mut paths: HashSet<PathBuf> = (0..FS_BURST_THRESHOLD + 1)
            .map(|i| PathBuf::from(format!("/p/f{i}.txt")))
            .collect();
        paths.insert(PathBuf::from("/"));

        let fanout = collapse_fs_burst(&paths);

        assert!(fanout.contains(&std::path::Path::new("/")));
    }
}
