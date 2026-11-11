//! Command palette modal result handler.

use anyhow::Result;

use super::super::App;

impl App {
    /// Handle result from the command palette modal.
    ///
    /// `value` carries a `usize` — the index into `command_palette_actions`.
    pub(in crate::app) fn handle_command_palette_result(
        &mut self,
        value: termide_modal::ModalOutput,
    ) -> Result<()> {
        let index = match value.get::<usize>() {
            Some(&i) => i,
            None => return Ok(()),
        };

        if let Some(actions) = self.command_palette_actions.take() {
            if let Some(action_name) = actions.into_iter().nth(index) {
                self.handle_app_action_by_name(&action_name)?;
            }
        }

        Ok(())
    }
}
