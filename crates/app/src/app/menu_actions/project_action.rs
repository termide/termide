//! The current project's menu, opened from the `≡` after its name in the
//! menu bar: move, copy, change root, close.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use termide_ui_render::{
    get_project_action_menu_items, panel_action_dropdown_position, DropdownItem,
    PROJECT_ACTION_CHANGE_ROOT, PROJECT_ACTION_CLOSE, PROJECT_ACTION_COPY, PROJECT_ACTION_MOVE,
};

use super::super::App;
use crate::state::ProjectsOrigin;

impl App {
    /// Open the current project's menu under its `≡` at `anchor_x`.
    pub(in crate::app) fn open_project_action_menu(&mut self, anchor_x: u16) {
        self.state.close_menu();
        self.state.ui.close_all_submenus();
        self.state.ui.project_action_menu.open(anchor_x);
        self.state.needs_redraw = true;
    }

    fn project_action_menu_items(&self) -> Vec<DropdownItem> {
        get_project_action_menu_items(self.open_projects.count() > 1)
    }

    /// Handle a click while the menu is open: run the item clicked, or
    /// close the menu for a click elsewhere.
    pub(in crate::app) fn handle_project_action_menu_click(
        &mut self,
        x: u16,
        y: u16,
    ) -> Result<()> {
        let items = self.project_action_menu_items();
        let (dropdown_x, dropdown_y) = panel_action_dropdown_position(
            &items,
            self.state.ui.project_action_menu.anchor_x,
            0,
            self.state.terminal.width,
            self.state.terminal.height,
        );
        if let Some(index) = super::super::mouse::submenu::hit_dropdown_item(
            x,
            y,
            dropdown_x,
            dropdown_y,
            &items,
            self.state.ui.project_action_menu.selected,
            self.screen_rect(),
        ) {
            self.state.ui.project_action_menu.selected = index;
            return self.execute_project_action_menu_action();
        }
        self.state.ui.project_action_menu.close();
        self.state.needs_redraw = true;
        Ok(())
    }

    /// Handle a key while the menu is open. The keys its items show do
    /// their items here too.
    pub(in crate::app) fn handle_project_action_menu_key(&mut self, key: KeyEvent) -> Result<()> {
        let items = self.project_action_menu_items();
        let count = items.len();
        let by_key = |action: &str| items.iter().position(|item| item.key == action);
        let shortcut = match key.code {
            KeyCode::F(6) => by_key(PROJECT_ACTION_MOVE),
            KeyCode::F(5) => by_key(PROJECT_ACTION_COPY),
            KeyCode::F(8) | KeyCode::Delete => by_key(PROJECT_ACTION_CLOSE),
            _ => None,
        };
        if let Some(index) = shortcut {
            self.state.ui.project_action_menu.selected = index;
            return self.execute_project_action_menu_action();
        }
        let menu = &mut self.state.ui.project_action_menu;
        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Right => menu.close(),
            KeyCode::Up => menu.selected = (menu.selected + count - 1) % count,
            KeyCode::Down => menu.selected = (menu.selected + 1) % count,
            KeyCode::Enter => return self.execute_project_action_menu_action(),
            _ => return Ok(()),
        }
        self.state.needs_redraw = true;
        Ok(())
    }

    /// Run the selected item on the current project.
    fn execute_project_action_menu_action(&mut self) -> Result<()> {
        let items = self.project_action_menu_items();
        let key = items
            .get(self.state.ui.project_action_menu.selected)
            .map(|item| item.key.clone());
        self.state.ui.project_action_menu.close();
        self.state.needs_redraw = true;
        let root = self.project_root.clone();
        let button = self.open_projects.position(&root).unwrap_or(0);
        let from = ProjectsOrigin::Button(button);
        match key.as_deref() {
            Some(PROJECT_ACTION_MOVE) => self.prompt_project_transfer(root, false, from),
            Some(PROJECT_ACTION_COPY) => self.prompt_project_transfer(root, true, from),
            Some(PROJECT_ACTION_CHANGE_ROOT) => self.handle_change_root_path()?,
            Some(PROJECT_ACTION_CLOSE) => self.confirm_close_project(root, from),
            _ => {}
        }
        Ok(())
    }
}
