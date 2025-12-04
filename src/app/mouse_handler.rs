use anyhow::Result;
use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};

use super::App;
use crate::{
    constants::DEFAULT_FM_WIDTH,
    ui::dropdown::{get_help_items, get_tools_items},
};

impl App {
    /// Handle mouse event
    pub(super) fn handle_mouse_event(&mut self, mouse: crossterm::event::MouseEvent) -> Result<()> {
        // Handle modal mouse events first if a modal is open
        if self.state.active_modal.is_some() {
            let modal_area = Rect {
                x: 0,
                y: 0,
                width: self.state.terminal.width,
                height: self.state.terminal.height,
            };
            self.handle_modal_mouse(mouse, modal_area)?;
            return Ok(());
        }

        // Click on menu
        if mouse.row == 0 && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            self.handle_menu_click(mouse.column)?;
            return Ok(());
        }

        // Click on dropdown when menu is open
        if self.state.ui.menu_open && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            self.handle_dropdown_click(mouse.column, mouse.row)?;
            return Ok(());
        }

        // If menu is open, close it on click outside menu
        if self.state.ui.menu_open && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            self.state.close_menu();
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

        // For scroll - forward to panel under cursor (doesn't require focus)
        if matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            self.forward_scroll_to_panel_at_cursor(mouse)?;
            return Ok(());
        }

        // Other mouse events - to active panel
        self.forward_mouse_to_panel(mouse)?;

        Ok(())
    }

    /// Forward mouse event to active panel
    fn forward_mouse_to_panel(&mut self, mouse: crossterm::event::MouseEvent) -> Result<()> {
        // Determine active panel area
        let panel_area = self.get_active_panel_area();

        // Handle mouse event and collect results
        let file_to_open = if let Some(panel) = self.layout_manager.active_panel_mut() {
            panel.handle_mouse(mouse, panel_area)?;
            panel.take_file_to_open()
        } else {
            None
        };

        // Handle file opening in editor (same logic as in key_handler.rs)
        if let Some(file_path) = file_to_open {
            self.close_welcome_panels();
            let filename = file_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?");
            let t = crate::i18n::t();
            crate::logger::info(format!("Attempting to open file: {}", filename));

            match crate::panels::editor::Editor::open_file_with_config(
                file_path.clone(),
                self.state.editor_config(),
            ) {
                Ok(editor_panel) => {
                    self.add_panel(Box::new(editor_panel));
                    crate::logger::info(format!("File '{}' opened in editor", filename));
                    self.state.set_info(t.editor_file_opened(filename));
                }
                Err(e) => {
                    let error_msg = t.status_error_open_file(filename, &e.to_string());
                    crate::logger::error(format!("Error opening '{}': {}", filename, e));
                    self.state.set_error(error_msg);
                }
            }
        }

        Ok(())
    }

    /// Forward scroll to panel under mouse cursor
    fn forward_scroll_to_panel_at_cursor(
        &mut self,
        mouse: crossterm::event::MouseEvent,
    ) -> Result<()> {
        // TODO: Implement proper scroll forwarding with LayoutManager
        // For now, forward to active panel
        let panel_area = self.get_active_panel_area();
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            panel.handle_mouse(mouse, panel_area)?;
        }

        Ok(())
    }

    /// Get active panel area
    fn get_active_panel_area(&self) -> Rect {
        // Use calculate_panel_rects() to get all panel areas with proper layout calculation
        let panel_rects = self.calculate_panel_rects();

        // Find the active panel based on current focus
        match self.layout_manager.focus {
            crate::layout_manager::FocusTarget::FileManager => {
                // Find FileManager rect (marked with group_idx = usize::MAX)
                for (group_idx, _panel_idx, rect, _is_expanded) in panel_rects {
                    if group_idx == usize::MAX {
                        return rect;
                    }
                }
            }
            crate::layout_manager::FocusTarget::Group(focused_group_idx) => {
                // Find expanded panel in the focused group
                for (group_idx, _panel_idx, rect, is_expanded) in panel_rects {
                    if group_idx == focused_group_idx && is_expanded {
                        return rect;
                    }
                }
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

    /// Handle click on panel [X] button or [▶]/[▼] expand/collapse button
    /// Returns true if a button was clicked
    fn handle_panel_close_click(&mut self, click_x: u16, click_y: u16) -> Result<bool> {
        let panel_rects = self.calculate_panel_rects();

        for (group_idx, panel_idx, rect, is_expanded) in panel_rects {
            // Check if click is on this panel's top line
            if click_y != rect.y {
                continue;
            }

            // Check if click is within the panel's horizontal bounds
            if click_x < rect.x || click_x >= rect.x + rect.width {
                continue;
            }

            let relative_x = click_x - rect.x;

            // Button format: ─[X][▶] Title ─── (collapsed)
            //          or:   ┌[X][▼] Title ──┐ (expanded)
            // [X] button: offsets 1-3
            // [▶]/[▼] button: offsets 4-6

            if (1..=3).contains(&relative_x) {
                // Click on [X] button - close panel with confirmation if needed
                crate::logger::debug("Panel close button [X] clicked");
                // First, activate the clicked panel
                if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                    group.set_expanded(panel_idx);
                }
                self.layout_manager.focus = crate::layout_manager::FocusTarget::Group(group_idx);

                // Now use the same close logic as keyboard shortcut (with confirmation)
                self.handle_close_panel_request(0)?;
                return Ok(true);
            } else if (4..=6).contains(&relative_x) {
                // Click on [▶]/[▼] button - expand/collapse panel
                if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                    if is_expanded && group.len() > 1 {
                        // Currently expanded - collapse by expanding next panel
                        let next_idx = (panel_idx + 1) % group.len();
                        group.set_expanded(next_idx);
                    } else {
                        // Currently collapsed - expand this panel
                        group.set_expanded(panel_idx);
                        // Also make this group active
                        self.layout_manager.focus =
                            crate::layout_manager::FocusTarget::Group(group_idx);
                    }
                }
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
                // Check if this is the FileManager (marked with group_idx = usize::MAX)
                if group_idx == usize::MAX {
                    self.layout_manager.focus = crate::layout_manager::FocusTarget::FileManager;
                } else {
                    // Click on a regular panel group - make it active
                    self.layout_manager.focus =
                        crate::layout_manager::FocusTarget::Group(group_idx);
                    if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                        group.set_expanded(panel_idx);
                    }
                }
                return Ok(());
            }
        }

        Ok(())
    }

    /// Handle click on menu
    fn handle_menu_click(&mut self, x: u16) -> Result<()> {
        let mut current_x = 1_u16;

        // Get menu items with translations
        let menu_items = crate::ui::menu::get_menu_items();

        for (i, item) in menu_items.iter().enumerate() {
            let item_width = item.len() as u16;
            if x >= current_x && x < current_x + item_width {
                // Set selected item and immediately execute action
                self.state.ui.selected_menu_item = Some(i);
                self.execute_menu_action()?;
                return Ok(());
            }
            current_x += item_width + 2; // +2 for spaces
        }

        Ok(())
    }

    /// Handle click on dropdown
    fn handle_dropdown_click(&mut self, _x: u16, y: u16) -> Result<()> {
        if let Some(menu_index) = self.state.ui.selected_menu_item {
            // Dropdown starts from row 1
            if y >= 2 {
                // -2 for menu row and top border of dropdown
                let item_index = (y - 2) as usize;

                let item_count = match menu_index {
                    0 => get_tools_items().len(),
                    1 => get_help_items().len(),
                    _ => 0,
                };

                if item_index < item_count {
                    self.state.ui.selected_dropdown_item = item_index;
                    self.execute_menu_action()?;
                }
            }
        }

        Ok(())
    }

    /// Calculate panel rectangles for mouse hit testing
    /// Returns Vec<(group_idx, panel_idx, rect, is_expanded)>
    fn calculate_panel_rects(&self) -> Vec<(usize, usize, Rect, bool)> {
        let mut result = Vec::new();

        let width = self.state.terminal.width;
        let height = self.state.terminal.height;

        // Main area: from row 1 to height-2 (excluding menu and status bar)
        let main_area = Rect {
            x: 0,
            y: 1,
            width,
            height: height.saturating_sub(2),
        };

        // Calculate horizontal split: FM | Groups
        let has_fm = self.layout_manager.has_file_manager();
        let fm_width = if has_fm { DEFAULT_FM_WIDTH } else { 0 };

        let horizontal_constraints = if has_fm {
            vec![
                Constraint::Length(fm_width),
                Constraint::Min(0), // Groups area
            ]
        } else {
            vec![Constraint::Min(0)] // Only groups
        };

        let horizontal_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(horizontal_constraints)
            .split(main_area);

        let chunk_offset = if has_fm { 1 } else { 0 };

        // Add FileManager to click detection if it exists
        if has_fm {
            let fm_area = horizontal_chunks[0];
            // FileManager uses special marker: group_idx = usize::MAX
            // This lets handle_panel_focus_click() distinguish it from regular groups
            result.push((usize::MAX, 0, fm_area, true));
        }

        // Calculate group areas
        if !self.layout_manager.panel_groups.is_empty() {
            let groups_area = horizontal_chunks[chunk_offset];

            // Calculate horizontal constraints for groups (using widths)
            // Группы могут иметь фиксированную ширину или auto-width
            let group_constraints: Vec<Constraint> = self
                .layout_manager
                .panel_groups
                .iter()
                .map(|g| {
                    let width = g.width.unwrap_or(groups_area.width);
                    Constraint::Length(width.max(20))
                })
                .collect();

            let group_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(group_constraints)
                .split(groups_area);

            // Process each group
            for (group_idx, group) in self.layout_manager.panel_groups.iter().enumerate() {
                if group.is_empty() || group_chunks[group_idx].height == 0 {
                    continue;
                }

                let group_area = group_chunks[group_idx];
                let expanded_idx = group.expanded_index();

                // Build vertical constraints for panels in group
                let vertical_constraints: Vec<Constraint> = (0..group.len())
                    .map(|i| {
                        if i == expanded_idx {
                            Constraint::Min(0) // Expanded panel
                        } else {
                            Constraint::Length(1) // Collapsed panel (1 line)
                        }
                    })
                    .collect();

                let vertical_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(vertical_constraints)
                    .split(group_area);

                // Add each panel's rect to results
                for panel_idx in 0..group.len() {
                    let is_expanded = panel_idx == expanded_idx;
                    result.push((
                        group_idx,
                        panel_idx,
                        vertical_chunks[panel_idx],
                        is_expanded,
                    ));
                }
            }
        }

        result
    }
}
