//! Pending action types for modal result handling.

use std::path::PathBuf;
use std::sync::Arc;

use termide_vfs::{VfsManager, VfsPath};

use crate::batch::BatchOperation;

/// Action pending modal result
#[derive(Debug, Clone)]
pub enum PendingAction {
    /// Create new file in specified directory
    CreateFile { directory: PathBuf },
    /// Open a path typed in a viewer's "go to" prompt, routed by type.
    /// Relative paths resolve against `base_dir`.
    ViewPath { base_dir: PathBuf },
    /// Create new directory in specified directory
    CreateDirectory { directory: PathBuf },
    /// Delete files/directories (one or multiple)
    DeletePath { paths: Vec<PathBuf> },
    /// Delete remote files/directories (one or multiple)
    DeleteRemotePath {
        paths: Vec<VfsPath>,
        vfs_manager: Arc<VfsManager>,
    },
    /// Clean up a partial remote file that was left behind by a
    /// cancelled upload. Fire-and-forget delete that silently
    /// tolerates a "not found" outcome (the cancel may have happened
    /// before any bytes hit the server).
    CleanupPartialRemote {
        path: VfsPath,
        vfs_manager: Arc<VfsManager>,
    },
    /// Copy files/directories (one or multiple)
    CopyPath {
        sources: Vec<PathBuf>,
        target_directory: Option<PathBuf>,
        create_symlink: bool,
        create_relative_symlink: bool,
    },
    /// Move files/directories (one or multiple)
    MovePath {
        sources: Vec<PathBuf>,
        target_directory: Option<PathBuf>,
    },
    /// Save unnamed file (Save As)
    SaveFileAs { directory: PathBuf },
    /// Write in-memory content to a Save-As-chosen path (from a read-only
    /// viewer such as the diagram panel). Relative names resolve against
    /// `directory`.
    SaveContentAs { directory: PathBuf, content: String },
    /// Close panel (with confirmation if there are unsaved changes)
    ClosePanel,
    /// Close editor with choice: save, don't save, cancel
    CloseEditorWithSave,
    /// Close editor with external changes (file changed on disk)
    CloseEditorExternal,
    /// Close editor with conflict (local changes + external changes)
    CloseEditorConflict,
    /// Batch file operation (copy/move)
    BatchFileOperation { operation: BatchOperation },
    /// Continue batch operation after conflict resolution
    ContinueBatchOperation { operation: BatchOperation },
    /// Request rename pattern and apply to file
    RenameWithPattern {
        operation: BatchOperation,
        original_name: String,
    },
    /// Override tab_size for the active editor panel (runtime only, not persisted).
    ChangeEditorTabSize,
    /// Go to a line number entered in the editor's status-bar Pos modal.
    GotoLine,
    /// Switch to next panel
    NextPanel,
    /// Switch to previous panel
    PrevPanel,
    /// Quit application (with confirmation if there are unsaved changes)
    QuitApplication,
    /// Switch to another session
    SwitchSession,
    /// Create new session in specified directory
    NewSession,
    /// Delete session (with confirmation)
    DeleteSession { path: PathBuf },
    /// Delete bookmark (with confirmation)
    DeleteBookmark {
        path: String,
        is_project: bool,
        /// Group name to restore nested menu after deletion
        group: Option<String>,
        /// Selected index in parent bookmarks submenu to restore on return
        selected: usize,
    },
    /// Edit an existing bookmark (replace old with new)
    EditBookmark {
        original_path: String,
        /// Original group of the bookmark being edited (for precise removal)
        original_group: Option<String>,
        was_project: bool,
        /// Group name to restore nested menu on return
        group: Option<String>,
        is_project: bool,
        selected: usize,
    },
    /// Delete all bookmarks in a group (with confirmation)
    DeleteBookmarkGroup {
        group: String,
        is_project: bool,
        /// Selected index in parent bookmarks submenu to restore on return
        selected: usize,
    },
    /// Change root path of current session
    ChangeRootPath,
    /// Open Git Status panel
    OpenGitStatus,
    /// Open Git Log panel
    OpenGitLog,
    /// Git file action from File Info modal
    GitFileAction {
        /// The file path to operate on
        file_path: PathBuf,
        /// Repository root path
        repo_path: PathBuf,
        /// Whether the file is staged
        is_staged: bool,
    },
    /// Git commit action
    GitCommit {
        /// Repository root path
        repo_path: PathBuf,
    },
    /// Git revert file action (with confirmation)
    GitRevertFile {
        /// The file path to revert
        file_path: PathBuf,
        /// Repository root path
        repo_path: PathBuf,
        /// Whether the file is staged
        is_staged: bool,
    },
    /// Git revert all changes action (with confirmation)
    GitRevertAll {
        /// Repository root path
        repo_path: PathBuf,
    },
    /// Switch active panel's working directory
    SwitchDirectory,
    /// Add a bookmark
    AddBookmark {
        /// Group name to restore nested menu on return
        group: Option<String>,
        is_project: bool,
        selected: usize,
    },
    /// Go to path/URL (supports local paths and remote URLs like sftp://)
    GoToPath { current_directory: PathBuf },
    /// VFS information message (connection cancelled, error, etc.)
    VfsMessage,
    /// Handle cancelled copy/move operation cleanup
    CancelCopyCleanup {
        /// Path to the partial file/directory being copied
        partial_path: PathBuf,
        /// All destination paths created during this batch operation
        all_dest_paths: Vec<PathBuf>,
        /// Whether this is a directory (true) or file (false)
        is_directory: bool,
        /// Optional batch operation to continue after handling
        batch_operation: Option<Box<BatchOperation>>,
    },
    /// Change file permissions (Unix chmod)
    ChangePermissions { file_path: PathBuf },
    /// Follow symlink — navigate to symlink target
    FollowSymlink { target_path: PathBuf },
    /// Resolve a file conflict for an OperationManager operation
    ResolveOperationConflict {
        /// The operation ID waiting for resolution
        operation_id: termide_file_ops::OperationId,
    },
    /// LSP rename symbol: applies WorkspaceEdit after user confirms new name
    LspRenameSymbol {
        file_path: PathBuf,
        line: usize,
        column: usize,
    },
    /// Command palette — user chose a command by index
    CommandPalette,
    /// Git stash push — create new stash with user-provided message
    GitStashPush {
        /// Repository root path
        repo_path: PathBuf,
    },
    /// Git stash drop — drop stash entry after confirmation
    GitStashDrop {
        /// Repository root path
        repo_path: PathBuf,
        /// Stash index to drop
        index: usize,
    },
    /// Git stash rename — change stash message
    GitStashRename {
        /// Repository root path
        repo_path: PathBuf,
        /// Stash index to rename
        index: usize,
    },
    /// Git stash action — user chose an action from context menu (Pop/Apply/Drop/Diff)
    GitStashAction {
        /// Repository root path
        repo_path: PathBuf,
        /// Stash index
        index: usize,
        /// Stash ref string (e.g. "stash@{0}") for diff
        ref_str: String,
    },
    /// Create a new command via CommandConfigModal (create mode)
    CreateCommand,
    /// Edit an existing command via CommandConfigModal (edit mode)
    EditCommand {
        /// Command name / TOML key
        command_name: String,
        /// Whether the existing command comes from project-local config.
        is_project: bool,
        /// Group name for nested submenu restoration
        group: Option<String>,
        /// Selected index to restore on return
        selected: usize,
    },
    /// Run a command with user-provided parameters
    RunCommandWithParams {
        command: termide_config::commands::CommandItem,
    },
    /// Delete a command (with confirmation)
    DeleteCommand {
        /// Command name / TOML key.
        command_name: String,
        /// Whether this is a project-local command.
        is_project: bool,
        /// Selected index in commands submenu to restore on return
        selected: usize,
    },
    /// Rename a command
    RenameCommand {
        /// Command name / TOML key.
        command_name: String,
        /// Whether this is a project-local command.
        is_project: bool,
        /// Group name for nested submenu restoration
        group: Option<String>,
        /// Selected index to restore on return
        selected: usize,
    },
    /// Rename a bookmark (change description)
    RenameBookmark {
        path: String,
        group: Option<String>,
        is_project: bool,
        /// Selected index to restore on return
        selected: usize,
    },
    /// Retry a git network operation with an SSH key passphrase the user just
    /// entered in the (masked) password modal.
    GitSshPassphraseRetry {
        /// "fetch" | "pull" | "push"
        operation: String,
        /// Repository root path
        repo_path: PathBuf,
    },
    /// Apply settings from the Settings modal
    Settings,
    /// Confirm-modal result for "Remove project override". On `true` the
    /// `<project>/.termide/config.toml` file is deleted.
    RemoveProjectOverride,
    /// Confirm-modal result for cancelling a running background operation
    /// (triggered by Escape in the operations panel).
    CancelOperation(termide_file_ops::OperationId),
    /// Confirm-modal result for replacing every content-search match in the
    /// active file-manager panel with the given text.
    ReplaceInContent { replace_with: String },
    /// Confirm-modal result for saving the active binary (hex) editor to disk.
    SaveBinary,
    /// Result of the DB single-column filter modal. The result value carries
    /// the column/operator/value; it is applied to the active DB panel.
    DbFilter,
    /// Result of the DB connection-lost recovery dialog: the chosen button's
    /// action id ("reconnect" / "close") is routed to the active DB panel.
    DbConnectionError,
    /// Result of the DB row-detail modal: the row pre-formatted in each copy
    /// format. The chosen button's action id selects which to put on the
    /// clipboard ("copy_tsv" / "copy_json" / "copy_insert").
    DbRowDetail {
        /// Tab-separated values.
        tsv: String,
        /// JSON object `{col: value, …}`.
        json: String,
        /// `INSERT INTO … VALUES (…);` statement.
        insert: String,
    },
}
