//! Repository selection manager for git panels.
//!
//! Provides common logic for managing repository selection across git panels.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use crate::{find_all_repos, find_toplevel_repo, find_toplevel_repos};

/// Default depth git panels recurse into a repo root to discover submodules.
/// `0` would skip the submodule walk entirely.
const SUBMODULE_DEPTH: usize = 2;

/// Order repositories by display name (case-insensitive), with the full path
/// as a tiebreak — the dropdown shows `get_repo_name` (the last path
/// component), so a raw path sort looked unsorted by name to the user.
fn sort_by_display_name(repos: &mut [PathBuf]) {
    repos.sort_by(|a, b| {
        crate::get_repo_name(a)
            .to_lowercase()
            .cmp(&crate::get_repo_name(b).to_lowercase())
            .then_with(|| a.cmp(b))
    });
}

/// Depth to scan DOWN from an input path that is not itself inside a repository,
/// to discover nested repositories — covers opening termide in a directory that
/// merely *contains* git projects (`~/projects` with `repo-a/`, `repo-b/`).
const NESTED_REPO_DEPTH: usize = 2;

/// Manages repository selection for git panels.
///
/// The constructor finds top-level repo roots synchronously (cheap: just
/// walks up from each path until it sees a `.git` directory) and kicks
/// off the recursive submodule walk on a background thread. The panel
/// is usable immediately with the root repos; submodules join the list
/// the first time [`Self::poll`] is called after the walk completes,
/// without blocking the UI.
pub struct RepoManager {
    repos: Vec<PathBuf>,
    selected: usize,
    /// Background submodule walk in flight — `None` once it has landed
    /// (or if it was never started, e.g. on an empty repo list).
    submodule_rx: Option<mpsc::Receiver<Vec<PathBuf>>>,
}

impl RepoManager {
    /// Create a new repo manager from a list of paths.
    ///
    /// Discovers top-level repo roots immediately and spawns the
    /// submodule walk in the background. Use [`Self::poll`] from the
    /// panel's update tick to fold the submodule results in when ready.
    pub fn new(paths: &[PathBuf]) -> Self {
        let mut roots = find_toplevel_repos(paths);
        sort_by_display_name(&mut roots);
        let submodule_rx = spawn_repo_walk(&roots, paths);
        Self {
            repos: roots,
            selected: 0,
            submodule_rx,
        }
    }

    /// Create a repo manager for a specific repository.
    ///
    /// Walks up from `repo_path` to find the top-level repo root the
    /// same way [`Self::new`] does, then spawns the submodule walk in
    /// the background. Returns an empty manager if `repo_path` does
    /// not live under a git repository.
    pub fn for_repo(repo_path: PathBuf) -> Self {
        let roots = match find_toplevel_repo(&repo_path) {
            Some(root) => vec![root],
            None => Vec::new(),
        };
        // `repo_path` is a specific repo; no downward nested-repo scan needed.
        let submodule_rx = spawn_repo_walk(&roots, &[]);
        Self {
            repos: roots,
            selected: 0,
            submodule_rx,
        }
    }

