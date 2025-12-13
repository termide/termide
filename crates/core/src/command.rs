//! Panel command types for tick processing.
//!
//! These commands allow the App to communicate with panels without downcasting,
//! replacing the unsafe `dyn Any` pattern with type-safe commands.

use std::path::{Path, PathBuf};

/// Commands that can be sent to panels during tick/watcher processing.
#[derive(Debug, Clone)]
pub enum PanelCommand<'a> {
    // === Git integration commands ===
    /// Request panel's git repository root (if applicable).
    /// Response: `CommandResult::RepoRoot(Option<PathBuf>)`
    GetRepoRoot,

    /// Notify panel about git status changes in specified repositories.
    /// Panel should update its git diff if its file is in one of these repos.
    /// Response: `CommandResult::NeedsRedraw(bool)`
    OnGitUpdate {
        /// Repository paths that were updated
        repo_paths: &'a [&'a Path],
    },

    /// Check and apply pending git diff updates (debounced buffer changes).
    /// Response: `CommandResult::None`
    CheckPendingGitDiff,

    /// Check for async git diff results from background thread.
    /// Response: `CommandResult::NeedsRedraw(bool)`
    CheckGitDiffReceiver,

    /// Check if file was modified externally.
    /// Response: `CommandResult::None`
    CheckExternalModification,

    // === Filesystem watcher commands ===
    /// Request panel's filesystem watch info.
    /// Response: `CommandResult::FsWatchInfo { ... }`
    GetFsWatchInfo,

    /// Set the watched root directory for the panel.
    /// Response: `CommandResult::None`
    SetFsWatchRoot {
        /// Root directory to watch (None to clear)
        root: Option<PathBuf>,
        /// Whether the root is a git repository
        is_git_repo: bool,
    },

    /// Notify panel about filesystem changes.
    /// Panel should reload if the change affects it.
    /// Response: `CommandResult::NeedsRedraw(bool)`
    OnFsUpdate {
        /// Path that was changed
        changed_path: &'a Path,
    },

    /// Reload panel content from source.
    /// Response: `CommandResult::NeedsRedraw(bool)`
    Reload,

    // === Terminal commands ===
    /// Resize terminal panel.
    /// Response: `CommandResult::None`
    Resize {
        /// New row count
        rows: u16,
        /// New column count
        cols: u16,
    },

    // === Editor state queries ===
    /// Query editor modification status.
    /// Response: `CommandResult::ModificationStatus { ... }`
    GetModificationStatus,

    /// Save editor content to file.
    /// Response: `CommandResult::SaveResult { success, error }`
    Save,

    /// Close editor without saving.
    /// Response: `CommandResult::None`
    CloseWithoutSaving,

    // === FileManager commands ===
    /// Refresh file manager directory listing.
    /// Response: `CommandResult::NeedsRedraw(bool)`
    RefreshDirectory,
}

/// Result of handling a panel command.
#[derive(Debug, Clone, Default)]
pub enum CommandResult {
    /// No response / command was ignored.
    #[default]
    None,

    /// Whether the panel needs to be redrawn.
    NeedsRedraw(bool),

    /// Git repository root (response to GetRepoRoot).
    RepoRoot(Option<PathBuf>),

    /// File path (for Editor panels).
    FilePath(Option<PathBuf>),

    /// Filesystem watch info (response to GetFsWatchInfo).
    FsWatchInfo {
        /// Currently watched root directory
        watched_root: Option<PathBuf>,
        /// Current working directory
        current_path: PathBuf,
        /// Whether the watched root is a git repository
        is_git_repo: bool,
    },

    /// Editor modification status (response to GetModificationStatus).
    ModificationStatus {
        /// Whether buffer has unsaved changes
        is_modified: bool,
        /// Whether file was modified externally
        has_external_change: bool,
    },

    /// Save operation result.
    SaveResult {
        /// Whether save was successful
        success: bool,
        /// Error message if save failed
        error: Option<String>,
    },
}

