//! Batch file operation types.

use std::path::PathBuf;
use std::time::SystemTime;

use chrono::{DateTime, Local};

/// Message about background directory size calculation result
#[derive(Debug)]
pub struct DirSizeResult {
    pub size: u64,
    /// Immediate (non-recursive) child count of the directory, when known.
    pub item_count: Option<usize>,
}

/// Batch operation type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchOperationType {
    Copy,
    Move,
}

/// Automatic conflict resolution mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictMode {
    /// Ask each time
    Ask,
    /// Automatically overwrite all
    OverwriteAll,
    /// Automatically skip all
    SkipAll,
}

/// File rename pattern
#[derive(Debug, Clone)]
pub struct RenamePattern {
    template: String,
}

impl RenamePattern {
    /// Create new rename pattern
    pub fn new(template: String) -> Self {
        Self { template }
    }

    /// Apply pattern to filename
    pub fn apply(
        &self,
        original_name: &str,
        counter: usize,
        created: Option<SystemTime>,
        modified: Option<SystemTime>,
    ) -> String {
        // Collect parts without allocating Strings - just &str slices
        let parts: Vec<&str> = original_name.split('.').collect();
        let mut result = self.template.clone();

        // Replace $0 (full name)
        result = result.replace("$0", original_name);

        // Replace $1-9 (parts from left)
        for i in 1..=9 {
            let placeholder = format!("${}", i);
            let value = parts.get(i - 1).copied().unwrap_or("");
            result = result.replace(&placeholder, value);
        }

        // Replace $-1 to $-9 (parts from right)
        for i in 1..=9 {
            let placeholder = format!("$-{}", i);
            let idx = parts.len().saturating_sub(i);
            let value = parts.get(idx).copied().unwrap_or("");
            result = result.replace(&placeholder, value);
        }

        // Replace $I (counter)
        result = result.replace("$I", &counter.to_string());

        // Replace $C (creation time)
        if let Some(time) = created {
            result = result.replace("$C", &Self::format_time(time));
        } else {
            result = result.replace("$C", "");
        }

        // Replace $M (modification time)
        if let Some(time) = modified {
            result = result.replace("$M", &Self::format_time(time));
        } else {
            result = result.replace("$M", "");
        }

        result
    }

    /// Format time to YYYYMMDD_HHMMSS string
    fn format_time(time: SystemTime) -> String {
        let datetime: DateTime<Local> = time.into();
        datetime.format("%Y%m%d_%H%M%S").to_string()
    }

    /// Get preview result for example
    pub fn preview(&self, example_name: &str) -> String {
        self.apply(example_name, 1, None, None)
    }

    /// Check if result contains forbidden characters
    pub fn is_valid_result(&self, result: &str) -> bool {
        // Forbidden characters in filenames
        let forbidden = ['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0'];
        !result.is_empty() && !result.chars().any(|c| forbidden.contains(&c))
    }
}

/// Pause state for batch operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseState {
    /// Operation is running
    Running,
    /// Operation is paused by user
    Paused,
}

/// Batch file operation with conflict support
#[derive(Debug, Clone)]
pub struct BatchOperation {
    /// Operation type
    pub operation_type: BatchOperationType,
    /// List of files to process
    pub sources: Vec<PathBuf>,
    /// Target directory
    pub destination: PathBuf,
    /// Whether the original user input explicitly marked destination as a directory
    pub destination_is_directory: bool,
    /// Current index of file being processed
    pub current_index: usize,
    /// Conflict resolution mode
    pub conflict_mode: ConflictMode,
    /// Rename pattern for RenameAll
    pub rename_pattern: Option<RenamePattern>,
    /// Counter for $I variable in pattern
    pub rename_counter: usize,
    /// Statistics: successfully processed
    pub success_count: usize,
    /// Statistics: errors
    pub error_count: usize,
    /// Statistics: skipped
    pub skipped_count: usize,
    /// Pause state for batch operation
    pub pause_state: PauseState,
    /// Paths of successfully copied/moved destinations (for cleanup on cancel)
    pub completed_destinations: Vec<PathBuf>,
    /// Cumulative files completed from previous batch items (for multi-folder downloads)
    pub cumulative_files_completed: usize,
    /// Cumulative bytes completed from previous batch items
    pub cumulative_bytes_completed: u64,
    /// Total files across all batch items (when known)
    pub cumulative_total_files: usize,
    /// Total bytes across all batch items (when known)
    pub cumulative_total_bytes: u64,
}

