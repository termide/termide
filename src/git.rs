use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

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
                    if line.starts_with("!! ") {
                        let file_path = &line[3..];
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
}

impl GitStatusCache {
    /// Get status for file
    pub fn get_status(&self, file_name: &str) -> GitStatus {
        let path = PathBuf::from(file_name);

        // First check ignored
        if self.ignored_files.contains(&path) {
            return GitStatus::Ignored;
        }

        // Then check in status_map
        self.status_map
            .get(&path)
            .copied()
            .unwrap_or(GitStatus::Unmodified)
    }

    /// Check if file is ignored
    pub fn is_ignored(&self, file_name: &str) -> bool {
        self.ignored_files.contains(&PathBuf::from(file_name))
    }
}
