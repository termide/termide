//! Unified filesystem and git watcher for termide.
//!
//! Provides filesystem change notifications with git awareness.
//! - Watches files/directories with reference counting
//! - Filters .git/ events to only commit-related changes
//! - Separate debounce: 300ms for files, 250ms for git

use anyhow::{Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
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

/// What a walk on a worker thread was started for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WalkKind {
    /// The repo's whole watch set: the first registration, or a reload after
    /// its ignore rules changed. Authoritative — directories it no longer
    /// lists lose their watch.
    Full,
    /// Directories that appeared inside the repo after the full walk; added
    /// to the set, nothing removed.
    Extend,
}

/// What the worker thread hands back for one walk.
struct RepoWalk {
    /// Directories to install watches for (just the root when
    /// [`RECURSIVE_ROOT_WATCH`] is on).
    paths: Vec<PathBuf>,
    /// The repo's ignore rules, used to drop events from ignored subtrees
    /// that a recursive root watch delivers anyway. Only full walks carry it.
    ignore: Option<Gitignore>,
}

/// One watched repository.
///
/// The entry exists from the first [`UnifiedWatcher::watch_repository`] to
/// the matching last [`UnifiedWatcher::unwatch_repository`], whatever stage
/// the install is in: the reference count is kept from the first call, not
/// from the moment the walk lands, so every acquire has its release.
#[derive(Debug)]
struct RepoWatch {
    /// How many holders asked for the repo.
    refs: usize,
    /// Directories with an installed watch.
    watched: HashSet<PathBuf>,
    /// Walked directories not yet handed to notify; drained
    /// [`INSTALL_CHUNK`] at a time by [`UnifiedWatcher::poll_pending`].
    queue: Vec<PathBuf>,
    /// Walks still running on worker threads.
    walks: Vec<(WalkKind, Receiver<RepoWalk>)>,
    /// The repo's ignore rules, consulted for events that a recursive root
    /// watch delivers from ignored subtrees.
    ignore: Option<Gitignore>,
}

impl RepoWatch {
    /// Whether a full walk is still running for the repo.
    fn full_walk_pending(&self) -> bool {
        self.walks.iter().any(|(kind, _)| *kind == WalkKind::Full)
    }

    /// Whether the watch set is complete: no full walk running and nothing
    /// left to install.
    #[cfg(test)]
    fn is_ready(&self) -> bool {
        !self.full_walk_pending() && self.queue.is_empty()
    }

    /// Take a finished walk's result into the install queue.
    fn land(&mut self, kind: WalkKind, walk: RepoWalk, watcher: &mut dyn notify::Watcher) {
        match kind {
            WalkKind::Full => {
                let fresh: HashSet<PathBuf> = walk.paths.into_iter().collect();
                // Directories the rules now exclude lose their watch.
                let stale: Vec<PathBuf> = self.watched.difference(&fresh).cloned().collect();
                for path in stale {
                    let _ = watcher.unwatch(&path);
                    self.watched.remove(&path);
                }
                self.queue = fresh
                    .into_iter()
                    .filter(|path| !self.watched.contains(path))
                    .collect();
                self.ignore = walk.ignore;
            }
            WalkKind::Extend => {
                let known: HashSet<&PathBuf> = self.queue.iter().collect();
                let fresh: Vec<PathBuf> = walk
                    .paths
                    .into_iter()
                    .filter(|path| !self.watched.contains(path) && !known.contains(path))
                    .collect();
                self.queue.extend(fresh);
            }
        }
    }
}

/// Maximum number of `notify::Watcher::watch` calls processed per tick.
/// Each call is a syscall (inotify_add_watch on Linux); 256 keeps a
/// burst under ~3 ms on a healthy machine while still draining a
/// 5000-directory repo within ~20 ticks (well under one second of
/// real time at typical tick rates).
const INSTALL_CHUNK: usize = 256;

/// Watch a repository with one recursive watch on its root instead of a
/// non-recursive watch per directory.
///
/// FSEvents streams are recursive by nature, and notify's backend restarts
/// the whole stream on every `watch()` call: it joins the run-loop thread,
/// spawns a new one and calls `FSEventStreamCreate` over every path already
/// registered. Per-directory watches therefore cost O(N²) and block the main
/// loop for minutes on a repo with a few thousand directories (a tracked
/// `node_modules` is enough). One recursive watch is one restart; events
/// from ignored subtrees are filtered when they arrive instead of by never
/// subscribing to them. inotify has no recursive mode and a cheap
/// per-directory syscall, so Linux keeps the walk.
const RECURSIVE_ROOT_WATCH: bool = cfg!(target_os = "macos");

