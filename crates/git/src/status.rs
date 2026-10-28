//! Working-tree status: porcelain parsing, the per-directory status cache,
//! and repository-level ahead/behind/uncommitted summaries.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use crate::command::{
    git_command_stdout, git_command_stdout_bytes, nul_fields, path_from_git_bytes,
};
use crate::is_available;

/// How `git status` reports ignored paths.
///
/// `matching` lists what an ignore pattern matches and stops there, so a
/// matched directory such as `target/` is one line and is never walked. The
/// default (`traditional`) walks every ignored directory to decide whether it
/// is ignored as a whole, which costs seconds of CPU on a large build tree.
/// The price: a directory that no pattern matches but holds only ignored
/// files is reported file by file instead of as `dir/`.
const IGNORED_MODE: &str = "--ignored=matching";

/// Git file status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GitStatus {
    #[default]
    Unmodified,
    Modified,
    Added,
    Deleted,
    Ignored,
}

/// Parse git status porcelain code to GitStatus enum.
fn parse_status_code(code: &str) -> GitStatus {
    match code {
        "!!" => GitStatus::Ignored,
        " M" | "M " | "MM" => GitStatus::Modified,
        "A " | " A" | "AM" | "AA" => GitStatus::Added,
        " D" | "D " | "DD" => GitStatus::Deleted,
        "??" => GitStatus::Added,
        _ => GitStatus::Unmodified,
    }
}

/// One entry of `git status --porcelain=v1 -z`.
#[derive(Debug, PartialEq)]
struct PorcelainEntry {
    code: [u8; 2],
    path: PathBuf,
    /// The source of a rename or copy.
    from: Option<PathBuf>,
}

/// Parse `git status --porcelain=v1 -z`: `XY path`, NUL-terminated and
/// unquoted; a rename or copy is followed by its source as one more field.
fn parse_porcelain_z(stdout: &[u8]) -> Vec<PorcelainEntry> {
    let mut fields = nul_fields(stdout);
    let mut entries = Vec::new();
    while let Some(field) = fields.next() {
        let (Some(code), Some(path)) = (field.get(..2), field.get(3..)) else {
            continue;
        };
        let code = [code[0], code[1]];
        let from = if code.iter().any(|c| matches!(c, b'R' | b'C')) {
            fields.next().map(path_from_git_bytes)
        } else {
            None
        };
        entries.push(PorcelainEntry {
            code,
            path: path_from_git_bytes(path),
            from,
        });
    }
    entries
}

/// Insert under the real path and, when that is not valid UTF-8, also
/// under its lossy rendering: lookups by `&str` (the file manager's display
/// names) find the lossy key, lookups by `&Path` the real one.
fn insert_path<V: Copy>(map: &mut HashMap<PathBuf, V>, path: &Path, value: V) {
    if path.to_str().is_none() {
        map.insert(PathBuf::from(path.to_string_lossy().into_owned()), value);
    }
    map.insert(path.to_path_buf(), value);
}

fn insert_path_set(set: &mut HashSet<PathBuf>, path: &Path) {
    if path.to_str().is_none() {
        set.insert(PathBuf::from(path.to_string_lossy().into_owned()));
    }
    set.insert(path.to_path_buf());
}

