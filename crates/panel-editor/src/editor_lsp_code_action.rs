//! LSP code actions for the Editor: request, popup navigation, and edit/command dispatch.

use super::{Editor, LspManager};

impl Editor {
    // =========================================================================
    // LSP Code Actions
    // =========================================================================

    /// Mark that code actions were requested (Ctrl+.). The actual request is
    /// issued by the app layer, which holds the `LspManager`.
    pub fn trigger_code_action(&mut self) {
        self.lsp.code_action_requested = true;
    }

    /// Consume the code-action request flag.
    pub fn take_code_action_request(&mut self) -> bool {
        std::mem::take(&mut self.lsp.code_action_requested)
    }

    /// Request code actions for the current line (so a quick-fix like "Import
    /// class" sees the line's diagnostic as context).
    pub fn request_code_action(&mut self, lsp_manager: &LspManager) {
        // Make sure the server sees the current buffer (including unsaved typing)
        // before it computes actions, so the edit it returns aligns with ours.
        self.flush_lsp_changes(lsp_manager);
        let Some(path) = self.buffer.file_path().map(|p| p.to_path_buf()) else {
            return;
        };
        let line = self.cursor.line as u32;
        let line_len = self
            .buffer
            .line(self.cursor.line)
            .map(|l| l.trim_end_matches('\n').chars().count())
            .unwrap_or(0) as u32;
        let range = lsp_types::Range::new(
            lsp_types::Position::new(line, 0),
            lsp_types::Position::new(line, line_len),
        );
        self.lsp.request_code_action(&path, range, lsp_manager);
    }

    /// Poll for a code-action response and open the popup if any actions apply.
    pub fn poll_code_action(&mut self) {
        if let Some(response) = self.lsp.poll_code_action() {
            self.lsp.code_action_popup =
                crate::code_action_popup::CodeActionPopup::from_response(response);
        }
    }

    /// Whether the code-action popup is open.
    pub fn has_code_action_popup(&self) -> bool {
        self.lsp.code_action_popup.is_some()
    }

    /// Accept the selected code action and close the popup. An action is applied
    /// by whatever it carries: an inline `edit` (applied directly), a `command`
    /// (run via `workspace/executeCommand`, e.g. phpactor "Import class"), or
    /// neither — in which case its edit is fetched via `codeAction/resolve`.
    pub fn accept_code_action(&mut self) {
        let action = self
            .lsp
            .code_action_popup
            .as_ref()
            .and_then(|popup| popup.selected_code_action());
        self.lsp.code_action_popup = None;

        if let Some(action) = action {
            let actionable = action.edit.is_some() || action.command.is_some();
            if let Some(edit) = action.edit.clone() {
                self.lsp.pending_code_action_edit = Some(edit);
            }
            if let Some(command) = action.command.clone() {
                self.lsp.pending_code_action_command = Some(command);
            }
            if !actionable {
                self.lsp.pending_code_action_resolve = Some(action);
            }
        }
    }

    /// Take the pending code-action `WorkspaceEdit`, if one is ready to apply.
    pub fn take_code_action_edit(&mut self) -> Option<lsp_types::WorkspaceEdit> {
        self.lsp.pending_code_action_edit.take()
    }

    /// Take a code action whose edit must be resolved before applying.
    pub fn take_code_action_resolve(&mut self) -> Option<lsp_types::CodeAction> {
        self.lsp.pending_code_action_resolve.take()
    }

    /// Take a command-based code action's command, if one is pending.
    pub fn take_code_action_command(&mut self) -> Option<lsp_types::Command> {
        self.lsp.pending_code_action_command.take()
    }

    /// Run a command-based code action via `workspace/executeCommand`.
    pub fn request_execute_command(
        &mut self,
        command: lsp_types::Command,
        lsp_manager: &LspManager,
    ) {
        // Sync the buffer to the server first: it will compute the import edit
        // against this content and push it back via workspace/applyEdit.
        self.flush_lsp_changes(lsp_manager);
        if let Some(path) = self.buffer.file_path().map(|p| p.to_path_buf()) {
            self.lsp
                .request_execute_command(&path, command, lsp_manager);
        }
    }

    /// Issue a `codeAction/resolve` for an accepted, edit-less action.
    pub fn request_code_action_resolve(
        &mut self,
        action: lsp_types::CodeAction,
        lsp_manager: &LspManager,
    ) {
        if let Some(path) = self.buffer.file_path().map(|p| p.to_path_buf()) {
            self.lsp
                .request_code_action_resolve(&path, action, lsp_manager);
        }
    }

    /// Poll for a resolved code action; stash its edit (or command) for the app
    /// to apply or execute.
    pub fn poll_code_action_resolve(&mut self) {
        if let Some(action) = self.lsp.poll_code_action_resolve() {
            if let Some(edit) = action.edit {
                self.lsp.pending_code_action_edit = Some(edit);
            }
            if let Some(command) = action.command {
                self.lsp.pending_code_action_command = Some(command);
            }
        }
    }

    /// Cancel the code-action popup.
    pub fn cancel_code_action(&mut self) {
        self.lsp.code_action_popup = None;
    }

    /// Select the next code action.
    pub fn next_code_action(&mut self) {
        if let Some(popup) = &mut self.lsp.code_action_popup {
            popup.select_next();
        }
    }

    /// Select the previous code action.
    pub fn prev_code_action(&mut self) {
        if let Some(popup) = &mut self.lsp.code_action_popup {
            popup.select_prev();
        }
    }
}
