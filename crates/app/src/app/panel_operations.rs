//! Panel operations: movement, resize, and close handling.
//!
//! Handles panel manipulation including stacking, swapping, and resizing.

use anyhow::Result;

use super::App;
use crate::state::{ActiveModal, PendingAction};
use termide_core::{CommandResult, PanelCommand};
use termide_i18n as i18n;
use termide_logger as logger;

impl App {
    /// Handle panel close request with confirmation if needed
    pub(crate) fn handle_close_panel_request(&mut self, _panel_index: usize) -> Result<()> {
        logger::debug("Panel close requested");
        // Check if confirmation is required before closing active panel
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(_message) = panel.needs_close_confirmation() {
                logger::warn("Close requested for panel requiring confirmation");
                // Check modification status via command (works for Editor panels)
                let mod_status = panel.handle_command(PanelCommand::GetModificationStatus);
                if let CommandResult::ModificationStatus {
                    is_modified,
                    has_external_change: has_external,
                } = mod_status
                {
                    use termide_modal::SelectModal;
                    let t = i18n::t();

                    if is_modified && has_external {
                        // Conflict: both local and external changes
                        let modal = SelectModal::single(
                            t.editor_close_conflict(),
                            t.editor_close_conflict_question(),
                            vec![
                                t.editor_overwrite_disk().to_string(),
                                t.editor_reload_from_disk().to_string(),
                                t.editor_cancel().to_string(),
                            ],
                        );
                        let action = PendingAction::CloseEditorConflict { panel_index: 0 };
                        self.state
                            .set_pending_action(action, ActiveModal::Select(Box::new(modal)));
                        return Ok(());
                    } else if is_modified {
                        // Only local changes
                        let modal = SelectModal::single(
                            t.editor_close_unsaved(),
                            t.editor_close_unsaved_question(),
                            vec![
                                t.editor_save_and_close().to_string(),
                                t.editor_close_without_saving().to_string(),
                                t.editor_cancel().to_string(),
                            ],
                        );
                        let action = PendingAction::CloseEditorWithSave { panel_index: 0 };
                        self.state
                            .set_pending_action(action, ActiveModal::Select(Box::new(modal)));
                        return Ok(());
                    } else if has_external {
                        // Only external changes
                        let modal = SelectModal::single(
                            t.editor_close_external(),
                            t.editor_close_external_question(),
                            vec![
                                t.editor_overwrite_disk().to_string(),
                                t.editor_keep_disk_close().to_string(),
                                t.editor_reload_into_editor().to_string(),
                                t.editor_cancel().to_string(),
                            ],
                        );
                        let action = PendingAction::CloseEditorExternal { panel_index: 0 };
                        self.state
                            .set_pending_action(action, ActiveModal::Select(Box::new(modal)));
                        return Ok(());
                    }
                } else {
                    // For other panels show simple confirmation
                    let t = i18n::t();
                    let modal = termide_modal::ConfirmModal::new(t.modal_yes(), &_message);
                    let action = PendingAction::ClosePanel { panel_index: 0 };
                    self.state
                        .set_pending_action(action, ActiveModal::Confirm(Box::new(modal)));
                    return Ok(());
                }
            }
        }

