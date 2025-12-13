//! Modal result handling and batch operation processing for termide.
//!
//! This crate provides:
//! - `ModalResultHandler` trait for processing modal results
//! - `BatchOperationState` state machine for file operations
//! - `BatchOperationProcessor` for managing batch operation workflows
//!
//! # Architecture
//!
//! Modal results flow through a well-defined state machine:
//!
//! ```text
//! User Action → Modal → Result → Handler → Commands
//!                                    ↓
//!                         BatchOperationProcessor
//!                                    ↓
//!                         State Machine Transitions
//! ```

use std::path::PathBuf;

use anyhow::Result;

use termide_app_core::AppCommand;
use termide_state::{BatchOperation, BatchOperationType, ConflictMode, RenamePattern};

// ============================================================================
// Modal Result Types
// ============================================================================

/// Result from a confirmation modal (Yes/No).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmResult {
    pub confirmed: bool,
}

/// Result from an input modal (text entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputResult {
    pub value: String,
}

/// Result from a select modal (single/multi selection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectResult {
    /// Selected indices (single selection has one element)
    pub indices: Vec<usize>,
}

/// Result from a conflict resolution modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictResult {
    /// Overwrite the existing file
    Overwrite,
    /// Skip this file
    Skip,
    /// Overwrite all remaining conflicts
    OverwriteAll,
    /// Skip all remaining conflicts
    SkipAll,
    /// Rename this file (custom pattern)
    Rename,
    /// Cancel the entire operation
    Cancel,
}

// ============================================================================
// Batch Operation State Machine
// ============================================================================

/// State of a batch file operation.
///
/// Represents the current state of a multi-file copy/move operation,
/// including conflict handling.
#[derive(Debug, Clone, Default)]
pub enum BatchOperationState {
    /// No operation in progress
    #[default]
    Idle,

    /// Waiting for destination directory selection
    AwaitingDestination {
        /// Source files to process
        sources: Vec<PathBuf>,
        /// Operation type (Copy or Move)
        op_type: BatchOperationType,
    },

    /// Waiting for conflict resolution
    AwaitingConflict {
        /// The ongoing batch operation
        operation: BatchOperation,
        /// Path that has a conflict
        conflict_path: PathBuf,
    },

    /// Waiting for rename pattern input
    AwaitingRename {
        /// The ongoing batch operation
        operation: BatchOperation,
        /// Original filename for preview
        original_name: String,
    },

    /// Operation is in progress (processing files)
    InProgress {
        /// The ongoing batch operation
        operation: BatchOperation,
    },

    /// Operation completed
    Completed {
        /// Final operation state with statistics
        operation: BatchOperation,
    },
}

impl BatchOperationState {
    /// Check if state is idle (no operation in progress).
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    /// Check if state is waiting for user input.
    pub fn is_awaiting_input(&self) -> bool {
        matches!(
            self,
            Self::AwaitingDestination { .. }
                | Self::AwaitingConflict { .. }
                | Self::AwaitingRename { .. }
        )
    }

    /// Check if operation is in progress.
    pub fn is_in_progress(&self) -> bool {
        matches!(self, Self::InProgress { .. })
    }

    /// Check if operation is completed.
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }

    /// Get operation if available.
    pub fn operation(&self) -> Option<&BatchOperation> {
        match self {
            Self::AwaitingConflict { operation, .. }
            | Self::AwaitingRename { operation, .. }
            | Self::InProgress { operation }
            | Self::Completed { operation } => Some(operation),
            _ => None,
        }
    }
}

// ============================================================================
// Batch Operation Processor
// ============================================================================

/// Result of processing a batch operation step.
#[derive(Debug, Clone)]
pub enum ProcessResult {
    /// Continue to next file
    Continue,
    /// Need to show conflict modal for this path
    NeedsConflictResolution { path: PathBuf },
    /// Need rename pattern input
    NeedsRenamePattern { original_name: String },
    /// Operation completed
    Completed,
    /// Operation cancelled
    Cancelled,
    /// Error occurred
    Error { message: String },
}