impl CommandResult {
    /// Check if the result indicates a redraw is needed.
    pub fn needs_redraw(&self) -> bool {
        matches!(self, CommandResult::NeedsRedraw(true))
    }

    /// Get repository root from result, if present.
    pub fn repo_root(&self) -> Option<&Path> {
        match self {
            CommandResult::RepoRoot(Some(path)) => Some(path.as_path()),
            _ => None,
        }
    }

    /// Get filesystem watch info from result, if present.
    pub fn fs_watch_info(&self) -> Option<(&Option<PathBuf>, &Path, bool)> {
        match self {
            CommandResult::FsWatchInfo {
                watched_root,
                current_path,
                is_git_repo,
            } => Some((watched_root, current_path.as_path(), *is_git_repo)),
            _ => None,
        }
    }

    /// Get modification status from result, if present.
    pub fn modification_status(&self) -> Option<(bool, bool)> {
        match self {
            CommandResult::ModificationStatus {
                is_modified,
                has_external_change,
            } => Some((*is_modified, *has_external_change)),
            _ => None,
        }
    }

    /// Get save result from result, if present.
    pub fn save_result(&self) -> Option<(bool, Option<&str>)> {
        match self {
            CommandResult::SaveResult { success, error } => Some((*success, error.as_deref())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_result_default() {
        let result = CommandResult::default();
        assert!(matches!(result, CommandResult::None));
    }

    #[test]
    fn test_command_result_needs_redraw() {
        assert!(!CommandResult::None.needs_redraw());
        assert!(!CommandResult::NeedsRedraw(false).needs_redraw());
        assert!(CommandResult::NeedsRedraw(true).needs_redraw());
    }

    #[test]
    fn test_command_result_repo_root() {
        let none = CommandResult::None;
        assert!(none.repo_root().is_none());

        let empty = CommandResult::RepoRoot(None);
        assert!(empty.repo_root().is_none());

        let path = PathBuf::from("/home/user/project");
        let some = CommandResult::RepoRoot(Some(path.clone()));
        assert_eq!(some.repo_root(), Some(path.as_path()));
    }

    #[test]
    fn test_command_result_fs_watch_info() {
        let none = CommandResult::None;
        assert!(none.fs_watch_info().is_none());

        let info = CommandResult::FsWatchInfo {
            watched_root: Some(PathBuf::from("/repo")),
            current_path: PathBuf::from("/repo/src"),
            is_git_repo: true,
        };
        let (root, path, is_git) = info.fs_watch_info().unwrap();
        assert_eq!(root.as_ref().unwrap().as_path(), Path::new("/repo"));
        assert_eq!(path, Path::new("/repo/src"));
        assert!(is_git);
    }

    #[test]
    fn test_command_result_modification_status() {
        let none = CommandResult::None;
        assert!(none.modification_status().is_none());

        let status = CommandResult::ModificationStatus {
            is_modified: true,
            has_external_change: false,
        };
        let (modified, external) = status.modification_status().unwrap();
        assert!(modified);
        assert!(!external);
    }

    #[test]
    fn test_command_result_save_result() {
        let none = CommandResult::None;
        assert!(none.save_result().is_none());

        let success = CommandResult::SaveResult {
            success: true,
            error: None,
        };
        let (ok, err) = success.save_result().unwrap();
        assert!(ok);
        assert!(err.is_none());

        let failure = CommandResult::SaveResult {
            success: false,
            error: Some("Permission denied".to_string()),
        };
        let (ok, err) = failure.save_result().unwrap();
        assert!(!ok);
        assert_eq!(err, Some("Permission denied"));
    }

    #[test]
    fn test_panel_command_clone() {
        let cmd = PanelCommand::GetRepoRoot;
        let cloned = cmd.clone();
        assert!(matches!(cloned, PanelCommand::GetRepoRoot));

        let cmd = PanelCommand::Resize { rows: 24, cols: 80 };
        let cloned = cmd.clone();
        assert!(matches!(
            cloned,
            PanelCommand::Resize { rows: 24, cols: 80 }
        ));
    }
}
