//! Modal-prompt event handlers (confirm, input, select).

use crate::app::App;
use termide_i18n as i18n;

impl App {
    /// Handle ShowSelect event - show selection modal
    pub(super) fn event_show_select(
        &mut self,
        title: String,
        options: Vec<String>,
        on_select: termide_core::SelectAction,
    ) {
        use crate::state::{ActiveModal, PendingAction};
        use termide_modal::SelectModal;

        // Map SelectAction to PendingAction
        let pending_action = match on_select {
            // A panel-defined selection (e.g. an agent permission prompt);
            // the chosen index goes back to the panels as
            // `PanelCommand::SelectionMade`.
            termide_core::SelectAction::Custom(action) => PendingAction::PanelSelection { action },
        };

        let modal = SelectModal::single(title, "", options);
        self.state
            .set_pending_action(pending_action, ActiveModal::Select(Box::new(modal)));
    }

    /// Handle ShowInput event - show input modal
    pub(in crate::app) fn event_show_input(
        &mut self,
        prompt: String,
        initial_value: String,
        on_submit: termide_core::InputAction,
    ) {
        use crate::state::{ActiveModal, PendingAction};
        use termide_modal::InputModal;

        // Map InputAction to PendingAction
        let pending_action = match &on_submit {
            // A panel-defined prompt (e.g. renaming an agent conversation);
            // the text goes back to the panels as
            // `PanelCommand::InputSubmitted`.
            termide_core::InputAction::Custom(action) => PendingAction::PanelInput {
                action: action.clone(),
            },
            termide_core::InputAction::ViewPath { base_dir } => PendingAction::ViewPath {
                base_dir: base_dir.clone(),
            },
            termide_core::InputAction::RenameSymbol {
                file_path,
                line,
                column,
            } => PendingAction::LspRenameSymbol {
                file_path: file_path.clone(),
                line: *line,
                column: *column,
            },
        };

        let mut modal =
            InputModal::with_default(i18n::t().modal_input_title(), prompt, &initial_value);
        if let PendingAction::ViewPath { base_dir } = &pending_action {
            modal.set_suggestions(Box::new(crate::app::modal::PathSuggestions::new(
                base_dir.clone(),
                self.project_root.clone(),
            )));
        }
        self.state
            .set_pending_action(pending_action, ActiveModal::Input(Box::new(modal)));
    }

    /// Handle ShowConfirm event - show confirmation modal
    pub(in crate::app) fn event_show_confirm(
        &mut self,
        message: String,
        on_confirm: termide_core::ConfirmAction,
    ) {
        use crate::state::{ActiveModal, PendingAction};
        use termide_modal::ConfirmModal;

        // Determine title based on action type
        let t = i18n::t();
        let is_quit = matches!(on_confirm, termide_core::ConfirmAction::QuitApplication);
        let title = if is_quit {
            t.app_quit_title()
        } else {
            t.modal_confirm_title()
        };

        // Map ConfirmAction to PendingAction
        let pending_action = match on_confirm {
            termide_core::ConfirmAction::QuitApplication => PendingAction::QuitApplication,
            termide_core::ConfirmAction::CancelOperation(op_id) => {
                PendingAction::CancelOperation(op_id)
            }
            termide_core::ConfirmAction::ReplaceInContent(replace_with) => {
                PendingAction::ReplaceInContent { replace_with }
            }
            termide_core::ConfirmAction::SaveBinary => PendingAction::SaveBinary,
            termide_core::ConfirmAction::Custom(action) => PendingAction::PanelConfirm { action },
        };

        // Create confirmation modal. A panel's own confirmation is often a
        // deletion, destructive and not undone, so it starts on "No" — every
        // other confirmation keeps "Yes" as the default answer.
        let modal = ConfirmModal::new(title, message);
        let modal = if matches!(pending_action, PendingAction::PanelConfirm { .. }) {
            modal.defaulting_to_no()
        } else {
            modal
        };
        self.state
            .set_pending_action(pending_action, ActiveModal::Confirm(Box::new(modal)));
    }

    /// Handle RefreshChecklist event: bring the checklist a panel already
    /// raised up to date, in place. A refresh for a list that is not open —
    /// closed since, or never raised — is dropped: the panel asked the app to
    /// keep a window up to date, and there is none.
    pub(in crate::app) fn event_refresh_checklist(
        &mut self,
        refresh: termide_core::ChecklistRefresh,
    ) {
        use crate::state::{ActiveModal, PendingAction};

        if !matches!(
            self.state.pending_action,
            Some(PendingAction::PanelChecklist { ref action }) if *action == refresh.action
        ) {
            return;
        }
        let Some(ActiveModal::Checklist(modal)) = self.state.active_modal.as_mut() else {
            return;
        };
        if modal.refresh(refresh.items, refresh.groups, refresh.prompt) {
            self.state.needs_redraw = true;
        }
    }
}
