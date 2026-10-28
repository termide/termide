//! Filesystem and git watcher event handling.
//!
//! Handles unified watcher events for filesystem changes and git operations.

use std::collections::{HashMap, HashSet};

use termide_core::{CommandResult, Panel, PanelCommand};
use termide_git::find_repo_root;
use termide_watcher::{UnifiedWatcher, WatchEvent, WatchTarget};

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

/// What `panel` needs watched: the repository its directory belongs to, or
/// the directory itself outside any repository. A file manager or git panel
/// is told the repository root it is in.
fn panel_watch_target(panel: &mut Box<dyn Panel>) -> Option<WatchTarget> {
    let CommandResult::FsWatchInfo {
        watched_root,
        current_path,
        is_git_repo,
    } = panel.handle_command(PanelCommand::GetFsWatchInfo)
    else {
        return None;
    };
    if let (true, Some(root)) = (is_git_repo, watched_root) {
        return Some(WatchTarget::Repo(root));
    }
    if current_path.as_os_str().is_empty() {
        return None;
    }
    let repo_root = find_repo_root(&current_path);
    let is_git_repo = repo_root.is_some();
    let root = repo_root.unwrap_or(current_path);
    panel.handle_command(PanelCommand::SetFsWatchRoot {
        root: Some(root.clone()),
        is_git_repo,
    });
    Some(if is_git_repo {
        WatchTarget::Repo(root)
    } else {
        WatchTarget::Dir(root)
    })
}

/// Bring the watches `held` on `watcher` to `wanted`, one per panel wanting a
/// target: acquire what is missing, then release what no panel wants any
/// more. A failed acquire is not held, so the next pass retries it.
fn reconcile_watches(
    watcher: &mut UnifiedWatcher,
    held: &mut HashMap<WatchTarget, usize>,
    wanted: &HashMap<WatchTarget, usize>,
) {
    for (target, &want) in wanted {
        let have = held.get(target).copied().unwrap_or(0);
        for _ in have..want {
            match watcher.watch(target) {
                Ok(()) => *held.entry(target.clone()).or_insert(0) += 1,
                Err(e) => {
                    log::warn!("Failed to watch {target:?}: {e}");
                    break;
                }
            }
        }
    }
    held.retain(|target, have| {
        let want = wanted.get(target).copied().unwrap_or(0);
        while *have > want {
            watcher.unwatch(target);
            *have -= 1;
        }
        *have > 0
    });
}