/// Get git status for directory (synchronous version for compatibility).
pub fn get_git_status(dir: &Path) -> Option<GitStatusCache> {
    if !is_available() {
        return None;
    }

    // Single git command to check repo and get root path
    let repo_root_str = git_command_stdout(dir, &["rev-parse", "--show-toplevel"])?;
    let repo_root = PathBuf::from(repo_root_str.trim());

    // Canonicalize both paths to resolve symlinks before strip_prefix
    // This fixes the issue where FileManager uses symlink paths (e.g., /home/nvn/...)
    // but git rev-parse returns real paths (e.g., /Data/...)
    let dir_canonical = dunce::canonicalize(dir).ok();
    let repo_root_canonical = dunce::canonicalize(&repo_root).ok();

    let relative_path = match (&dir_canonical, &repo_root_canonical) {
        (Some(d), Some(r)) => d
            .strip_prefix(r)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| {
                log::warn!(
                    "strip_prefix failed after canonicalize: dir={:?}, repo_root={:?}",
                    d,
                    r
                );
                PathBuf::new()
            }),
        _ => {
            // Fallback to direct strip_prefix if canonicalize fails
            log::warn!(
                "canonicalize failed, trying direct strip_prefix: dir={:?}, repo_root={:?}",
                dir,
                repo_root
            );
            dir.strip_prefix(&repo_root)
                .map(|p| p.to_path_buf())
                .unwrap_or_default()
        }
    };

    // Single git status command - parse both status and ignored files.
    // `-z` gives raw, unquoted names, so one name that is not UTF-8 (or that
    // holds a quote or a tab) cannot drop or garble the whole status.
    let mut status_map = HashMap::new();
    let mut ignored_files = HashSet::new();

    if let Some(stdout) = git_command_stdout_bytes(
        &repo_root,
        &["status", "--porcelain=v1", "-z", IGNORED_MODE],
    ) {
        for entry in parse_porcelain_z(&stdout) {
            let status = if &entry.code == b"!!" {
                // Also add to ignored_files for parent directory checks
                insert_path_set(&mut ignored_files, &entry.path);
                GitStatus::Ignored
            } else if let Some(from) = &entry.from {
                // A rename shows its destination as new and its source as
                // gone, as an undetected rename (`D` + `??`) would; a copy
                // leaves its source alone.
                if entry.code.contains(&b'R') {
                    insert_path(&mut status_map, from, GitStatus::Deleted);
                }
                GitStatus::Added
            } else {
                match std::str::from_utf8(&entry.code)
                    .map_or(GitStatus::Unmodified, parse_status_code)
                {
                    GitStatus::Unmodified => continue,
                    s => s,
                }
            };

            insert_path(&mut status_map, &entry.path, status);
        }
    }

    // Pre-compute directories with changes for O(1) lookup
    let dirs_with_changes: HashSet<PathBuf> = status_map
        .iter()
        .filter(|(_, status)| **status != GitStatus::Unmodified && **status != GitStatus::Ignored)
        .flat_map(|(path, _)| {
            // Collect all ancestor directories of this path
            let mut ancestors = Vec::new();
            let mut current = path.as_path();
            while let Some(parent) = current.parent() {
                if parent.as_os_str().is_empty() {
                    break;
                }
                ancestors.push(parent.to_path_buf());
                current = parent;
            }
            ancestors
        })
        .collect();

    Some(GitStatusCache {
        status_map,
        ignored_files,
        relative_path,
        dirs_with_changes,
    })
}

/// Result type for async git status loading.
pub struct GitStatusAsyncResult {
    /// Directory path this result is for
    pub dir: PathBuf,
    /// Git status cache (None if not a git repo or error)
    pub cache: Option<GitStatusCache>,
}

/// Load git status asynchronously in a background thread.
/// Returns a receiver that will receive the result when complete.
pub fn get_git_status_async(dir: PathBuf) -> mpsc::Receiver<GitStatusAsyncResult> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let cache = get_git_status(&dir);
        let _ = tx.send(GitStatusAsyncResult { dir, cache });
    });
    rx
}

/// Git status cache for directory.
#[derive(Debug, Clone)]
pub struct GitStatusCache {
    status_map: HashMap<PathBuf, GitStatus>,
    ignored_files: HashSet<PathBuf>,
    relative_path: PathBuf,
    /// Pre-computed set of directories that contain changes (for O(1) lookup)
    dirs_with_changes: HashSet<PathBuf>,
}

impl GitStatusCache {
    fn is_parent_ignored(&self, path: &Path) -> bool {
        let mut current = path;
        while let Some(parent) = current.parent() {
            if self.ignored_files.contains(parent) {
                return true;
            }
            if let Some(&GitStatus::Ignored) = self.status_map.get(parent) {
                return true;
            }
            current = parent;
            if parent.as_os_str().is_empty() {
                break;
            }
        }
        false
    }

    fn is_parent_untracked(&self, path: &Path) -> bool {
        let mut current = path;
        while let Some(parent) = current.parent() {
            if let Some(&GitStatus::Added) = self.status_map.get(parent) {
                return true;
            }
            current = parent;
            if parent.as_os_str().is_empty() {
                break;
            }
        }
        false
    }

