//! Panel-layout utilities for mouse hit-testing: divider drag, per-group
//! rectangle calculation, and coalesced scroll forwarding to panels.

use anyhow::Result;
use ratatui::layout::Rect;

use crate::app::App;

impl App {
    /// Handle click on divider to start resize drag.
    /// Returns true if click was on a divider.
    pub(in crate::app) fn handle_divider_click(&mut self, x: u16, y: u16) -> Result<bool> {
        let terminal_height = self.state.terminal.height;

        if let Some(divider_idx) =
            self.layout_manager
                .find_divider_at_position(x, y, terminal_height)
        {
            // Get current widths of adjacent groups
            let left_width = self
                .layout_manager
                .panel_groups
                .get(divider_idx)
                .and_then(|g| g.width)
                .unwrap_or(0);
            let right_width = self
                .layout_manager
                .panel_groups
                .get(divider_idx + 1)
                .and_then(|g| g.width)
                .unwrap_or(0);

            // Start drag
            self.state
                .ui
                .drag
                .start(divider_idx, x, left_width, right_width);
            self.state.needs_redraw = true;

            return Ok(true);
        }

        Ok(false)
    }

    /// Handle divider drag — track cursor position and draw ghost line.
    /// Only the ghost divider line is rendered (lightweight), while actual
    /// panel resize is deferred to mouse release.
    pub(in crate::app) fn handle_divider_drag(&mut self, current_x: u16) -> Result<()> {
        if self.state.ui.drag.last_applied_x == Some(current_x) {
            return Ok(());
        }
        self.state.ui.drag.last_applied_x = Some(current_x);
        // Trigger redraw for the ghost line — this is lightweight because
        // only ~2 columns change (ratatui diff rendering sends minimal data).
        self.state.needs_redraw = true;
        Ok(())
    }

    /// Handle divider drag end — apply final widths and redraw once.
    pub(in crate::app) fn handle_divider_drag_end(&mut self) -> Result<()> {
        // Apply the accumulated drag position as a single resize
        if let (Some(divider_idx), Some(final_x)) = (
            self.state.ui.drag.active_divider,
            self.state.ui.drag.last_applied_x,
        ) {
            let delta = final_x as i32 - self.state.ui.drag.start_x as i32;
            let (start_left, start_right) = self.state.ui.drag.start_widths;

            let min_width = self.state.config.general.min_panel_width;
            let total_width = start_left + start_right;

            let new_left = (start_left as i32 + delta)
                .max(min_width as i32)
                .min((total_width - min_width) as i32) as u16;
            let new_right = total_width - new_left;

            self.layout_manager
                .resize_groups(divider_idx, new_left, new_right);
        }

        self.state.ui.drag.end();
        self.state.needs_redraw = true;

        // Save the layout with new widths
        self.auto_save_layout();

        Ok(())
    }

    /// Hit-test a vertical divider (the bottom border of an expanded
    /// panel that has another panel below it within the same group).
    ///
    /// Returns `(group_idx, upper_panel_idx)` when `(x, y)` lands on
    /// such a divider row. Skips the last panel of every group (no
    /// neighbour below) and accordion-collapsed panels (no bottom
    /// border row at all).
    pub(in crate::app) fn find_vertical_divider_at(
        &self,
        x: u16,
        y: u16,
    ) -> Option<(usize, usize)> {
        let rects = self.calculate_panel_rects();
        // Group panels by group_idx so we know which is last.
        let mut last_panel_idx: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::new();
        for (gi, pi, _, _) in &rects {
            let entry = last_panel_idx.entry(*gi).or_insert(*pi);
            if *pi > *entry {
                *entry = *pi;
            }
        }
        for (gi, pi, rect, _) in &rects {
            if last_panel_idx.get(gi).copied() == Some(*pi) {
                continue; // last in group → no divider below
            }
            if rect.height < 2 {
                continue; // accordion → no own bottom border
            }
            let bottom_y = rect.y + rect.height - 1;
            if y == bottom_y && x >= rect.x && x < rect.x + rect.width {
                return Some((*gi, *pi));
            }
        }
        None
    }

    /// Begin a vertical-divider drag rooted at the given panel-pair.
    pub(in crate::app) fn handle_v_divider_click(&mut self, x: u16, y: u16) -> Result<bool> {
        if let Some((group_idx, upper_panel_idx)) = self.find_vertical_divider_at(x, y) {
            let _ = x;
            self.state.ui.vdrag.start(group_idx, upper_panel_idx, y);
            self.state.needs_redraw = true;
            return Ok(true);
        }
        Ok(false)
    }

    /// Track the cursor row during a v-divider drag (ghost preview).
    pub(in crate::app) fn handle_v_divider_drag(&mut self, current_y: u16) -> Result<()> {
        if self.state.ui.vdrag.last_applied_y == Some(current_y) {
            return Ok(());
        }
        self.state.ui.vdrag.last_applied_y = Some(current_y);
        self.state.needs_redraw = true;
        Ok(())
    }

