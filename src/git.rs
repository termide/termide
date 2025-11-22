// Future API methods
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

mod watcher;
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
    let ignored = get_ignored_files(dir);

    // Get file statuses via git status
    let mut status_map = HashMap::new();

    if let Ok(output) = Command::new("git")
        .arg("status")
        .arg("--porcelain=v1")
        .arg("--ignored")
        .current_dir(dir)
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
fn get_ignored_files(dir: &Path) -> HashSet<PathBuf> {
    let mut ignored = HashSet::new();

    if let Ok(output) = Command::new("git")
        .arg("status")
        .arg("--porcelain=v1")
        .arg("--ignored")
        .current_dir(dir)
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
    /// Get status for file
    pub fn get_status(&self, file_name: &str) -> GitStatus {
        // Build full path: relative_path + file_name
        let full_path = if self.relative_path.as_os_str().is_empty() {
            PathBuf::from(file_name)
        } else {
            self.relative_path.join(file_name)
        };

        // First check ignored
        if self.ignored_files.contains(&full_path) {
            return GitStatus::Ignored;
        }

        // Then check in status_map
        self.status_map
            .get(&full_path)
            .copied()
            .unwrap_or(GitStatus::Unmodified)
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

        // Then check if any nested files have changes
        if self.has_changes_in_directory(dir_name) {
            return GitStatus::Modified;
        }

        GitStatus::Unmodified
    }
}
