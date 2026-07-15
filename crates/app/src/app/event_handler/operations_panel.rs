//! Operations-panel event handlers and lifecycle helpers.

#![allow(deprecated)]

use anyhow::Result;

use crate::app::App;
use crate::state::PendingAction;

impl App {
    // ========================================================================
    // Operations Panel Methods
    // ========================================================================

    /// Start a tracked operation with auto-opening of Operations panel.
    /// This wraps start_operation_now and adds tracking + panel opening.
    #[allow(clippy::too_many_arguments)]
    pub fn start_tracked_operation(
        &mut self,
        request: termide_file_ops::OperationRequest,
        vfs_manager: std::sync::Arc<termide_vfs::VfsManager>,
        op_type: crate::state::OperationType,
        source: String,
        dest: String,
        total_files: usize,
        total_bytes: u64,
    ) -> anyhow::Result<termide_file_ops::OperationId> {
        // Start the operation
        let operation_id = self.state.start_operation_now(request, vfs_manager)?;

        // Track the operation
        self.state.track_operation(
            operation_id,
            op_type,
            source,
            dest,
            total_files,
            total_bytes,
        );

        // Open the operations panel with focus on the new operation
        let _ = self.open_operations_panel_with_focus(operation_id);

        Ok(operation_id)
    }

