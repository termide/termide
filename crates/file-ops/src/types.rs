//! Unified type definitions for file operations.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use termide_vfs::VfsPath;

/// Unique identifier for an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OperationId(pub u64);

impl OperationId {
    /// Create a new operation ID.
    pub fn new(id: u64) -> Self {
        Self(id)
    }
}

impl std::fmt::Display for OperationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "op-{}", self.0)
    }
}

/// Phase of an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationPhase {
    /// Scanning/counting files and calculating total size.
    Scanning,
    /// Actively transferring data.
    Transferring,
    /// Cleaning up (e.g., deleting source for move operations).
    Cleaning,
    /// Operation completed successfully.
    Completed,
    /// Operation failed.
    Failed,
    /// Operation was cancelled.
    Cancelled,
}

/// Type of file operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationType {
    /// Copy file(s).
    Copy,
    /// Move file(s) (copy + delete source).
    Move,
    /// Delete file(s).
    Delete,
    /// Download from remote to local (single or multiple files).
    Download,
    /// Upload from local to remote (single or multiple files).
    Upload,
}

/// Priority level for operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum OperationPriority {
    /// Low priority - background operations.
    Low = 0,
    /// Normal priority - user-initiated operations.
    #[default]
    Normal = 1,
    /// High priority - urgent operations.
    High = 2,
    /// Immediate - bypass queue.
    Immediate = 3,
}

/// Unified path that can be local or remote.
#[derive(Debug, Clone)]
pub enum OperationPath {
    /// Local filesystem path.
    Local(PathBuf),
    /// Remote VFS path.
    Remote(VfsPath),
}

impl OperationPath {
    /// Create a local path.
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::Local(path.into())
    }

    /// Create a remote path.
    pub fn remote(path: VfsPath) -> Self {
        Self::Remote(path)
    }

    /// Check if this is a local path.
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }

    /// Check if this is a remote path.
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote(_))
    }

    /// Get the file name.
    pub fn file_name(&self) -> Option<String> {
        match self {
            Self::Local(p) => p.file_name().and_then(|n| n.to_str()).map(String::from),
            Self::Remote(p) => p.file_name().and_then(|n| n.to_str()).map(String::from),
        }
    }

    /// Convert to display string.
    pub fn display(&self) -> String {
        match self {
            Self::Local(p) => p.display().to_string(),
            Self::Remote(p) => p.to_string(),
        }
    }
}

impl From<PathBuf> for OperationPath {
    fn from(path: PathBuf) -> Self {
        Self::Local(path)
    }
}

impl From<VfsPath> for OperationPath {
    fn from(path: VfsPath) -> Self {
        if path.is_local() {
            Self::Local(path.path)
        } else {
            Self::Remote(path)
        }
    }
}

/// Unified progress information for all operations.
#[derive(Debug, Clone)]
pub struct OperationProgress {
    /// Current phase of the operation.
    pub phase: OperationPhase,
    /// Bytes transferred so far (overall).
    pub bytes_transferred: u64,
    /// Total bytes to transfer (0 if unknown).
    pub total_bytes: u64,
    /// Files completed so far.
    pub files_completed: usize,
    /// Total files to process (0 if unknown).
    pub total_files: usize,
    /// Current item being processed.
    pub current_item: Option<String>,
    /// Current transfer speed in bytes per second.
    pub speed_bps: f64,
    /// Estimated time remaining in seconds.
    pub eta_seconds: Option<u64>,
    /// Current individual file bytes transferred (for batch operations).
    pub individual_file_bytes: u64,
    /// Current individual file total bytes (for batch operations).
    pub individual_file_total: u64,
}

impl Default for OperationProgress {
    fn default() -> Self {
        Self {
            phase: OperationPhase::Scanning,
            bytes_transferred: 0,
            total_bytes: 0,
            files_completed: 0,
            total_files: 0,
            current_item: None,
            speed_bps: 0.0,
            eta_seconds: None,
            individual_file_bytes: 0,
            individual_file_total: 0,
        }
    }
}

impl OperationProgress {
    /// Create a new progress instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create progress for scanning phase.
    pub fn scanning() -> Self {
        Self {
            phase: OperationPhase::Scanning,
            ..Default::default()
        }
    }

