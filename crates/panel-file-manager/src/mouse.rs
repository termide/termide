//! Mouse handling for the file manager: inline search-bar clicks, results-zone
//! selection, wheel scrolling, and tree click/drag with expand-icon hit-tests.

use ratatui::layout::Rect;

use termide_core::PanelEvent;
use termide_modal::FindBarAction;

use crate::search_bar::BarFocus;
use crate::FileManager;

impl FileManager {
    /// Handle a mouse event. Trait `Panel::handle_mouse` delegates here.
    pub(crate) fn on_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        panel_area: Rect,
    ) -> Vec<PanelEvent> {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

        // A click on the inline content bar is owned by the bar. Areas were
        // recorded in absolute screen coordinates during render, so the click
        // coordinates compare directly.
        if self.search_bar.is_some()
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            let on_bar = self
                .search_bar
                .as_ref()
                .is_some_and(|b| b.click_hits_bar(mouse.column, mouse.row));
            if on_bar {
                let mut bar = self.search_bar.take().unwrap();
                let action = bar.handle_mouse(mouse);
                self.search_bar = Some(bar);
                self.bar_focus = BarFocus::Input;
                return match action {
                    Some(FindBarAction::QueryChanged) => {
                        self.rerun_search();
                        vec![PanelEvent::NeedsRedraw]
                    }
                    Some(FindBarAction::Next) => {
                        self.search_next();
                        vec![PanelEvent::NeedsRedraw]
                    }
                    Some(FindBarAction::Previous) => {
                        self.search_prev();
                        vec![PanelEvent::NeedsRedraw]
                    }
                    Some(FindBarAction::ReplaceAll) => self.content_replace_all_event(),
                    _ => vec![PanelEvent::NeedsRedraw],
                };
            }
        }

        // While the inline search bar is open it owns the panel's mouse: clicks
        // in the results zone move/select the result, double-click opens it (or
        // toggles a file header), and the wheel walks matches.
        if self.search_bar.is_some() {
            if let Some(rarea) = self.search_results_area {
                match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        if let Some(s) = self.file_search.as_mut() {
                            s.prev_result();
                        }
                        return vec![PanelEvent::NeedsRedraw];
                    }
                    MouseEventKind::ScrollDown => {
                        if let Some(s) = self.file_search.as_mut() {
                            s.next_result();
                        }
                        return vec![PanelEvent::NeedsRedraw];
                    }
                    MouseEventKind::Down(MouseButton::Left)
                        if mouse.column >= rarea.x
                            && mouse.column < rarea.x + rarea.width
                            && mouse.row >= rarea.y
                            && mouse.row < rarea.y + rarea.height =>
                    {
                        self.bar_focus = BarFocus::Results;
                        let line = (mouse.row - rarea.y) as usize;
                        let col = mouse.column.saturating_sub(rarea.x) as usize;
                        // A click on the collapse triangle toggles that group;
                        // a click on the selection checkbox toggles selection.
                        if self
                            .file_search
                            .as_mut()
                            .map(|s| {
                                s.toggle_collapse_at_visual_click(line, col)
                                    || s.toggle_selection_at_visual_click(line, col)
                            })
                            .unwrap_or(false)
                        {
                            return vec![PanelEvent::NeedsRedraw];
                        }
                        // Otherwise place the cursor on the clicked row (snaps to
                        // the nearest selectable row).
                        if let Some(s) = self.file_search.as_mut() {
                            s.cursor_at_visual_line(line);
                        }
                        let idx = self.file_search.as_ref().map(|s| s.cursor).unwrap_or(0);

                        if self.click_tracker.is_double_click(&idx) {
                            self.click_tracker.reset();
                            // Double-click opens the selection, or toggles a
                            // directory's collapse when it can't be opened.
                            let opens = self
                                .file_search
                                .as_ref()
                                .and_then(|s| s.get_selected_result())
                                .is_some();
                            if opens {
                                let open = self.close_search_with_selection();
                                self.search_bar = None;
                                self.bar_focus = BarFocus::Input;
                                let mut out = vec![PanelEvent::NeedsRedraw];
                                out.extend(open);
                                return out;
                            } else if let Some(s) = self.file_search.as_mut() {
                                s.toggle_collapse_at_cursor();
                            }
                        } else {
                            self.click_tracker.record(idx);
                        }
                        self.sync_bar_status();
                        return vec![PanelEvent::NeedsRedraw];
                    }
                    _ => {}
                }
                self.sync_bar_status();
            }
            // Don't let other mouse events fall through to the tree handler.
            return vec![];
        }

        // Handle scroll first (works anywhere in panel)
        let visible_height = panel_area.height.saturating_sub(2) as usize;
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(3);
                // Keep selected in visible area so render doesn't reset scroll
                if self.selected >= self.scroll_offset + visible_height {
                    self.selected = (self.scroll_offset + visible_height).saturating_sub(1);
                }
                return vec![];
            }
            MouseEventKind::ScrollDown => {
                let max_scroll = self.visible_count().saturating_sub(visible_height);
                self.scroll_offset = (self.scroll_offset + 3).min(max_scroll);
                // Keep selected in visible area so render doesn't reset scroll
                if self.selected < self.scroll_offset {
                    self.selected = self.scroll_offset;
                }
                return vec![];
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // End drag - handle this ALWAYS, even if outside panel
                self.selection.end_drag();
                return vec![];
            }
            _ => {}
        }

        // Check that click is inside content area (not on borders)
        let inner_area = Rect {
            x: panel_area.x + 1,
            y: panel_area.y + 1,
            width: panel_area.width.saturating_sub(2),
            height: panel_area.height.saturating_sub(2),
        };

        // Check that click is inside inner area
        if mouse.column < inner_area.x
            || mouse.column >= inner_area.x + inner_area.width
            || mouse.row < inner_area.y
            || mouse.row >= inner_area.y + inner_area.height
        {
            return vec![];
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Determine index of clicked item
                let relative_row = (mouse.row - inner_area.y) as usize;
                let clicked_index = self.scroll_offset + relative_row;

                if clicked_index < self.visible_count() {
                    // Check modifiers
                    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                        // Shift+click - select range from selected to clicked_index
                        let start = self.selected.min(clicked_index);
                        let end = self.selected.max(clicked_index);
                        self.selection.dragged.clear();
                        for i in start..=end {
                            self.selection.select(i);
                            self.selection.dragged.insert(i);
                        }
                        self.selected = clicked_index;
                        self.selection.drag_start = Some(clicked_index);
                        self.selection.start_shift_drag(clicked_index);
                    } else if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                        // Ctrl+click - toggle selection on clicked element
                        self.selection.toggle(clicked_index);
                        self.selected = clicked_index;
                        self.selection.start_ctrl_drag(clicked_index);
                    } else {
                        // Check if click is on the expand/collapse icon area for directories
                        let relative_col = (mouse.column - inner_area.x) as usize;
                        let is_dir_icon_click = if let Some(te) = self.tree_entry_at(clicked_index)
                        {
                            let prefix_width = self
                                .tree_prefixes
                                .get(clicked_index)
                                .map(|p| unicode_width::UnicodeWidthStr::width(p.as_str()))
                                .unwrap_or(0);
                            // Icon is at prefix_width + 1 (attr char) position
                            te.expanded.is_some() && relative_col <= prefix_width + 1
                        } else {
                            false
                        };

                        if is_dir_icon_click {
                            // Click on ▶/▼ icon — toggle expand/collapse
                            self.selected = clicked_index;
                            self.toggle_expand(clicked_index);
                            self.click_tracker.reset();
                        } else {
                            // Check for double click using ClickTracker
                            let is_double_click =
                                self.click_tracker.is_double_click(&clicked_index);

                            if is_double_click {
                                // Double click - open file/directory
                                self.selected = clicked_index;
                                let event = self.enter();
                                self.click_tracker.reset();
                                if let Some(e) = event {
                                    return vec![e];
                                }
                            } else {
                                // Single click - select item
                                self.selected = clicked_index;
                                self.click_tracker.record(clicked_index);
                            }
                        }
                        self.selection.end_drag();
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Handle drag only if there's an active drag mode
                if self.selection.is_dragging() {
                    let relative_row = (mouse.row - inner_area.y) as usize;
                    let current_index = self.scroll_offset + relative_row;

                    if current_index < self.visible_count() {
                        // Process drag will select or toggle based on drag mode
                        self.selection.process_drag(current_index);
                        self.selected = current_index;
                    }
                }
            }
            _ => {}
        }

        vec![]
    }
}
