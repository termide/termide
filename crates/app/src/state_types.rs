//! Auxiliary application state types — operation handles, pending state, and sub-state groups.

use std::path::PathBuf;
use std::sync::{mpsc, Arc};

use termide_vfs::{VfsManager, VfsPath};

/// Which resource modal is open (for auto-refresh in tick handler).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceModalKind {
    Cpu,
    Ram,
    Network,
    Disk,
}

/// Result of background git operation (push/pull)
#[derive(Debug)]
pub struct GitOperationResult {
    /// Operation type: "push" or "pull"
    pub operation: String,
    /// Whether the operation succeeded
    pub success: bool,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
}

/// Handle for background git operation (allows cancellation)
pub struct GitOperationHandle {
    /// Receiver for operation result
    pub receiver: mpsc::Receiver<GitOperationResult>,
    /// Process ID for cancellation
    pub pid: u32,
    /// Operation type: "push" or "pull"
    pub operation: String,
    /// Repository the operation ran in (used to retry after a passphrase prompt)
    pub repo_path: std::path::PathBuf,
    /// When the operation was started (for timeout detection)
    pub started_at: std::time::Instant,
}

impl std::fmt::Debug for GitOperationHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitOperationHandle")
            .field("pid", &self.pid)
            .field("operation", &self.operation)
            .finish_non_exhaustive()
    }
}

/// Result of background command operation (.report. commands)
#[derive(Debug)]
pub struct CommandOperationResult {
    /// Command display name
    pub command_name: String,
    /// Whether the command succeeded (exit code 0)
    pub success: bool,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
}

/// Handle for background command operation
pub struct CommandOperationHandle {
    /// Receiver for operation result
    pub receiver: mpsc::Receiver<CommandOperationResult>,
    /// Command display name
    pub command_name: String,
    /// Operation ID for tracking in Operations panel
    pub operation_id: Option<termide_file_ops::OperationId>,
    /// Process ID for killing the command on cancel
    pub pid: Option<u32>,
}

/// Kill a background command process by PID (cross-platform).
/// On Unix, uses negative PID to kill the entire process session.
/// Requires the child to have been started with `setsid()` via `pre_exec`
/// (done in `build_command_command()`) so that `-pid` targets only the child's session.
pub fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    {
        // Kill the entire session (negative PID) with SIGKILL.
        // Works because build_command_command() calls setsid() in pre_exec.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        use std::process::Command;
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .status();
    }
}

impl std::fmt::Debug for CommandOperationHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandOperationHandle")
            .field("command_name", &self.command_name)
            .finish_non_exhaustive()
    }
}

/// Pending editor download — tracks a download operation that should open an editor on completion.
/// Used when opening remote files: the download runs via OperationManager, and on completion
/// the editor is opened with the downloaded temp file.
pub struct PendingEditorDownload {
    /// OperationManager operation ID for the download
    pub operation_id: termide_file_ops::OperationId,
    /// Remote path being downloaded
    pub remote_path: VfsPath,
    /// Local temp path where file is being downloaded
    pub temp_path: PathBuf,
    /// Editor config for opening the file
    pub config: termide_panel_editor::EditorConfig,
    /// VFS manager reference for opening the editor
    pub vfs_manager: Arc<VfsManager>,
}

impl std::fmt::Debug for PendingEditorDownload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingEditorDownload")
            .field("operation_id", &self.operation_id)
            .field("remote_path", &self.remote_path.to_url_string())
            .field("temp_path", &self.temp_path)
            .finish_non_exhaustive()
    }
}