    /// Create progress for scanning phase with details.
    pub fn scanning_details(
        files_found: usize,
        bytes_found: u64,
        current_dir: Option<String>,
    ) -> Self {
        Self {
            phase: OperationPhase::Scanning,
            files_completed: files_found,
            total_bytes: bytes_found,
            current_item: current_dir,
            ..Default::default()
        }
    }

    /// Create progress for transferring phase.
    pub fn transferring(
        bytes_transferred: u64,
        total_bytes: u64,
        files_completed: usize,
        total_files: usize,
    ) -> Self {
        Self {
            phase: OperationPhase::Transferring,
            bytes_transferred,
            total_bytes,
            files_completed,
            total_files,
            ..Default::default()
        }
    }

    /// Create progress for cleaning phase.
    pub fn cleaning(files_completed: usize, total_files: usize) -> Self {
        Self {
            phase: OperationPhase::Cleaning,
            files_completed,
            total_files,
            ..Default::default()
        }
    }

    /// Create progress for completed phase.
    pub fn completed(bytes_transferred: u64, files_completed: usize, total_files: usize) -> Self {
        Self {
            phase: OperationPhase::Completed,
            bytes_transferred,
            total_bytes: bytes_transferred,
            files_completed,
            total_files,
            ..Default::default()
        }
    }

    /// Set current item being processed.
    pub fn with_item(mut self, item: impl Into<String>) -> Self {
        self.current_item = Some(item.into());
        self
    }

    /// Set transfer speed and ETA.
    pub fn with_speed(mut self, speed_bps: f64, eta_seconds: Option<u64>) -> Self {
        self.speed_bps = speed_bps;
        self.eta_seconds = eta_seconds;
        self
    }

    /// Set individual file progress (for batch operations).
    pub fn with_individual(mut self, bytes: u64, total: u64) -> Self {
        self.individual_file_bytes = bytes;
        self.individual_file_total = total;
        self
    }

    /// Set bytes transferred and total.
    pub fn with_bytes(mut self, transferred: u64, total: u64) -> Self {
        self.bytes_transferred = transferred;
        self.total_bytes = total;
        self
    }

    /// Calculate completion percentage (0.0 - 1.0).
    pub fn percentage(&self) -> f64 {
        if self.total_bytes > 0 {
            self.bytes_transferred as f64 / self.total_bytes as f64
        } else if self.total_files > 0 {
            self.files_completed as f64 / self.total_files as f64
        } else {
            0.0
        }
    }

    /// Check if the operation is complete.
    pub fn is_complete(&self) -> bool {
        matches!(
            self.phase,
            OperationPhase::Completed | OperationPhase::Failed | OperationPhase::Cancelled
        )
    }
}

/// Control flags for an operation.
#[derive(Debug, Clone)]
pub struct OperationControl {
    /// Flag to pause the operation.
    pub pause_flag: Arc<AtomicBool>,
    /// Flag to cancel the operation.
    pub cancel_flag: Arc<AtomicBool>,
}

impl Default for OperationControl {
    fn default() -> Self {
        Self::new()
    }
}

impl OperationControl {
    /// Create new operation control flags.
    pub fn new() -> Self {
        Self {
            pause_flag: Arc::new(AtomicBool::new(false)),
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set paused state.
    pub fn set_paused(&self, paused: bool) {
        self.pause_flag.store(paused, Ordering::Relaxed);
    }

    /// Check if paused.
    pub fn is_paused(&self) -> bool {
        self.pause_flag.load(Ordering::Relaxed)
    }

    /// Cancel the operation.
    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::Relaxed);
    }

