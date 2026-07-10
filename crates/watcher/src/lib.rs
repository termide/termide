//! Unified filesystem and git watcher for termide.
//!
//! Provides filesystem change notifications with git awareness.
//! - Watches files/directories with reference counting
//! - Filters .git/ events to only commit-related changes
//! - Separate debounce: 300ms for files, 250ms for git

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebouncedEvent, Debouncer};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

/// Debounce duration for filesystem events.
pub const FS_DEBOUNCE_MS: u64 = 300;
/// Extra coalescing window for git events, measured from the FIRST change in a
/// burst (not sliding) so a commit's `.git` writes surface within a bounded
/// delay even while the repo keeps churning.
pub const GIT_DEBOUNCE_MS: u64 = 250;

/// Watch event types emitted by UnifiedWatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// File changed (for Editor: external modification check)
    FileChanged(PathBuf),
    /// Directory changed (for FileManager: refresh listing)
    DirectoryChanged {
        /// Root path being watched
        root: PathBuf,
        /// Actual path that changed
        changed: PathBuf,
    },
    /// Git commit occurred (for all: update git status/diff)
    GitCommit(PathBuf),
    /// .gitignore changed - watcher needs reinitialization
    GitignoreChanged(PathBuf),
}

/// Internal event from debouncer callback.
#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum InternalEvent {
    /// Regular filesystem change
    FsChange { changed_path: PathBuf },
    /// Git-related change (needs additional debounce)
    GitChange { repo_root: PathBuf },
    /// .gitignore changed
    GitignoreChange { repo_root: PathBuf },
}

/// Unified watcher for filesystem and git changes.
///
/// Per-repo install progress for [`UnifiedWatcher::poll_pending`].
///
/// Holds the path queue (already walked, not yet handed to notify) and
/// the set of paths already installed for this repo. We don't update
/// `UnifiedWatcher::watched_repos` until the queue drains so that
/// `is_watching_repo` returns `true` across the full install window —
/// otherwise `register_panel_watchers` would race with the install and
/// schedule a duplicate walk on the next tick.
#[derive(Debug)]
struct RepoInstallState {
    repo_root: PathBuf,
    queue: Vec<PathBuf>,
    installed: HashSet<PathBuf>,
}

/// Maximum number of `notify::Watcher::watch` calls processed per tick.
/// Each call is a syscall (inotify_add_watch on Linux); 256 keeps a
/// burst under ~3 ms on a healthy machine while still draining a
/// 5000-directory repo within ~20 ticks (well under one second of
/// real time at typical tick rates).
const INSTALL_CHUNK: usize = 256;

/// Combines functionality of FileSystemWatcher and GitWatcher:
/// - Reference counting for all watches
/// - Filters .git/ to only index/HEAD/refs changes -> GitCommit events
/// - Base debounce 300ms for files, manual 250ms debounce for git
#[derive(Debug)]
pub struct UnifiedWatcher {
    debouncer: Debouncer<RecommendedWatcher>,
    /// Git repos: repo_root -> (reference_count, watched_paths)
    watched_repos: HashMap<PathBuf, (usize, HashSet<PathBuf>)>,
    /// Repos whose directory walk is still running on a worker thread.
    /// `poll_pending` drains these into `pending_repo_installs` when
    /// ready. `is_watching_repo` reports `true` for pending repos so
    /// callers don't restart the walk.
    pending_repo_walks: HashMap<PathBuf, Receiver<Vec<PathBuf>>>,
    /// Walks that have completed and are now being installed chunk by
    /// chunk. Each tick `poll_pending` consumes at most
    /// `INSTALL_CHUNK` paths so a repo with thousands of directories
    /// doesn't spike a single tick into a visible freeze.
    pending_repo_installs: Vec<RepoInstallState>,
    /// Non-git dirs: dir_path -> reference count (NonRecursive mode)
    watched_dirs: HashMap<PathBuf, usize>,
    /// Receiver for internal events from debouncer callback
    internal_rx: Receiver<InternalEvent>,
    /// Pending git events waiting for 1000ms debounce
    pending_git: HashMap<PathBuf, Instant>,
    /// Pending gitignore changes waiting for debounce
    pending_gitignore: HashMap<PathBuf, Instant>,
    /// Pending fs changes to emit
    pending_fs: HashSet<PathBuf>,
}

