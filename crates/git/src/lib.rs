//! Git integration for termide.
//!
//! Provides git status, diff information, and repository utilities.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub mod diff;
pub mod watcher;

pub use diff::{load_original_async, GitDiffAsyncResult, GitDiffCache, LineStatus};
pub use watcher::{create_git_watcher, GitStatusUpdate, GitWatcher};

/// Get git status for a specific file relative to repo root.
pub fn file_status(repo_root: &Path, file_path: &Path) -> GitStatus {
    let relative = match file_path.strip_prefix(repo_root) {
        Ok(rel) => rel,
        Err(_) => return GitStatus::default(),
    };

    // Check if file is ignored
    if let Ok(output) = Command::new("git")
        .args(["check-ignore", "-q"])
        .arg(relative)
        .current_dir(repo_root)
        .output()
    {
        if output.status.success() {
            return GitStatus::Ignored;
        }
    }

    // Get status
    if let Ok(output) = Command::new("git")
        .args(["status", "--porcelain=v1", "--"])
        .arg(relative)
        .current_dir(repo_root)
        .output()
    {
        if output.status.success() {
            if let Ok(stdout) = String::from_utf8(output.stdout) {
                if let Some(line) = stdout.lines().next() {
                    if line.len() >= 2 {
                        let status_code = &line[0..2];
                        return match status_code {
                            "!!" => GitStatus::Ignored,
                            " M" | "M " | "MM" => GitStatus::Modified,
                            "A " | " A" | "AM" | "AA" => GitStatus::Added,
                            " D" | "D " | "DD" => GitStatus::Deleted,
                            "??" => GitStatus::Added,
                            _ => GitStatus::Unmodified,
                        };
                    }
                }
            }
        }
    }

    GitStatus::Unmodified
}

/// Global flag for git availability on system.
static GIT_AVAILABLE: OnceLock<bool> = OnceLock::new();

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

/// Check if git is available on system.
pub fn is_available() -> bool {
    *GIT_AVAILABLE.get_or_init(|| {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    })
}

/// Alias for backward compatibility.
#[inline]
pub fn check_git_available() -> bool {
    is_available()
}

/// Find git repository root by walking up from a path.
pub fn find_repo_root(path: &Path) -> Option<PathBuf> {
    let mut current = path;
    loop {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

/// Get git status for directory.
pub fn get_git_status(dir: &Path) -> Option<GitStatusCache> {
    if !is_available() {
        return None;
    }

    let is_repo = Command::new("git")
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .current_dir(dir)
        .output()
        .ok()
        .map(|output| output.status.success())
        .unwrap_or(false);

    if !is_repo {
        return None;
    }

    let repo_root = Command::new("git")
        .arg("rev-parse")
        .arg("--show-toplevel")
        .current_dir(dir)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|s| PathBuf::from(s.trim()))
            } else {
                None
            }
        })
        .unwrap_or_else(|| dir.to_path_buf());

    let relative_path = dir
        .strip_prefix(&repo_root)
        .unwrap_or(Path::new(""))
        .to_path_buf();

    let ignored = get_ignored_files(&repo_root);
    let mut status_map = HashMap::new();

    if let Ok(output) = Command::new("git")
        .arg("status")
        .arg("--porcelain=v1")
        .arg("--ignored")
        .current_dir(&repo_root)
        .output()
    {
        if output.status.success() {
            if let Ok(stdout) = String::from_utf8(output.stdout) {
                for line in stdout.lines() {
                    if line.len() < 4 {
                        continue;
                    }

                    let status_code = &line[0..2];
                    let file_path = &line[3..];

                    let status = match status_code {
                        "!!" => GitStatus::Ignored,
                        " M" | "M " | "MM" => GitStatus::Modified,
                        "A " | " A" | "AM" | "AA" => GitStatus::Added,
                        " D" | "D " | "DD" => GitStatus::Deleted,
                        "??" => GitStatus::Added,
                        _ => continue,
                    };

                    status_map.insert(PathBuf::from(file_path), status);
                }
            }
        }
    }

    Some(GitStatusCache {
        status_map,
        ignored_files: ignored,
        relative_path,
    })
}

fn get_ignored_files(repo_root: &Path) -> HashSet<PathBuf> {
    let mut ignored = HashSet::new();

    if let Ok(output) = Command::new("git")
        .arg("status")
        .arg("--porcelain=v1")
        .arg("--ignored")
        .current_dir(repo_root)
        .output()
    {
        if output.status.success() {
            if let Ok(stdout) = String::from_utf8(output.stdout) {
                for line in stdout.lines() {
                    if let Some(file_path) = line.strip_prefix("!! ") {
                        ignored.insert(PathBuf::from(file_path));
                    }
                }
            }
        }
    }

    ignored
}

/// Git status cache for directory.
#[derive(Debug)]
pub struct GitStatusCache {
    status_map: HashMap<PathBuf, GitStatus>,
    ignored_files: HashSet<PathBuf>,
    relative_path: PathBuf,
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

