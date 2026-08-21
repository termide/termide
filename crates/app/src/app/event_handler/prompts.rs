//! Modal-prompt event handlers (confirm, input, select, conflict).

#![allow(deprecated)]

use std::path::PathBuf;

use crate::app::App;
use termide_i18n as i18n;

impl App {
    /// Handle ShowConflict event - show file conflict resolution modal
    pub(super) fn event_show_conflict(
        &mut self,
        source: PathBuf,
        destination: PathBuf,
        remaining: usize,
    ) {
        use crate::state::{ActiveModal, BatchOperation, BatchOperationType, PendingAction};
        use termide_modal::ConflictModal;

        // Create a minimal batch operation for conflict resolution
        let operation = BatchOperation::new(
            BatchOperationType::Copy, // Default to copy, actual type determined by context
            vec![source.clone()],
            destination.parent().unwrap_or(&destination).to_path_buf(),
        );

        let modal = ConflictModal::new(&source, &destination, remaining);
        self.state.set_pending_action(
            PendingAction::ContinueBatchOperation { operation },
            ActiveModal::Conflict(Box::new(modal)),
        );
    }

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
            termide_core::SelectAction::SelectTheme => {
                // Theme selection is handled differently
                return;
            }
            termide_core::SelectAction::SelectLanguage => {
                // Language selection is handled differently
                return;
            }
            termide_core::SelectAction::SelectEncoding => {
                // Encoding selection is handled differently
                return;
            }
            termide_core::SelectAction::CloseEditorChoice => PendingAction::CloseEditorWithSave,
            termide_core::SelectAction::Custom(_) => {
                // Custom actions not yet supported
                return;
            }
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
            termide_core::InputAction::RenameFile { from } => PendingAction::MovePath {
                sources: vec![from.clone()],
                target_directory: from.parent().map(|p| p.to_path_buf()),
            },
            termide_core::InputAction::CreateFile { in_dir } => PendingAction::CreateFile {
                directory: in_dir.clone(),
            },
            termide_core::InputAction::CreateDirectory { in_dir } => {
                PendingAction::CreateDirectory {
                    directory: in_dir.clone(),
                }
            }
            termide_core::InputAction::GotoLine => {
                // GotoLine is handled directly, not through modal
                return;
            }
            termide_core::InputAction::ViewPath { base_dir } => PendingAction::ViewPath {
                base_dir: base_dir.clone(),
            },
            termide_core::InputAction::SaveFileAs { directory } => PendingAction::SaveFileAs {
                directory: directory.clone(),
            },
            termide_core::InputAction::CopyTo { sources } => PendingAction::CopyPath {
                sources: sources.clone(),
                target_directory: None,
                create_symlink: false,
                create_relative_symlink: false,
            },
            termide_core::InputAction::MoveTo { sources } => PendingAction::MovePath {
                sources: sources.clone(),
                target_directory: None,
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
            termide_core::InputAction::GitSshPassphrase {
                operation,
                repo_path,
            } => PendingAction::GitSshPassphraseRetry {
                operation: operation.clone(),
                repo_path: repo_path.clone(),
            },
        };

        // Create input modal — mask the field for passphrase entry.
        let is_password = matches!(
            on_submit,
            termide_core::InputAction::GitSshPassphrase { .. }
        );
        let modal = if is_password {
            InputModal::new("SSH Passphrase", prompt).password()
        } else {
            InputModal::with_default("Input", prompt, &initial_value)
        };
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
            termide_core::ConfirmAction::DeleteFile(path) => {
                PendingAction::DeletePath { paths: vec![path] }
            }
            termide_core::ConfirmAction::DeletePaths(paths) => PendingAction::DeletePath { paths },
            termide_core::ConfirmAction::DeleteDirectory(path) => {
                PendingAction::DeletePath { paths: vec![path] }
            }
            termide_core::ConfirmAction::DiscardChanges(_path) => PendingAction::ClosePanel,
            termide_core::ConfirmAction::CloseWithoutSaving => PendingAction::CloseEditorWithSave,
            termide_core::ConfirmAction::QuitApplication => PendingAction::QuitApplication,
            termide_core::ConfirmAction::CancelOperation(op_id) => {
                PendingAction::CancelOperation(op_id)
            }
            termide_core::ConfirmAction::ReplaceInContent(replace_with) => {
                PendingAction::ReplaceInContent { replace_with }
            }
            termide_core::ConfirmAction::SaveBinary => PendingAction::SaveBinary,
        };

        // Create confirmation modal. Deleting is destructive and cannot be
        // undone, so that prompt starts on "No" — every other confirmation
        // keeps "Yes" as the default answer.
        let modal = ConfirmModal::new(title, message);
        let modal = if matches!(pending_action, PendingAction::DeletePath { .. }) {
            modal.defaulting_to_no()
        } else {
            modal
        };
        self.state
            .set_pending_action(pending_action, ActiveModal::Confirm(Box::new(modal)));
    }
}