    pub fn get_status(&self, file_name: &str) -> GitStatus {
        let full_path = if self.relative_path.as_os_str().is_empty() {
            PathBuf::from(file_name)
        } else {
            self.relative_path.join(file_name)
        };

        if self.ignored_files.contains(&full_path) {
            return GitStatus::Ignored;
        }

        if let Some(&status) = self.status_map.get(&full_path) {
            return status;
        }

        if self.is_parent_ignored(&full_path) {
            return GitStatus::Ignored;
        }

        if self.is_parent_untracked(&full_path) {
            return GitStatus::Added;
        }

        GitStatus::Unmodified
    }

    pub fn has_changes_in_directory(&self, dir_name: &str) -> bool {
        let full_dir = if self.relative_path.as_os_str().is_empty() {
            PathBuf::from(dir_name)
        } else {
            self.relative_path.join(dir_name)
        };

        // O(1) lookup instead of O(n) iteration
        self.dirs_with_changes.contains(&full_dir)
    }

    pub fn get_directory_status(&self, dir_name: &str) -> GitStatus {
        let full_path = if self.relative_path.as_os_str().is_empty() {
            PathBuf::from(dir_name)
        } else {
            self.relative_path.join(dir_name)
        };

        if let Some(&status) = self.status_map.get(&full_path) {
            if status == GitStatus::Deleted {
                // A directory may have replaced the deleted file/symlink —
                // git reports "D  name" + "?? name/" with trailing slash.
                let dir_path = PathBuf::from(format!("{}/", full_path.display()));
                if let Some(&dir_status) = self.status_map.get(&dir_path) {
                    return dir_status;
                }
            }
            if status != GitStatus::Unmodified {
                return status;
            }
        }

        if self.is_parent_ignored(&full_path) {
            return GitStatus::Ignored;
        }

        if self.is_parent_untracked(&full_path) {
            return GitStatus::Added;
        }

        if self.has_changes_in_directory(dir_name) {
            return GitStatus::Modified;
        }

        GitStatus::Unmodified
    }

    pub fn get_deleted_files(&self) -> Vec<String> {
        let mut result = Vec::new();
        for (path, status) in &self.status_map {
            if *status != GitStatus::Deleted {
                continue;
            }

            let parent = path.parent();
            let parent_match = match parent {
                Some(p) if p.as_os_str().is_empty() => self.relative_path.as_os_str().is_empty(),
                Some(p) => p == self.relative_path,
                None => self.relative_path.as_os_str().is_empty(),
            };

            if parent_match {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    result.push(name.to_string());
                }
            }
        }

        result
    }

    /// Check if path (relative to repo root) is ignored or inside an ignored directory.
    ///
    /// Uses path component comparison to avoid string allocations in hot path.
    pub fn is_path_in_ignored(&self, relative_path: &Path) -> bool {
        // Check if exact path is ignored
        if self.ignored_files.contains(relative_path) {
            return true;
        }

        // Check if any ancestor is ignored (path is inside ignored directory)
        let mut ancestor = relative_path;
        while let Some(parent) = ancestor.parent() {
            if !parent.as_os_str().is_empty() && self.ignored_files.contains(parent) {
                return true;
            }
            ancestor = parent;
        }

        false
    }
}

/// Git repository status information.
#[derive(Debug, Clone, Copy)]
pub struct GitRepoStatus {
    pub uncommitted_changes: usize,
    pub ahead: usize,
    pub behind: usize,
    pub is_ignored: bool,
}