        GitStatus::Unmodified
    }

    pub fn is_ignored(&self, file_name: &str) -> bool {
        let full_path = if self.relative_path.as_os_str().is_empty() {
            PathBuf::from(file_name)
        } else {
            self.relative_path.join(file_name)
        };
        self.ignored_files.contains(&full_path)
    }

    pub fn has_changes_in_directory(&self, dir_name: &str) -> bool {
        let full_dir = if self.relative_path.as_os_str().is_empty() {
            PathBuf::from(dir_name)
        } else {
            self.relative_path.join(dir_name)
        };

        let dir_prefix = format!("{}/", full_dir.display());

        self.status_map.iter().any(|(path, status)| {
            if let Some(path_str) = path.to_str() {
                path_str.starts_with(&dir_prefix)
                    && *status != GitStatus::Unmodified
                    && *status != GitStatus::Ignored
            } else {
                false
            }
        })
    }

    pub fn get_directory_status(&self, dir_name: &str) -> GitStatus {
        let full_path = if self.relative_path.as_os_str().is_empty() {
            PathBuf::from(dir_name)
        } else {
            self.relative_path.join(dir_name)
        };

        if let Some(&status) = self.status_map.get(&full_path) {
            if status != GitStatus::Unmodified {
                return status;
            }
        }

        if self.is_parent_ignored(&full_path) {
            return GitStatus::Ignored;
        }

        if self.has_changes_in_directory(dir_name) {
            return GitStatus::Modified;
        }

        GitStatus::Unmodified
    }

    pub fn get_deleted_files(&self) -> Vec<String> {
        self.status_map
            .iter()
            .filter(|(path, status)| {
                **status == GitStatus::Deleted
                    && path
                        .parent()
                        .map(|p| p == self.relative_path)
                        .unwrap_or(self.relative_path.as_os_str().is_empty())
            })
            .filter_map(|(path, _)| path.file_name()?.to_str().map(String::from))
            .collect()
    }

    /// Check if path (relative to repo root) is ignored or inside an ignored directory.
    pub fn is_path_in_ignored(&self, relative_path: &Path) -> bool {
        let path_str = relative_path.to_string_lossy();

        self.ignored_files.iter().any(|ignored| {
            let ignored_str = ignored.to_string_lossy();
            // Normalize: remove trailing slash for comparison
            let ignored_normalized = ignored_str.trim_end_matches('/');

            // Exact match (file or directory name)
            if path_str == ignored_normalized {
                return true;
            }

            // Check if path is inside ignored directory
            let prefix = format!("{}/", ignored_normalized);
            path_str.starts_with(&prefix)
        })
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
pub fn get_repo_status(repo_path: &Path, item_path: &Path) -> Option<GitRepoStatus> {
    if !is_available() {
        return None;
    }

    let git_work_dir = if item_path.is_file() {
        item_path.parent().unwrap_or(repo_path)
    } else {
        item_path
    };

    let is_repo = Command::new("git")
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .current_dir(git_work_dir)
        .output()
        .ok()
        .map(|output| output.status.success())
        .unwrap_or(false);

    if !is_repo {
        return None;
    }

    let repo_root = Command::new("git")
        .arg("rev-parse")
        .arg("--show-toplevel")
        .current_dir(git_work_dir)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|s| PathBuf::from(s.trim()))
            } else {
                None
            }
        })?;

    let relative_path = item_path.strip_prefix(&repo_root).ok()?;
    let git_path = if relative_path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        relative_path
    };

    let is_ignored = Command::new("git")
        .args(["status", "--porcelain", "--ignored", "--"])
        .arg(git_path)
        .current_dir(&repo_root)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|stdout| stdout.lines().any(|line| line.starts_with("!! ")))
            } else {
                None
            }
        })
        .unwrap_or(false);

    let uncommitted_changes = Command::new("git")
        .args(["status", "--porcelain", "--"])
        .arg(git_path)
        .current_dir(&repo_root)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|stdout| stdout.lines().filter(|l| !l.starts_with("!!")).count())
            } else {
                None
            }
        })
        .unwrap_or(0);

    let ahead = Command::new("git")
        .args(["rev-list", "--count", "@{upstream}..HEAD", "--"])
        .arg(git_path)
        .current_dir(&repo_root)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
            } else {
                None
            }
        })
        .unwrap_or(0);

    let behind = Command::new("git")
        .args(["rev-list", "--count", "HEAD..@{upstream}", "--"])
        .arg(git_path)
        .current_dir(&repo_root)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
            } else {
                None
            }
        })
        .unwrap_or(0);

    Some(GitRepoStatus {
        uncommitted_changes,
        ahead,
        behind,
        is_ignored,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_repo_root() {
        let current = std::env::current_dir().unwrap();
        if let Some(root) = find_repo_root(&current) {
            assert!(root.join(".git").exists());
        }
    }
}