impl UnifiedWatcher {
    /// Create a new UnifiedWatcher.
    pub fn new() -> Result<Self> {
        let (internal_tx, internal_rx) = channel();

        let debouncer = new_debouncer(
            Duration::from_millis(FS_DEBOUNCE_MS),
            move |result: notify_debouncer_mini::DebounceEventResult| {
                if let Ok(events) = result {
                    for event in events {
                        Self::process_raw_event(&event, &internal_tx);
                    }
                }
            },
        )
        .context("Failed to create filesystem watcher")?;

        Ok(Self {
            debouncer,
            watched_repos: HashMap::new(),
            pending_repo_walks: HashMap::new(),
            pending_repo_installs: Vec::new(),
            watched_dirs: HashMap::new(),
            internal_rx,
            pending_git: HashMap::new(),
            pending_gitignore: HashMap::new(),
            pending_fs: HashSet::new(),
        })
    }

    /// Process raw event from debouncer, classify and send to internal channel.
    fn process_raw_event(event: &DebouncedEvent, tx: &Sender<InternalEvent>) {
        let path = &event.path;

        // Check if this is a .gitignore change
        if path.file_name() == Some(OsStr::new(".gitignore")) {
            // Find repo root by walking up to .git directory
            if let Some(repo_root) = Self::find_repo_root(path) {
                let _ = tx.send(InternalEvent::GitignoreChange { repo_root });
            }
            // Also emit as regular fs change so FileManager updates
            let _ = tx.send(InternalEvent::FsChange {
                changed_path: path.clone(),
            });
            return;
        }

        // Check if this is a .git/ event
        if Self::is_git_path(path) {
            // Filter to only commit-related files
            if Self::is_commit_related(path) {
                if let Some(repo_root) = Self::find_repo_root_from_git_path(path) {
                    let _ = tx.send(InternalEvent::GitChange { repo_root });
                }
            }
            // Ignore other .git/* events
        } else {
            // Regular filesystem change
            let _ = tx.send(InternalEvent::FsChange {
                changed_path: path.clone(),
            });
        }
    }

    /// Find repository root by walking up from a path.
    fn find_repo_root(path: &Path) -> Option<PathBuf> {
        let mut current = path.parent()?;
        loop {
            if current.join(".git").exists() {
                return Some(current.to_path_buf());
            }
            current = current.parent()?;
        }
    }

    /// Check if path is inside .git directory.
    fn is_git_path(path: &Path) -> bool {
        path.components().any(|c| c.as_os_str() == ".git")
    }

    /// Check if git path is commit-related (index, HEAD, refs, merge/rebase state).
    fn is_commit_related(path: &Path) -> bool {
        if let Some(
            "index" | "HEAD" | "MERGE_HEAD" | "FETCH_HEAD" | "REBASE_HEAD" | "CHERRY_PICK_HEAD"
            | "ORIG_HEAD" | "COMMIT_EDITMSG",
        ) = path.file_name().and_then(|n| n.to_str())
        {
            return true;
        }
        // Also check for refs/* and logs/* changes
        let path_str = path.to_string_lossy();
        path_str.contains("/refs/") || path_str.contains("/logs/")
    }

