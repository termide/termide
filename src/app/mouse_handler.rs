use anyhow::Result;
use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::layout::Rect;

use super::App;
use crate::{
    state::LayoutMode,
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
        let file_to_open = if let Some(panel) = self.panels.get_mut(self.state.active_panel) {
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
            self.state
                .log_info(format!("Attempting to open file: {}", filename));

            match crate::panels::editor::Editor::open_file(file_path.clone()) {
                Ok(editor_panel) => {
                    self.panels.add_panel(Box::new(editor_panel));
                    let new_panel_index = self.panels.count().saturating_sub(1);
                    self.state.set_active_panel(new_panel_index);
                    self.state
                        .log_success(format!("File '{}' opened in editor", filename));
                    self.state.set_info(t.editor_file_opened(filename));
                }
                Err(e) => {
                    let error_msg = t.status_error_open_file(filename, &e.to_string());
                    self.state
                        .log_error(format!("Error opening '{}': {}", filename, e));
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
        let height = self.state.terminal.height;

        // Check that cursor is in panel area (not menu and not status bar)
        if mouse.row < 1 || mouse.row >= height - 1 {
            return Ok(());
        }

        match self.state.layout_mode {
            LayoutMode::Single => {
                // In Single mode one panel - forward scroll to active
                let panel_area = self.get_active_panel_area();
                if let Some(panel) = self.panels.get_mut(self.state.active_panel) {
                    panel.handle_mouse(mouse, panel_area)?;
                }
            }
            LayoutMode::MultiPanel => {
                let fm_width = self.state.layout_info.fm_width.unwrap_or(30);
                let width = self.state.terminal.width;

                // Determine panel under cursor
                if mouse.column < fm_width {
                    // Cursor over FM (panel 0)
                    let panel_area = Rect {
                        x: 0,
                        y: 1,
                        width: fm_width,
                        height: height.saturating_sub(2),
                    };
                    if let Some(panel) = self.panels.get_mut(0) {
                        panel.handle_mouse(mouse, panel_area)?;
                    }
                } else {
                    // Cursor over main panels
                    let visible_main: Vec<usize> = self
                        .panels
                        .visible_indices()
                        .into_iter()
                        .filter(|&i| i > 0)
                        .collect();

                    if !visible_main.is_empty() {
                        let main_area_width = width - fm_width;
                        let panel_width = main_area_width / visible_main.len() as u16;

                        for (chunk_idx, &panel_index) in visible_main.iter().enumerate() {
                            let panel_x = fm_width + (chunk_idx as u16 * panel_width);

                            if mouse.column >= panel_x && mouse.column < panel_x + panel_width {
                                let panel_area = Rect {
                                    x: panel_x,
                                    y: 1,
                                    width: panel_width,
                                    height: height.saturating_sub(2),
                                };
                                if let Some(panel) = self.panels.get_mut(panel_index) {
                                    panel.handle_mouse(mouse, panel_area)?;
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Get active panel area
    fn get_active_panel_area(&self) -> Rect {
        let width = self.state.terminal.width;
        let height = self.state.terminal.height;

        match self.state.layout_mode {
            LayoutMode::Single => {
                // In Single mode panel occupies full width
                Rect {
                    x: 0,
                    y: 1,
                    width,
                    height: height.saturating_sub(2),
                }
            }
            LayoutMode::MultiPanel => {
                let fm_width = self.state.layout_info.fm_width.unwrap_or(30);
                let active_index = self.state.active_panel;

                if active_index == 0 {
                    // FM panel on the left
                    Rect {
                        x: 0,
                        y: 1,
                        width: fm_width,
                        height: height.saturating_sub(2),
                    }
                } else {
                    // Main panels on the right
                    let visible_main: Vec<usize> = self
                        .panels
                        .visible_indices()
                        .into_iter()
                        .filter(|&i| i > 0)
                        .collect();

                    if let Some(chunk_idx) = visible_main.iter().position(|&i| i == active_index) {
                        let main_area_width = width - fm_width;
                        let panel_width = main_area_width / visible_main.len() as u16;
                        let panel_x = fm_width + (chunk_idx as u16 * panel_width);

                        Rect {
                            x: panel_x,
                            y: 1,
                            width: panel_width,
                            height: height.saturating_sub(2),
                        }
                    } else {
                        // Fallback
                        Rect {
                            x: fm_width,
                            y: 1,
                            width: width - fm_width,
                            height: height.saturating_sub(2),
                        }
                    }
                }
            }
        }
    }

    /// Handle click on panel [X] button
    /// Returns true if panel was closed
    fn handle_panel_close_click(&mut self, click_x: u16, click_y: u16) -> Result<bool> {
        // Panel area: rows 1..(height-1), last row - status
        let height = self.state.terminal.height;

        // Check that click is in panel area
        if click_y < 1 || click_y >= height - 1 {
            return Ok(false);
        }

        match self.state.layout_mode {
            LayoutMode::Single => {
                // In Single mode one panel occupies full width
                if self.panels.has_visible_panels() {
                    let active_index = self.state.active_panel;

                    if let Some(panel) = self.panels.get_mut(active_index) {
                        let title = panel.title();
                        let can_close =
                            crate::ui::panel_helpers::can_close_panel(active_index, &self.state);

                        // Check click on [X] of active panel
                        if crate::ui::panel_helpers::is_click_on_close_button(
                            click_x, click_y, 0, 1, &title, can_close,
                        ) {
                            self.panels.close_panel(active_index);

                            // Switch to next visible panel
                            if self.panels.count() > 0 {
                                let visible = self.panels.visible_indices();
                                if !visible.is_empty() {
                                    if active_index >= self.panels.count() {
                                        self.state.active_panel = *visible.last().unwrap();
                                    } else {
                                        self.state.active_panel = visible
                                            .iter()
                                            .find(|&&i| i >= active_index)
                                            .or_else(|| visible.last())
                                            .copied()
                                            .unwrap_or(0);
                                    }
                                }
                            }
                            return Ok(true);
                        }
                    }
                }
            }
            LayoutMode::MultiPanel => {
                // In MultiPanel mode: FM on left (30 characters) + main panels on right
                let fm_width = self.state.layout_info.fm_width.unwrap_or(30);

                // Check click on [X] FM (panel 0)
                if click_x < fm_width {
                    if let Some(fm_panel) = self.panels.get_mut(0) {
                        let title = fm_panel.title();
                        let can_close = crate::ui::panel_helpers::can_close_panel(0, &self.state);

                        if crate::ui::panel_helpers::is_click_on_close_button(
                            click_x, click_y, 0, 1, &title, can_close,
                        ) {
                            // FM cannot be closed in MultiPanel mode (can_close will be false)
                            return Ok(false);
                        }
                    }
                }

                // Check click on [X] of main panels
                let visible_main: Vec<usize> = self
                    .panels
                    .visible_indices()
                    .into_iter()
                    .filter(|&i| i > 0)
                    .collect();

                if !visible_main.is_empty() {
                    let width = self.state.terminal.width;
                    let main_area_width = width - fm_width;
                    let panel_width = main_area_width / visible_main.len() as u16;

                    for (chunk_idx, &panel_index) in visible_main.iter().enumerate() {
                        let panel_x = fm_width + (chunk_idx as u16 * panel_width);

                        // Get panel information
                        let (title, can_close) = if let Some(panel) =
                            self.panels.get_mut(panel_index)
                        {
                            (
                                panel.title(),
                                crate::ui::panel_helpers::can_close_panel(panel_index, &self.state),
                            )
                        } else {
                            continue;
                        };

                        if crate::ui::panel_helpers::is_click_on_close_button(
                            click_x, click_y, panel_x, 1, &title, can_close,
                        ) {
                            // Close panel
                            self.panels.close_panel(panel_index);

                            // Switch to next visible panel
                            if self.panels.count() > 0 {
                                let visible = self.panels.visible_indices();
                                if !visible.is_empty() {
                                    if panel_index == self.state.active_panel {
                                        // If closed active panel
                                        self.state.active_panel = visible
                                            .iter()
                                            .find(|&&i| i >= panel_index)
                                            .or_else(|| visible.last())
                                            .copied()
                                            .unwrap_or(0);
                                    } else if panel_index < self.state.active_panel {
                                        // If closed panel left of active, shift index
                                        if self.state.active_panel > 0 {
                                            self.state.active_panel -= 1;
                                        }
                                    }
                                }
                            }
                            return Ok(true);
                        }
                    }
                }
            }
        }

        Ok(false)
    }

    /// Handle click on panel to switch focus
    fn handle_panel_focus_click(&mut self, click_x: u16, click_y: u16) -> Result<()> {
        // Panel area: rows 1..(height-1), last row - status
        let height = self.state.terminal.height;

        // Check that click is in panel area
        if click_y < 1 || click_y >= height - 1 {
            return Ok(());
        }

        match self.state.layout_mode {
            LayoutMode::Single => {
                // In Single mode one panel, focus already on it
                // Do nothing
            }
            LayoutMode::MultiPanel => {
                // In MultiPanel mode: FM on left (30 characters) + main panels on right
                let fm_width = self.state.layout_info.fm_width.unwrap_or(30);

                // Check click on FM (panel 0)
                if click_x < fm_width {
                    self.state.set_active_panel(0);
                    return Ok(());
                }

                // Check click on main panels
                let visible_main: Vec<usize> = self
                    .panels
                    .visible_indices()
                    .into_iter()
                    .filter(|&i| i > 0)
                    .collect();

                if !visible_main.is_empty() {
                    let width = self.state.terminal.width;
                    let main_area_width = width - fm_width;
                    let panel_width = main_area_width / visible_main.len() as u16;

                    for (chunk_idx, &panel_index) in visible_main.iter().enumerate() {
                        let panel_x_start = fm_width + (chunk_idx as u16 * panel_width);
                        let panel_x_end = panel_x_start + panel_width;

                        if click_x >= panel_x_start && click_x < panel_x_end {
                            self.state.set_active_panel(panel_index);
                            return Ok(());
                        }
                    }
                }
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
}
