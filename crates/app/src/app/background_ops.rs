//! Miscellaneous per-tick background pollers that don't warrant their own
//! module: directory-size results, panel git-status/diff draining, and
//! `.report.` / `.bg.` command completion.
//!
//! Larger background subsystems live in sibling modules: viewer URL fetching
//! ([`bg_fetch`](super::bg_fetch)), git network operations
//! ([`bg_git`](super::bg_git)), LSP polling ([`bg_lsp`](super::bg_lsp)), and
//! system-resource / spinner ticks ([`bg_resource`](super::bg_resource)).

use std::sync::mpsc::TryRecvError;

use termide_core::PanelCommand;
use termide_modal::InfoModal;

use crate::state::ActiveModal;
use crate::PanelExt;

use super::App;

impl App {
    /// Check channel for directory size calculation results
    pub(super) fn check_dir_size_update(&mut self) {
        use termide_panel_file_manager::FileManager;

        if let Some(rx) = &self.state.dir_size_receiver {
            // Try to receive result without blocking
            if let Ok(result) = rx.try_recv() {
                let t = termide_i18n::t();
                let formatted_size = FileManager::format_size_static(result.size);
                // Keep the immediate child count in parentheses on the size
                // line, matching what was shown while calculating.
                let formatted_size = match result.item_count {
                    Some(n) => format!("{} ({})", formatted_size, t.file_info_items(n)),
                    None => formatted_size,
                };

                // Update Info or InfoAction modal if open
                match &mut self.state.active_modal {
                    Some(ActiveModal::Info(ref mut modal)) => {
                        modal.update_value(t.file_info_size(), formatted_size);
                        self.state.needs_redraw = true;
                    }
                    Some(ActiveModal::InfoAction(ref mut modal)) => {
                        modal.update_value(t.file_info_size(), formatted_size);
                        self.state.needs_redraw = true;
                    }
                    _ => {}
                }

                // Clear channel
                self.state.dir_size_receiver = None;
            }
        }
    }

    /// Single-pass check of all background panel updates:
    /// - async git status results (FileManager panels)
    /// - async directory reloads (FileManager panels, watcher-triggered)
    /// - pending git diff updates and async git diff results (all panels)
    ///
    /// Consolidated into one panel iteration to avoid 3 separate `iter_all_panels_mut()` passes.
    pub(super) fn check_background_panel_updates(&mut self) {
        for panel in self.layout_manager.iter_all_panels_mut() {
            // FileManager: drain async git status receiver
            if let Some(fm) = panel.as_file_manager_mut() {
                if fm.check_git_status_async() {
                    self.state.needs_redraw = true;
                }
            }
            // FileManager: drain async directory reload receiver
            if let Some(fm) = panel.as_file_manager_mut() {
                if fm.check_async_reload() {
                    self.state.needs_redraw = true;
                }
            }
            // All panels: check debounced git diff buffer updates
            panel.handle_command(PanelCommand::CheckPendingGitDiff);
            // All panels: drain async git diff result receiver
            if panel
                .handle_command(PanelCommand::CheckGitDiffReceiver)
                .needs_redraw()
            {
                self.state.needs_redraw = true;
            }
        }
    }

    /// Check for background command operation results (.report. commands)
    pub(super) fn check_command_operation_result(&mut self) {
        if self.state.command_operation_handles.is_empty() {
            return;
        }

        let mut last_result_modal = None;

        self.state.command_operation_handles.retain(|handle| {
            match handle.receiver.try_recv() {
                Ok(result) => {
                    // Remove from Operations panel
                    if let Some(op_id) = handle.operation_id {
                        self.state.active_operations.remove(&op_id);
                    }

                    // Build modal (last completed command wins if multiple finish same tick)
                    let title = if result.success {
                        format!("{} \u{2713}", result.command_name)
                    } else {
                        format!("{} \u{2717}", result.command_name)
                    };

                    let mut lines = vec![];
                    for line in result.stdout.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            lines.push((String::new(), trimmed.to_string()));
                        }
                    }
                    for line in result.stderr.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            lines.push((String::new(), trimmed.to_string()));
                        }
                    }
                    if lines.is_empty() {
                        lines.push((String::new(), "(no output)".to_string()));
                    }

                    last_result_modal = Some((title, lines));
                    false // remove from list
                }
                Err(TryRecvError::Empty) => true, // keep polling
                Err(TryRecvError::Disconnected) => {
                    if let Some(op_id) = handle.operation_id {
                        self.state.active_operations.remove(&op_id);
                    }
                    false // remove
                }
            }
        });

        // Show modal for the last completed command
        if let Some((title, lines)) = last_result_modal {
            let modal = InfoModal::new(&title, lines);
            self.state.active_modal = Some(ActiveModal::Info(Box::new(modal)));
            self.state.needs_redraw = true;
        }
    }

    /// Check for completed background commands (.bg.) and remove from Operations panel.
    pub(super) fn check_bg_command_completion(&mut self) {
        self.state.bg_command_handles.retain(|(op_id, rx, _pid)| {
            match rx.try_recv() {
                Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.state.active_operations.remove(op_id);
                    self.state.needs_redraw = true;
                    false // remove from list
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true, // keep polling
            }
        });
    }
}