    /// Find repository root from a path inside .git directory.
    /// Canonicalizes the result to match paths stored by panels (resolves symlinks/mounts).
    fn find_repo_root_from_git_path(path: &Path) -> Option<PathBuf> {
        let mut current = path;
        while let Some(parent) = current.parent() {
            if parent.file_name().and_then(|n| n.to_str()) == Some(".git") {
                let repo_root = parent.parent()?;
                return Some(
                    std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf()),
                );
            }
            current = parent;
        }
        None
    }

    /// Start watching a git repository root, respecting .gitignore.
    ///
    /// The `WalkBuilder` traversal that collects the per-directory list
    /// is the slow part — on a repo that tracks `$HOME` it can take
    /// seconds. It runs on a worker thread; the actual `watcher.watch`
    /// installs happen when [`Self::poll_pending`] picks up the result.
    /// Until then [`Self::is_watching_repo`] reports `true` for the
    /// pending root so callers don't re-spawn the walk.
    ///
    /// File-system events emitted before the walk finishes are simply
    /// missed — same behaviour as the synchronous path while it was
    /// still mid-walk; once watches are installed the panel picks up
    /// changes from there on out.
    pub fn watch_repository(&mut self, repo_root: PathBuf) -> Result<()> {
        // Increment reference count if already fully watching
        if let Some((count, _)) = self.watched_repos.get_mut(&repo_root) {
            *count += 1;
            return Ok(());
        }
        // Already pending — keep one walk in flight per root. The
        // caller has no separate refcount on the pending entry; the
        // implicit count is 1 and gets folded into watched_repos when
        // the worker reports back.
        if self.pending_repo_walks.contains_key(&repo_root) {
            return Ok(());
        }

        let (tx, rx) = channel();
        let repo = repo_root.clone();
        std::thread::spawn(move || {
            let paths = collect_repo_watch_paths(&repo);
            let _ = tx.send(paths);
        });
        self.pending_repo_walks.insert(repo_root, rx);
        Ok(())
    }

    /// Stop watching a git repository (decrement reference count).
    /// Only unwatches when count reaches 0.
    pub fn unwatch_repository(&mut self, repo_root: &Path) {
        if let Some((count, _)) = self.watched_repos.get_mut(repo_root) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                // Remove and get the watched paths
                if let Some((_, watched_paths)) = self.watched_repos.remove(repo_root) {
                    let watcher = self.debouncer.watcher();
                    // Unwatch all paths that were watched for this repo
                    for path in watched_paths {
                        let _ = watcher.unwatch(&path);
                    }
                }
            }
        }
        // If the registration was still mid-walk or mid-install, drop
        // it so we don't keep installing watches the caller no longer
        // wants. Anything already in `installed` stays in notify until
        // the panel decides to register again or shut down.
        self.pending_repo_walks.remove(repo_root);
        self.pending_repo_installs
            .retain(|s| s.repo_root != repo_root);
    }

    /// Check if repository root is being watched (including the pending
    /// walk and chunked install phases). Returning `true` here keeps
    /// `register_panel_watchers` from spinning up a second walk for the
    /// same root.
    pub fn is_watching_repo(&self, repo_root: &Path) -> bool {
        self.watched_repos.contains_key(repo_root)
            || self.pending_repo_walks.contains_key(repo_root)
            || self
                .pending_repo_installs
                .iter()
                .any(|s| s.repo_root == repo_root)
    }

    /// Drain background work in two stages so a single tick never
    /// spikes:
    /// 1. Pull finished `WalkBuilder` results off
    ///    `pending_repo_walks` and queue them for install.
    /// 2. Install up to `INSTALL_CHUNK` watches per tick from the
    ///    queue. Each `watcher.watch` is a syscall, and a repo with
    ///    thousands of dirs would otherwise stall the main loop for
    ///    ~tens of milliseconds.
    ///
    /// Returns `true` if any state changed (a walk landed or a chunk
    /// installed) so the caller can request a redraw.
    pub fn poll_pending(&mut self) -> bool {
        let mut any_progress = false;

        // Stage 1: move finished walks into the install queue. Capture the
        // walk result in the SAME `try_recv` — the channel delivers the paths
        // exactly once, so a second `try_recv` would find it empty/disconnected
        // and silently drop the repo (its watches would never install).
        let mut landed: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
        let mut drop_disconnected: Vec<PathBuf> = Vec::new();
        for (repo, rx) in &self.pending_repo_walks {
            match rx.try_recv() {
                Ok(paths) => landed.push((repo.clone(), paths)),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    drop_disconnected.push(repo.clone())
                }
            }
        }
        for repo in drop_disconnected {
            self.pending_repo_walks.remove(&repo);
            any_progress = true;
        }
        for (repo, paths) in landed {
            self.pending_repo_walks.remove(&repo);
            self.pending_repo_installs.push(RepoInstallState {
                repo_root: repo,
                queue: paths,
                installed: HashSet::new(),
            });
            any_progress = true;
        }

        // Stage 2: install up to INSTALL_CHUNK watches across all
        // pending repos this tick. Round-robin across repos keeps a
        // single huge repo from starving smaller siblings.
        let mut budget = INSTALL_CHUNK;
        while budget > 0 && !self.pending_repo_installs.is_empty() {
            let mut made_progress = false;
            let mut finished_indices: Vec<usize> = Vec::new();
            for (i, state) in self.pending_repo_installs.iter_mut().enumerate() {
                if budget == 0 {
                    break;
                }
                if state.queue.is_empty() {
                    finished_indices.push(i);
                    continue;
                }
                let take = budget.min(state.queue.len());
                let watcher = self.debouncer.watcher();
                for path in state.queue.drain(state.queue.len() - take..) {
                    if watcher.watch(&path, RecursiveMode::NonRecursive).is_ok() {
                        state.installed.insert(path);
                    }
                }
                budget -= take;
                made_progress = true;
                if state.queue.is_empty() {
                    finished_indices.push(i);
                }
            }
            if !made_progress {
                break;
            }
            // Walk in reverse so removals don't shift earlier indices.
            for i in finished_indices.into_iter().rev() {
                let done = self.pending_repo_installs.swap_remove(i);
                self.watched_repos
                    .insert(done.repo_root, (1, done.installed));
                any_progress = true;
            }
        }

        any_progress
    }

    /// Install non-recursive watches for directories that appeared inside an
    /// already-watched git repo after registration (e.g. a terminal/agent ran
    /// `mkdir`). The initial repo walk only covers directories that existed at
    /// registration time, and watches are non-recursive — so without this a new
    /// subtree emits no events at all. Respects `.gitignore` (via
    /// [`collect_repo_watch_paths`]) and is additive: it extends the repo's
    /// watched-path set without touching its reference count.
    fn watch_new_repo_dirs(&mut self, changed: &[PathBuf]) {
        if self.watched_repos.is_empty() {
            return;
        }

        // Plan the installs first (immutable reads of `watched_repos`) so the
        // mutable watcher/`watched_repos` borrows below don't overlap.
        let mut plan: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
        for path in changed {
            if !path.is_dir() {
                continue;
            }
            let Some(repo_root) = self
                .watched_repos
                .keys()
                .find(|r| path.starts_with(r))
                .cloned()
            else {
                continue;
            };
            let Some((_, watched)) = self.watched_repos.get(&repo_root) else {
                continue;
            };
            if watched.contains(path) {
                continue; // already watched — nothing new below it
            }
            let fresh: Vec<PathBuf> = collect_repo_watch_paths(path)
                .into_iter()
                .filter(|p| !watched.contains(p))
                .collect();
            if !fresh.is_empty() {
                plan.push((repo_root, fresh));
            }
        }

        for (repo_root, fresh) in plan {
            let mut installed = Vec::new();
            {
                let watcher = self.debouncer.watcher();
                for p in &fresh {
                    if watcher.watch(p, RecursiveMode::NonRecursive).is_ok() {
                        installed.push(p.clone());
                    }
                }
            }
            if let Some((_, watched)) = self.watched_repos.get_mut(&repo_root) {
                watched.extend(installed);
            }
        }
    }

    /// Start watching a non-git directory (non-recursive, direct children only).
    /// Increments reference count if already watching.
    pub fn watch_directory(&mut self, dir_path: PathBuf) -> Result<()> {
        // Increment reference count if already watching
        if let Some(count) = self.watched_dirs.get_mut(&dir_path) {
            *count += 1;
            return Ok(());
        }

        let watcher = self.debouncer.watcher();
        watcher.watch(&dir_path, RecursiveMode::NonRecursive)?;

        self.watched_dirs.insert(dir_path, 1);
        Ok(())
    }

    /// Stop watching a non-git directory (decrement reference count).
    /// Only unwatches when count reaches 0.
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

    /// Check if non-git directory is being watched.
    pub fn is_watching_dir(&self, dir_path: &Path) -> bool {
        self.watched_dirs.contains_key(dir_path)
    }

    /// Poll for pending events.
    /// Call this periodically (e.g., on tick) to get accumulated events.
    pub fn poll_events(&mut self) -> Vec<WatchEvent> {
        // Collect all internal events
        while let Ok(event) = self.internal_rx.try_recv() {
            match event {
                InternalEvent::FsChange { changed_path } => {
                    self.pending_fs.insert(changed_path);
                }
                InternalEvent::GitChange { repo_root } => {
                    // Fixed window from the first change in a burst — do NOT
                    // reset on every event, or continuous repo churn would keep
                    // pushing the deadline out and the update would never fire.
                    self.pending_git
                        .entry(repo_root)
                        .or_insert_with(Instant::now);
                }
                InternalEvent::GitignoreChange { repo_root } => {
                    // Update timestamp for gitignore debounce
                    self.pending_gitignore.insert(repo_root, Instant::now());
                }
            }
        }

        let mut events = Vec::new();
        let now = Instant::now();

        // Emit git events that have been debounced for 1000ms
        let git_debounce = Duration::from_millis(GIT_DEBOUNCE_MS);
        let ready_git: Vec<PathBuf> = self
            .pending_git
            .iter()
            .filter(|(_, timestamp)| now.duration_since(**timestamp) >= git_debounce)
            .map(|(path, _)| path.clone())
            .collect();

        for repo_root in ready_git {
            self.pending_git.remove(&repo_root);
            events.push(WatchEvent::GitCommit(repo_root));
        }

        // Emit gitignore events that have been debounced for 1000ms
        let ready_gitignore: Vec<PathBuf> = self
            .pending_gitignore
            .iter()
            .filter(|(_, timestamp)| now.duration_since(**timestamp) >= git_debounce)
            .map(|(path, _)| path.clone())
            .collect();

        for repo_root in ready_gitignore {
            self.pending_gitignore.remove(&repo_root);
            events.push(WatchEvent::GitignoreChanged(repo_root));
        }

        // Emit filesystem events (already debounced by notify at 300ms)
        // Collect first to avoid borrow conflict
        let pending_fs: Vec<PathBuf> = self.pending_fs.drain().collect();
        // Subscribe any directories created since registration so their
        // contents keep emitting events (non-recursive watches don't cover
        // subdirectories created later).
        self.watch_new_repo_dirs(&pending_fs);
        for changed_path in pending_fs {
            // Find the watched root for this path
            let root = self.find_watched_root(&changed_path);
            events.push(WatchEvent::DirectoryChanged {
                root,
                changed: changed_path,
            });
        }

        events
    }

    /// Find the watched root that contains this path.
    fn find_watched_root(&self, path: &Path) -> PathBuf {
        // First check watched repos
        for repo_root in self.watched_repos.keys() {
            if path.starts_with(repo_root) {
                return repo_root.clone();
            }
        }

        // Then check watched directories
        for dir_path in self.watched_dirs.keys() {
            if path.starts_with(dir_path) {
                return dir_path.clone();
            }
        }

        // Fallback to parent directory
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| path.to_path_buf())
    }
}