/// Something to watch, as a holder asks for it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WatchTarget {
    /// A git repository root: every directory not ignored by its rules.
    Repo(PathBuf),
    /// A directory outside any repository: its direct children only.
    Dir(PathBuf),
}

/// Combines functionality of FileSystemWatcher and GitWatcher:
/// - Reference counting for all watches
/// - Filters .git/ to only index/HEAD/refs changes -> GitCommit events
/// - Base debounce 300ms for files, manual 250ms debounce for git
#[derive(Debug)]
pub struct UnifiedWatcher {
    debouncer: Debouncer<RecommendedWatcher>,
    /// Git repos by root, from the first acquire to the last release.
    repos: HashMap<PathBuf, RepoWatch>,
    /// Non-git dirs: dir_path -> reference count (NonRecursive mode)
    watched_dirs: HashMap<PathBuf, usize>,
    /// Receiver for internal events from debouncer callback
    internal_rx: Receiver<InternalEvent>,
    /// Pending git events waiting for the `GIT_DEBOUNCE_MS` debounce
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
            repos: HashMap::new(),
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
                    dunce::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf()),
                );
            }
            current = parent;
        }
        None
    }

    /// Acquire a watch on `target`; pair it with one [`Self::unwatch`].
    pub fn watch(&mut self, target: &WatchTarget) -> Result<()> {
        match target {
            WatchTarget::Repo(root) => self.watch_repository(root.clone()),
            WatchTarget::Dir(dir) => self.watch_directory(dir.clone()),
        }
    }

    /// Release one watch acquired with [`Self::watch`].
    pub fn unwatch(&mut self, target: &WatchTarget) {
        match target {
            WatchTarget::Repo(root) => self.unwatch_repository(root),
            WatchTarget::Dir(dir) => self.unwatch_directory(dir),
        }
    }

    /// Start watching a git repository root, respecting .gitignore, or count
    /// one more holder of a repo already watched.
    ///
    /// The `WalkBuilder` traversal that collects the per-directory list
    /// is the slow part — on a repo that tracks `$HOME` it can take
    /// seconds. It runs on a worker thread; the actual `watcher.watch`
    /// installs happen when [`Self::poll_pending`] picks up the result.
    ///
    /// File-system events emitted before the walk finishes are simply
    /// missed; once watches are installed the panel picks up changes from
    /// there on out.
    pub fn watch_repository(&mut self, repo_root: PathBuf) -> Result<()> {
        if let Some(repo) = self.repos.get_mut(&repo_root) {
            repo.refs += 1;
            return Ok(());
        }
        let walk = spawn_full_walk(&repo_root);
        self.repos.insert(
            repo_root,
            RepoWatch {
                refs: 1,
                watched: HashSet::new(),
                queue: Vec::new(),
                walks: vec![(WalkKind::Full, walk)],
                ignore: None,
            },
        );
        Ok(())
    }

    /// Release one holder of a git repository. The last release removes
    /// every watch installed for it and abandons its running walks.
    pub fn unwatch_repository(&mut self, repo_root: &Path) {
        let Some(repo) = self.repos.get_mut(repo_root) else {
            return;
        };
        repo.refs = repo.refs.saturating_sub(1);
        if repo.refs > 0 {
            return;
        }
        if let Some(repo) = self.repos.remove(repo_root) {
            let watcher = self.debouncer.watcher();
            for path in repo.watched {
                let _ = watcher.unwatch(&path);
            }
            // Dropping the receivers abandons the walks: their send fails.
        }
    }

    /// Re-read a watched repository's ignore rules after its `.gitignore`
    /// changed: a fresh full walk replaces the watch set when it lands,
    /// unwatching directories now ignored and adding those no longer
    /// ignored. The holders and their count are untouched, and the watches
    /// stay live meanwhile.
    pub fn reload_repo_ignores(&mut self, repo_root: &Path) {
        let Some(repo) = self.repos.get_mut(repo_root) else {
            return;
        };
        // A full walk still running read the old rules; the new one supersedes it.
        repo.walks.retain(|(kind, _)| *kind != WalkKind::Full);
        repo.walks
            .push((WalkKind::Full, spawn_full_walk(repo_root)));
    }

    /// Check if repository root is being watched, including while its walk
    /// runs and its watches install.
    pub fn is_watching_repo(&self, repo_root: &Path) -> bool {
        self.repos.contains_key(repo_root)
    }

    /// Drain background work in two stages so a single tick never
    /// spikes:
    /// 1. Take finished walks off the worker threads and queue their
    ///    directories for install.
    /// 2. Install up to `INSTALL_CHUNK` watches per tick from the
    ///    queues. Each `watcher.watch` is a syscall, and a repo with
    ///    thousands of dirs would otherwise stall the main loop for
    ///    ~tens of milliseconds.
    ///
    /// Returns `true` if any state changed (a walk landed or a chunk
    /// installed) so the caller can request a redraw.
    pub fn poll_pending(&mut self) -> bool {
        let mut any_progress = false;
        let watcher = self.debouncer.watcher();

        // Stage 1: each walk's channel delivers its result exactly once.
        for repo in self.repos.values_mut() {
            let mut landed: Vec<(WalkKind, RepoWalk)> = Vec::new();
            repo.walks.retain(|(kind, rx)| match rx.try_recv() {
                Ok(walk) => {
                    landed.push((*kind, walk));
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
            });
            for (kind, walk) in landed {
                repo.land(kind, walk, watcher);
                any_progress = true;
            }
        }

        // Stage 2: install up to INSTALL_CHUNK watches across all repos
        // this tick. Round-robin across repos keeps a single huge repo
        // from starving smaller siblings.
        let mode = if RECURSIVE_ROOT_WATCH {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        let mut budget = INSTALL_CHUNK;
        while budget > 0 {
            let waiting = self.repos.values().filter(|r| !r.queue.is_empty()).count();
            if waiting == 0 {
                break;
            }
            let share = (budget / waiting).max(1);
            for repo in self.repos.values_mut() {
                if budget == 0 {
                    break;
                }
                let take = share.min(budget).min(repo.queue.len());
                if take == 0 {
                    continue;
                }
                let from = repo.queue.len() - take;
                for path in repo.queue.drain(from..) {
                    if watcher.watch(&path, mode).is_ok() {
                        repo.watched.insert(path);
                    }
                }
                budget -= take;
                any_progress = true;
            }
        }

        any_progress
    }

    /// Install non-recursive watches for directories that appeared inside an
    /// already-watched git repo after registration (e.g. a terminal/agent ran
    /// `mkdir`). The initial repo walk only covers directories that existed at
    /// registration time, and watches are non-recursive — so without this a new
    /// subtree emits no events at all.
    ///
    /// Telling directories from files and walking the new subtrees takes a
    /// `stat` per changed path and a walk per new directory — a `git clone` or
    /// an unpacked archive inside the repo is thousands of each — so it runs
    /// on a worker thread, and the result is installed by
    /// [`Self::poll_pending`] in chunks like the initial walk. Respects
    /// `.gitignore` (via [`collect_repo_watch_paths`]) and is additive: it
    /// extends the repo's watch set without touching its reference count.
    fn watch_new_repo_dirs(&mut self, changed: &[PathBuf]) {
        // A recursive root watch already covers directories created later.
        if RECURSIVE_ROOT_WATCH || self.repos.is_empty() {
            return;
        }

        let mut candidates: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        for path in changed {
            // The innermost watched repo owns the path.
            let Some((root, repo)) = self
                .repos
                .iter()
                .filter(|(root, _)| path.starts_with(root))
                .max_by_key(|(root, _)| root.as_os_str().len())
            else {
                continue;
            };
            // A full walk still running covers it; a watched path is known.
            if repo.full_walk_pending() || repo.watched.contains(path) {
                continue;
            }
            candidates
                .entry(root.clone())
                .or_default()
                .push(path.clone());
        }

        for (root, mut paths) in candidates {
            // A subtree's walk covers every path below it.
            paths.sort();
            let mut tops: Vec<PathBuf> = Vec::new();
            for path in paths {
                if tops.last().is_some_and(|top| path.starts_with(top)) {
                    continue;
                }
                tops.push(path);
            }
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let paths = tops
                    .iter()
                    .filter(|path| path.is_dir())
                    .flat_map(|path| collect_repo_watch_paths(path))
                    .collect();
                let _ = tx.send(RepoWalk {
                    paths,
                    ignore: None,
                });
            });
            if let Some(repo) = self.repos.get_mut(&root) {
                repo.walks.push((WalkKind::Extend, rx));
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
                    if !self.is_ignored_in_repo(&changed_path) {
                        self.pending_fs.insert(changed_path);
                    }
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

        // Emit git events that have waited out GIT_DEBOUNCE_MS
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

        // Emit gitignore events that have waited out the same debounce
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

    /// Whether `path` lies in an ignored subtree of a watched repo — what the
    /// per-directory walk achieves by never subscribing to such directories.
    fn is_ignored_in_repo(&self, path: &Path) -> bool {
        self.repos
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.as_os_str().len())
            .and_then(|(_, repo)| repo.ignore.as_ref())
            .is_some_and(|ignore| ignored_by(ignore, path))
    }

    /// Find the watched root that contains this path.
    fn find_watched_root(&self, path: &Path) -> PathBuf {
        // First check watched repos
        for repo_root in self.repos.keys() {
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

/// The repo's own ignore rules (`.gitignore` at the root and
/// `.git/info/exclude`), or `None` when it has neither.
fn repo_gitignore(repo_root: &Path) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(repo_root);
    let mut any = false;
    for file in [
        repo_root.join(".gitignore"),
        repo_root.join(".git/info/exclude"),
    ] {
        if file.is_file() {
            builder.add(&file);
            any = true;
        }
    }
    if !any {
        return None;
    }
    builder.build().ok()
}

/// Whether `path` (inside the matcher's root) or any of its parents is
/// ignored. Directory-only patterns such as `node_modules/` match through
/// the parent check even after the entry itself has been deleted.
fn ignored_by(ignore: &Gitignore, path: &Path) -> bool {
    ignore
        .matched_path_or_any_parents(path, path.is_dir())
        .is_ignore()
}

/// Start the full walk of `repo_root` on a worker thread.
fn spawn_full_walk(repo_root: &Path) -> Receiver<RepoWalk> {
    let (tx, rx) = channel();
    let repo = repo_root.to_path_buf();
    std::thread::spawn(move || {
        let paths = if RECURSIVE_ROOT_WATCH {
            vec![repo.clone()]
        } else {
            collect_repo_watch_paths(&repo)
        };
        let ignore = RECURSIVE_ROOT_WATCH
            .then(|| repo_gitignore(&repo))
            .flatten();
        let _ = tx.send(RepoWalk { paths, ignore });
    });
    rx
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

    /// Drive the async walks and chunked installs until `done` holds.
    fn drive_until(w: &mut UnifiedWatcher, what: &str, done: impl Fn(&UnifiedWatcher) -> bool) {
        for _ in 0..2000 {
            w.poll_pending();
            if done(w) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("timed out waiting until {what}");
    }

    fn wait_until_watched(w: &mut UnifiedWatcher, root: &Path) {
        drive_until(w, "the repo is fully watched", |w| {
            w.repos.get(root).is_some_and(RepoWatch::is_ready)
        });
    }

    fn temp_repo(gitignore: Option<&str>) -> (tempfile::TempDir, PathBuf) {
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
        fs::create_dir(root.join("src")).unwrap();
        if let Some(rules) = gitignore {
            fs::write(root.join(".gitignore"), rules).unwrap();
        }
        (tmp, root)
    }

    #[test]
    fn ignored_subtrees_are_filtered_by_the_repo_rules() {
        let (_tmp, root) = temp_repo(Some("node_modules/\n*.log\n"));
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        let ignore = repo_gitignore(&root).expect("rules present");
        assert!(ignored_by(&ignore, &root.join("node_modules/pkg/index.js")));
        // A deleted entry is checked through its parents.
        assert!(ignored_by(
            &ignore,
            &root.join("node_modules/gone/index.js")
        ));
        assert!(ignored_by(&ignore, &root.join("src/debug.log")));
        assert!(!ignored_by(&ignore, &root.join("src/main.rs")));

        let (_tmp, bare) = temp_repo(None);
        assert!(repo_gitignore(&bare).is_none());
    }

    /// Every acquire counts, also those made while the walk still runs:
    /// the repo stays watched until the last holder releases it.
    #[test]
    fn repo_holders_are_counted_from_the_first_acquire() {
        let (_tmp, root) = temp_repo(None);
        let mut w = UnifiedWatcher::new().unwrap();
        let target = WatchTarget::Repo(root.clone());
        w.watch(&target).unwrap();
        w.watch(&target).unwrap(); // the walk has not landed yet
        wait_until_watched(&mut w, &root);
        w.watch(&target).unwrap();
        assert_eq!(w.repos[&root].refs, 3);

        w.unwatch(&target);
        w.unwatch(&target);
        assert!(w.is_watching_repo(&root), "one holder is left");
        w.unwatch(&target);
        assert!(!w.is_watching_repo(&root));
        // A release past the last is a no-op, not an underflow.
        w.unwatch(&target);
        assert!(!w.is_watching_repo(&root));
    }

    /// Releasing a repo whose walk is still running drops it at once.
    #[test]
    fn releasing_a_pending_repo_drops_it() {
        let (_tmp, root) = temp_repo(None);
        let mut w = UnifiedWatcher::new().unwrap();
        w.watch_repository(root.clone()).unwrap();
        w.unwatch_repository(&root);
        assert!(!w.is_watching_repo(&root));
        // The abandoned walk landing later installs nothing.
        std::thread::sleep(std::time::Duration::from_millis(20));
        w.poll_pending();
        assert!(w.repos.is_empty());
    }

    /// A `.gitignore` change re-reads the rules without touching the holders.
    #[test]
    fn reloading_ignores_keeps_holders_and_applies_new_rules() {
        let (_tmp, root) = temp_repo(Some("node_modules/\n"));
        std::fs::create_dir_all(root.join("build/out")).unwrap();
        let mut w = UnifiedWatcher::new().unwrap();
        w.watch_repository(root.clone()).unwrap();
        w.watch_repository(root.clone()).unwrap();
        wait_until_watched(&mut w, &root);
        if RECURSIVE_ROOT_WATCH {
            assert!(!w.is_ignored_in_repo(&root.join("build/out/a.o")));
        } else {
            assert!(w.repos[&root].watched.contains(&root.join("build/out")));
        }

        std::fs::write(root.join(".gitignore"), "node_modules/\nbuild/\n").unwrap();
        w.reload_repo_ignores(&root);
        drive_until(&mut w, "the new rules apply", |w| {
            w.repos[&root].is_ready()
                && if RECURSIVE_ROOT_WATCH {
                    w.is_ignored_in_repo(&root.join("build/out/a.o"))
                } else {
                    !w.repos[&root].watched.contains(&root.join("build"))
                }
        });
        assert!(w.repos[&root].watched.contains(&root));
        assert_eq!(w.repos[&root].refs, 2, "holders are untouched");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn a_repo_is_one_recursive_watch_on_the_root() {
        // FSEvents: the root alone is registered, and directories created
        // later need no extra subscription.
        let (_tmp, root) = temp_repo(Some("node_modules/\n"));
        let mut w = UnifiedWatcher::new().unwrap();
        w.watch_repository(root.clone()).unwrap();
        wait_until_watched(&mut w, &root);

        let watched = &w.repos[&root].watched;
        assert_eq!(watched.iter().collect::<Vec<_>>(), vec![&root]);
        assert!(w.repos[&root].ignore.is_some());
        assert!(w.is_ignored_in_repo(&root.join("node_modules/x/y.js")));
        assert!(!w.is_ignored_in_repo(&root.join("src/y.js")));

        std::fs::create_dir_all(root.join("newdir/sub")).unwrap();
        w.watch_new_repo_dirs(&[root.join("newdir")]);
        assert!(w.repos[&root].walks.is_empty(), "no walk is needed");
        assert_eq!(w.repos[&root].watched.len(), 1);
        assert_eq!(w.repos[&root].refs, 1, "refcount must be preserved");
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn watches_directories_created_after_registration() {
        // Regression: the initial repo walk only covers directories that exist
        // at registration; watches are non-recursive. A subtree created later
        // (e.g. by a terminal/agent) must get its own watches so git status
        // keeps updating without navigating away and back.
        use std::fs;

        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
        fs::create_dir(root.join("src")).unwrap();

        let mut w = UnifiedWatcher::new().unwrap();
        w.watch_repository(root.clone()).unwrap();
        wait_until_watched(&mut w, &root);
        assert!(
            !w.repos[&root].watched.contains(&root.join("newdir")),
            "newdir does not exist yet, must not be watched"
        );

        // Create a nested subtree after registration and subscribe it. The
        // file below it is covered by the directory's walk; the plain file
        // is told apart on the worker thread and adds nothing.
        fs::create_dir_all(root.join("newdir/sub")).unwrap();
        fs::write(root.join("newdir/sub/a.txt"), b"a").unwrap();
        fs::write(root.join("plain.txt"), b"p").unwrap();
        w.watch_new_repo_dirs(&[
            root.join("newdir/sub/a.txt"),
            root.join("newdir"),
            root.join("plain.txt"),
        ]);
        assert_eq!(w.repos[&root].walks.len(), 1, "one walk per batch");
        drive_until(&mut w, "the new subtree is watched", |w| {
            w.repos[&root].walks.is_empty() && w.repos[&root].queue.is_empty()
        });

        let watched = &w.repos[&root].watched;
        assert!(
            watched.contains(&root.join("newdir")),
            "the created directory must be watched: {watched:?}"
        );
        assert!(
            watched.contains(&root.join("newdir/sub")),
            "nested created directories must be watched too: {watched:?}"
        );
        assert!(!watched.contains(&root.join("plain.txt")));
        // Reference count is untouched by the additive install.
        assert_eq!(w.repos[&root].refs, 1, "refcount must be preserved");
    }
}