/// Processor for batch file operations.
///
/// Manages the state machine for multi-file copy/move operations,
/// handling conflicts and producing commands for the orchestrator.
#[derive(Debug, Default)]
pub struct BatchOperationProcessor {
    state: BatchOperationState,
}

impl BatchOperationProcessor {
    /// Create a new processor.
    pub fn new() -> Self {
        Self {
            state: BatchOperationState::Idle,
        }
    }

    /// Get current state.
    pub fn state(&self) -> &BatchOperationState {
        &self.state
    }

    /// Start a new batch operation.
    ///
    /// Transitions from Idle to AwaitingDestination.
    pub fn start(&mut self, sources: Vec<PathBuf>, op_type: BatchOperationType) -> Result<()> {
        if !self.state.is_idle() {
            anyhow::bail!("Operation already in progress");
        }

        self.state = BatchOperationState::AwaitingDestination { sources, op_type };
        Ok(())
    }

    /// Set destination and begin processing.
    ///
    /// Transitions from AwaitingDestination to InProgress.
    pub fn set_destination(&mut self, destination: PathBuf) -> Result<()> {
        let (sources, op_type) = match &self.state {
            BatchOperationState::AwaitingDestination { sources, op_type } => {
                (sources.clone(), *op_type)
            }
            _ => anyhow::bail!("Not awaiting destination"),
        };

        let operation = BatchOperation::new(op_type, sources, destination);
        self.state = BatchOperationState::InProgress { operation };
        Ok(())
    }

    /// Process the current file in the batch.
    ///
    /// Returns what action the orchestrator should take next.
    pub fn process_current<F>(&mut self, check_conflict: F) -> ProcessResult
    where
        F: Fn(&PathBuf, &PathBuf) -> bool,
    {
        let operation = match &mut self.state {
            BatchOperationState::InProgress { operation } => operation,
            _ => {
                return ProcessResult::Error {
                    message: "No operation in progress".to_string(),
                }
            }
        };

        // Check if operation is complete
        if operation.is_complete() {
            let op = operation.clone();
            self.state = BatchOperationState::Completed { operation: op };
            return ProcessResult::Completed;
        }

        // Get current source
        let source = match operation.current_source() {
            Some(s) => s.clone(),
            None => {
                let op = operation.clone();
                self.state = BatchOperationState::Completed { operation: op };
                return ProcessResult::Completed;
            }
        };

        // Check for conflict
        let dest_path = operation.destination.join(
            source
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("")),
        );

        if check_conflict(&source, &dest_path) {
            // Check conflict mode
            match operation.conflict_mode {
                ConflictMode::Ask => {
                    let op = operation.clone();
                    let result_path = dest_path.clone();
                    self.state = BatchOperationState::AwaitingConflict {
                        operation: op,
                        conflict_path: dest_path,
                    };
                    return ProcessResult::NeedsConflictResolution { path: result_path };
                }
                ConflictMode::OverwriteAll => {
                    // Will overwrite, continue
                }
                ConflictMode::SkipAll => {
                    // Skip this file
                    operation.increment_skipped();
                    operation.advance();
                    return ProcessResult::Continue;
                }
            }
        }

