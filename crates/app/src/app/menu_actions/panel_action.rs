//! Panel action context menu — opened from the `[≡]` button on a panel header.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use termide_ui_render::{
    get_panel_action_menu_items, panel_action_dropdown_position, DropdownItem, PANEL_ACTION_CLOSE,
    PANEL_ACTION_MOVE_DOWN, PANEL_ACTION_MOVE_LEFT, PANEL_ACTION_MOVE_RIGHT, PANEL_ACTION_MOVE_UP,
    PANEL_ACTION_SPLIT,
};

use super::super::App;

impl App {
    /// Open the panel action menu for the currently active panel. Anchors the
    /// dropdown to the panel's `[≡]` button (top-left of the header).
    pub(in crate::app) fn open_panel_action_menu_for_active(&mut self) -> Result<()> {
        let group_idx = self.layout_manager.focus;
        let panel_idx = match self.layout_manager.panel_groups.get(group_idx) {
            Some(group) if !group.is_empty() => group.expanded_index(),
            _ => return Ok(()),
        };

        let anchor = self
            .calculate_panel_rects()
            .into_iter()
            .find(|(gi, pi, _, _)| *gi == group_idx && *pi == panel_idx)
            .map(|(_, _, rect, _)| (rect.x, rect.y));

        let (anchor_x, anchor_y) = match anchor {
            Some(xy) => xy,
            None => return Ok(()),
        };

        self.state.ui.close_all_submenus();
        self.state
            .ui
            .panel_action_menu
            .open(group_idx, panel_idx, anchor_x, anchor_y);
        self.state.needs_redraw = true;
        Ok(())
    }

    /// Build items for the currently-open panel action menu, using the
    /// targeted panel's group index to derive context-dependent filters.
    /// Panel-specific entries (e.g. "Copy diagram") are prepended above the
    /// generic move/close items.
    fn panel_action_menu_items(&self) -> Vec<DropdownItem> {
        let group_count = self.layout_manager.panel_groups.len();
        let group_idx = self.state.ui.panel_action_menu.group_idx;
        let panel_idx = self.state.ui.panel_action_menu.panel_idx;
        let current_group_len = self
            .layout_manager
            .panel_groups
            .get(group_idx)
            .map(|g| g.len())
            .unwrap_or(0);

        let mut items: Vec<DropdownItem> = self
            .layout_manager
            .panel_groups
            .get(group_idx)
            .and_then(|g| g.panels().get(panel_idx))
            .map(|p| {
                p.context_menu_items()
                    .into_iter()
                    .map(|(label, action)| DropdownItem::new(&label, action))
                    .collect()
            })
            .unwrap_or_default();
        items.extend(get_panel_action_menu_items(
            group_count,
            current_group_len,
            Some(&self.state.config.general.keybindings),
        ));
        items
    }

    /// Activate the panel that the menu was opened for, so actions that
    /// operate on the active panel target the correct one.
    fn activate_menu_target_panel(&mut self) {
        let group_idx = self.state.ui.panel_action_menu.group_idx;
        let panel_idx = self.state.ui.panel_action_menu.panel_idx;
        if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
            if panel_idx < group.len() {
                group.set_expanded(panel_idx);
            }
        }
        self.layout_manager.focus = group_idx;
    }

    /// Handle a click while the panel action menu is open. If the click is
    /// inside the dropdown, execute the corresponding action; otherwise close
    /// the menu.
    pub(in crate::app) fn handle_panel_action_menu_click(&mut self, x: u16, y: u16) -> Result<()> {
        let items = self.panel_action_menu_items();
        let (anchor_x, anchor_y) = (
            self.state.ui.panel_action_menu.anchor_x,
            self.state.ui.panel_action_menu.anchor_y,
        );
        let (dropdown_x, dropdown_y) = panel_action_dropdown_position(
            &items,
            anchor_x,
            anchor_y,
            self.state.terminal.width,
            self.state.terminal.height,
        );

        if let Some(index) = super::super::mouse::submenu::hit_dropdown_item(
            x,
            y,
            dropdown_x,
            dropdown_y,
            &items,
            self.state.ui.panel_action_menu.selected,
            self.screen_rect(),
        ) {
            self.state.ui.panel_action_menu.selected = index;
            self.execute_panel_action_menu_action()?;
            return Ok(());
        }

        self.state.ui.panel_action_menu.close();
        self.state.needs_redraw = true;
        Ok(())
    }

    /// Handle a key while the panel action menu is open.
    pub(in crate::app) fn handle_panel_action_menu_key(&mut self, key: KeyEvent) -> Result<()> {
        let items = self.panel_action_menu_items();
        let count = items.len();
        if count == 0 {
            self.state.ui.panel_action_menu.close();
            self.state.needs_redraw = true;
            return Ok(());
        }

        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Right => {
                self.state.ui.panel_action_menu.close();
                self.state.needs_redraw = true;
            }
            KeyCode::Up => {
                let sel = &mut self.state.ui.panel_action_menu.selected;
                *sel = if *sel == 0 { count - 1 } else { *sel - 1 };
                self.state.needs_redraw = true;
            }
            KeyCode::Down => {
                let sel = &mut self.state.ui.panel_action_menu.selected;
                *sel = (*sel + 1) % count;
                self.state.needs_redraw = true;
            }
            KeyCode::Enter => {
                self.execute_panel_action_menu_action()?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Execute the currently-selected panel action menu item.
    fn execute_panel_action_menu_action(&mut self) -> Result<()> {
        let items = self.panel_action_menu_items();
        let selected = self.state.ui.panel_action_menu.selected;
        let key = match items.get(selected) {
            Some(i) => i.key.clone(),
            None => {
                self.state.ui.panel_action_menu.close();
                self.state.needs_redraw = true;
                return Ok(());
            }
        };

        self.activate_menu_target_panel();
        self.state.ui.panel_action_menu.close();
        self.state.needs_redraw = true;

        let terminal_width = self.state.terminal.width;
        let layout_result: Option<(&str, Result<()>)> = match key.as_str() {
            PANEL_ACTION_CLOSE => {
                self.handle_close_panel_request()?;
                None
            }
            PANEL_ACTION_SPLIT => Some((
                "Cannot toggle stacking",
                self.layout_manager.toggle_panel_stacking(terminal_width),
            )),
            PANEL_ACTION_MOVE_LEFT => Some((
                "Cannot move panel",
                self.layout_manager.move_panel_to_prev_group(terminal_width),
            )),
            PANEL_ACTION_MOVE_RIGHT => Some((
                "Cannot move panel",
                self.layout_manager.move_panel_to_next_group(terminal_width),
            )),
            PANEL_ACTION_MOVE_UP => Some((
                "Cannot move panel",
                self.layout_manager.move_panel_up_in_group(),
            )),
            PANEL_ACTION_MOVE_DOWN => Some((
                "Cannot move panel",
                self.layout_manager.move_panel_down_in_group(),
            )),
            // Panel-specific entry (e.g. "Copy diagram"): route to the now-active
            // panel via the same path as a status-bar chip click.
            other => {
                let events = self
                    .layout_manager
                    .active_panel_mut()
                    .map(|p| p.handle_status_action(other))
                    .unwrap_or_default();
                self.process_panel_events(events)?;
                None
            }
        };

        if let Some((label, result)) = layout_result {
            self.handle_layout_op(label, result);
        }
        Ok(())
    }
}
