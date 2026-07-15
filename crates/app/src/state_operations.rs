//! Operation-manager and Operations-panel tracking methods for `AppState`.

use std::sync::Arc;

use termide_file_ops::{
    BackgroundOperationSummary, OperationEvent, OperationManager, OperationRequest,
};
use termide_state::{ActiveOperation, OperationType};
use termide_vfs::VfsManager;

use crate::state::AppState;

impl AppState {
    // ========================================================================
    // Operation Manager Methods
    // ========================================================================

    /// Initialize the operation manager with a VFS manager.
    /// This should be called when the first VFS operation is needed.
    pub fn init_operation_manager(&mut self, vfs_manager: Arc<VfsManager>) {
        if self.operation_manager.is_none() {
            self.operation_manager = Some(OperationManager::new(vfs_manager));
        }
    }

    /// Get reference to operation manager if initialized.
    pub fn operation_manager(&self) -> Option<&OperationManager> {
        self.operation_manager.as_ref()
    }

    /// Get mutable reference to operation manager if initialized.
    pub fn operation_manager_mut(&mut self) -> Option<&mut OperationManager> {
        self.operation_manager.as_mut()
    }

    /// Queue a file operation. Returns the operation ID if successful.
    /// Initializes the operation manager with the provided VFS manager if needed.
    pub fn queue_operation(
        &mut self,
        request: OperationRequest,
        vfs_manager: Arc<VfsManager>,
    ) -> Result<termide_file_ops::OperationId, termide_file_ops::OperationError> {
        self.init_operation_manager(vfs_manager.clone());
        let mgr = self
            .operation_manager_mut()
            .expect("operation_manager just initialized");
        mgr.set_vfs_manager(vfs_manager);
        mgr.queue_operation(request)
    }

    /// Start a file operation immediately (bypassing the queue).
    /// Initializes the operation manager with the provided VFS manager if needed.
    pub fn start_operation_now(
        &mut self,
        request: OperationRequest,
        vfs_manager: Arc<VfsManager>,
    ) -> Result<termide_file_ops::OperationId, termide_file_ops::OperationError> {
        self.init_operation_manager(vfs_manager.clone());
        let mgr = self
            .operation_manager_mut()
            .expect("operation_manager just initialized");
        mgr.set_vfs_manager(vfs_manager);
        mgr.start_now(request)
    }

    /// Poll operation manager for events. Returns empty vec if not initialized.
    pub fn poll_operations(&mut self) -> Vec<OperationEvent> {
        self.operation_manager_mut()
            .map(|m| m.poll())
            .unwrap_or_default()
    }

    /// Check if there are any active or queued operations.
    pub fn has_pending_operations(&self) -> bool {
        self.operation_manager()
            .map(|m| m.has_operations())
            .unwrap_or(false)
    }

    /// Get summary of background operations for status bar display.
    pub fn background_operations_summary(&self) -> Option<BackgroundOperationSummary> {
        self.operation_manager()
            .map(|m| m.background_summary())
            .filter(|s| s.has_operations())
    }

    /// Resolve a conflict for an operation waiting for user decision.
    pub fn resolve_operation_conflict(
        &mut self,
        operation_id: termide_file_ops::OperationId,
        resolution: termide_file_ops::ConflictResolution,
    ) -> bool {
        self.operation_manager_mut()
            .map(|m| m.resolve_conflict(operation_id, resolution))
            .unwrap_or(false)
    }

    // ========================================================================
    // Active Operations Panel Methods
    // ========================================================================

    /// Start tracking a new operation in the Operations panel.
    pub fn track_operation(
        &mut self,
        id: termide_file_ops::OperationId,
        op_type: OperationType,
        source: String,
        dest: String,
        total_files: usize,
        total_bytes: u64,
    ) {
        let op = ActiveOperation::new(id, op_type, source, dest, total_files, total_bytes);
        self.active_operations.insert(id, op);
        self.operations_panel_dirty = true;
    }

    /// Get operations list sorted by start time (newest first).
    pub fn operations_list(&self) -> Vec<&ActiveOperation> {
        let mut ops: Vec<_> = self.active_operations.values().collect();
        ops.sort_by_key(|op| std::cmp::Reverse(op.started_at));
        ops
    }

    /// Check if there are any active operations being tracked.
    pub fn has_active_operations(&self) -> bool {
        !self.active_operations.is_empty()
    }

    /// Remove an operation from tracking (e.g., when completed/cancelled).
    pub fn untrack_operation(&mut self, id: termide_file_ops::OperationId) {
        if self.active_operations.remove(&id).is_some() {
            self.operations_panel_dirty = true;
        }
    }

    /// Start tracking a batch operation.
    /// Returns synthetic OperationId for the batch.
    pub fn start_batch_tracking(
        &mut self,
        op_type: OperationType,
        source: String,
        dest: String,
        total_files: usize,
        total_bytes: u64,
    ) -> termide_file_ops::OperationId {
        // Generate synthetic ID (wraps around if exhausted, which is practically impossible)
        self.batch.id_counter = self.batch.id_counter.wrapping_add(1);
        let batch_id = termide_file_ops::OperationId::new(self.batch.id_counter);

        // Create tracked operation
        self.track_operation(batch_id, op_type, source, dest, total_files, total_bytes);
        self.batch.tracking_id = Some(batch_id);

        batch_id
    }

    /// Generate a synthetic OperationId for non-OperationManager use (commands, etc.).
    pub fn next_synthetic_operation_id(&mut self) -> termide_file_ops::OperationId {
        self.batch.id_counter = self.batch.id_counter.wrapping_add(1);
        termide_file_ops::OperationId::new(self.batch.id_counter)
    }

    /// Finish batch tracking - remove the batch operation from active_operations.
    pub fn finish_batch_tracking(&mut self) {
        if let Some(batch_id) = self.batch.tracking_id.take() {
            if self.active_operations.remove(&batch_id).is_some() {
                self.operations_panel_dirty = true;
            }
        }
        self.batch.sub_operation_id = None;
    }

    /// Update batch tracked operation file-level progress.
    /// Only updates file counts; byte-level progress is managed by poll_operation_manager.
    pub fn update_batch_progress(&mut self, files_completed: usize, total_files: usize) {
        if let Some(batch_id) = self.batch.tracking_id {
            if let Some(op) = self.active_operations.get_mut(&batch_id) {
                op.progress.files_completed = files_completed;
                op.progress.total_files = total_files;
            }
        }
    }

    /// Set batch tracked operation pause state.
    pub fn set_batch_paused(&mut self, paused: bool) {
        if let Some(batch_id) = self.batch.tracking_id {
            if let Some(op) = self.active_operations.get_mut(&batch_id) {
                op.is_paused = paused;
                if paused {
                    op.speed_tracker.reset();
                }
            }
        }
    }
}