impl App {
    /// Bring the watcher in step with the panels: each panel, on screen or in
    /// a project parked in the background, holds one watch on what it shows,
    /// and a watch no panel wants any more — its panel closed or moved
    /// elsewhere — is released. Called when panels are added, closed or
    /// navigated, not on every tick.
    pub(super) fn register_panel_watchers(&mut self) {
        let Some(watcher) = &mut self.state.watcher else {
            return;
        };

        let mut wanted: HashMap<WatchTarget, usize> = HashMap::new();
        let parked = self
            .open_projects
            .parked_mut()
            .flat_map(|(_, parked)| parked.panels_mut());
        for panel in self.layout_manager.iter_all_panels_mut().chain(parked) {
            if let Some(target) = panel_watch_target(panel) {
                *wanted.entry(target).or_insert(0) += 1;
            }
        }
        reconcile_watches(watcher, &mut self.state.held_watches, &wanted);

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

        // A changed .gitignore changes which directories need a watch
        for repo_root in gitignore_changed_repos {
            watcher.reload_repo_ignores(&repo_root);
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

        // A repository's index, HEAD or refs moved: a commit, a checkout, a
        // stage — for the plugins that asked of every open project it
        // concerns, those parked in the background included.
        for repo in &git_repos {
            for project in super::plugins::projects_of_repo(self.open_projects.roots(), repo) {
                self.plugins.host.emit(
                    &project,
                    termide_plugins::protocol::events::GIT_CHANGED,
                    &serde_json::json!({ "root": repo }),
                );
            }
        }

        // Process git events — panels showing content get the update, panels
        // collapsed to their title bar get marked stale
        let area_height = self.panel_area_height();
        if !git_repos.is_empty() {
            let repo_paths: Vec<&std::path::Path> = git_repos.iter().map(|p| p.as_path()).collect();

            for (panel, is_visible) in self
                .layout_manager
                .iter_all_panels_with_visibility_mut(area_height)
            {
                if is_visible {
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

        self.fan_out_fs_changes(&fs_paths);
    }

    /// Deliver changed paths to the panels: those showing content get an
    /// `OnFsUpdate`, those collapsed to their title bar are marked stale.
    /// Used for the watcher's batches and for changes a panel reports itself.
    pub(super) fn fan_out_fs_changes(&mut self, fs_paths: &HashSet<std::path::PathBuf>) {
        if fs_paths.is_empty() {
            return;
        }
        let area_height = self.panel_area_height();
        let fanout = collapse_fs_burst(fs_paths);

        // Invalidate FM directory-size cache for any ancestor containing
        // a changed path. Panels keep stale totals until this event lands,
        // which is why navigating away and back does NOT recompute sizes —
        // only real FS activity does. Driven by the collapsed set: a cached
        // directory contains a file exactly when it contains that file's
        // directory, so one representative per directory gives the same answer
        // for a fraction of the comparisons.
        termide_panel_file_manager::shared_dir_size_cache().invalidate_ancestors_of_all(&fanout);

        // Process filesystem events — same split as for git events above
        for (panel, is_visible) in self
            .layout_manager
            .iter_all_panels_with_visibility_mut(area_height)
        {
            if is_visible {
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
        self.notify_outline_on_fs_change(fs_paths);
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

    fn temp_repo() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        (tmp, root)
    }

    fn wanted(targets: &[&WatchTarget]) -> HashMap<WatchTarget, usize> {
        let mut wanted = HashMap::new();
        for target in targets {
            *wanted.entry((*target).clone()).or_insert(0) += 1;
        }
        wanted
    }

    /// Two panels in one repository hold a watch each: closing one keeps the
    /// repository watched for the other, closing both releases it.
    #[test]
    fn a_shared_repo_stays_watched_until_its_last_panel_goes() {
        let (_tmp, root) = temp_repo();
        let repo = WatchTarget::Repo(root.clone());
        let mut watcher = UnifiedWatcher::new().unwrap();
        let mut held = HashMap::new();

        reconcile_watches(&mut watcher, &mut held, &wanted(&[&repo, &repo]));
        assert_eq!(held[&repo], 2);

        reconcile_watches(&mut watcher, &mut held, &wanted(&[&repo]));
        assert_eq!(held[&repo], 1);
        assert!(
            watcher.is_watching_repo(&root),
            "the other panel still needs it"
        );

        reconcile_watches(&mut watcher, &mut held, &wanted(&[]));
        assert!(held.is_empty());
        assert!(!watcher.is_watching_repo(&root));
    }

    /// Navigating away releases the directory left, git or not, and a pass
    /// with nothing changed acquires nothing more.
    #[test]
    fn navigating_away_releases_the_directory_left() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let dir_a = WatchTarget::Dir(dunce::canonicalize(a.path()).unwrap());
        let dir_b = WatchTarget::Dir(dunce::canonicalize(b.path()).unwrap());
        let mut watcher = UnifiedWatcher::new().unwrap();
        let mut held = HashMap::new();

        reconcile_watches(&mut watcher, &mut held, &wanted(&[&dir_a]));
        reconcile_watches(&mut watcher, &mut held, &wanted(&[&dir_a]));
        assert_eq!(held[&dir_a], 1, "a repeated pass is idempotent");

        reconcile_watches(&mut watcher, &mut held, &wanted(&[&dir_b]));
        assert!(!held.contains_key(&dir_a));
        let WatchTarget::Dir(path_a) = &dir_a else {
            unreachable!()
        };
        assert!(!watcher.is_watching_dir(path_a));
        assert_eq!(held[&dir_b], 1);
    }

    /// A directory that cannot be watched is not held, so nothing is
    /// released for it later.
    #[test]
    fn a_failed_watch_is_not_held() {
        let gone = WatchTarget::Dir(PathBuf::from("/nonexistent/termide-watch-test"));
        let mut watcher = UnifiedWatcher::new().unwrap();
        let mut held = HashMap::new();
        reconcile_watches(&mut watcher, &mut held, &wanted(&[&gone]));
        assert!(held.is_empty());
    }

    /// A file manager is watched by the repository it is in, and learns its
    /// root; outside a repository by its directory.
    #[test]
    fn file_manager_targets_its_repo_or_its_directory() {
        let (_tmp, root) = temp_repo();
        let mut fm: Box<dyn Panel> = Box::new(
            termide_panel_file_manager::FileManager::new_with_path(root.join("src")),
        );
        assert_eq!(
            panel_watch_target(&mut fm),
            Some(WatchTarget::Repo(root.clone()))
        );
        assert!(matches!(
            fm.handle_command(PanelCommand::GetFsWatchInfo),
            CommandResult::FsWatchInfo { watched_root: Some(r), is_git_repo: true, .. } if r == root
        ));

        let plain = tempfile::tempdir().unwrap();
        let plain = dunce::canonicalize(plain.path()).unwrap();
        let mut fm: Box<dyn Panel> = Box::new(
            termide_panel_file_manager::FileManager::new_with_path(plain.clone()),
        );
        assert_eq!(panel_watch_target(&mut fm), Some(WatchTarget::Dir(plain)));
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
