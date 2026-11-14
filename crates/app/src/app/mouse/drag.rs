//! Panel header drag handlers: grab a panel by its top border, then
//! drop it on another panel/group to move/reorder/cross-group, or drop
//! on the source panel itself to resize the divider above.

use anyhow::Result;

use termide_layout::{PanelDragIntent, PanelDropTarget, MIN_PANEL_HEIGHT};

use crate::app::App;

impl App {
    /// Update the drag cursor position on a `Drag(Left)` event. Promotes
    /// a pending drag to active once the threshold is crossed.
    pub(in crate::app) fn handle_panel_drag_move(&mut self, x: u16, y: u16) -> Result<()> {
        let became_active = self.state.ui.panel_drag.update_cursor(x, y);
        if became_active || self.state.ui.panel_drag.active {
            self.state.needs_redraw = true;
        }
        Ok(())
    }

    /// Finalise the drag on `Up(Left)`. If the drag never became active
    /// (threshold not crossed) we simply clear the state — the click path
    /// that fired on Down has already handled activation.
    ///
    /// Drop semantics (spatial classification):
    /// - Cursor over the source panel itself → resize the divider above
    ///   it (no-op for the top panel of a group, which has no divider
    ///   above).
    /// - Cursor over a different panel of the source group → reorder
    ///   inside the column (insert before/after the panel under the
    ///   cursor).
    /// - Cursor in another column or in a between-groups gutter → move
    ///   the panel across groups.
    /// - Cursor outside any valid zone → no-op.
    pub(in crate::app) fn handle_panel_drag_end(&mut self, x: u16, y: u16) -> Result<()> {
        let was_active = self.state.ui.panel_drag.active;
        let source = self.state.ui.panel_drag.source;
        self.state.ui.panel_drag.cancel();
        self.state.needs_redraw = true;

        if !was_active {
            return Ok(());
        }

        let Some(src) = source else {
            return Ok(());
        };

        let rects = self.calculate_panel_rects();
        let intent =
            termide_layout::classify_panel_drag(&rects, src.group_idx, src.panel_idx, x, y);

        match intent {
            PanelDragIntent::ResizeAbove { divider_y } => {
                let area_height = self.state.terminal.height.saturating_sub(2);
                let upper_boundary_y = rects
                    .iter()
                    .find(|(gi, pi, _, _)| *gi == src.group_idx && *pi == src.panel_idx)
                    .map(|(_, _, rect, _)| rect.y);
                if let Some(boundary_y) = upper_boundary_y {
                    let delta = divider_y as i32 - boundary_y as i32;
                    if delta != 0 {
                        if let Some(group) = self.layout_manager.panel_groups.get_mut(src.group_idx)
                        {
                            group.resize_panel_divider(src.panel_idx - 1, delta, area_height);
                        }
                        self.auto_save_layout();
                    }
                }
            }
            PanelDragIntent::Move { target, drop_y } => {
                let area_height = self.state.terminal.height.saturating_sub(2);
                let available_width = self.state.terminal.width;

                // Exit the fullscreen preset on the target group before
                // anything else. While the preset is active every
                // non-focused panel is one row tall, so a drop into
                // such a sliver would leave the dragged panel with no
                // room to take. Restoring the cached free-resize
                // heights gives the user the layout they had before
                // they hit `Alt+F11`, which is what they expect to
                // drop into.
                if let PanelDropTarget::IntoGroup { group_idx, .. } = target {
                    if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                        group.exit_fullscreen_preset(area_height);
                    }
                }

                // Refresh rects after a possible fullscreen exit on the
                // target group — panel rects shift when heights change.
                let rects = self.calculate_panel_rects();

                // For cross-group `IntoGroup` drops, capture the target
                // panel's pre-move geometry and the target group's
                // pre-move heights so we can split the target panel at
                // the cursor row after the move.
                let split_override = match target {
                    PanelDropTarget::IntoGroup {
                        group_idx,
                        at_position,
                    } if group_idx != src.group_idx => rects
                        .iter()
                        .find(|(gi, _, r, _)| {
                            *gi == group_idx && drop_y >= r.y && drop_y < r.y + r.height
                        })
                        .map(|(_, pi, r, _)| (*pi, r.y, r.height))
                        .and_then(|(target_pre_idx, target_y, target_h)| {
                            self.layout_manager
                                .panel_groups
                                .get(group_idx)
                                .map(|g| g.effective_split_heights(area_height))
                                .map(|pre_heights| {
                                    (
                                        group_idx,
                                        at_position,
                                        target_pre_idx,
                                        target_y,
                                        target_h,
                                        pre_heights,
                                    )
                                })
                        }),
                    _ => None,
                };

                let result = self
                    .layout_manager
                    .move_panel_to(src.group_idx, src.panel_idx, target, available_width)
                    .map(|_| ());
                self.handle_layout_op(termide_i18n::t().layout_cannot_move_panel(), result);

                if let Some((
                    target_gid,
                    at_position,
                    target_pre_idx,
                    target_y,
                    target_h,
                    mut new_heights,
                )) = split_override
                {
                    if target_h >= 2 * MIN_PANEL_HEIGHT && target_pre_idx < new_heights.len() {
                        let raw = drop_y.saturating_sub(target_y);
                        let upper_h = raw.max(MIN_PANEL_HEIGHT).min(target_h - MIN_PANEL_HEIGHT);
                        let lower_h = target_h - upper_h;

                        if at_position <= target_pre_idx {
                            // Dragged inserted before target → dragged
                            // takes the upper rows, target the lower.
                            new_heights[target_pre_idx] = lower_h;
                            new_heights.insert(target_pre_idx, upper_h);
                        } else {
                            // Dragged inserted after target → target
                            // keeps the upper rows, dragged the lower.
                            new_heights[target_pre_idx] = upper_h;
                            new_heights.insert(target_pre_idx + 1, lower_h);
                        }

                        if let Some(group) = self.layout_manager.panel_groups.get_mut(target_gid) {
                            if new_heights.len() == group.len() {
                                group.set_split_heights(new_heights);
                            }
                        }
                    }
                }
            }
            PanelDragIntent::Cancel => {}
        }
        Ok(())
    }
}
