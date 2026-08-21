//! Scrollbar thumb dragging.
//!
//! Panel borders carry two things at once: the scrollbars panels draw on them
//! and the grab zones that resize the layout. The split is by hit-test — a
//! press on the thumb scrolls the panel, everything else on the border
//! (including the bar's track) keeps resizing the layout as before. Panels
//! report their thumbs through `PanelCommand::GetScrollBars`, so a border
//! without a bar behaves exactly as it did.

use termide_core::{CommandResult, PanelCommand, ScrollAxis, ScrollBarGeometry};

use crate::app::App;
use crate::state::ScrollBarDrag;

impl App {
    /// Start a thumb drag if `(x, y)` landed on a scrollbar thumb.
    ///
    /// Runs before the divider hit-tests so a thumb press is not swallowed by
    /// the layout resize. Returns `true` when the press was consumed.
    ///
    /// Focus is deliberately left alone: dragging a bar of a neighbouring
    /// panel scrolls it in place, the same way the wheel already does.
    pub(in crate::app) fn handle_scrollbar_press(&mut self, x: u16, y: u16) -> bool {
        let Some((group_idx, panel_idx, _)) = self.find_panel_at(x, y) else {
            return false;
        };
        let Some(bar) = self.panel_scrollbar_at(group_idx, panel_idx, x, y) else {
            return false;
        };
        let Some(grab_offset) = bar.grab_offset(x, y) else {
            return false;
        };
        self.state.scrollbar_drag = Some(ScrollBarDrag {
            group_idx,
            panel_idx,
            axis: bar.axis,
            grab_offset,
        });
        true
    }

    /// Apply an in-progress thumb drag to the panel's scroll offset.
    pub(in crate::app) fn handle_scrollbar_drag(&mut self, x: u16, y: u16) {
        let Some(drag) = self.state.scrollbar_drag else {
            return;
        };
        // Re-read the geometry instead of caching it at grab time: the thumb
        // moves with every applied offset, and the track can change length if
        // the panel was resized mid-drag.
        let Some(bar) = self.panel_scrollbar(drag.group_idx, drag.panel_idx, drag.axis) else {
            return;
        };
        let offset = bar.offset_for_drag(x, y, drag.grab_offset);
        if offset == bar.offset {
            return;
        }
        let result = self.send_panel_command(
            drag.group_idx,
            drag.panel_idx,
            PanelCommand::SetScrollOffset {
                axis: drag.axis,
                offset,
            },
        );
        if result.is_some_and(|r| r.needs_redraw()) {
            self.state.needs_redraw = true;
        }
    }

    /// Finish a thumb drag.
    pub(in crate::app) fn handle_scrollbar_drag_end(&mut self) {
        if self.state.scrollbar_drag.take().is_some() {
            self.state.needs_redraw = true;
        }
    }

    /// Whether a thumb drag is in progress.
    pub(in crate::app) fn is_dragging_scrollbar(&self) -> bool {
        self.state.scrollbar_drag.is_some()
    }

    /// The panel's bar whose thumb sits under `(x, y)`, if any.
    fn panel_scrollbar_at(
        &mut self,
        group_idx: usize,
        panel_idx: usize,
        x: u16,
        y: u16,
    ) -> Option<ScrollBarGeometry> {
        self.panel_scrollbars(group_idx, panel_idx)?.hit_thumb(x, y)
    }

    /// The panel's bar for one axis, as of its last render.
    fn panel_scrollbar(
        &mut self,
        group_idx: usize,
        panel_idx: usize,
        axis: ScrollAxis,
    ) -> Option<ScrollBarGeometry> {
        let bars = self.panel_scrollbars(group_idx, panel_idx)?;
        match axis {
            ScrollAxis::Vertical => bars.vertical,
            ScrollAxis::Horizontal => bars.horizontal,
        }
    }

    /// Ask a panel for the scrollbars it drew last.
    fn panel_scrollbars(
        &mut self,
        group_idx: usize,
        panel_idx: usize,
    ) -> Option<termide_core::ScrollBars> {
        match self.send_panel_command(group_idx, panel_idx, PanelCommand::GetScrollBars)? {
            CommandResult::ScrollBars(bars) => Some(bars),
            _ => None,
        }
    }

    /// Send a command to one panel by position, `None` if that panel is gone.
    fn send_panel_command(
        &mut self,
        group_idx: usize,
        panel_idx: usize,
        cmd: PanelCommand<'_>,
    ) -> Option<CommandResult> {
        Some(
            self.layout_manager
                .panel_groups
                .get_mut(group_idx)?
                .panels_mut()
                .get_mut(panel_idx)?
                .handle_command(cmd),
        )
    }
}