/// Pending remote delete for move operations (delete source after download completes).
///
/// When downloading from remote with is_move=true, we need to delete the source
/// after the download succeeds. This stores the VFS info needed for that deletion.
/// Grouped batch-operation state.
///
/// Bundles all fields that together describe a single in-flight batch of
/// file operations managed by `OperationManager`. Before this struct, the
/// four sub-fields were loose members of `AppState`, making it hard to see
/// they move as a group and easy to forget to reset one on completion.
#[derive(Debug, Default)]
pub struct BatchOperationState {
    /// Queued upload state for batch uploads via `OperationManager`.
    pub pending_upload: Option<PendingBatchUpload>,
    /// Queued remote delete (used by move operations — delete source after download).
    pub pending_delete: Option<PendingRemoteDelete>,
    /// Synthetic `OperationId` that represents the entire batch in the Operations panel.
    /// Individual sub-operation ids of `OperationManager` are mapped onto this entry.
    pub tracking_id: Option<termide_file_ops::OperationId>,
    /// `OperationManager` id of the individual file operation currently running
    /// inside the batch — used to bridge pause/cancel from the batch UI to the worker.
    pub sub_operation_id: Option<termide_file_ops::OperationId>,
    /// Counter for generating synthetic batch `OperationId`s.
    pub id_counter: u64,
}

pub struct PendingRemoteDelete {
    /// VFS source path to delete
    pub vfs_source: termide_vfs::VfsPath,
    /// VFS manager for the delete operation
    pub vfs_manager: std::sync::Arc<termide_vfs::VfsManager>,
}

impl std::fmt::Debug for PendingRemoteDelete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingRemoteDelete")
            .field("vfs_source", &self.vfs_source.to_url_string())
            .finish_non_exhaustive()
    }
}

/// Pending batch upload state (tracks remaining files after current upload completes).
///
/// When uploading multiple files via OperationManager, this tracks the batch state
/// so we can continue with the next file after each upload completes.
pub struct PendingBatchUpload {
    /// All source files to upload
    pub all_sources: Vec<PathBuf>,
    /// Current file index in the batch
    pub current_index: usize,
    /// Remote destination base URL (directory)
    pub dest_base_url: String,
    /// VFS manager for the upload
    pub vfs_manager: std::sync::Arc<termide_vfs::VfsManager>,
    /// Whether this is a move operation (delete source after upload)
    pub is_move: bool,
    /// Current source path being uploaded (for move delete)
    pub current_source: PathBuf,
    /// Exact remote destination of the file *currently* being uploaded.
    /// Used by the cancel-cleanup modal so we delete the right partial
    /// file even on multi-file uploads (only the last file in flight
    /// is incomplete; previously-completed files in the batch are
    /// already on the server and must not be touched).
    pub current_remote_target: Option<termide_vfs::VfsPath>,
}

impl std::fmt::Debug for PendingBatchUpload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingBatchUpload")
            .field("current_index", &self.current_index)
            .field("total_files", &self.all_sources.len())
            .field("dest_base_url", &self.dest_base_url)
            .field("is_move", &self.is_move)
            .finish_non_exhaustive()
    }
}

/// Stash dropdown state (entries, repo context, modal flags).
#[derive(Debug, Default)]
pub struct StashState {
    /// Cached stash entries for the stash dropdown
    pub entries: Vec<termide_git::StashEntry>,
    /// Repository path for stash dropdown operations
    pub repo_path: Option<std::path::PathBuf>,
    /// Whether the repo has local changes (for stash "New" item visibility)
    pub has_changes: bool,
    /// Checkbox state from stash push InputModal (include untracked files)
    pub include_untracked: bool,
}

/// Cache state — menus, commands registry, disk space.
#[derive(Debug, Default)]
pub struct CacheState {
    /// Cached shell list for the shell picker submenu (populated on open, cleared on close).
    pub shells: Vec<termide_panel_terminal::shell_utils::ShellInfo>,
    /// Cached disk space for the active panel (updated on tick, used in status bar rendering).
    pub disk_space: Option<termide_system_monitor::DiskSpaceInfo>,
    /// Cached commands registry (invalidated on menu close and filesystem changes)
    pub commands_registry: Option<termide_config::commands::CommandsRegistry>,
    /// Cached global hotkey table (invalidated when commands_registry is)
    pub hotkey_table: Option<termide_core::HotkeyTable>,
}