        ProcessResult::Continue
    }

    /// Handle conflict resolution result.
    pub fn resolve_conflict(&mut self, result: ConflictResult) -> ProcessResult {
        let (operation, conflict_path) = match &self.state {
            BatchOperationState::AwaitingConflict {
                operation,
                conflict_path,
            } => (operation.clone(), conflict_path.clone()),
            _ => {
                return ProcessResult::Error {
                    message: "Not awaiting conflict resolution".to_string(),
                }
            }
        };

        let mut operation = operation;

        match result {
            ConflictResult::Overwrite => {
                // Continue with overwrite
                self.state = BatchOperationState::InProgress { operation };
                ProcessResult::Continue
            }
            ConflictResult::Skip => {
                operation.increment_skipped();
                operation.advance();
                self.state = BatchOperationState::InProgress { operation };
                ProcessResult::Continue
            }
            ConflictResult::OverwriteAll => {
                operation.set_conflict_mode(ConflictMode::OverwriteAll);
                self.state = BatchOperationState::InProgress { operation };
                ProcessResult::Continue
            }
            ConflictResult::SkipAll => {
                operation.set_conflict_mode(ConflictMode::SkipAll);
                operation.increment_skipped();
                operation.advance();
                self.state = BatchOperationState::InProgress { operation };
                ProcessResult::Continue
            }
            ConflictResult::Rename => {
                let original_name = conflict_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("file")
                    .to_string();
                self.state = BatchOperationState::AwaitingRename {
                    operation,
                    original_name: original_name.clone(),
                };
                ProcessResult::NeedsRenamePattern { original_name }
            }
            ConflictResult::Cancel => {
                self.state = BatchOperationState::Idle;
                ProcessResult::Cancelled
            }
        }
    }

    /// Apply rename pattern and continue.
    pub fn apply_rename(&mut self, pattern: RenamePattern) -> ProcessResult {
        let operation = match &self.state {
            BatchOperationState::AwaitingRename { operation, .. } => operation.clone(),
            _ => {
                return ProcessResult::Error {
                    message: "Not awaiting rename".to_string(),
                }
            }
        };

        let mut operation = operation;
        operation.set_rename_pattern(pattern);
        self.state = BatchOperationState::InProgress { operation };
        ProcessResult::Continue
    }

    /// Mark current file as processed successfully.
    pub fn mark_success(&mut self) {
        if let BatchOperationState::InProgress { operation } = &mut self.state {
            operation.increment_success();
            operation.advance();
        }
    }

    /// Mark current file as failed.
    pub fn mark_error(&mut self) {
        if let BatchOperationState::InProgress { operation } = &mut self.state {
            operation.increment_error();
            operation.advance();
        }
    }

    /// Cancel the operation.
    pub fn cancel(&mut self) {
        self.state = BatchOperationState::Idle;
    }

    /// Reset to idle state.
    pub fn reset(&mut self) {
        self.state = BatchOperationState::Idle;
    }

    /// Get operation statistics.
    pub fn statistics(&self) -> Option<(usize, usize, usize, usize)> {
        self.state.operation().map(|op| {
            (
                op.success_count,
                op.error_count,
                op.skipped_count,
                op.total_count(),
            )
        })
    }
}

// ============================================================================
// Modal Result Handler Trait
// ============================================================================

/// Trait for handling modal dialog results.
///
/// Implementations process results from different modal types
/// and produce commands for the application orchestrator.
pub trait ModalResultHandler {
    /// Handle confirmation result (Yes/No dialog).
    fn handle_confirm(&mut self, action: &str, result: ConfirmResult) -> Result<Vec<AppCommand>>;

    /// Handle input result (text entry dialog).
    fn handle_input(&mut self, action: &str, result: InputResult) -> Result<Vec<AppCommand>>;

    /// Handle selection result (select dialog).
    fn handle_select(&mut self, action: &str, result: SelectResult) -> Result<Vec<AppCommand>>;

    /// Handle conflict resolution result.
    fn handle_conflict(&mut self, result: ConflictResult) -> Result<Vec<AppCommand>>;
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_sources() -> Vec<PathBuf> {
        vec![
            PathBuf::from("/src/file1.txt"),
            PathBuf::from("/src/file2.txt"),
            PathBuf::from("/src/file3.txt"),
        ]
    }

    #[test]
    fn test_batch_operation_state_default() {
        let state = BatchOperationState::default();
        assert!(state.is_idle());
        assert!(!state.is_in_progress());
        assert!(!state.is_completed());
    }

    #[test]
    fn test_processor_start() {
        let mut processor = BatchOperationProcessor::new();
        assert!(processor.state().is_idle());

        processor
            .start(test_sources(), BatchOperationType::Copy)
            .unwrap();

        assert!(matches!(
            processor.state(),
            BatchOperationState::AwaitingDestination { .. }
        ));
    }

    #[test]
    fn test_processor_set_destination() {
        let mut processor = BatchOperationProcessor::new();
        processor
            .start(test_sources(), BatchOperationType::Copy)
            .unwrap();

        processor.set_destination(PathBuf::from("/dest")).unwrap();

        assert!(processor.state().is_in_progress());
    }