    /// Apply the accumulated drag delta on release.
    pub(in crate::app) fn handle_v_divider_drag_end(&mut self) -> Result<()> {
        if let (Some((group_idx, upper_panel_idx)), Some(final_y)) = (
            self.state.ui.vdrag.active,
            self.state.ui.vdrag.last_applied_y,
        ) {
            let delta = final_y as i32 - self.state.ui.vdrag.start_y as i32;
            if delta != 0 {
                let area_height = self.state.terminal.height.saturating_sub(2);
                if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                    group.resize_panel_divider(upper_panel_idx, delta, area_height);
                }
                self.auto_save_layout();
            }
        }
        self.state.ui.vdrag.end();
        self.state.needs_redraw = true;
        Ok(())
    }

    /// Find the panel directly under `(x, y)` regardless of focus state.
    ///
    /// Returns `(group_idx, panel_idx, rect)`. Use this when an event
    /// should be routed to whatever panel the cursor is over (mouse
    /// scroll, click hit-testing on a non-focused panel) rather than
    /// to the focused panel within the group.
    pub(in crate::app) fn find_panel_at(&self, x: u16, y: u16) -> Option<(usize, usize, Rect)> {
        for (group_idx, panel_idx, rect, _is_expanded) in self.calculate_panel_rects() {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return Some((group_idx, panel_idx, rect));
            }
        }
        None
    }

    /// Calculate panel rectangles for mouse hit testing.
    /// Height of the area the panel columns share: the terminal minus the
    /// menu bar above and the status bar below. Layout questions that need a
    /// column height — which panels show content, where dividers sit — must
    /// use this so they agree with what is drawn.
    pub(in crate::app) fn panel_area_height(&self) -> u16 {
        self.state.terminal.height.saturating_sub(2)
    }

    /// Returns `Vec<(group_idx, panel_idx, rect, is_expanded)>`.
    pub(in crate::app) fn calculate_panel_rects(&self) -> Vec<(usize, usize, Rect, bool)> {
        let width = self.state.terminal.width;
        let main_area = Rect {
            x: 0,
            y: 1,
            width,
            height: self.panel_area_height(),
        };
        termide_layout::calculate_panel_rects(&self.layout_manager.panel_groups, main_area)
    }

    /// Handle coalesced scroll events (batched for performance).
    ///
    /// This method processes multiple scroll events that have been coalesced
    /// into a single event with a combined delta value.
    pub(in crate::app) fn handle_coalesced_scroll(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        delta: i32,
    ) -> Result<()> {
        // Track scroll timing for throttling heavy operations in Event::Tick
        self.state.last_mouse_scroll = Some(std::time::Instant::now());

        // When a modal is open, forward the wheel event to it instead of the
        // panel underneath — otherwise scrollable modals (InfoModal for the
        // report-command output, list modals, etc.) never see wheel input and
        // look broken to the user.
        if self.state.has_modal() {
            let kind = if delta < 0 {
                crossterm::event::MouseEventKind::ScrollUp
            } else if delta > 0 {
                crossterm::event::MouseEventKind::ScrollDown
            } else {
                return Ok(());
            };
            let modal_area = ratatui::layout::Rect {
                x: 0,
                y: 0,
                width: self.state.terminal.width,
                height: self.state.terminal.height,
            };
            // Coalescing lost per-step granularity; replay it so the modal's
            // per-tick scroll step (e.g. 3 lines in InfoModal) actually maps
            // to the physical notch count.
            let steps = delta.unsigned_abs();
            for _ in 0..steps {
                let ev = crossterm::event::MouseEvent { kind, ..mouse };
                self.handle_modal_mouse(ev, modal_area)?;
            }
            return Ok(());
        }

        // A view shown over the layout takes the wheel.
        if self.overlay_scroll(mouse, delta)? {
            return Ok(());
        }

        // An open menu/submenu dropdown takes the wheel: replay it as Up/Down
        // key presses through the existing dropdown navigation, reusing each
        // dropdown's item-count and separator-skipping logic.
        if self.any_menu_dropdown_open() {
            use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
            let code = if delta < 0 {
                KeyCode::Up
            } else if delta > 0 {
                KeyCode::Down
            } else {
                return Ok(());
            };
            for _ in 0..delta.unsigned_abs() {
                self.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE))?;
            }
            return Ok(());
        }

        self.forward_coalesced_scroll_to_panel(mouse, delta)
    }

    /// Forward coalesced scroll to the panel under the mouse cursor.
    /// The cursor's panel — not the focused one — receives the wheel,
    /// so scrolling an unfocused panel works without first clicking it.
    fn forward_coalesced_scroll_to_panel(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        delta: i32,
    ) -> Result<()> {
        if let Some((group_idx, panel_idx, rect)) = self.find_panel_at(mouse.column, mouse.row) {
            if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                if let Some(panel) = group.panels_mut().get_mut(panel_idx) {
                    let events = panel.handle_scroll(delta, rect);
                    self.process_panel_events(events)?;
                }
            }
        }

        Ok(())
    }
}
