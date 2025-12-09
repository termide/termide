// Future API methods
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

mod diff;
mod watcher;

pub use diff::{load_original_async, GitDiffAsyncResult, GitDiffCache, LineStatus};
pub use watcher::{create_git_watcher, GitStatusUpdate, GitWatcher};

/// Global flag for git availability on system
static GIT_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Git file status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitStatus {
    /// File not modified
    Unmodified,
    /// File modified
    Modified,
    /// New file (staged or unstaged)
    Added,
    /// File deleted
    Deleted,
    /// File in .gitignore
    Ignored,
}

/// Find git repository root by walking up from a path
/// Returns None if the path is not inside a git repository
pub fn find_repo_root(path: &Path) -> Option<PathBuf> {
    let mut current = path;
    loop {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

/// Check if git is available on system
pub fn check_git_available() -> bool {
    *GIT_AVAILABLE.get_or_init(|| {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    })
}

/// Get git status for directory
pub fn get_git_status(dir: &Path) -> Option<GitStatusCache> {
    if !check_git_available() {
        return None;
    }

    // Check if we're in a git repository
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

    // Get repository root
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

    // Calculate relative path from repository root to current directory
    let relative_path = dir
        .strip_prefix(&repo_root)
        .unwrap_or(Path::new(""))
        .to_path_buf();

    // Get list of ignored files
    let ignored = get_ignored_files(&repo_root);

    // Get file statuses via git status
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
                        "??" => GitStatus::Added, // Untracked treated as Added
                        _ => continue,
                    };

                    // Normalize path
                    let normalized_path = PathBuf::from(file_path);
                    status_map.insert(normalized_path, status);
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

/// Get list of ignored files
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

/// Git status cache for directory
#[derive(Debug)]
pub struct GitStatusCache {
    status_map: HashMap<PathBuf, GitStatus>,
    ignored_files: HashSet<PathBuf>,
    /// Relative path from repository root to the directory being cached
    relative_path: PathBuf,
}

impl GitStatusCache {
    /// Check if any parent directory of the given path is ignored
    fn is_parent_ignored(&self, path: &Path) -> bool {
        // Check all parent directories
        let mut current = path;
        while let Some(parent) = current.parent() {
            // Check in ignored_files set
            if self.ignored_files.contains(parent) {
                return true;
            }
            // Check in status_map
            if let Some(&GitStatus::Ignored) = self.status_map.get(parent) {
                return true;
            }
            // Move up to next parent
            current = parent;
            // Stop at empty path (root)
            if parent.as_os_str().is_empty() {
                break;
            }
        }
        false
    }

    /// Get status for file
    pub fn get_status(&self, file_name: &str) -> GitStatus {
        // Build full path: relative_path + file_name
        let full_path = if self.relative_path.as_os_str().is_empty() {
            PathBuf::from(file_name)
        } else {
            self.relative_path.join(file_name)
        };

        // First check if file itself is ignored
        if self.ignored_files.contains(&full_path) {
            return GitStatus::Ignored;
        }

        // Then check in status_map for exact match
        if let Some(&status) = self.status_map.get(&full_path) {
            return status;
        }

        // Finally, check if any parent directory is ignored
        if self.is_parent_ignored(&full_path) {
            return GitStatus::Ignored;
        }

        GitStatus::Unmodified
    }

    /// Check if file is ignored
    pub fn is_ignored(&self, file_name: &str) -> bool {
        let full_path = if self.relative_path.as_os_str().is_empty() {
            PathBuf::from(file_name)
        } else {
            self.relative_path.join(file_name)
        };
        self.ignored_files.contains(&full_path)
    }

    /// Check if path (relative to repo root) is ignored or inside an ignored directory
    pub fn is_path_in_ignored(&self, relative_path: &Path) -> bool {
        let path_str = relative_path.to_string_lossy();

        self.ignored_files.iter().any(|ignored| {
            let ignored_str = ignored.to_string_lossy();
            // Normalize: remove trailing slash for comparison
            // (git status outputs "target/" but we need to match "target/debug/foo")
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

    /// Check if directory contains any changes recursively
    pub fn has_changes_in_directory(&self, dir_name: &str) -> bool {
        // Build full path for directory
        let full_dir = if self.relative_path.as_os_str().is_empty() {
            PathBuf::from(dir_name)
        } else {
            self.relative_path.join(dir_name)
        };

        let dir_prefix = format!("{}/", full_dir.display());

        self.status_map.iter().any(|(path, status)| {
            // Check if path is inside this directory
            if let Some(path_str) = path.to_str() {
                path_str.starts_with(&dir_prefix)
                    && *status != GitStatus::Unmodified
                    && *status != GitStatus::Ignored
            } else {
                false
            }
        })
    }

    /// Get status for directory (checks nested files recursively)
    pub fn get_directory_status(&self, dir_name: &str) -> GitStatus {
        // Build full path for directory
        let full_path = if self.relative_path.as_os_str().is_empty() {
            PathBuf::from(dir_name)
        } else {
            self.relative_path.join(dir_name)
        };

        // First check if directory itself has status
        if let Some(&status) = self.status_map.get(&full_path) {
            if status != GitStatus::Unmodified {
                return status;
            }
        }

        // Check if directory is inside an ignored parent directory
        if self.is_parent_ignored(&full_path) {
            return GitStatus::Ignored;
        }

        // Then check if any nested files have changes
        if self.has_changes_in_directory(dir_name) {
            return GitStatus::Modified;
        }

        GitStatus::Unmodified
    }

    /// Get list of deleted files in the current directory (for virtual entries)
    pub fn get_deleted_files(&self) -> Vec<String> {
        self.status_map
            .iter()
            .filter(|(path, status)| {
                // Only Deleted status
                **status == GitStatus::Deleted
                    // Only files in current directory (not nested)
                    && path.parent().map(|p| p == self.relative_path).unwrap_or(
                        // If no parent, only match if relative_path is empty (repo root)
                        self.relative_path.as_os_str().is_empty(),
                    )
            })
            .filter_map(|(path, _)| path.file_name()?.to_str().map(String::from))
            .collect()
    }
}

/// Git repository status information
#[derive(Debug, Clone, Copy)]
pub struct GitRepoStatus {
    /// Number of uncommitted changes (staged + unstaged)
    pub uncommitted_changes: usize,
    /// Number of commits ahead of remote (not pushed)
    pub ahead: usize,
    /// Number of commits behind remote (not pulled)
    pub behind: usize,
    /// Whether the file/directory is ignored by .gitignore
    pub is_ignored: bool,
}

/// Get git repository status for a specific file or directory
/// Returns information about uncommitted changes, ahead/behind status filtered by path
///
/// # Arguments
/// * `repo_path` - Path to the git repository (used for git commands working directory)
/// * `item_path` - Absolute path to the specific file or directory to check
pub fn get_repo_status(repo_path: &Path, item_path: &Path) -> Option<GitRepoStatus> {
    if !check_git_available() {
        return None;
    }

    // Determine working directory for git commands from item_path
    // If item_path is a file, use its parent directory
    // If item_path is a directory, use it directly
    let git_work_dir = if item_path.is_file() {
        item_path.parent().unwrap_or(repo_path)
    } else {
        item_path
    };

    // Check if the item is in a git repository
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

    // Get repository root to calculate relative path
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

    // Calculate relative path from repository root to the item
    let relative_path = item_path.strip_prefix(&repo_root).ok()?;

    // Use "." if path is empty (item is repository root itself)
    // Git doesn't accept empty string as pathspec
    let git_path = if relative_path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        relative_path
    };

    // Check if file/directory is ignored by .gitignore
    // Uses git status --porcelain --ignored to match the logic used for file list display
    let is_ignored = Command::new("git")
        .arg("status")
        .arg("--porcelain")
        .arg("--ignored")
        .arg("--")
        .arg(git_path)
        .current_dir(&repo_root)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok().map(|stdout| {
                    // Check if the specific path is ignored
                    // For directories, only mark as ignored if the directory itself is ignored,
                    // not if it just contains some ignored files
                    if item_path.is_dir() {
                        // For directories, check if the directory path appears as ignored
                        let dir_pattern = git_path.to_str().unwrap_or("");
                        stdout.lines().any(|line| {
                            if let Some(ignored_path) = line.strip_prefix("!! ") {
                                // Match exact directory or directory with trailing slash
                                ignored_path == dir_pattern
                                    || ignored_path == format!("{}/", dir_pattern)
                            } else {
                                false
                            }
                        })
                    } else {
                        // For files, check if the file path appears as ignored
                        let file_pattern = git_path.to_str().unwrap_or("");
                        stdout.lines().any(|line| {
                            if let Some(ignored_path) = line.strip_prefix("!! ") {
                                ignored_path == file_pattern
                            } else {
                                false
                            }
                        })
                    }
                })
            } else {
                None
            }
        })
        .unwrap_or(false);

    // Count uncommitted changes for this specific path
    let uncommitted_changes = Command::new("git")
        .arg("status")
        .arg("--porcelain")
        .arg("--")
        .arg(git_path)
        .current_dir(&repo_root)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok().map(|stdout| {
                    stdout
                        .lines()
                        .filter(|line| {
                            // Count all lines except ignored files
                            !line.starts_with("!!")
                        })
                        .count()
                })
            } else {
                None
            }
        })
        .unwrap_or(0);

    // Get ahead count (commits not pushed that affect this path)
    let ahead = Command::new("git")
        .arg("rev-list")
        .arg("--count")
        .arg("@{upstream}..HEAD")
        .arg("--")
        .arg(git_path)
        .current_dir(&repo_root)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .and_then(|s| s.trim().parse::<usize>().ok())
            } else {
                None
            }
        })
        .unwrap_or(0);

    // Get behind count (commits not pulled that affect this path)
    let behind = Command::new("git")
        .arg("rev-list")
        .arg("--count")
        .arg("HEAD..@{upstream}")
        .arg("--")
        .arg(git_path)
        .current_dir(&repo_root)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .and_then(|s| s.trim().parse::<usize>().ok())
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