    /// Drain the background submodule walk if it has finished.
    ///
    /// The walk result is *merged* into the known set rather than replacing
    /// it. The walk only ever sees the paths handed to the last
    /// [`Self::update`] (or the constructor), so a plain swap dropped repos
    /// discovered from another source — most visibly the repository a
    /// [`Self::for_repo`] panel was opened for, which disappeared from the
    /// dropdown as soon as unrelated panel paths arrived. Entries whose
    /// `.git` is gone are pruned so the merged list cannot accumulate
    /// deleted repositories.
    ///
    /// Returns `true` once the list changed so the caller can trigger a
    /// redraw. Subsequent calls are no-ops until a new walk is spawned
    /// by [`Self::update`].
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.submodule_rx else {
            return false;
        };
        match rx.try_recv() {
            Ok(full) => {
                let current = self.current().map(|p| p.to_path_buf());
                let mut merged = full;
                for repo in &self.repos {
                    if !merged.contains(repo) && repo.join(".git").exists() {
                        merged.push(repo.clone());
                    }
                }
                sort_by_display_name(&mut merged);
                let changed = merged != self.repos;
                self.repos = merged;
                if let Some(c) = current {
                    self.selected = self.repos.iter().position(|r| r == &c).unwrap_or(0);
                }
                self.submodule_rx = None;
                changed
            }
            // Walk hasn't completed yet — keep the receiver around. A
            // disconnected channel (worker thread panicked) is treated
            // the same as "nothing yet"; the next `update()` will reset.
            Err(_) => false,
        }
    }

    /// Get the currently selected repository path.
    pub fn current(&self) -> Option<&Path> {
        self.repos.get(self.selected).map(|p| p.as_path())
    }

    /// Get all discovered repositories.
    pub fn repos(&self) -> &[PathBuf] {
        &self.repos
    }

    /// Get the index of the selected repository.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Select a repository by index.
    pub fn select(&mut self, index: usize) {
        if index < self.repos.len() {
            self.selected = index;
        }
    }

    /// Select the next repository (wrapping to first).
    pub fn select_next(&mut self) {
        if !self.repos.is_empty() {
            self.selected = (self.selected + 1) % self.repos.len();
        }
    }

    /// Select the previous repository (wrapping to last).
    pub fn select_prev(&mut self) {
        if !self.repos.is_empty() {
            self.selected = self.selected.checked_sub(1).unwrap_or(self.repos.len() - 1);
        }
    }

    /// Update repositories from new paths.
    ///
    /// Replaces the top-level set immediately and re-spawns the
    /// submodule walk in the background. Preserves the current
    /// selection if it still exists.
    /// Returns true if the immediate top-level list changed — note that
    /// submodules joining later via [`Self::poll`] also return true
    /// from that call.
    pub fn update(&mut self, paths: &[PathBuf]) -> bool {
        let current = self.current().map(|p| p.to_path_buf());
        let mut new_roots = find_toplevel_repos(paths);
        sort_by_display_name(&mut new_roots);

        // Merge the top-level baseline with the repos we already hold rather
        // than replacing outright. The current set may include submodules and
        // nested projects that a previous async walk surfaced (via `poll`); the
        // bare `new_roots` never contains those, so a plain swap would drop them
        // and — if the selected repo was one of them — reset the selection to
        // the first entry on every navigation. Merging keeps every known repo
        // (so the selection survives); the freshly spawned walk then folds its
        // own findings in via `poll()`, which merges the same way.
        let changed = if new_roots.is_empty() {
            false
        } else {
            let mut merged = new_roots.clone();
            for r in &self.repos {
                if !merged.contains(r) {
                    merged.push(r.clone());
                }
            }
            sort_by_display_name(&mut merged);
            let actually_changed = merged != self.repos;
            self.repos = merged;
            self.selected = current
                .and_then(|c| self.repos.iter().position(|r| r == &c))
                .unwrap_or(0);
            actually_changed
        };
        // Always restart the walk so a later poll() picks up new/removed
        // submodules and nested repos even when the baseline was unchanged.
        self.submodule_rx = spawn_repo_walk(&new_roots, paths);
        changed
    }

    /// Check if there are multiple repositories.
    pub fn has_multiple(&self) -> bool {
        self.repos.len() > 1
    }

    /// Check if there are no repositories.
    pub fn is_empty(&self) -> bool {
        self.repos.is_empty()
    }

    /// Get the number of repositories.
    pub fn len(&self) -> usize {
        self.repos.len()
    }
}