impl BatchOperation {
    /// Create new batch operation
    pub fn new(
        operation_type: BatchOperationType,
        sources: Vec<PathBuf>,
        destination: PathBuf,
    ) -> Self {
        Self {
            operation_type,
            sources,
            destination,
            destination_is_directory: false,
            current_index: 0,
            conflict_mode: ConflictMode::Ask,
            rename_pattern: None,
            rename_counter: 1,
            success_count: 0,
            error_count: 0,
            skipped_count: 0,
            pause_state: PauseState::Running,
            completed_destinations: Vec::new(),
            cumulative_files_completed: 0,
            cumulative_bytes_completed: 0,
            cumulative_total_files: 0,
            cumulative_total_bytes: 0,
        }
    }

    /// Mark that the original destination input explicitly referred to a directory.
    pub fn with_destination_directory(mut self, destination_is_directory: bool) -> Self {
        self.destination_is_directory = destination_is_directory;
        self
    }

    /// Add a successfully completed destination path
    pub fn add_completed_destination(&mut self, path: PathBuf) {
        self.completed_destinations.push(path);
    }

    /// Set rename pattern
    pub fn set_rename_pattern(&mut self, pattern: RenamePattern) {
        self.rename_pattern = Some(pattern);
    }

    /// Get and increment rename counter
    pub fn get_and_increment_rename_counter(&mut self) -> usize {
        let counter = self.rename_counter;
        self.rename_counter += 1;
        counter
    }

    /// Get current file being processed
    pub fn current_source(&self) -> Option<&PathBuf> {
        self.sources.get(self.current_index)
    }

    /// Check if operation is complete
    pub fn is_complete(&self) -> bool {
        self.current_index >= self.sources.len()
    }

    /// Advance to next file
    pub fn advance(&mut self) {
        self.current_index += 1;
    }

    /// Total number of files
    pub fn total_count(&self) -> usize {
        self.sources.len()
    }

    /// Set conflict resolution mode
    pub fn set_conflict_mode(&mut self, mode: ConflictMode) {
        self.conflict_mode = mode;
    }

    /// Increment success counter
    pub fn increment_success(&mut self) {
        self.success_count += 1;
    }

    /// Increment error counter
    pub fn increment_error(&mut self) {
        self.error_count += 1;
    }

    /// Increment skipped counter
    pub fn increment_skipped(&mut self) {
        self.skipped_count += 1;
    }

    /// Get the last successfully processed source filename
    /// Returns the filename of the file at current_index - 1 if available
    pub fn last_successful_filename(&self) -> Option<String> {
        if self.current_index == 0 || self.success_count == 0 {
            return None;
        }

        // Get the file that was just processed (current_index - 1)
        self.sources
            .get(self.current_index.saturating_sub(1))
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(|s| s.to_string())
    }

    /// Get destination path reference
    pub fn destination_path(&self) -> &PathBuf {
        &self.destination
    }