/// Get git repository status for a specific file or directory.
/// Optimized to use minimal git process spawns (2 instead of 6).
pub fn get_repo_status(repo_path: &Path, item_path: &Path) -> Option<GitRepoStatus> {
    if !is_available() {
        return None;
    }

    let git_work_dir = if item_path.is_file() {
        item_path.parent().unwrap_or(repo_path)
    } else {
        item_path
    };

    // Single call to get repo root (also validates we're in a git repo)
    let repo_root_str = git_command_stdout(git_work_dir, &["rev-parse", "--show-toplevel"])?;
    let repo_root = PathBuf::from(repo_root_str.trim());

    let relative_path = item_path.strip_prefix(&repo_root).ok()?;
    let is_repo_root = relative_path.as_os_str().is_empty();
    let git_path_str = if is_repo_root {
        ".".to_string()
    } else {
        relative_path.to_string_lossy().into_owned()
    };

    // Single git status call with branch info and ignored files
    // Output format:
    //   ## branch...origin/branch [ahead N, behind M]
    //   !! ignored/file
    //    M modified/file
    // Only lines are counted here, so a name that is not UTF-8 is decoded
    // lossily rather than discarding the whole output.
    let status_output = git_command_stdout_bytes(
        &repo_root,
        &[
            "-c",
            "core.quotepath=false",
            "status",
            "--porcelain=v1",
            "-b",
            IGNORED_MODE,
            "--",
            &git_path_str,
        ],
    )
    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    .unwrap_or_default();

    let (ahead, behind, uncommitted_changes, is_ignored) =
        parse_git_status_output(&status_output, is_repo_root);

    Some(GitRepoStatus {
        uncommitted_changes,
        ahead,
        behind,
        is_ignored,
    })
}

