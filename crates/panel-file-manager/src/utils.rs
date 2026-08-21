use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use termide_git::truncate_right;
use termide_ui::constants::{GIGABYTE, KILOBYTE, MEGABYTE};

use super::FileEntry;

/// Get attribute character (selection checkmark or directory flags)
/// Returns 1 character
pub fn get_attribute(entry: &FileEntry, is_selected: bool) -> &'static str {
    if is_selected {
        return "+";
    }

    // For directories: show R if read-only
    if entry.is_dir && entry.is_readonly {
        return "R";
    }

    " "
}

/// Truncate file name to specified display width
pub fn truncate_name(name: &str, max_len: usize) -> String {
    truncate_right(name, max_len)
}

/// Format file size in human-readable format (compact, whole numbers only).
/// Used in file panel columns where space is limited.
pub fn format_size_compact(bytes: u64) -> String {
    let t = termide_i18n::t();
    if bytes >= GIGABYTE {
        format!(
            "{:.0} {}",
            bytes as f64 / GIGABYTE as f64,
            t.size_gigabytes()
        )
    } else if bytes >= MEGABYTE {
        format!(
            "{:.0} {}",
            bytes as f64 / MEGABYTE as f64,
            t.size_megabytes()
        )
    } else if bytes >= KILOBYTE {
        format!(
            "{:.0} {}",
            bytes as f64 / KILOBYTE as f64,
            t.size_kilobytes()
        )
    } else {
        format!("{} {}", bytes, t.size_bytes())
    }
}

/// Format file size in human-readable format (detailed).
/// B, KB — whole numbers; MB — one decimal; GB+ — two decimals.
/// Used in file info modal where precision matters.
pub fn format_size(bytes: u64) -> String {
    let t = termide_i18n::t();
    if bytes >= GIGABYTE {
        format!(
            "{:.2} {}",
            bytes as f64 / GIGABYTE as f64,
            t.size_gigabytes()
        )
    } else if bytes >= MEGABYTE {
        format!(
            "{:.1} {}",
            bytes as f64 / MEGABYTE as f64,
            t.size_megabytes()
        )
    } else if bytes >= KILOBYTE {
        format!(
            "{:.0} {}",
            bytes as f64 / KILOBYTE as f64,
            t.size_kilobytes()
        )
    } else {
        format!("{} {}", bytes, t.size_bytes())
    }
}

/// Result of a time-bounded directory size walk.
///
/// `overflowed == true` means the walk was cut short because `budget`
/// elapsed before the tree was fully traversed; in that case `size`
/// holds the partial total accumulated so far (never a final number).
#[derive(Debug, Clone, Copy)]
pub struct DirSizeOutcome {
    pub size: u64,
    pub overflowed: bool,
}

/// Outcome of trying to claim a size walk through the shared cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// Result is already in the cache — no work needed.
    AlreadyCached,
    /// Another panel is currently walking this path — wait for it.
    InProgress,
    /// Caller has exclusive ownership and must compute.
    Claimed,
}

/// Process-wide shared cache for directory sizes shown in FM wide view.
///
/// Multiple FM panels open on overlapping trees share results through
/// this cache. The `inflight` set ensures that only one panel walks any
/// given path at a time; other panels observing the path see `InProgress`
/// and wait for the completion to land in `entries`. A monotonic
/// `generation` counter ticks on every mutation so panels can cheaply
/// detect "something changed" and trigger a redraw.
///
/// Invalidation is soft: entries are **marked stale** rather than
/// removed, so the UI keeps showing the last-known number while the
/// recompute runs in the background. On completion the stale flag is
/// cleared and the new value takes over.
#[derive(Default)]
pub struct DirSizeCache {
    entries: Mutex<HashMap<PathBuf, DirSizeOutcome>>,
    stale: Mutex<HashSet<PathBuf>>,
    inflight: Mutex<HashSet<PathBuf>>,
    generation: AtomicU64,
}