    /// Check if cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::Relaxed)
    }

    /// Check for cancellation, returning error if cancelled.
    pub fn check_cancelled(&self) -> Result<(), OperationError> {
        if self.is_cancelled() {
            Err(OperationError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Wait while paused, checking for cancellation.
    pub fn wait_if_paused(&self) -> Result<(), OperationError> {
        while self.is_paused() {
            if self.is_cancelled() {
                return Err(OperationError::Cancelled);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Ok(())
    }
}

/// Result of an operation.
#[derive(Debug, Clone)]
pub enum OperationResult {
    /// Operation completed successfully.
    Success,
    /// Operation completed with a result path.
    SuccessWithPath(PathBuf),
    /// Operation partially succeeded (some items skipped or failed).
    PartialSuccess {
        /// Number of items completed successfully.
        completed: usize,
        /// Number of items skipped.
        skipped: usize,
        /// Number of items that failed.
        failed: usize,
        /// List of failed file paths with error messages.
        failed_files: Vec<String>,
    },
    /// Operation failed.
    Failed(String),
    /// Operation was cancelled.
    Cancelled,
}

impl OperationResult {
    /// Check if the result is success (full or partial).
    pub fn is_success(&self) -> bool {
        matches!(
            self,
            Self::Success | Self::SuccessWithPath(_) | Self::PartialSuccess { .. }
        )
    }
}

/// Error type for file operations.
#[derive(Debug, thiserror::Error)]
pub enum OperationError {
    /// Operation was cancelled.
    #[error("Operation cancelled")]
    Cancelled,

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// VFS error.
    #[error("VFS error: {0}")]
    Vfs(#[from] termide_vfs::VfsError),

    /// Operation not found.
    #[error("Operation not found: {0}")]
    NotFound(OperationId),

    /// Queue is full.
    #[error("Operation queue is full")]
    QueueFull,

    /// Invalid operation.
    #[error("Invalid operation: {0}")]
    Invalid(String),
}

/// Events emitted by the operation system.
#[derive(Debug, Clone)]
pub enum OperationEvent {
    /// Operation started.
    Started(OperationId),
    /// Operation progress updated.
    Progress(OperationId, OperationProgress),
    /// Operation completed.
    Completed(OperationId, OperationResult),
    /// Operation paused.
    Paused(OperationId),
    /// Operation resumed.
    Resumed(OperationId),
    /// Conflict detected - operation waiting for user decision.
    ConflictDetected(OperationId, ConflictInfo),
}

/// Information about a file conflict.
#[derive(Debug, Clone)]
pub struct ConflictInfo {
    /// Source file path.
    pub source: OperationPath,
    /// Destination file path (existing file).
    pub destination: OperationPath,
    /// Source file size in bytes.
    pub source_size: u64,
    /// Destination file size in bytes.
    pub dest_size: u64,
    /// Source file modification time (Unix timestamp).
    pub source_modified: Option<u64>,
    /// Destination file modification time (Unix timestamp).
    pub dest_modified: Option<u64>,
    /// Number of remaining items in the batch.
    pub remaining_items: usize,
}

/// User's decision for handling a conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictResolution {
    /// Overwrite the destination file.
    Overwrite,
    /// Skip this file.
    Skip,
    /// Rename this file to the given name.
    Rename(String),
    /// Overwrite all remaining conflicts.
    OverwriteAll,
    /// Skip all remaining conflicts.
    SkipAll,
    /// Rename all remaining conflicts with auto-generated names.
    RenameAll,
    /// Cancel the entire operation.
    Cancel,
}

/// Conflict handling mode for batch operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictMode {
    /// Ask the user for each conflict.
    #[default]
    Ask,
    /// Automatically overwrite all conflicts.
    OverwriteAll,
    /// Automatically skip all conflicts.
    SkipAll,
    /// Automatically rename all conflicts with auto-generated names.
    RenameAll,
}

/// Request to create a new operation.
#[derive(Debug, Clone)]
pub struct OperationRequest {
    /// Type of operation.
    pub op_type: OperationType,
    /// Source path(s).
    pub sources: Vec<OperationPath>,
    /// Destination path (for copy/move/download/upload).
    pub destination: Option<OperationPath>,
    /// Priority level.
    pub priority: OperationPriority,
    /// Whether this is a move operation (delete source after copy).
    pub is_move: bool,
    /// How to handle file conflicts.
    pub conflict_mode: ConflictMode,
}

impl OperationRequest {
    /// Create a copy operation request.
    pub fn copy(sources: Vec<OperationPath>, destination: OperationPath) -> Self {
        Self {
            op_type: OperationType::Copy,
            sources,
            destination: Some(destination),
            priority: OperationPriority::Normal,
            is_move: false,
            conflict_mode: ConflictMode::Ask,
        }
    }

    /// Create a move operation request.
    pub fn r#move(sources: Vec<OperationPath>, destination: OperationPath) -> Self {
        Self {
            op_type: OperationType::Move,
            sources,
            destination: Some(destination),
            priority: OperationPriority::Normal,
            is_move: true,
            conflict_mode: ConflictMode::Ask,
        }
    }

    /// Create a delete operation request.
    pub fn delete(sources: Vec<OperationPath>) -> Self {
        Self {
            op_type: OperationType::Delete,
            sources,
            destination: None,
            priority: OperationPriority::Normal,
            is_move: false,
            conflict_mode: ConflictMode::Ask,
        }
    }

    /// Create a download operation request (remote to local).
    pub fn download(remote: VfsPath, local: PathBuf) -> Self {
        Self {
            op_type: OperationType::Download,
            sources: vec![OperationPath::Remote(remote)],
            destination: Some(OperationPath::Local(local)),
            priority: OperationPriority::Normal,
            is_move: false,
            conflict_mode: ConflictMode::Ask,
        }
    }

    /// Create an upload operation request (local to remote).
    pub fn upload(local: PathBuf, remote: VfsPath) -> Self {
        Self {
            op_type: OperationType::Upload,
            sources: vec![OperationPath::Local(local)],
            destination: Some(OperationPath::Remote(remote)),
            priority: OperationPriority::Normal,
            is_move: false,
            conflict_mode: ConflictMode::Ask,
        }
    }

    /// Set the priority.
    pub fn with_priority(mut self, priority: OperationPriority) -> Self {
        self.priority = priority;
        self
    }

    /// Set the conflict handling mode.
    pub fn with_conflict_mode(mut self, mode: ConflictMode) -> Self {
        self.conflict_mode = mode;
        self
    }
}

/// Information about a queued or running operation.
#[derive(Debug)]
pub struct OperationInfo {
    /// Operation ID.
    pub id: OperationId,
    /// Operation type.
    pub op_type: OperationType,
    /// Source paths.
    pub sources: Vec<OperationPath>,
    /// Destination path.
    pub destination: Option<OperationPath>,
    /// Current progress.
    pub progress: OperationProgress,
    /// Control flags.
    pub control: OperationControl,
    /// Whether the operation is currently active.
    pub is_active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // OperationProgress tests
    // =========================================================================

    #[test]
    fn test_progress_percentage_by_bytes() {
        let p = OperationProgress::transferring(500, 1000, 0, 0);
        assert!((p.percentage() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_progress_percentage_by_files() {
        let p = OperationProgress {
            phase: OperationPhase::Transferring,
            files_completed: 3,
            total_files: 10,
            ..Default::default()
        };
        assert!((p.percentage() - 0.3).abs() < 0.001);
    }

    #[test]
    fn test_progress_percentage_zero() {
        let p = OperationProgress::default();
        assert_eq!(p.percentage(), 0.0);
    }

    #[test]
    fn test_progress_is_complete() {
        assert!(OperationProgress::completed(100, 5, 5).is_complete());
        assert!(OperationProgress {
            phase: OperationPhase::Failed,
            ..Default::default()
        }
        .is_complete());
        assert!(OperationProgress {
            phase: OperationPhase::Cancelled,
            ..Default::default()
        }
        .is_complete());
        assert!(!OperationProgress::scanning().is_complete());
        assert!(!OperationProgress::transferring(0, 100, 0, 1).is_complete());
    }

    #[test]
    fn test_progress_builders() {
        let p = OperationProgress::scanning()
            .with_item("test.txt")
            .with_speed(1024.0, Some(10));
        assert_eq!(p.phase, OperationPhase::Scanning);
        assert_eq!(p.current_item, Some("test.txt".to_string()));
        assert_eq!(p.speed_bps, 1024.0);
        assert_eq!(p.eta_seconds, Some(10));
    }

    #[test]
    fn test_progress_with_individual() {
        let p = OperationProgress::transferring(100, 1000, 1, 5).with_individual(50, 200);
        assert_eq!(p.individual_file_bytes, 50);
        assert_eq!(p.individual_file_total, 200);
    }

    #[test]
    fn test_progress_with_bytes() {
        let p = OperationProgress::new().with_bytes(500, 1000);
        assert_eq!(p.bytes_transferred, 500);
        assert_eq!(p.total_bytes, 1000);
    }

    // =========================================================================
    // OperationControl tests
    // =========================================================================

    #[test]
    fn test_operation_control_pause_resume() {
        let ctrl = OperationControl::new();
        assert!(!ctrl.is_paused());
        ctrl.set_paused(true);
        assert!(ctrl.is_paused());
        ctrl.set_paused(false);
        assert!(!ctrl.is_paused());
    }

    #[test]
    fn test_operation_control_cancel() {
        let ctrl = OperationControl::new();
        assert!(!ctrl.is_cancelled());
        assert!(ctrl.check_cancelled().is_ok());

        ctrl.cancel();
        assert!(ctrl.is_cancelled());
        assert!(ctrl.check_cancelled().is_err());
    }

    #[test]
    fn test_operation_control_clone_shares_state() {
        let ctrl = OperationControl::new();
        let clone = ctrl.clone();

        ctrl.cancel();
        assert!(clone.is_cancelled());

        ctrl.set_paused(true);
        assert!(clone.is_paused());
    }

    // =========================================================================
    // OperationResult tests
    // =========================================================================

    #[test]
    fn test_operation_result_is_success() {
        assert!(OperationResult::Success.is_success());
        assert!(OperationResult::SuccessWithPath(PathBuf::from("/x")).is_success());
        assert!(OperationResult::PartialSuccess {
            completed: 5,
            skipped: 1,
            failed: 0,
            failed_files: vec![],
        }
        .is_success());
        assert!(!OperationResult::Failed("err".to_string()).is_success());
        assert!(!OperationResult::Cancelled.is_success());
    }

    // =========================================================================
    // OperationPath tests
    // =========================================================================

    #[test]
    fn test_operation_path_local() {
        let p = OperationPath::local("/test/file.txt");
        assert!(p.is_local());
        assert!(!p.is_remote());
        assert_eq!(p.file_name(), Some("file.txt".to_string()));
        assert_eq!(p.display(), "/test/file.txt");
    }

    #[test]
    fn test_operation_path_from_pathbuf() {
        let p: OperationPath = PathBuf::from("/test").into();
        assert!(p.is_local());
    }

    // =========================================================================
    // OperationRequest tests
    // =========================================================================

    #[test]
    fn test_request_copy() {
        let req = OperationRequest::copy(
            vec![OperationPath::local("/src")],
            OperationPath::local("/dst"),
        );
        assert_eq!(req.op_type, OperationType::Copy);
        assert!(!req.is_move);
        assert_eq!(req.priority, OperationPriority::Normal);
        assert_eq!(req.conflict_mode, ConflictMode::Ask);
    }

    #[test]
    fn test_request_move() {
        let req = OperationRequest::r#move(
            vec![OperationPath::local("/src")],
            OperationPath::local("/dst"),
        );
        assert!(req.is_move);
    }

    #[test]
    fn test_request_delete() {
        let req = OperationRequest::delete(vec![OperationPath::local("/src")]);
        assert_eq!(req.op_type, OperationType::Delete);
        assert!(req.destination.is_none());
    }

    #[test]
    fn test_request_with_modifiers() {
        let req = OperationRequest::copy(
            vec![OperationPath::local("/src")],
            OperationPath::local("/dst"),
        )
        .with_priority(OperationPriority::High)
        .with_conflict_mode(ConflictMode::OverwriteAll);

        assert_eq!(req.priority, OperationPriority::High);
        assert_eq!(req.conflict_mode, ConflictMode::OverwriteAll);
    }

    // =========================================================================
    // BackgroundOperationSummary tests
    // =========================================================================

    #[test]
    fn test_summary_no_operations() {
        let summary = BackgroundOperationSummary::default();
        assert!(!summary.has_operations());
        assert_eq!(summary.percentage(), 0.0);
        assert_eq!(summary.status_text(), "");
    }

    #[test]
    fn test_summary_single_active() {
        let summary = BackgroundOperationSummary {
            active_count: 1,
            total_bytes_transferred: 50,
            total_bytes: 100,
            current_activity: Some("Copying".to_string()),
            ..Default::default()
        };
        assert!(summary.has_operations());
        assert!((summary.percentage() - 0.5).abs() < 0.001);
        assert_eq!(summary.status_text(), "Copying 50%");
    }

    #[test]
    fn test_summary_with_queue() {
        let summary = BackgroundOperationSummary {
            active_count: 1,
            queued_count: 3,
            total_bytes_transferred: 25,
            total_bytes: 100,
            current_activity: Some("Moving".to_string()),
            ..Default::default()
        };
        assert_eq!(summary.status_text(), "Moving 25% (+3 queued)");
    }

    #[test]
    fn test_summary_speed_text() {
        let summary = BackgroundOperationSummary {
            speed_bps: 500.0,
            ..Default::default()
        };
        assert_eq!(summary.speed_text(), "500 B/s");

        let summary = BackgroundOperationSummary {
            speed_bps: 2048.0,
            ..Default::default()
        };
        assert_eq!(summary.speed_text(), "2.0 KB/s");

        let summary = BackgroundOperationSummary {
            speed_bps: 5.0 * 1024.0 * 1024.0,
            ..Default::default()
        };
        assert_eq!(summary.speed_text(), "5.0 MB/s");
    }

    // =========================================================================
    // OperationId tests
    // =========================================================================

    #[test]
    fn test_operation_id_display() {
        let id = OperationId::new(42);
        assert_eq!(format!("{}", id), "op-42");
    }

    #[test]
    fn test_operation_id_equality() {
        assert_eq!(OperationId::new(1), OperationId::new(1));
        assert_ne!(OperationId::new(1), OperationId::new(2));
    }
}

/// Summary of background operations for status bar display.
#[derive(Debug, Clone, Default)]
pub struct BackgroundOperationSummary {
    /// Number of active operations running in background.
    pub active_count: usize,
    /// Number of queued operations waiting.
    pub queued_count: usize,
    /// Total bytes transferred across all background operations.
    pub total_bytes_transferred: u64,
    /// Total bytes to transfer across all background operations.
    pub total_bytes: u64,
    /// Number of files completed across all background operations.
    pub files_completed: usize,
    /// Total files to process across all background operations.
    pub total_files: usize,
    /// Overall transfer speed in bytes per second.
    pub speed_bps: f64,
    /// Whether any operation is paused.
    pub any_paused: bool,
    /// Short description of current activity.
    pub current_activity: Option<String>,
}

impl BackgroundOperationSummary {
    /// Calculate overall completion percentage (0.0 - 1.0).
    pub fn percentage(&self) -> f64 {
        if self.total_bytes > 0 {
            self.total_bytes_transferred as f64 / self.total_bytes as f64
        } else if self.total_files > 0 {
            self.files_completed as f64 / self.total_files as f64
        } else {
            0.0
        }
    }

    /// Check if there are any operations (active or queued).
    pub fn has_operations(&self) -> bool {
        self.active_count > 0 || self.queued_count > 0
    }

    /// Format for status bar display.
    pub fn status_text(&self) -> String {
        if !self.has_operations() {
            return String::new();
        }

        let percent = (self.percentage() * 100.0) as u8;

        if self.active_count == 1 && self.queued_count == 0 {
            // Single active operation, no queue
            if let Some(ref activity) = self.current_activity {
                format!("{} {}%", activity, percent)
            } else {
                format!("Operation {}%", percent)
            }
        } else if self.active_count == 0 && self.queued_count > 0 {
            // Only queued operations (waiting)
            format!("{} queued", self.queued_count)
        } else if self.queued_count > 0 {
            // Active + queued
            if let Some(ref activity) = self.current_activity {
                format!("{} {}% (+{} queued)", activity, percent, self.queued_count)
            } else {
                format!(
                    "{} active {}% (+{} queued)",
                    self.active_count, percent, self.queued_count
                )
            }
        } else {
            // Multiple active, no queue
            format!("{} ops {}%", self.active_count, percent)
        }
    }

    /// Format speed for display.
    pub fn speed_text(&self) -> String {
        if self.speed_bps < 1024.0 {
            format!("{:.0} B/s", self.speed_bps)
        } else if self.speed_bps < 1024.0 * 1024.0 {
            format!("{:.1} KB/s", self.speed_bps / 1024.0)
        } else {
            format!("{:.1} MB/s", self.speed_bps / (1024.0 * 1024.0))
        }
    }
}