/// Parse git status --porcelain=v1 -b --ignored=matching output.
/// Returns (ahead, behind, uncommitted_changes, is_ignored).
fn parse_git_status_output(output: &str, is_repo_root: bool) -> (usize, usize, usize, bool) {
    let mut ahead = 0;
    let mut behind = 0;
    let mut uncommitted_changes = 0;
    let mut is_ignored = false;

    for line in output.lines() {
        if line.starts_with("## ") {
            // Parse branch line: "## main...origin/main [ahead 2, behind 1]"
            if let Some(bracket_start) = line.find('[') {
                let tracking_info = &line[bracket_start..];
                // Parse ahead count
                if let Some(ahead_pos) = tracking_info.find("ahead ") {
                    let start = ahead_pos + 6;
                    let end = tracking_info[start..]
                        .find(|c: char| !c.is_ascii_digit())
                        .map(|i| start + i)
                        .unwrap_or(tracking_info.len());
                    ahead = tracking_info[start..end].parse().unwrap_or(0);
                }
                // Parse behind count
                if let Some(behind_pos) = tracking_info.find("behind ") {
                    let start = behind_pos + 7;
                    let end = tracking_info[start..]
                        .find(|c: char| !c.is_ascii_digit())
                        .map(|i| start + i)
                        .unwrap_or(tracking_info.len());
                    behind = tracking_info[start..end].parse().unwrap_or(0);
                }
            }
        } else if line.starts_with("!! ") {
            // Ignored file - only count if not repo root
            if !is_repo_root {
                is_ignored = true;
            }
        } else if line.len() >= 2 && !line.starts_with("## ") {
            // Any other status line is an uncommitted change
            uncommitted_changes += 1;
        }
    }

    (ahead, behind, uncommitted_changes, is_ignored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_git_status_branch_with_tracking() {
        let output = "## main...origin/main [ahead 2, behind 3]\n M file.rs\n";
        let (ahead, behind, changes, ignored) = parse_git_status_output(output, false);
        assert_eq!(ahead, 2);
        assert_eq!(behind, 3);
        assert_eq!(changes, 1);
        assert!(!ignored);
    }

    #[test]
    fn test_parse_git_status_ahead_only() {
        let output = "## feature...origin/feature [ahead 5]\n";
        let (ahead, behind, changes, _) = parse_git_status_output(output, false);
        assert_eq!(ahead, 5);
        assert_eq!(behind, 0);
        assert_eq!(changes, 0);
    }

    #[test]
    fn test_parse_git_status_behind_only() {
        let output = "## main...origin/main [behind 1]\n";
        let (ahead, behind, _, _) = parse_git_status_output(output, false);
        assert_eq!(ahead, 0);
        assert_eq!(behind, 1);
    }

    #[test]
    fn test_parse_git_status_ignored_files() {
        let output = "## main\n!! ignored.txt\n M changed.rs\n";
        let (_, _, changes, ignored) = parse_git_status_output(output, false);
        assert!(ignored);
        assert_eq!(changes, 1); // Only the M line, not the !! line
    }

    #[test]
    fn test_parse_git_status_repo_root_not_ignored() {
        let output = "## main\n!! some_ignored\n";
        let (_, _, _, ignored) = parse_git_status_output(output, true);
        assert!(!ignored); // Repo root cannot be ignored
    }

    #[test]
    fn test_parse_git_status_no_tracking() {
        let output = "## main\n M file.rs\n?? new.txt\n";
        let (ahead, behind, changes, _) = parse_git_status_output(output, false);
        assert_eq!(ahead, 0);
        assert_eq!(behind, 0);
        assert_eq!(changes, 2); // M and ?? lines
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    fn touch(root: &Path, relative: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    #[test]
    fn ignored_directories_are_reported_without_walking_them() {
        let dir = tempfile::tempdir().unwrap();
        // Canonical, as `git rev-parse --show-toplevel` reports it: the macOS
        // temp directory sits behind the `/var` -> `/private/var` symlink.
        let root = &dunce::canonicalize(dir.path()).unwrap();
        git(root, &["init", "-q"]);
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
        git(root, &["add", ".gitignore"]);
        git(root, &["commit", "-q", "-m", "init"]);
        touch(root, "target/debug/deep/artifact");
        touch(root, "logs/run.log");

        let cache = get_git_status(root).unwrap();

        // The matched directory is a single entry: its contents are never
        // listed, which is what keeps a large build tree from being walked.
        assert_eq!(cache.get_directory_status("target"), GitStatus::Ignored);
        assert!(cache
            .ignored_files
            .iter()
            .all(|path| !path.starts_with("target/debug")));
        let repo = get_repo_status(root, &root.join("target/debug")).unwrap();
        assert!(repo.is_ignored);

        // A directory no pattern matches is reported file by file.
        assert!(cache.ignored_files.contains(Path::new("logs/run.log")));
        assert!(!cache.ignored_files.contains(Path::new("logs")));
    }

    #[test]
    fn porcelain_z_parses_renames_and_unquoted_names() {
        let out = b"R  new.rs\0old.rs\0 M a \"b\".rs\0?? dir/\0C  copy\0orig\0!! target/\0";
        let entries = parse_porcelain_z(out);
        let simple = |code: &[u8; 2], path: &str| PorcelainEntry {
            code: *code,
            path: PathBuf::from(path),
            from: None,
        };
        assert_eq!(
            entries,
            [
                PorcelainEntry {
                    code: *b"R ",
                    path: PathBuf::from("new.rs"),
                    from: Some(PathBuf::from("old.rs")),
                },
                simple(b" M", "a \"b\".rs"),
                simple(b"??", "dir/"),
                PorcelainEntry {
                    code: *b"C ",
                    path: PathBuf::from("copy"),
                    from: Some(PathBuf::from("orig")),
                },
                simple(b"!!", "target/"),
            ]
        );
    }

    #[test]
    fn staged_renames_and_non_utf8_names_keep_status_working() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dunce::canonicalize(dir.path()).unwrap();
        git(root, &["init", "-q"]);
        touch(root, "old.rs");
        touch(root, "keep.rs");
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "init"]);
        git(root, &["mv", "old.rs", "new.rs"]);
        std::fs::write(root.join("keep.rs"), "changed").unwrap();
        // APFS refuses names that are not UTF-8; where the filesystem takes
        // them, one must not switch status off for the whole repository.
        #[cfg(unix)]
        let odd = {
            use std::os::unix::ffi::OsStrExt;
            let name = std::ffi::OsStr::from_bytes(b"odd\xff");
            std::fs::write(root.join(name), "").is_ok()
        };

        let cache = get_git_status(root).unwrap();

        assert_eq!(cache.get_status("new.rs"), GitStatus::Added);
        assert_eq!(cache.get_status("keep.rs"), GitStatus::Modified);
        assert_eq!(cache.get_deleted_files(), ["old.rs"]);
        #[cfg(unix)]
        if odd {
            assert_eq!(cache.get_status("odd\u{FFFD}"), GitStatus::Added);
        }
    }
}