impl DirSizeCache {
    /// Monotonic counter — increments on any mutation (insert/invalidate).
    /// Panels compare the last-seen value to decide when to redraw.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn get(&self, path: &Path) -> Option<DirSizeOutcome> {
        self.entries.lock().ok()?.get(path).copied()
    }

    /// True if `path` has been marked for recompute but no fresh value
    /// has landed yet. `get()` still returns the old value.
    pub fn is_stale(&self, path: &Path) -> bool {
        self.stale.lock().map(|s| s.contains(path)).unwrap_or(false)
    }

    /// Try to acquire exclusive ownership of a walk for `path`. A stale
    /// cache entry is still claimable — the walk will refresh it.
    pub fn claim(&self, path: &Path) -> ClaimOutcome {
        let cached = self
            .entries
            .lock()
            .ok()
            .map(|e| e.contains_key(path))
            .unwrap_or(false);
        if cached && !self.is_stale(path) {
            return ClaimOutcome::AlreadyCached;
        }
        let Ok(mut inflight) = self.inflight.lock() else {
            return ClaimOutcome::InProgress;
        };
        if inflight.contains(path) {
            ClaimOutcome::InProgress
        } else {
            inflight.insert(path.to_path_buf());
            ClaimOutcome::Claimed
        }
    }

    /// Insert a precomputed result directly, bypassing the claim/complete
    /// dance. Used when a full walk happens outside the wide-view
    /// scheduler — e.g. the file-info modal kicks off an unbounded walk
    /// on Space, and we want every panel that displays that directory
    /// in the wide view to pick the exact number up immediately.
    ///
    /// If another walker is currently in flight for the same path, we
    /// also clear their inflight slot — their `complete()` call will then
    /// no-op, leaving our exact result in place rather than overwriting
    /// it with a possibly-overflowed budgeted walk.
    pub fn insert(&self, path: PathBuf, outcome: DirSizeOutcome) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(path.clone(), outcome);
        }
        if let Ok(mut stale) = self.stale.lock() {
            stale.remove(&path);
        }
        if let Ok(mut inflight) = self.inflight.lock() {
            inflight.remove(&path);
        }
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Deposit a completed result. Clears the stale flag and replaces
    /// the old value. Silently dropped if the claim was revoked.
    pub fn complete(&self, path: PathBuf, outcome: DirSizeOutcome) {
        let Ok(mut inflight) = self.inflight.lock() else {
            return;
        };
        if !inflight.remove(&path) {
            return;
        }
        drop(inflight);
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(path.clone(), outcome);
        }
        if let Ok(mut stale) = self.stale.lock() {
            stale.remove(&path);
        }
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Mark every cached entry rooted at `root` as stale. The old value
    /// stays visible; the scheduler picks it up for recompute.
    /// Intended for explicit user reload (Ctrl+R).
    pub fn invalidate_subtree(&self, root: &Path) {
        let mut any_marked = false;
        if let Ok(entries) = self.entries.lock() {
            if let Ok(mut stale) = self.stale.lock() {
                for key in entries.keys() {
                    if key.starts_with(root) && stale.insert(key.clone()) {
                        any_marked = true;
                    }
                }
            }
        }
        if any_marked {
            self.generation.fetch_add(1, Ordering::Release);
        }
    }

    /// Mark cached entries whose path is an ancestor of `changed`
    /// (including the path itself) as stale. Intended for FS-watcher
    /// events: a file under `/a/b/c` mutating flags `/a`, `/a/b`,
    /// `/a/b/c` for recompute, but leaves sibling subtrees alone.
    pub fn invalidate_ancestors(&self, changed: &Path) {
        self.invalidate_ancestors_of_all(std::slice::from_ref(&changed));
    }

    /// Invalidate every cached directory that contains any of `changed`.
    ///
    /// The batch form exists because a single filesystem burst — deleting a
    /// large tree — arrives as tens of thousands of paths that all resolve to
    /// the same few cached ancestors. Per-path invalidation locked both maps
    /// once per path and rescanned every key: 176ms of main-thread stall for a
    /// 35k-path burst against a 40-entry cache. Here the locks are taken once
    /// and each key stops at its first matching path.
    pub fn invalidate_ancestors_of_all(&self, changed: &[&Path]) {
        if changed.is_empty() {
            return;
        }
        let mut any_marked = false;
        if let Ok(entries) = self.entries.lock() {
            if entries.is_empty() {
                return;
            }
            if let Ok(mut stale) = self.stale.lock() {
                for key in entries.keys() {
                    if changed.iter().any(|path| path.starts_with(key)) && stale.insert(key.clone())
                    {
                        any_marked = true;
                    }
                }
            }
        }
        if any_marked {
            self.generation.fetch_add(1, Ordering::Release);
        }
    }
}

