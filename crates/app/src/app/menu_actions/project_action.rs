//! An open project's menu, opened from its button in the menu bar as a menu
//! title opens its dropdown: switch, move, copy, change root, close.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use termide_ui_render::{
    panel_action_dropdown_position, PROJECT_ACTION_CHANGE_ROOT, PROJECT_ACTION_CLOSE,
    PROJECT_ACTION_COPY, PROJECT_ACTION_MOVE, PROJECT_ACTION_SWITCH,
};

use super::super::App;
use crate::state::ProjectsOrigin;

impl App {
    /// Open the menu of project button `index` under the button, the menu
    /// bar selection on it. The status bar shows the project's full path,
    /// which its button may have cut.
    pub(in crate::app) fn open_project_button_menu(&mut self, index: usize) {
        self.state.close_indicator_modal();
        self.state.ui.close_all_submenus();
        let anchor_x = self.menu_bar().project_x(index).unwrap_or(0);
        self.state.ui.project_action_menu.open(index, anchor_x);
        if let Some(view) = self.state.open_projects.get(index) {
            let path = termide_core::util::shorten_home_path(&view.root.display().to_string());
            self.state.set_info(path);
        }
        self.state.needs_redraw = true;
    }

    /// Handle a click below the menu bar while the menu is open: run the
    /// item clicked, or close the menu bar for a click elsewhere.
    pub(in crate::app) fn handle_project_action_menu_click(
        &mut self,
        x: u16,
        y: u16,
    ) -> Result<()> {
        let items = self.state.project_action_items();
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
        self.state.close_menu();
        self.state.needs_redraw = true;
        Ok(())
    }

    /// Handle a key while the menu is open. The keys its items show do
    /// their items here too; `←`/`→` walk the menu bar, and the keys that
    /// move a project in the Projects menu move this one's button.
    pub(in crate::app) fn handle_project_action_menu_key(&mut self, key: KeyEvent) -> Result<()> {
        if let Some(step) = self.project_step_of(&key) {
            let project = self.state.ui.project_action_menu.project;
            self.move_project_button(project, step);
            return Ok(());
        }
        let items = self.state.project_action_items();
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
            KeyCode::Esc => self.state.close_menu(),
            KeyCode::Left => return self.switch_to_prev_menu(),
            KeyCode::Right => return self.switch_to_next_menu(),
            KeyCode::Up => menu.choose_prev(count),
            KeyCode::Down => menu.choose_next(count),
            KeyCode::Enter if menu.chosen => return self.execute_project_action_menu_action(),
            _ => return Ok(()),
        }
        self.state.needs_redraw = true;
        Ok(())
    }

    /// Run the selected item on the menu's project.
    fn execute_project_action_menu_action(&mut self) -> Result<()> {
        let items = self.state.project_action_items();
        let key = items
            .get(self.state.ui.project_action_menu.selected)
            .map(|item| item.key.clone());
        let button = self.state.ui.project_action_menu.project;
        self.state.close_menu();
        self.state.needs_redraw = true;
        let Some(root) = self.listed_open_roots().get(button).cloned() else {
            return Ok(());
        };
        let from = ProjectsOrigin::Button(button);
        match key.as_deref() {
            Some(PROJECT_ACTION_SWITCH) => self.switch_to_project(root)?,
            Some(PROJECT_ACTION_MOVE) => self.prompt_project_transfer(root, false, from),
            Some(PROJECT_ACTION_COPY) => self.prompt_project_transfer(root, true, from),
            Some(PROJECT_ACTION_CHANGE_ROOT) => self.handle_change_root_path()?,
            Some(PROJECT_ACTION_CLOSE) => self.confirm_close_project(root, from),
            _ => {}
        }
        Ok(())
    }
}
