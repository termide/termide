//! Mouse event handling for the application.

use anyhow::Result;
use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use super::App;
use crate::PanelExt;
use termide_i18n as i18n;

use crate::state::{ActiveModal, PendingAction};
use termide_modal as modal;

use termide_theme::Theme;

impl App {
    /// Handle mouse event
    pub(super) fn handle_mouse_event(&mut self, mouse: crossterm::event::MouseEvent) -> Result<()> {
        // A scrollbar thumb drag owns the mouse until release, ahead of the
        // divider drags: the thumb sits on the same border they resize.
        if self.is_dragging_scrollbar() {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.handle_scrollbar_drag(mouse.column, mouse.row);
                    return Ok(());
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.handle_scrollbar_drag_end();
                    return Ok(());
                }
                _ => {}
            }
        }

        // Handle divider drag first (highest priority for smooth resize)
        if self.state.ui.drag.is_dragging() {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.handle_divider_drag(mouse.column)?;
                    return Ok(());
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.handle_divider_drag_end()?;
                    return Ok(());
                }
                _ => {}
            }
        }

        // Vertical divider drag (between two panels of the same group).
        if self.state.ui.vdrag.is_dragging() {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.handle_v_divider_drag(mouse.row)?;
                    return Ok(());
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.handle_v_divider_drag_end()?;
                    return Ok(());
                }
                _ => {}
            }
        }

        // Handle panel drag (grab by top border) — drag & drop of whole panels
        if self.state.ui.panel_drag.is_pending_or_active() {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.handle_panel_drag_move(mouse.column, mouse.row)?;
                    return Ok(());
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.handle_panel_drag_end(mouse.column, mouse.row)?;
                    return Ok(());
                }
                _ => {}
            }
        }

        // Handle modal mouse events first if a modal is open
        if self.state.has_modal() {
            // Indicator modals (menu-integrated): click anywhere closes and falls through
            let is_indicator_modal = self.state.is_menu_open()
                && (self.state.resource_modal_kind.is_some()
                    || matches!(self.state.active_modal, Some(ActiveModal::Calendar(_))));

            if is_indicator_modal && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.state.close_indicator_modal();
                // Fall through to menu bar / panel click handling below
            } else {
                let modal_area = Rect {
                    x: 0,
                    y: 0,
                    width: self.state.terminal.width,
                    height: self.state.terminal.height,
                };
                self.handle_modal_mouse(mouse, modal_area)?;
                return Ok(());
            }
        }

        // Scroll events should reach panels when no modal is active
        // This allows scrolling terminal history, editor, etc.
        if matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            // Track scroll timing for throttling heavy operations in Event::Tick
            self.state.last_mouse_scroll = Some(std::time::Instant::now());
            self.state.pending_scroll_render = true;
            // An open menu/submenu dropdown takes the wheel (replayed as an
            // Up/Down key press through its existing navigation).
            if self.any_menu_dropdown_open() {
                use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
                let code = if matches!(mouse.kind, MouseEventKind::ScrollUp) {
                    KeyCode::Up
                } else {
                    KeyCode::Down
                };
                self.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE))?;
                return Ok(());
            }
            self.forward_scroll_to_panel_at_cursor(mouse)?;
            return Ok(());
        }

        // Click on menu bar (row 0)
        if mouse.row == 0 && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            self.handle_menu_click(mouse.column)?;
            return Ok(());
        }

        // Click on status bar (bottom row)
        let status_bar_row = self.state.terminal.height.saturating_sub(1);
        if mouse.row == status_bar_row
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            self.handle_status_bar_click(mouse.column)?;
            return Ok(());
        }

        // Handle Sessions submenu clicks when it's open
        if self.state.ui.sessions_submenu.open
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.handle_sessions_submenu_click(mouse.column, mouse.row)?
        {
            return Ok(());
        }

        // Handle Preferences submenu clicks when submenu is open
        if self.state.ui.options_submenu.open
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.handle_submenu_click(mouse.column, mouse.row)?
        {
            return Ok(());
        }

        // Handle Tools submenu clicks when it's open
        if self.state.ui.tools_submenu.open
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.handle_tools_submenu_click(mouse.column, mouse.row)?
        {
            return Ok(());
        }

        // Handle Commands submenu clicks when it's open
        if self.state.ui.commands_submenu.open
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.handle_commands_submenu_click(mouse.column, mouse.row)?
        {
            return Ok(());
        }

        // Handle Stash dropdown clicks when it's open
        if self.state.ui.stash_submenu.open
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            self.handle_stash_dropdown_click(mouse.column, mouse.row)?;
            return Ok(());
        }

        // Handle Bookmarks submenu clicks when it's open
        if self.state.ui.bookmarks_submenu.open
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.handle_bookmarks_submenu_click(mouse.column, mouse.row)?
        {
            return Ok(());
        }

        // Handle Panel Action menu clicks when it's open
        if self.state.ui.panel_action_menu.open
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            self.handle_panel_action_menu_click(mouse.column, mouse.row)?;
            return Ok(());
        }

        // Handle per-operation popup menu clicks when it's open
        if self.state.ui.operation_action_menu.open
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            self.handle_operation_action_menu_click(mouse.column, mouse.row)?;
            return Ok(());
        }

        // If menu is open, close it on click outside menu
        if self.state.is_menu_open()
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            self.state.close_menu();
            return Ok(());
        }

        // Grabbing a scrollbar thumb wins over the divider grab zones that
        // share the same border column/row. Only the thumb is claimed; the
        // track and bar-less borders fall through to the resize below.
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.handle_scrollbar_press(mouse.column, mouse.row)
        {
            return Ok(());
        }

        // Check click on divider for resize (before panel click handling)
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.handle_divider_click(mouse.column, mouse.row)?
        {
            return Ok(());
        }

        // Check click on a vertical divider (bottom border between two
        // panels of the same group) for in-group resize.
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.handle_v_divider_click(mouse.column, mouse.row)?
        {
            return Ok(());
        }

        // Check click on panel [X] button
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            if self.handle_panel_close_click(mouse.column, mouse.row)? {
                return Ok(());
            }

            // Check click on panel to switch focus
            self.handle_panel_focus_click(mouse.column, mouse.row)?;
        }

        // Scroll events handled at the top of this function (before modal check)

        // Other mouse events - to active panel
        self.forward_mouse_to_panel(mouse)?;

        Ok(())
    }

    /// Forward mouse event to active panel
    fn forward_mouse_to_panel(&mut self, mouse: crossterm::event::MouseEvent) -> Result<()> {
        use crate::panel_ext::PanelExt;

        // Determine active panel area
        let panel_area = self.get_active_panel_area();

        // Handle mouse event and collect results
        let (mut events, modal_request) =
            if let Some(panel) = self.layout_manager.active_panel_mut() {
                let events = panel.handle_mouse(mouse, panel_area);
                let modal_request = panel.take_modal_request();
                (events, modal_request)
            } else {
                (vec![], None)
            };

        // Handle editor-specific LSP requests (Ctrl+click go-to-definition)
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                // Handle go-to-definition request (Ctrl+click)
                if let Some((line, col)) = editor.take_definition_request() {
                    if let Some(ref lsp_manager) = self.state.lsp_manager {
                        editor.request_definition(line, col, lsp_manager);
                    }
                }

                // Poll for definition response (returns PanelEvent::OpenFileAt)
                if let Some(event) = editor.poll_definition() {
                    events.push(event);
                }
            }
        }

        // Process panel events (new event-based architecture)
        self.process_panel_events(events)?;

        // Handle modal window request from panel (legacy, still used)
        if let Some((action, modal)) = modal_request {
            self.handle_modal_request(action, modal)?;
        }

        Ok(())
    }

    /// Forward scroll to the panel under the mouse cursor — including
    /// unfocused panels. Without this, a wheel notch over a non-focused
    /// split-mode neighbour would be silently delivered to the focused
    /// panel instead, surprising the user.
    fn forward_scroll_to_panel_at_cursor(
        &mut self,
        mouse: crossterm::event::MouseEvent,
    ) -> Result<()> {
        if let Some((group_idx, panel_idx, rect)) = self.find_panel_at(mouse.column, mouse.row) {
            if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                if let Some(panel) = group.panels_mut().get_mut(panel_idx) {
                    let events = panel.handle_mouse(mouse, rect);
                    self.process_panel_events(events)?;
                }
            }
        }

        Ok(())
    }

    /// Get active panel area
    fn get_active_panel_area(&self) -> Rect {
        let focused_group_idx = self.layout_manager.focus;

        // Find expanded panel rect in the focused group
        for (group_idx, _panel_idx, rect, is_expanded) in self.calculate_panel_rects() {
            if group_idx == focused_group_idx && is_expanded {
                return rect;
            }
        }

        // Fallback: return full main area if active panel not found
        let width = self.state.terminal.width;
        let height = self.state.terminal.height;
        Rect {
            x: 0,
            y: 1,
            width,
            height: height.saturating_sub(2),
        }
    }

    /// Handle click on panel `[≡]` button or title area.
    /// Returns true if a button or title was clicked.
    fn handle_panel_close_click(&mut self, click_x: u16, click_y: u16) -> Result<bool> {
        let panel_rects = self.calculate_panel_rects();

        for (group_idx, panel_idx, rect, _is_expanded) in panel_rects {
            // Check if click is on this panel's top line
            if click_y != rect.y {
                continue;
            }

            // Check if click is within the panel's horizontal bounds
            if click_x < rect.x || click_x >= rect.x + rect.width {
                continue;
            }

            let relative_x = click_x - rect.x;

            // Title bar layout: `─[≡] Title ─── ─` (or `┌` / `├` corner
            // depending on position in the group). The action-menu
            // button occupies offsets 1..=3; everything from offset 4
            // onward (after the trailing space) is treated as title.

            if (1..=3).contains(&relative_x) {
                // Click on [≡] button — open panel action context menu
                self.state.ui.close_all_submenus();
                self.state
                    .ui
                    .panel_action_menu
                    .open(group_idx, panel_idx, rect.x, rect.y);
                self.state.needs_redraw = true;
                return Ok(true);
            }

            let title_start: u16 = 4;

            if relative_x >= title_start {
                // Record a pending panel drag. If the cursor moves past
                // the threshold before Up we'll promote this to an active
                // drag; otherwise the click behaviour below fires as-is.
                self.state
                    .ui
                    .panel_drag
                    .begin_pending(group_idx, panel_idx, click_x, click_y);

                // Check if this panel was already active before click
                let was_active = group_idx == self.layout_manager.focus
                    && self
                        .layout_manager
                        .panel_groups
                        .get(group_idx)
                        .map(|g| g.expanded_index() == panel_idx)
                        .unwrap_or(false);

                // Always activate the clicked panel
                if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                    group.set_expanded(panel_idx);
                }
                self.layout_manager.focus = group_idx;

                if was_active {
                    // Panel was already active — check for double-click
                    if self
                        .title_click_tracker
                        .is_double_click(&(click_x, click_y))
                    {
                        self.title_click_tracker.reset();
                        // Double-click on active FileManager title → open directory picker
                        if let Some(panel) = self.layout_manager.active_panel_mut() {
                            if let Some(fm) = panel.as_file_manager_mut() {
                                let current_path = fm.current_path().to_path_buf();
                                let t = i18n::t();
                                let modal = modal::DirectoryPickerModal::new(
                                    current_path,
                                    t.directory_switcher_title().to_string(),
                                    t.directory_picker_move().to_string(),
                                );
                                self.state.set_pending_action(
                                    PendingAction::SwitchDirectory,
                                    ActiveModal::DirectoryPicker(Box::new(modal)),
                                );
                                return Ok(true);
                            }
                        }
                    }
                }

                // Record this click for double-click detection
                self.title_click_tracker.record((click_x, click_y));
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Handle click on panel to switch focus
    fn handle_panel_focus_click(&mut self, click_x: u16, click_y: u16) -> Result<()> {
        let panel_rects = self.calculate_panel_rects();

        for (group_idx, panel_idx, rect, _is_expanded) in panel_rects {
            // Check if click is within this panel's bounds
            if click_x >= rect.x
                && click_x < rect.x + rect.width
                && click_y >= rect.y
                && click_y < rect.y + rect.height
            {
                // Close completion popup only when focus is actually changing to different group
                let focus_changing = group_idx != self.layout_manager.focus;
                if focus_changing {
                    if let Some(panel) = self.layout_manager.active_panel_mut() {
                        if let Some(editor) = panel.as_editor_mut() {
                            editor.cancel_completion();
                        }
                    }
                }

                // Click on a panel group - make it active
                self.layout_manager.focus = group_idx;
                if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                    group.set_expanded(panel_idx);
                }
                return Ok(());
            }
        }

        Ok(())
    }

    /// Handle click on menu bar
    fn handle_menu_click(&mut self, x: u16) -> Result<()> {
        let mut current_x = 1_u16;

        // Get menu items with translations
        let menu_items = termide_ui_render::menu::get_menu_items();

        for (i, item) in menu_items.iter().enumerate() {
            let item_width = item.width() as u16;
            if x >= current_x && x < current_x + item_width {
                // Toggle: if this menu item is already open, close it
                if self.state.is_menu_open() && self.state.ui.selected_menu_item == Some(i) {
                    // Restore theme if nested submenu was open
                    if let Some(original_name) = self.state.ui.theme_preview_original.take() {
                        self.state.theme = Theme::get_by_name(&original_name);
                    }
                    self.state.close_menu();
                } else {
                    // Open menu with the selected item
                    self.state.open_menu(Some(i));
                    self.execute_menu_action()?;
                }
                return Ok(());
            }
            current_x += item_width + 2; // +2 for spaces
        }

        // Check network/CPU/RAM/clock indicator clicks (right side of menu bar)
        let (net_range, cpu_range, ram_range, clock_range) = self.get_indicator_ranges();

        use termide_ui_render::{
            INDICATOR_CLOCK_INDEX, INDICATOR_CPU_INDEX, INDICATOR_NET_INDEX, INDICATOR_RAM_INDEX,
        };

        let indicator = if net_range.contains(&x) {
            Some((INDICATOR_NET_INDEX, net_range.start))
        } else if cpu_range.contains(&x) {
            Some((INDICATOR_CPU_INDEX, cpu_range.start))
        } else if ram_range.contains(&x) {
            Some((INDICATOR_RAM_INDEX, ram_range.start))
        } else if clock_range.contains(&x) {
            Some((INDICATOR_CLOCK_INDEX, clock_range.start))
        } else {
            None
        };

        if let Some((index, anchor_x)) = indicator {
            // Toggle: if this indicator is already open, close it
            if self.state.is_menu_open() && self.state.ui.selected_menu_item == Some(index) {
                self.state.close_indicator_modal();
                self.state.close_menu();
                return Ok(());
            }

            // Open menu state so Left/Right navigation works
            self.state.ui.menu_open = true;
            self.state.ui.selected_menu_item = Some(index);
            self.state.ui.close_all_submenus();

            if index == INDICATOR_CLOCK_INDEX {
                let modal = termide_modal::CalendarModal::new().with_anchor(anchor_x, 1);
                self.state.active_modal =
                    Some(termide_modal::ActiveModal::Calendar(Box::new(modal)));
                self.state.needs_redraw = true;
            } else {
                let kind = match index {
                    INDICATOR_NET_INDEX => crate::state::ResourceModalKind::Network,
                    INDICATOR_CPU_INDEX => crate::state::ResourceModalKind::Cpu,
                    _ => crate::state::ResourceModalKind::Ram,
                };
                self.open_resource_modal_at(kind, Some((anchor_x, 1)));
            }
            return Ok(());
        }

        Ok(())
    }

    /// Handle click on status bar (bottom row)
    fn handle_status_bar_click(&mut self, x: u16) -> Result<()> {
        // Clickable status-bar segments contributed by the focused panel
        // (editor: Pos / Tab / file-type / Hex / View-Edit; binary viewer:
        // Hex/Text). Layout matches the renderer's `segment_hit_areas`.
        let seg_action = {
            let segs = self
                .layout_manager
                .active_panel()
                .map(|p| p.status_segments())
                .unwrap_or_default();
            termide_ui_render::segment_hit_areas(&segs, 0)
                .into_iter()
                .find(|h| (h.start..h.end).contains(&x))
                .map(|h| h.action)
        };
        if let Some(action) = seg_action {
            use crate::panel_ext::PanelExt;
            // A segment action may both emit events and request a modal (e.g.
            // the editor's Tab segment opens the tab-size input).
            let (events, modal_request) = if let Some(p) = self.layout_manager.active_panel_mut() {
                (p.handle_status_action(action), p.take_modal_request())
            } else {
                (vec![], None)
            };
            self.process_panel_events(events)?;
            if let Some((act, modal)) = modal_request {
                self.handle_modal_request(act, modal)?;
            }
            self.state.needs_redraw = true;
            return Ok(());
        }

        // Check if disk indicator is present (right-aligned in status bar)
        // Disk indicator text is formatted as " DEVICE: used/totalGB (percent%) "
        // and is right-aligned, so it occupies the last N columns
        let disk_info = self.get_active_panel_disk_space();
        if let Some(disk) = disk_info {
            use termide_system_monitor::DiskSpaceInfoExt;
            let disk_text = format!(" {} ", disk.format_space());
            let disk_start = self
                .state
                .terminal
                .width
                .saturating_sub(disk_text.len() as u16);
            if x >= disk_start {
                use termide_ui_render::INDICATOR_DISK_INDEX;
                // Toggle: if this indicator is already open, close it
                if self.state.is_menu_open()
                    && self.state.ui.selected_menu_item == Some(INDICATOR_DISK_INDEX)
                {
                    self.state.close_indicator_modal();
                    self.state.close_menu();
                    return Ok(());
                }
                // Open as menu-integrated indicator
                self.state.ui.menu_open = true;
                self.state.ui.selected_menu_item = Some(INDICATOR_DISK_INDEX);
                self.state.ui.close_all_submenus();
                self.open_indicator_as_submenu(INDICATOR_DISK_INDEX);
                return Ok(());
            }
        }
        Ok(())
    }
}