/// Accessor for the process-wide shared directory-size cache.
pub fn shared_dir_size_cache() -> &'static DirSizeCache {
    static CACHE: OnceLock<DirSizeCache> = OnceLock::new();
    CACHE.get_or_init(DirSizeCache::default)
}

/// Iteratively walk `path` and sum file sizes, stopping at `budget`.
///
/// Mirrors [`calculate_dir_size`] (same symlink policy — `entry.metadata()`
/// follows symlinks, breadth-first traversal with a queue) but returns
/// an overflow flag so callers can render a marker instead of a stale
/// partial number when the walk didn't finish in time.
pub fn calculate_dir_size_bounded(path: &Path, budget: Duration) -> DirSizeOutcome {
    use std::collections::VecDeque;

    let start = Instant::now();
    let mut total: u64 = 0;
    let mut queue: VecDeque<std::path::PathBuf> = VecDeque::new();
    queue.push_back(path.to_path_buf());

    while let Some(dir) = queue.pop_front() {
        if start.elapsed() >= budget {
            return DirSizeOutcome {
                size: total,
                overflowed: true,
            };
        }
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if start.elapsed() >= budget {
                return DirSizeOutcome {
                    size: total,
                    overflowed: true,
                };
            }
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    total = total.saturating_add(meta.len());
                } else if meta.is_dir() {
                    queue.push_back(entry.path());
                }
            }
        }
    }

    DirSizeOutcome {
        size: total,
        overflowed: false,
    }
}

/// Iteratively calculate directory size (without recursion, protected from stack overflow)
pub fn calculate_dir_size(path: &Path) -> u64 {
    use std::collections::VecDeque;

    let mut total_size = 0u64;
    let mut dirs_to_process = VecDeque::new();
    dirs_to_process.push_back(path.to_path_buf());

    // Iterative traversal with explicit stack
    while let Some(current_dir) = dirs_to_process.pop_front() {
        if let Ok(entries) = fs::read_dir(&current_dir) {
            for entry in entries.flatten() {
                // Use symlink_metadata to not follow symlinks
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_file() {
                        total_size += metadata.len();
                    } else if metadata.is_dir() {
                        // Add directory to queue for processing
                        dirs_to_process.push_back(entry.path());
                    }
                    // Ignore symlinks to avoid cycles
                }
            }
        }
    }

    total_size
}

/// Get user name by UID
/// Returns symbolic name if available, otherwise numeric ID
#[cfg(unix)]
pub fn get_user_name(uid: u32) -> String {
    // SAFETY: getpwuid is a POSIX function that returns a pointer to a static
    // passwd struct or NULL. We check for NULL before dereferencing. The returned
    // pointer is valid until the next call to getpwuid/getpwnam, but we immediately
    // copy the string data so this is safe. The pw_name field is a null-terminated
    // C string that we convert safely via CStr::from_ptr after NULL check.
    unsafe {
        let pwd = libc::getpwuid(uid);
        if !pwd.is_null() {
            let name_ptr = (*pwd).pw_name;
            if !name_ptr.is_null() {
                if let Ok(name) = std::ffi::CStr::from_ptr(name_ptr).to_str() {
                    return name.to_string();
                }
            }
        }
    }
    uid.to_string()
}