    /// Update operations panel data before rendering.
    /// Called from render loop to sync panel with active_operations.
    pub fn update_operations_panel(&mut self) {
        let has_ops = self.state.has_active_operations();
        // Skip if no operations and panel was already synced empty
        if !has_ops && !self.state.operations_panel_dirty {
            return;
        }
        if !has_ops {
            // All operations finished — close the panel entirely
            self.state.operations_panel_dirty = false;
            self.state.last_operations_elapsed_redraw = None;
            self.close_operations_panel();
            return;
        }
        self.state.operations_panel_dirty = true;
        // Force redraw every 1s to update elapsed time display in operation cards
        let should_redraw = self
            .state
            .last_operations_elapsed_redraw
            .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(1));
        if should_redraw {
            self.state.last_operations_elapsed_redraw = Some(std::time::Instant::now());
            self.state.needs_redraw = true;
        }
        // Find operations panel and update its data
        for group in &mut self.layout_manager.panel_groups {
            for panel in group.panels_mut() {
                if let Some(ops_panel) = panel
                    .as_any_mut()
                    .downcast_mut::<termide_panel_operations::OperationsPanel>()
                {
                    let ops_list = self.state.operations_list();
                    ops_panel.update_operations(&ops_list);
                    return;
                }
            }
        }
    }

    // ========================================================================
    // Operations Panel Event Handlers
    // ========================================================================

    /// Handle ToggleOperationPause event - pause or resume an operation
    pub(super) fn event_toggle_operation_pause(&mut self, op_id: termide_file_ops::OperationId) {
        // Check if operation is paused
        let is_paused = self
            .state
            .active_operations
            .get(&op_id)
            .map(|op| op.is_paused)
            .unwrap_or(false);

        // Resolve batch tracking ID to actual OperationManager sub-operation ID
        let real_id = if self.state.batch.tracking_id == Some(op_id) {
            self.state.batch.sub_operation_id.unwrap_or(op_id)
        } else {
            op_id
        };

        if let Some(manager) = self.state.operation_manager_mut() {
            if is_paused {
                manager.resume(real_id);
            } else {
                manager.pause(real_id);
            }
        }

        // Update is_paused on the ActiveOperation so the card icon
        // and the per-operation menu pick up the new state. Cover both
        // single-file ops and batch ops — set_batch_paused only fires
        // when the targeted id is the batch tracking id, which left
        // standalone uploads with stale is_paused=false.
        if let Some(op) = self.state.active_operations.get_mut(&op_id) {
            op.is_paused = !is_paused;
            if !is_paused {
                op.speed_tracker.reset();
            }
        }
        // Update batch tracking paused state (UI card)
        self.state.set_batch_paused(!is_paused);

        // Also sync pause state into the pending BatchOperation so that
        // process_batch_operation() won't start the next sub-op while paused.
        if self.state.batch.tracking_id == Some(op_id) {
            if let Some(PendingAction::ContinueBatchOperation { ref mut operation }) =
                self.state.pending_action
            {
                operation.pause_state = if !is_paused {
                    termide_state::PauseState::Paused
                } else {
                    termide_state::PauseState::Running
                };
            }
        }

        self.state.needs_redraw = true;
    }

    /// Handle CancelOperation event - cancel an operation
    pub(in crate::app) fn event_cancel_operation(&mut self, op_id: termide_file_ops::OperationId) {
        // Check if this is a command operation (not managed by OperationManager)
        if let Some(op) = self.state.active_operations.get(&op_id) {
            if op.op_type.is_command() {
                // Kill the process and remove from tracking
                use crate::state::kill_process_tree;

                // Kill bg_command process if present
                if let Some(pos) = self
                    .state
                    .bg_command_handles
                    .iter()
                    .position(|(id, _, _)| *id == op_id)
                {
                    let (_, _, pid) = self.state.bg_command_handles.remove(pos);
                    kill_process_tree(pid);
                }

                // Kill report command process if it matches
                if let Some(pos) = self
                    .state
                    .command_operation_handles
                    .iter()
                    .position(|h| h.operation_id == Some(op_id))
                {
                    let handle = self.state.command_operation_handles.remove(pos);
                    if let Some(pid) = handle.pid {
                        kill_process_tree(pid);
                    }
                }

                self.state.untrack_operation(op_id);
                self.state.needs_redraw = true;
                return;
            }
        }

        // Resolve batch tracking ID to actual OperationManager sub-operation ID
        let real_id = if self.state.batch.tracking_id == Some(op_id) {
            self.state.batch.sub_operation_id.unwrap_or(op_id)
        } else {
            op_id
        };

        if let Some(manager) = self.state.operation_manager_mut() {
            manager.cancel(real_id);
        }
        self.state.needs_redraw = true;
    }

    /// Open or expand the Operations panel without stealing focus.
    /// The panel is inserted right after the currently expanded panel in the accordion,
    /// so when it closes, the previous panel will naturally be shown again.
    pub(in crate::app) fn open_operations_panel(&mut self) -> Result<()> {
        use termide_panel_operations::OperationsPanel;

        // Check if operations panel already exists — expand it without changing focus
        for (group_idx, group) in self.layout_manager.panel_groups.iter().enumerate() {
            for (panel_idx, panel) in group.panels().iter().enumerate() {
                if panel.name() == "operations" {
                    if let Some(group) = self.layout_manager.get_group_mut(group_idx) {
                        group.set_expanded(panel_idx);
                    }
                    return Ok(());
                }
            }
        }

        // Uses WidthPreference::PreferNarrow from OperationsPanel
        let panel = Box::new(OperationsPanel::new());
        self.add_panel_without_focus(panel);
        self.auto_save_session();
        Ok(())
    }

    /// Open operations panel and select specific operation without stealing focus.
    pub(in crate::app) fn open_operations_panel_with_focus(
        &mut self,
        op_id: termide_file_ops::OperationId,
    ) -> Result<()> {
        self.open_operations_panel()?;

        // Find the operations panel, update its data and select the operation
        for group in self.layout_manager.panel_groups.iter_mut() {
            for panel in group.panels_mut().iter_mut() {
                if let Some(ops_panel) = panel
                    .as_any_mut()
                    .downcast_mut::<termide_panel_operations::OperationsPanel>()
                {
                    // Update operations snapshot
                    let ops_list = self.state.operations_list();
                    ops_panel.update_operations(&ops_list);

                    // Select the specific operation
                    if let Some(index) = ops_list.iter().position(|op| op.id == op_id) {
                        ops_panel.set_selected(index);
                    }

                    return Ok(());
                }
            }
        }
        Ok(())
    }
}