/// Spawn the background repository walk and return its receiver. Two kinds of
/// discovery run off the UI thread and fold into one result:
/// - for each top-level `root`, its submodules (down to [`SUBMODULE_DEPTH`]);
/// - for each `scan_path` that is *not* inside a repository, nested repos under
///   it (down to [`NESTED_REPO_DEPTH`]) — the "folder of git projects" case.
///
/// Returns `None` when there is nothing to walk so callers can skip the
/// `poll()` round-trip entirely.
fn spawn_repo_walk(
    roots: &[PathBuf],
    scan_paths: &[PathBuf],
) -> Option<mpsc::Receiver<Vec<PathBuf>>> {
    if roots.is_empty() && scan_paths.is_empty() {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    let roots: Vec<PathBuf> = roots.to_vec();
    let scan_paths: Vec<PathBuf> = scan_paths.to_vec();
    std::thread::spawn(move || {
        use std::collections::HashSet;
        let mut all: HashSet<PathBuf> = roots.iter().cloned().collect();
        for root in &roots {
            for submodule in find_all_repos(root, SUBMODULE_DEPTH) {
                all.insert(submodule);
            }
        }
        // Scan downward from any path that isn't itself a repo root, to surface
        // nested project repos. We deliberately do NOT skip paths that merely
        // live *inside* some ancestor repo: a whole-home/whole-disk repo (e.g.
        // `~/.git` dotfiles) would otherwise suppress discovery of the real
        // projects under a container directory. A repo root is left to the
        // submodule walk above.
        for path in &scan_paths {
            if path.join(".git").exists() {
                continue;
            }
            for repo in find_all_repos(path, NESTED_REPO_DEPTH) {
                all.insert(repo);
            }
        }
        let mut full: Vec<PathBuf> = all.into_iter().collect();
        sort_by_display_name(&mut full);
        let _ = tx.send(full);
    });
    Some(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty() {
        let manager = RepoManager::new(&[]);
        assert!(manager.is_empty());
        assert!(manager.current().is_none());
        assert!(!manager.has_multiple());
    }

    #[test]
    fn test_for_repo() {
        // for_repo now searches for submodules, so with a non-existent path it returns empty
        let manager = RepoManager::for_repo(PathBuf::from("/test/repo"));
        assert!(manager.is_empty()); // No actual git repo at this path
    }

    #[test]
    fn sorts_repos_by_display_name_not_path() {
        // Path order would put a-dir/Zebra first; name order is apple, mango,
        // Zebra (case-insensitive).
        let mut repos = vec![
            PathBuf::from("/x/z-dir/apple"),
            PathBuf::from("/x/a-dir/Zebra"),
            PathBuf::from("/x/m-dir/mango"),
        ];
        sort_by_display_name(&mut repos);
        let names: Vec<String> = repos.iter().map(|p| crate::get_repo_name(p)).collect();
        assert_eq!(names, vec!["apple", "mango", "Zebra"]);
    }

    #[test]
    fn update_preserves_selection_of_async_discovered_repo() {
        use std::fs;
        // Two real top-level repos so find_toplevel_repos returns them.
        let tmp = std::env::temp_dir().join(format!("termide-rm-sel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("a/.git")).unwrap();
        fs::create_dir_all(tmp.join("b/.git")).unwrap();
        let a = std::fs::canonicalize(tmp.join("a")).unwrap();
        let b = std::fs::canonicalize(tmp.join("b")).unwrap();

        let mut mgr = RepoManager::new(&[a.clone(), b.clone()]);
        // Simulate a submodule/nested repo the async walk surfaced and that the
        // user then selected.
        let nested = a.join("vendor/lib");
        if !mgr.repos.contains(&nested) {
            mgr.repos.push(nested.clone());
        }
        mgr.selected = mgr.repos.iter().position(|r| r == &nested).unwrap();

        // A path update that only re-derives the top-level set must keep the
        // nested repo and the selection on it.
        mgr.update(&[a.clone(), b.clone()]);
        let _ = fs::remove_dir_all(&tmp);

        assert!(
            mgr.repos().contains(&nested),
            "async-discovered repo was dropped: {:?}",
            mgr.repos()
        );
        assert_eq!(
            mgr.current(),
            Some(nested.as_path()),
            "selection reset off the nested repo"
        );
    }

    // A panel opened for one specific repository must keep that repository in
    // the dropdown after unrelated panel paths arrive and the background walk
    // lands — the walk never sees the `for_repo` root, so `poll()` has to merge
    // instead of replacing.
    #[test]
    fn poll_keeps_repo_the_panel_was_opened_for() {
        use std::fs;
        let tmp = std::env::temp_dir().join(format!("termide-rm-poll-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let deep = tmp.join("container/lvl1/lvl2/deeprepo");
        fs::create_dir_all(deep.join(".git")).unwrap();
        let container = std::fs::canonicalize(tmp.join("container")).unwrap();
        let deep = std::fs::canonicalize(&deep).unwrap();

        let mut mgr = RepoManager::for_repo(deep.clone());
        assert_eq!(mgr.current(), Some(deep.as_path()));

        // Panel paths for a directory that only *contains* the repo, deeper
        // than the nested scan reaches.
        mgr.update(std::slice::from_ref(&container));
        let mut tries = 0;
        while mgr.submodule_rx.is_some() && tries < 300 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            mgr.poll();
            tries += 1;
        }

        let repos = mgr.repos().to_vec();
        let current = mgr.current().map(|p| p.to_path_buf());
        let _ = fs::remove_dir_all(&tmp);
        assert!(
            repos.contains(&deep),
            "the panel's own repo was dropped by poll(): {repos:?}"
        );
        assert_eq!(
            current.as_deref(),
            Some(deep.as_path()),
            "selection moved off the panel's own repo"
        );
    }

    // Repos that disappeared from disk must not survive the merge forever.
    #[test]
    fn poll_prunes_deleted_repos() {
        use std::fs;
        let tmp = std::env::temp_dir().join(format!("termide-rm-prune-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("live/.git")).unwrap();
        let live = std::fs::canonicalize(tmp.join("live")).unwrap();

        let mut mgr = RepoManager::new(std::slice::from_ref(&live));
        let gone = tmp.join("gone");
        mgr.repos.push(gone.clone());
        let mut tries = 0;
        while mgr.submodule_rx.is_some() && tries < 300 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            mgr.poll();
            tries += 1;
        }

        let repos = mgr.repos().to_vec();
        let _ = fs::remove_dir_all(&tmp);
        assert!(repos.contains(&live), "live repo missing: {repos:?}");
        assert!(
            !repos.contains(&gone),
            "deleted repo survived the merge: {repos:?}"
        );
    }

    #[test]
    fn test_select_bounds() {
        let mut manager = RepoManager::new(&[]);
        manager.select(10); // Out of bounds on empty
        assert_eq!(manager.selected_index(), 0); // Unchanged
    }

    // Opening termide in a directory that is not itself a repo but contains
    // git projects should surface those nested repos via the async walk — even
    // when that directory lives inside an ancestor repo (e.g. a whole-home
    // dotfiles repo), which must not suppress discovery.
    #[test]
    fn discovers_nested_repos_under_non_repo_root() {
        use std::fs;
        let tmp = std::env::temp_dir().join(format!("termide-rm-nested-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("repo-a/.git")).unwrap();
        fs::create_dir_all(tmp.join("repo-b/.git")).unwrap();
        fs::create_dir_all(tmp.join("plain")).unwrap(); // not a repo

        let mut mgr = RepoManager::new(std::slice::from_ref(&tmp));
        // The nested scan is async, so poll until repo-a lands (or we give up).
        // Don't wait on is_empty(): an ancestor repo (e.g. a stray /tmp/.git)
        // can make the list non-empty immediately without the nested repos yet.
        let mut tries = 0;
        while !mgr.repos().iter().any(|r| r.ends_with("repo-a")) && tries < 300 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            mgr.poll();
            tries += 1;
        }

        let repos = mgr.repos().to_vec();
        let _ = fs::remove_dir_all(&tmp);
        assert!(
            repos.iter().any(|r| r.ends_with("repo-a")),
            "repo-a not discovered: {repos:?}"
        );
        assert!(
            repos.iter().any(|r| r.ends_with("repo-b")),
            "repo-b not discovered: {repos:?}"
        );
    }
}