        // Close active panel without confirmation
        self.close_panel_at_index(0);
        Ok(())
    }

    /// Close all Welcome panels (called before opening new panel)
    pub(super) fn close_welcome_panels(&mut self) {
        logger::debug("Closing Welcome panel(s)");
        let mut groups_to_remove = Vec::new();

        for group_idx in (0..self.layout_manager.panel_groups.len()).rev() {
            if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                let mut panels_to_remove = Vec::new();

                for panel_idx in (0..group.len()).rev() {
                    if let Some(panel) = group.panels().get(panel_idx) {
                        if panel.is_welcome_panel() {
                            panels_to_remove.push(panel_idx);
                        }
                    }
                }

                for panel_idx in panels_to_remove {
                    group.remove_panel(panel_idx);
                }

                if group.is_empty() {
                    groups_to_remove.push(group_idx);
                }
            }
        }

        let groups_were_removed = !groups_to_remove.is_empty();
        for group_idx in groups_to_remove {
            self.layout_manager.panel_groups.remove(group_idx);
        }

        if !self.layout_manager.panel_groups.is_empty()
            && self.layout_manager.focus >= self.layout_manager.panel_groups.len()
        {
            self.layout_manager.focus = self.layout_manager.panel_groups.len() - 1;
        }

        if groups_were_removed {
            let terminal_width = self.state.terminal.width;
            self.layout_manager
                .redistribute_widths_proportionally(terminal_width);
        }
    }

    /// Alt+PageUp: move panel up in group, or move group left if at top
    pub(super) fn handle_swap_panel_left(&mut self) -> Result<()> {
        let terminal_width = self.state.terminal.width;
        let active_group_idx = self.layout_manager.focus;

        if let Some(group) = self.layout_manager.panel_groups.get(active_group_idx) {
            if group.len() == 1 {
                self.layout_manager
                    .move_panel_to_prev_group(terminal_width)?;
            } else {
                let expanded_idx = group.expanded_index();
                if expanded_idx == 0 {
                    self.layout_manager
                        .move_panel_to_prev_group(terminal_width)?;
                } else {
                    self.layout_manager.move_panel_up_in_group()?;
                }
            }
        }

        self.auto_save_session();
        Ok(())
    }

    /// Alt+PageDown: move panel down in group, or move group right if at bottom
    pub(super) fn handle_swap_panel_right(&mut self) -> Result<()> {
        let terminal_width = self.state.terminal.width;
        let active_group_idx = self.layout_manager.focus;

        if let Some(group) = self.layout_manager.panel_groups.get(active_group_idx) {
            if group.len() == 1 {
                self.layout_manager
                    .move_panel_to_next_group(terminal_width)?;
            } else {
                let expanded_idx = group.expanded_index();
                if expanded_idx >= group.len() - 1 {
                    self.layout_manager
                        .move_panel_to_next_group(terminal_width)?;
                } else {
                    self.layout_manager.move_panel_down_in_group()?;
                }
            }
        }

        self.auto_save_session();
        Ok(())
    }

    /// Change active group width
    pub(super) fn handle_resize_panel(&mut self, delta: i16) -> Result<()> {
        if let Some(group_idx) = self.layout_manager.active_group_index() {
            if self.layout_manager.panel_groups.len() <= 1 {
                return Ok(());
            }

            let terminal_width = self.state.terminal.width;
            let available_width = terminal_width;

            // Freeze all auto-width groups before resize
            let actual_widths = self.layout_manager.calculate_actual_widths(available_width);
            for (idx, group) in self.layout_manager.panel_groups.iter_mut().enumerate() {
                if group.width.is_none() {
                    group.width = Some(actual_widths.get(idx).copied().unwrap_or(20));
                }
            }

            let current_width = self.layout_manager.panel_groups[group_idx].width.unwrap();
            let desired_new_width = ((current_width as i16 + delta).clamp(20, 300)) as u16;
            let actual_delta = desired_new_width as i16 - current_width as i16;

            if actual_delta == 0 {
                return Ok(());
            }

            // Collect other groups with their widths
            let other_groups: Vec<(usize, u16)> = self
                .layout_manager
                .panel_groups
                .iter()
                .enumerate()
                .filter(|(idx, _)| *idx != group_idx)
                .map(|(idx, g)| (idx, g.width.unwrap()))
                .collect();

            let total_other_width: u16 = other_groups.iter().map(|(_, w)| *w).sum();

            if total_other_width == 0 {
                return Ok(());
            }

            // Distribute delta proportionally across other groups
            let mut remaining_delta = -actual_delta;
            let mut new_widths: Vec<(usize, u16)> = Vec::new();

            for (i, &(idx, width)) in other_groups.iter().enumerate() {
                let is_last = i == other_groups.len() - 1;

                let delta_for_this = if is_last {
                    remaining_delta
                } else {
                    let proportion = width as f64 / total_other_width as f64;
                    ((-actual_delta as f64) * proportion).round() as i16
                };

                let new_width = ((width as i16 + delta_for_this).clamp(20, 300)) as u16;
                new_widths.push((idx, new_width));

                let actual_change = new_width as i16 - width as i16;
                remaining_delta -= actual_change;
            }

            // Apply new widths
            self.layout_manager.panel_groups[group_idx].width = Some(desired_new_width);

            for (idx, new_width) in new_widths {
                self.layout_manager.panel_groups[idx].width = Some(new_width);
            }

            // Correct balance if clamping broke zero-sum
            let total_new_width: u16 = self
                .layout_manager
                .panel_groups
                .iter()
                .map(|g| g.width.unwrap_or(20))
                .sum();

            if total_new_width != available_width {
                let other_widths_sum: u16 = self
                    .layout_manager
                    .panel_groups
                    .iter()
                    .enumerate()
                    .filter(|(idx, _)| *idx != group_idx)
                    .map(|(_, g)| g.width.unwrap_or(20))
                    .sum();

                let corrected_width = available_width.saturating_sub(other_widths_sum);
                self.layout_manager.panel_groups[group_idx].width =
                    Some(corrected_width.clamp(20, 300));
            }
            self.auto_save_session();
        }
        Ok(())
    }
}