    #[test]
    fn test_processor_no_conflicts() {
        let mut processor = BatchOperationProcessor::new();
        processor
            .start(test_sources(), BatchOperationType::Copy)
            .unwrap();
        processor.set_destination(PathBuf::from("/dest")).unwrap();

        // Process with no conflicts
        let result = processor.process_current(|_, _| false);
        assert!(matches!(result, ProcessResult::Continue));

        processor.mark_success();

        // Continue processing
        let result = processor.process_current(|_, _| false);
        assert!(matches!(result, ProcessResult::Continue));

        processor.mark_success();

        let result = processor.process_current(|_, _| false);
        assert!(matches!(result, ProcessResult::Continue));

        processor.mark_success();

        // Should complete
        let result = processor.process_current(|_, _| false);
        assert!(matches!(result, ProcessResult::Completed));

        assert!(processor.state().is_completed());
    }

    #[test]
    fn test_processor_with_conflict() {
        let mut processor = BatchOperationProcessor::new();
        processor
            .start(
                vec![PathBuf::from("/src/file.txt")],
                BatchOperationType::Copy,
            )
            .unwrap();
        processor.set_destination(PathBuf::from("/dest")).unwrap();

        // Process with conflict
        let result = processor.process_current(|_, _| true);
        assert!(matches!(
            result,
            ProcessResult::NeedsConflictResolution { .. }
        ));

        // Resolve with skip
        let result = processor.resolve_conflict(ConflictResult::Skip);
        assert!(matches!(result, ProcessResult::Continue));

        // Should complete after skip
        let result = processor.process_current(|_, _| false);
        assert!(matches!(result, ProcessResult::Completed));
    }

    #[test]
    fn test_processor_overwrite_all() {
        let mut processor = BatchOperationProcessor::new();
        processor
            .start(test_sources(), BatchOperationType::Move)
            .unwrap();
        processor.set_destination(PathBuf::from("/dest")).unwrap();

        // First file has conflict
        let result = processor.process_current(|_, _| true);
        assert!(matches!(
            result,
            ProcessResult::NeedsConflictResolution { .. }
        ));

        // Choose overwrite all
        let result = processor.resolve_conflict(ConflictResult::OverwriteAll);
        assert!(matches!(result, ProcessResult::Continue));

        processor.mark_success();

        // Next files should auto-overwrite (no more conflict dialogs)
        let result = processor.process_current(|_, _| true);
        assert!(matches!(result, ProcessResult::Continue));
    }

    #[test]
    fn test_processor_cancel() {
        let mut processor = BatchOperationProcessor::new();
        processor
            .start(test_sources(), BatchOperationType::Copy)
            .unwrap();
        processor.set_destination(PathBuf::from("/dest")).unwrap();

        processor.cancel();
        assert!(processor.state().is_idle());
    }

    #[test]
    fn test_processor_statistics() {
        let mut processor = BatchOperationProcessor::new();
        processor
            .start(test_sources(), BatchOperationType::Copy)
            .unwrap();
        processor.set_destination(PathBuf::from("/dest")).unwrap();

        // Process all without conflicts
        for _ in 0..3 {
            let _ = processor.process_current(|_, _| false);
            processor.mark_success();
        }

        let stats = processor.statistics();
        assert!(stats.is_some());
        let (success, error, skipped, total) = stats.unwrap();
        assert_eq!(success, 3);
        assert_eq!(error, 0);
        assert_eq!(skipped, 0);
        assert_eq!(total, 3);
    }

    #[test]
    fn test_conflict_result_variants() {
        assert_eq!(ConflictResult::Overwrite, ConflictResult::Overwrite);
        assert_ne!(ConflictResult::Overwrite, ConflictResult::Skip);
    }

    #[test]
    fn test_confirm_result() {
        let result = ConfirmResult { confirmed: true };
        assert!(result.confirmed);
    }

    #[test]
    fn test_input_result() {
        let result = InputResult {
            value: "test".to_string(),
        };
        assert_eq!(result.value, "test");
    }

    #[test]
    fn test_select_result() {
        let result = SelectResult {
            indices: vec![0, 2],
        };
        assert_eq!(result.indices.len(), 2);
    }
}