    /// Whether the destination should be treated as a directory.
    pub fn destination_is_directory(&self) -> bool {
        self.destination_is_directory
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_replacement() {
        let pattern = RenamePattern::new("$0".to_string());
        assert_eq!(pattern.preview("file.txt"), "file.txt");
    }

    #[test]
    fn test_parts_from_left() {
        let pattern = RenamePattern::new("$1_copy.$2".to_string());
        assert_eq!(pattern.preview("document.txt"), "document_copy.txt");
    }

    #[test]
    fn test_parts_from_right() {
        let pattern = RenamePattern::new("$1_backup.$-1".to_string());
        assert_eq!(pattern.preview("archive.tar.gz"), "archive_backup.gz");
    }

    #[test]
    fn test_counter() {
        let pattern = RenamePattern::new("$1_$I.$-1".to_string());
        assert_eq!(pattern.apply("file.txt", 5, None, None), "file_5.txt");
    }

    #[test]
    fn test_complex_pattern() {
        let pattern = RenamePattern::new("$1_$I.$2.$3".to_string());
        assert_eq!(pattern.preview("document.tar.gz"), "document_1.tar.gz");
    }

    #[test]
    fn test_missing_parts() {
        let pattern = RenamePattern::new("$1.$5".to_string());
        assert_eq!(pattern.preview("file.txt"), "file.");
    }

    #[test]
    fn test_validation() {
        let pattern = RenamePattern::new("$1_copy.$-1".to_string());
        assert!(pattern.is_valid_result("file_copy.txt"));
        assert!(!pattern.is_valid_result("file/copy.txt"));
        assert!(!pattern.is_valid_result("file:copy.txt"));
        assert!(!pattern.is_valid_result(""));
    }

    #[test]
    fn test_batch_operation_new() {
        let op = BatchOperation::new(
            BatchOperationType::Copy,
            vec![PathBuf::from("/a"), PathBuf::from("/b")],
            PathBuf::from("/dest"),
        );
        assert_eq!(op.total_count(), 2);
        assert!(!op.is_complete());
        assert!(!op.destination_is_directory());
    }

    #[test]
    fn test_batch_operation_lifecycle() {
        let mut op = BatchOperation::new(
            BatchOperationType::Copy,
            vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c"),
            ],
            PathBuf::from("/dest"),
        );

        assert_eq!(op.total_count(), 3);
        assert!(!op.is_complete());
        assert_eq!(op.current_source(), Some(&PathBuf::from("/a")));

        op.advance();
        op.increment_success();
        assert_eq!(op.current_index, 1);
        assert_eq!(op.success_count, 1);

        op.advance();
        op.increment_error();

        op.advance();
        op.increment_skipped();

        assert!(op.is_complete());
        assert_eq!(op.success_count, 1);
        assert_eq!(op.error_count, 1);
        assert_eq!(op.skipped_count, 1);
    }

    #[test]
    fn test_batch_operation_completed_destinations() {
        let mut op = BatchOperation::new(
            BatchOperationType::Move,
            vec![PathBuf::from("/a")],
            PathBuf::from("/dest"),
        );

        op.add_completed_destination(PathBuf::from("/dest/a"));
        assert_eq!(op.completed_destinations.len(), 1);
    }

    #[test]
    fn test_batch_operation_rename_counter() {
        let mut op = BatchOperation::new(
            BatchOperationType::Copy,
            vec![PathBuf::from("/a")],
            PathBuf::from("/dest"),
        );

        assert_eq!(op.get_and_increment_rename_counter(), 1);
        assert_eq!(op.get_and_increment_rename_counter(), 2);
        assert_eq!(op.get_and_increment_rename_counter(), 3);
    }

    #[test]
    fn test_batch_operation_last_successful_filename() {
        let mut op = BatchOperation::new(
            BatchOperationType::Copy,
            vec![PathBuf::from("/a/file.txt"), PathBuf::from("/b/other.rs")],
            PathBuf::from("/dest"),
        );

        // Before any processing
        assert_eq!(op.last_successful_filename(), None);

        // After processing first file
        op.advance();
        op.increment_success();
        assert_eq!(op.last_successful_filename(), Some("file.txt".to_string()));
    }

    #[test]
    fn test_batch_operation_pause_state() {
        let op = BatchOperation::new(
            BatchOperationType::Copy,
            vec![PathBuf::from("/a")],
            PathBuf::from("/dest"),
        );
        assert_eq!(op.pause_state, PauseState::Running);
    }

    #[test]
    fn test_batch_operation_destination_directory_intent() {
        let op = BatchOperation::new(
            BatchOperationType::Copy,
            vec![PathBuf::from("/a")],
            PathBuf::from("/dest"),
        )
        .with_destination_directory(true);
        assert!(op.destination_is_directory());
    }

    #[test]
    fn test_rename_pattern_no_extension() {
        let pattern = RenamePattern::new("$1_backup".to_string());
        assert_eq!(pattern.preview("README"), "README_backup");
    }

    #[test]
    fn test_rename_pattern_multiple_extensions() {
        let pattern = RenamePattern::new("$1.$2.$-1".to_string());
        assert_eq!(pattern.preview("file.tar.gz"), "file.tar.gz");
    }
}
