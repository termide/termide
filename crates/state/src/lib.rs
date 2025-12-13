//! State types and data structures for termide.
//!
//! This crate contains pure data types used throughout the application,
//! without dependencies on specific implementations.

use chrono::{DateTime, Local};
use std::path::PathBuf;
use std::time::SystemTime;

/// Message about background directory size calculation result
#[derive(Debug)]
pub struct DirSizeResult {
    pub size: u64,
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

/// Layout mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// Single panel mode (width 1-80)
    Single,
    /// Multi-panel mode (width > 100)
    MultiPanel,
}

/// Layout information
#[derive(Debug, Clone)]
pub struct LayoutInfo {
    /// Layout mode
    pub mode: LayoutMode,
    /// Number of main panels
    pub main_panels_count: usize,
    /// Width of one main panel
    #[allow(dead_code)]
    pub main_panel_width: u16,
}

impl LayoutInfo {
    /// Calculate layout based on terminal width
    pub fn calculate(width: u16) -> Self {
        use termide_config::constants::*;

        if width <= MIN_WIDTH_MULTI_PANEL {
            // Single panel mode for narrow terminals
            Self {
                mode: LayoutMode::Single,
                main_panels_count: 1,
                main_panel_width: width,
            }
        } else {
            // Multi-panel mode
            let main_panels_count = (width / MIN_MAIN_PANEL_WIDTH).max(1) as usize;
            let main_panel_width = width / main_panels_count as u16;

            Self {
                mode: LayoutMode::MultiPanel,
                main_panels_count,
                main_panel_width,
            }
        }
    }

    /// Get recommended layout description
    pub fn recommended_layout_str(&self) -> &'static str {
        match self.mode {
            LayoutMode::Single => "Single panel",
            LayoutMode::MultiPanel => match self.main_panels_count {
                1 => "1 panel",
                2 => "2 panels",
                3 => "3 panels",
                4 => "4 panels",
                5 => "5 panels",
                6 => "6 panels",
                7 => "7 panels",
                8 => "8 panels",
                9 => "9 panels",
                _ => "Many panels",
            },
        }
    }
}

/// UI components state
#[derive(Debug, Default)]
pub struct UiState {
    /// Is menu open
    pub menu_open: bool,
    /// Selected menu item (None if menu closed)
    pub selected_menu_item: Option<usize>,
    /// Selected item in dropdown list
    pub selected_dropdown_item: usize,
    /// Status line message (for displaying errors and notifications)
    pub status_message: Option<(String, bool)>, // (message, is_error)
}

/// Terminal state (dimensions)
#[derive(Debug, Clone, Copy)]
pub struct TerminalState {
    /// Terminal width
    pub width: u16,
    /// Terminal height
    pub height: u16,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            width: 80,
            height: 24,
        }
    }
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
        let parts = Self::split_filename(original_name);
        let mut result = self.template.clone();

        // Replace $0 (full name)
        result = result.replace("$0", original_name);

        // Replace $1-9 (parts from left)
        for i in 1..=9 {
            let placeholder = format!("${}", i);
            let value = parts.get(i - 1).map(|s| s.as_str()).unwrap_or("");
            result = result.replace(&placeholder, value);
        }

        // Replace $-1 to $-9 (parts from right)
        for i in 1..=9 {
            let placeholder = format!("$-{}", i);
            let idx = parts.len().saturating_sub(i);
            let value = parts.get(idx).map(|s| s.as_str()).unwrap_or("");
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

    /// Split filename into parts by dots
    fn split_filename(filename: &str) -> Vec<String> {
        filename.split('.').map(|s| s.to_string()).collect()
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

/// Batch file operation with conflict support
#[derive(Debug, Clone)]
pub struct BatchOperation {
    /// Operation type
    pub operation_type: BatchOperationType,
    /// List of files to process
    pub sources: Vec<PathBuf>,
    /// Target directory
    pub destination: PathBuf,
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
            current_index: 0,
            conflict_mode: ConflictMode::Ask,
            rename_pattern: None,
            rename_counter: 1,
            success_count: 0,
            error_count: 0,
            skipped_count: 0,
        }
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
}

/// Action pending modal result
#[derive(Debug, Clone)]
pub enum PendingAction {
    /// Create new file in specified directory
    CreateFile {
        panel_index: usize,
        directory: PathBuf,
    },
    /// Create new directory in specified directory
    CreateDirectory {
        panel_index: usize,
        directory: PathBuf,
    },
    /// Delete files/directories (one or multiple)
    DeletePath {
        panel_index: usize,
        paths: Vec<PathBuf>,
    },
    /// Copy files/directories (one or multiple)
    CopyPath {
        panel_index: usize,
        sources: Vec<PathBuf>,
        target_directory: Option<PathBuf>,
    },
    /// Move files/directories (one or multiple)
    MovePath {
        panel_index: usize,
        sources: Vec<PathBuf>,
        target_directory: Option<PathBuf>,
    },
    /// Save unnamed file (Save As)
    SaveFileAs {
        panel_index: usize,
        directory: PathBuf,
    },
    /// Close panel (with confirmation if there are unsaved changes)
    ClosePanel { panel_index: usize },
    /// Close editor with choice: save, don't save, cancel
    CloseEditorWithSave { panel_index: usize },
    /// Close editor with external changes (file changed on disk)
    CloseEditorExternal { panel_index: usize },
    /// Close editor with conflict (local changes + external changes)
    CloseEditorConflict { panel_index: usize },
    /// File overwrite decision when copying/moving
    #[allow(dead_code)]
    OverwriteDecision {
        panel_index: usize,
        source: PathBuf,
        destination: PathBuf,
        is_move: bool, // true for move, false for copy
    },
    /// Batch file operation (copy/move)
    #[allow(dead_code)]
    BatchFileOperation { operation: BatchOperation },
    /// Continue batch operation after conflict resolution
    ContinueBatchOperation { operation: BatchOperation },
    /// Request rename pattern and apply to file
    RenameWithPattern {
        operation: BatchOperation,
        original_name: String,
    },
    /// Text search in editor
    Search,
    /// Text replace in editor
    Replace,
    /// Switch to next panel
    NextPanel,
    /// Switch to previous panel
    PrevPanel,
    /// Quit application (with confirmation if there are unsaved changes)
    QuitApplication,
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
    }
}
