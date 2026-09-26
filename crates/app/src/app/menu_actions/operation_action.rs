//! Per-operation context menu — opened from the type icon on an
//! Operations panel card. Same look-and-feel as the panel `[≡]`
//! menu, scoped to one OperationId (Pause/Resume/Cancel).

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use termide_core::PanelEvent;
use termide_ui_render::{
    get_operation_action_menu_items, operation_action_dropdown_position, DropdownItem,
    OPERATION_ACTION_CANCEL, OPERATION_ACTION_PAUSE, OPERATION_ACTION_RESUME,
};

use super::super::App;

impl App {
    fn operation_action_menu_items(&self) -> Vec<DropdownItem> {
        get_operation_action_menu_items(
            self.state.ui.operation_action_menu.is_paused,
            self.state.ui.operation_action_menu.is_command,
        )
    }

    /// Handle a click while the operation action menu is open. Inside the
    /// dropdown → execute the corresponding action and close the menu;
    /// outside → just close.
    pub(in crate::app) fn handle_operation_action_menu_click(
        &mut self,
        x: u16,
        y: u16,
    ) -> Result<()> {
        let items = self.operation_action_menu_items();
        let (anchor_x, anchor_y) = (
            self.state.ui.operation_action_menu.anchor_x,
            self.state.ui.operation_action_menu.anchor_y,
        );
        let (dropdown_x, dropdown_y) = operation_action_dropdown_position(
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
            self.state.ui.operation_action_menu.selected,
            self.screen_rect(),
        ) {
            self.state.ui.operation_action_menu.selected = index;
            self.execute_operation_action_menu_action()?;
            return Ok(());
        }

        self.state.ui.operation_action_menu.close();
        self.state.needs_redraw = true;
        Ok(())
    }

    /// Keyboard navigation while the operation action menu is open.
    pub(in crate::app) fn handle_operation_action_menu_key(&mut self, key: KeyEvent) -> Result<()> {
        let items = self.operation_action_menu_items();
        let count = items.len();
        if count == 0 {
            self.state.ui.operation_action_menu.close();
            self.state.needs_redraw = true;
            return Ok(());
        }
        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Right => {
                self.state.ui.operation_action_menu.close();
                self.state.needs_redraw = true;
            }
            KeyCode::Up => {
                let sel = &mut self.state.ui.operation_action_menu.selected;
                *sel = if *sel == 0 { count - 1 } else { *sel - 1 };
                self.state.needs_redraw = true;
            }
            KeyCode::Down => {
                let sel = &mut self.state.ui.operation_action_menu.selected;
                *sel = (*sel + 1) % count;
                self.state.needs_redraw = true;
            }
            KeyCode::Enter => {
                self.execute_operation_action_menu_action()?;
            }
            _ => {}
        }
        Ok(())
    }

    fn execute_operation_action_menu_action(&mut self) -> Result<()> {
        let items = self.operation_action_menu_items();
        let selected = self.state.ui.operation_action_menu.selected;
        let key = match items.get(selected) {
            Some(i) => i.key.clone(),
            None => {
                self.state.ui.operation_action_menu.close();
                self.state.needs_redraw = true;
                return Ok(());
            }
        };

        let op_id = termide_file_ops::OperationId(self.state.ui.operation_action_menu.op_id);
        self.state.ui.operation_action_menu.close();
        self.state.needs_redraw = true;

        // Route through the same PanelEvent path the keyboard shortcuts
        // (Space / Esc / Delete) use, so popup-menu and key-driven cancels
        // run identical follow-up logic — e.g. the "Delete partial upload?"
        // modal is wired to the PanelEvent::CancelOperation handler.
        match key.as_str() {
            OPERATION_ACTION_PAUSE | OPERATION_ACTION_RESUME => {
                self.process_panel_events(vec![PanelEvent::ToggleOperationPause(op_id)])?;
            }
            OPERATION_ACTION_CANCEL => {
                // Cancelling always asks for confirmation first, same as the
                // keyboard shortcuts.
                let t = termide_i18n::t();
                self.process_panel_events(vec![PanelEvent::ShowConfirm {
                    message: t.operation_cancel_confirm().to_string(),
                    on_confirm: termide_core::ConfirmAction::CancelOperation(op_id),
                }])?;
            }
            _ => {}
        }
        Ok(())
    }
}
