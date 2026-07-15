//! LSP integration methods for the Editor.
//!
//! This module contains LSP lifecycle plumbing, diagnostics, and server-status
//! tracking. Feature-specific handlers live in sibling submodules:
//! - Completion & auto-completion: `editor_lsp_completion`
//! - Code actions: `editor_lsp_code_action`
//! - Hover / definition / references / rename: `editor_lsp_nav`

use super::{Editor, LspManager, ServerStatus};

impl Editor {
    // =========================================================================
    // LSP Initialization & Lifecycle
    // =========================================================================

    /// Initialize LSP for this editor's file.
    ///
    /// Should be called after opening a file to enable LSP features.
    /// This detects the language, starts the appropriate server if configured,
    /// and sends the `didOpen` notification.
    pub fn init_lsp(&mut self, lsp_manager: &mut LspManager) {
        if let Some(path) = self.buffer.file_path() {
            self.lsp.init_for_file(path, lsp_manager);

            if self.lsp.enabled {
                let content = self.buffer.to_string();
                self.lsp.did_open(path, &content, lsp_manager);
            }
        }
    }

    /// Notify LSP about buffer content change.
    ///
    /// Should be called after any text modification (insert, delete, etc.)
    /// to keep the language server in sync with the editor content.
    pub fn notify_lsp_change(&mut self, lsp_manager: &LspManager) {
        if let Some(path) = self.buffer.file_path() {
            let content = self.buffer.to_string();
            self.lsp.did_change(path, &content, lsp_manager);
        }
    }

    /// Cleanup LSP when editor is closed.
    ///
    /// Sends the `didClose` notification to the language server.
    pub fn cleanup_lsp(&self, lsp_manager: &LspManager) {
        if let Some(path) = self.buffer.file_path() {
            self.lsp.did_close(path, lsp_manager);
        }
    }

    /// Check if LSP is enabled for this editor.
    pub fn lsp_enabled(&self) -> bool {
        self.lsp.enabled
    }

    /// Get the language ID for this editor's file.
    pub fn lsp_language(&self) -> Option<&str> {
        self.lsp.language_id.as_deref()
    }

    /// Mark that the buffer has changed (for LSP notification).
    pub fn mark_lsp_changed(&mut self) {
        self.lsp.mark_changed();
    }

    /// Check if there are pending LSP changes that need to be sent.
    pub fn has_pending_lsp_change(&self) -> bool {
        self.lsp.has_pending_change()
    }

    /// Send pending LSP change notification if needed.
    ///
    /// Returns true if a notification was sent.
    pub fn flush_lsp_changes(&mut self, lsp_manager: &LspManager) -> bool {
        if self.lsp.has_pending_change() {
            self.notify_lsp_change(lsp_manager);
            self.lsp.clear_pending_change();
            true
        } else {
            false
        }
    }

    // =========================================================================
    // LSP Diagnostics
    // =========================================================================

    /// Update diagnostics from LSP.
    pub fn update_diagnostics(&mut self, diagnostics: Vec<lsp_types::Diagnostic>) {
        self.lsp.update_diagnostics(diagnostics);
        self.render_cache.invalidate_diagnostic_cache();
    }

    // =========================================================================
    // LSP Server Status
    // =========================================================================

    /// Check if LSP server is loading (for spinner display).
    pub fn is_lsp_loading(&self) -> bool {
        self.lsp.server_loading
    }

    /// Update server loading status from actual LSP server status.
    ///
    /// Returns true if status changed (needs redraw).
    pub fn update_lsp_loading_status(&mut self, lsp_manager: &LspManager) -> bool {
        // Clone paths to avoid borrow issues
        let file_path = match self.file_path() {
            Some(p) => p.to_path_buf(),
            None => return false,
        };
        let lang = match &self.lsp.language_id {
            Some(l) => l.clone(),
            None => return false,
        };

        // Get current server status and update status text
        let status = lsp_manager.server_status(&lang, &file_path);
        let new_status_text = match status {
            Some(ServerStatus::Starting) => Some("starting".to_string()),
            Some(ServerStatus::Indexing) => Some("indexing".to_string()),
            _ => None,
        };
        let status_text_changed = self.lsp.server_status_text != new_status_text;
        self.lsp.server_status_text = new_status_text;

        // Check if server went back to indexing (e.g., after file changes)
        if !self.lsp.server_loading && lsp_manager.server_is_indexing(&lang, &file_path) {
            self.lsp.server_loading = true;
            return true;
        }

        // Check if server became ready
        if self.lsp.server_loading && lsp_manager.server_is_ready(&lang, &file_path) {
            self.lsp.server_loading = false;
            return true;
        }

        // Return true if status text changed (for redraw)
        status_text_changed
    }
}