/// Get group name by GID
/// Returns symbolic name if available, otherwise numeric ID
#[cfg(unix)]
pub fn get_group_name(gid: u32) -> String {
    // SAFETY: getgrgid is a POSIX function that returns a pointer to a static
    // group struct or NULL. We check for NULL before dereferencing. The returned
    // pointer is valid until the next call to getgrgid/getgrnam, but we immediately
    // copy the string data so this is safe. The gr_name field is a null-terminated
    // C string that we convert safely via CStr::from_ptr after NULL check.
    unsafe {
        let grp = libc::getgrgid(gid);
        if !grp.is_null() {
            let name_ptr = (*grp).gr_name;
            if !name_ptr.is_null() {
                if let Ok(name) = std::ffi::CStr::from_ptr(name_ptr).to_str() {
                    return name.to_string();
                }
            }
        }
    }
    gid.to_string()
}

/// Format modification time in YYYY-MM-DD HH:MM:SS format
/// Returns 19 characters (time string or spaces)
pub fn format_modified_time(time: Option<SystemTime>) -> String {
    time.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .and_then(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0))
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "                   ".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(path: &Path, bytes: usize) {
        let mut f = fs::File::create(path).expect("create file");
        f.write_all(&vec![0u8; bytes]).expect("write file");
    }

    #[test]
    fn bounded_completes_small_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Flat tree with predictable total: 3 * 100 = 300 bytes.
        for i in 0..3 {
            write_file(&tmp.path().join(format!("f{i}.bin")), 100);
        }
        let sub = tmp.path().join("nested");
        fs::create_dir(&sub).unwrap();
        write_file(&sub.join("x.bin"), 50);

        let outcome = calculate_dir_size_bounded(tmp.path(), Duration::from_secs(60));
        assert!(!outcome.overflowed, "small tree must finish well under 60s");
        assert_eq!(outcome.size, 350);
    }

    #[test]
    fn shared_cache_claim_deduplicates_walks() {
        // Using a unique path so the global cache doesn't collide with
        // other tests in this process.
        let path = PathBuf::from("/__dir_size_cache_test__/dedup_A");

        let cache = DirSizeCache::default();
        assert_eq!(cache.claim(&path), ClaimOutcome::Claimed);
        // Second claim while first is in-flight must not race.
        assert_eq!(cache.claim(&path), ClaimOutcome::InProgress);

        cache.complete(
            path.clone(),
            DirSizeOutcome {
                size: 42,
                overflowed: false,
            },
        );
        // Now the value is cached and further claims short-circuit.
        assert_eq!(cache.claim(&path), ClaimOutcome::AlreadyCached);
        assert_eq!(cache.get(&path).map(|o| o.size), Some(42));
    }

    #[test]
    fn shared_cache_invalidate_marks_stale_keeping_old_value() {
        let root = PathBuf::from("/__dir_size_cache_test__/invalidate");
        let child = root.join("child");

        let cache = DirSizeCache::default();
        let old = DirSizeOutcome {
            size: 100,
            overflowed: false,
        };
        assert_eq!(cache.claim(&child), ClaimOutcome::Claimed);
        cache.complete(child.clone(), old);

        // Soft invalidation: the old number is still visible to the UI
        // while a fresh walk is queued in the background.
        cache.invalidate_subtree(&root);
        assert_eq!(
            cache.get(&child).map(|o| o.size),
            Some(100),
            "stale entry must still render the last known value"
        );
        assert!(cache.is_stale(&child));
        // And the entry is immediately claimable again for recompute.
        assert_eq!(cache.claim(&child), ClaimOutcome::Claimed);

        // On completion, new value replaces old and stale is cleared.
        let new = DirSizeOutcome {
            size: 250,
            overflowed: false,
        };
        cache.complete(child.clone(), new);
        assert_eq!(cache.get(&child).map(|o| o.size), Some(250));
        assert!(!cache.is_stale(&child));
    }

    #[test]
    fn shared_cache_invalidate_ancestors_targets_parents_only() {
        let parent = PathBuf::from("/__dir_size_cache_test__/ancestors");
        let child_file = parent.join("sub/file.txt");
        let sibling = PathBuf::from("/__dir_size_cache_test__/other");

        let cache = DirSizeCache::default();
        let outcome = DirSizeOutcome {
            size: 1,
            overflowed: false,
        };
        cache.claim(&parent);
        cache.complete(parent.clone(), outcome);
        cache.claim(&sibling);
        cache.complete(sibling.clone(), outcome);

        cache.invalidate_ancestors(&child_file);

        assert!(
            cache.is_stale(&parent),
            "ancestor of changed path must be marked stale"
        );
        assert!(
            !cache.is_stale(&sibling),
            "unrelated entry must not be marked stale"
        );
        // Old values stay visible while recompute is scheduled.
        assert!(cache.get(&parent).is_some());
        assert!(cache.get(&sibling).is_some());
    }

    #[test]
    fn shared_cache_generation_ticks_on_insert_and_invalidate() {
        let path = PathBuf::from("/__dir_size_cache_test__/generation");
        let cache = DirSizeCache::default();
        let g0 = cache.generation();

        assert_eq!(cache.claim(&path), ClaimOutcome::Claimed);
        // claim alone does not bump generation (no observable change).
        assert_eq!(cache.generation(), g0);

        cache.complete(
            path.clone(),
            DirSizeOutcome {
                size: 1,
                overflowed: false,
            },
        );
        let g1 = cache.generation();
        assert!(g1 > g0, "complete must bump generation");

        cache.invalidate_subtree(&path);
        assert!(cache.generation() > g1, "invalidate must bump generation");
    }

    #[test]
    fn bounded_stops_when_budget_exhausted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Enough entries that a zero-duration budget trips the deadline
        // on the very first loop iteration. We don't care exactly how much
        // was accumulated, only that overflowed is reported.
        for i in 0..32 {
            write_file(&tmp.path().join(format!("f{i}.bin")), 1024);
        }

        let outcome = calculate_dir_size_bounded(tmp.path(), Duration::from_nanos(0));
        assert!(outcome.overflowed, "zero-budget walk must overflow");
        // Partial total must never exceed reality.
        assert!(outcome.size <= 32 * 1024);
    }

    /// The batch form must mark exactly the ancestors the per-path form marks —
    /// it exists only to take the locks once for a whole filesystem burst.
    #[test]
    fn batch_invalidation_marks_the_same_ancestors() {
        let cached = DirSizeCache::default();
        for dir in ["/p/a", "/p/b", "/other"] {
            cached.insert(
                PathBuf::from(dir),
                DirSizeOutcome {
                    size: 1,
                    overflowed: false,
                },
            );
        }

        let changed = [
            Path::new("/p/a/deep/f.txt"),
            Path::new("/p/b/g.txt"),
            Path::new("/unrelated/h.txt"),
        ];
        cached.invalidate_ancestors_of_all(&changed);

        assert!(cached.is_stale(Path::new("/p/a")));
        assert!(cached.is_stale(Path::new("/p/b")));
        assert!(
            !cached.is_stale(Path::new("/other")),
            "an untouched directory must stay fresh"
        );
    }

    /// An empty batch must not bump the generation counter — a burst that
    /// touches nothing cached should not make panels redraw.
    #[test]
    fn batch_invalidation_of_nothing_is_a_no_op() {
        let cached = DirSizeCache::default();
        cached.insert(
            PathBuf::from("/p/a"),
            DirSizeOutcome {
                size: 1,
                overflowed: false,
            },
        );
        let before = cached.generation();

        cached.invalidate_ancestors_of_all(&[]);
        cached.invalidate_ancestors_of_all(&[Path::new("/elsewhere/f.txt")]);

        assert_eq!(cached.generation(), before);
        assert!(!cached.is_stale(Path::new("/p/a")));
    }
}