/// Create a unified watcher instance.
pub fn create_watcher() -> Result<UnifiedWatcher> {
    UnifiedWatcher::new()
}

/// Walk `repo_root` respecting `.gitignore` and collect the per-directory
/// list that `watch_repository` will install non-recursive watches for.
///
/// The walk is the slow part — on repos that track `$HOME` it can take
/// seconds — so the public API hides this behind a worker thread.
fn collect_repo_watch_paths(repo_root: &Path) -> Vec<PathBuf> {
    let git_objects_dir = repo_root.join(".git/objects");
    WalkBuilder::new(repo_root)
        .hidden(false) // we still need the `.git/` directory
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_some_and(|ft| ft.is_dir()))
        .map(|e| e.into_path())
        // Skip `.git/objects/<hash-prefix>/` — hundreds of subdirs that
        // emit no commit-relevant events.
        .filter(|path| !(path.starts_with(&git_objects_dir) && path != &git_objects_dir))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_git_path() {
        assert!(UnifiedWatcher::is_git_path(Path::new(
            "/home/user/project/.git/index"
        )));
        assert!(UnifiedWatcher::is_git_path(Path::new(
            "/home/user/project/.git/refs/heads/main"
        )));
        assert!(!UnifiedWatcher::is_git_path(Path::new(
            "/home/user/project/src/main.rs"
        )));
    }

    #[test]
    fn test_is_commit_related() {
        assert!(UnifiedWatcher::is_commit_related(Path::new(
            "/project/.git/index"
        )));
        assert!(UnifiedWatcher::is_commit_related(Path::new(
            "/project/.git/HEAD"
        )));
        assert!(UnifiedWatcher::is_commit_related(Path::new(
            "/project/.git/refs/heads/main"
        )));
        assert!(UnifiedWatcher::is_commit_related(Path::new(
            "/project/.git/logs/HEAD"
        )));
        assert!(!UnifiedWatcher::is_commit_related(Path::new(
            "/project/.git/objects/ab/cdef"
        )));
        assert!(!UnifiedWatcher::is_commit_related(Path::new(
            "/project/.git/config"
        )));
    }

    #[test]
    fn test_find_repo_root() {
        let path = PathBuf::from("/home/user/project/.git/refs/heads/main");
        let root = UnifiedWatcher::find_repo_root_from_git_path(&path);
        assert_eq!(root, Some(PathBuf::from("/home/user/project")));

        let path = PathBuf::from("/home/user/project/.git/index");
        let root = UnifiedWatcher::find_repo_root_from_git_path(&path);
        assert_eq!(root, Some(PathBuf::from("/home/user/project")));

        let path = PathBuf::from("/home/user/project/src/main.rs");
        let root = UnifiedWatcher::find_repo_root_from_git_path(&path);
        assert_eq!(root, None);
    }

    #[test]
    fn watches_directories_created_after_registration() {
        // Regression: the initial repo walk only covers directories that exist
        // at registration; watches are non-recursive. A subtree created later
        // (e.g. by a terminal/agent) must get its own watches so git status
        // keeps updating without navigating away and back.
        use std::fs;

        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
        fs::create_dir(root.join("src")).unwrap();

        let mut w = UnifiedWatcher::new().unwrap();
        w.watch_repository(root.clone()).unwrap();
        // Drive the async walk + chunked install to completion.
        for _ in 0..2000 {
            w.poll_pending();
            if w.watched_repos.contains_key(&root) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            w.watched_repos.contains_key(&root),
            "repo should be fully watched after registration"
        );
        assert!(
            !w.watched_repos[&root].1.contains(&root.join("newdir")),
            "newdir does not exist yet, must not be watched"
        );

        // Create a nested subtree after registration and subscribe it.
        fs::create_dir_all(root.join("newdir/sub")).unwrap();
        w.watch_new_repo_dirs(&[root.join("newdir")]);

        let watched = &w.watched_repos[&root].1;
        assert!(
            watched.contains(&root.join("newdir")),
            "the created directory must be watched: {watched:?}"
        );
        assert!(
            watched.contains(&root.join("newdir/sub")),
            "nested created directories must be watched too: {watched:?}"
        );
        // Reference count is untouched by the additive install.
        assert_eq!(w.watched_repos[&root].0, 1, "refcount must be preserved");
    }
}
