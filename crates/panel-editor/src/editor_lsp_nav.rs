//! LSP symbol navigation for the Editor: hover, go-to-definition, find references, and rename.

use termide_core::PanelEvent;

use crate::hover_popup;

use super::{Editor, LspManager};

impl Editor {
    // =========================================================================
    // LSP Hover
    // =========================================================================

    /// Check if hover was requested (via mouse hover) and clear the flag.
    pub fn take_hover_request(&mut self) -> Option<(usize, usize)> {
        self.lsp.pending_hover_request.take()
    }

    /// Request hover info from LSP at specified position.
    pub fn request_hover(&mut self, line: usize, column: usize, lsp_manager: &LspManager) {
        if let Some(path) = self.buffer.file_path() {
            self.lsp.request_hover(path, line, column, lsp_manager);
        }
    }

    /// Request hover info at the current cursor position.
    ///
    /// This schedules a hover request that will be processed in tick() where LspManager is available.
    pub fn request_hover_at_cursor(&mut self) {
        // Close any existing hover popup first
        self.close_hover_popup();
        // Store cursor position for hover request
        self.lsp.pending_hover_request = Some((self.cursor.line, self.cursor.column));
    }

    /// Poll for hover response and show popup if available.
    pub fn poll_hover(&mut self) {
        if let Some(response) = self.lsp.poll_hover() {
            if let Some(popup) = hover_popup::HoverPopup::from_hover(response) {
                self.lsp.hover_popup = Some(popup);
            }
        }
    }

    /// Close hover popup.
    pub fn close_hover_popup(&mut self) {
        self.lsp.hover_popup = None;
        self.lsp.hover_popup_rect = None;
        self.lsp.pending_ctrl_click = None;
    }

    /// Cancel hover timer and close popup.
    ///
    /// Call this on any key press to cancel pending hover requests.
    pub fn cancel_hover_and_close_popup(&mut self) {
        self.lsp.cancel_hover_timer();
        self.close_hover_popup();
    }

    /// Check if hover popup is open.
    pub fn has_hover_popup(&self) -> bool {
        self.lsp.hover_popup.is_some()
    }

    /// Check and trigger delayed hover request if timer expired.
    ///
    /// Call this periodically (e.g., in tick/poll).
    /// Returns true if hover was requested.
    pub fn check_hover_timer(&mut self, lsp_manager: &LspManager, delay_ms: u64) -> bool {
        // Don't trigger if hover popup is already open
        if self.lsp.hover_popup.is_some() {
            self.lsp.cancel_hover_timer();
            return false;
        }

        if let Some((line, col)) = self.lsp.check_hover_timer(delay_ms) {
            self.request_hover(line, col, lsp_manager);
            true
        } else {
            false
        }
    }

    // =========================================================================
    // LSP Go-to-Definition
    // =========================================================================

    /// Check if go-to-definition was requested (via Ctrl+click) and clear the flag.
    pub fn take_definition_request(&mut self) -> Option<(usize, usize)> {
        self.lsp.pending_definition_request.take()
    }

    /// Request go-to-definition at cursor position.
    ///
    /// This schedules a definition request that will be processed in tick() where LspManager is available.
    pub fn request_definition_at_cursor(&mut self) {
        // Store cursor position for definition request
        self.lsp.pending_definition_request = Some((self.cursor.line, self.cursor.column));
    }

    /// Request go-to-definition from LSP at specified position.
    pub fn request_definition(&mut self, line: usize, column: usize, lsp_manager: &LspManager) {
        if let Some(path) = self.buffer.file_path() {
            self.lsp.request_definition(path, line, column, lsp_manager);
        }
    }

    /// Poll for definition response and convert to PanelEvent.
    ///
    /// Returns `Some(PanelEvent::OpenFileAt)` if a definition location was received.
    pub fn poll_definition(&mut self) -> Option<PanelEvent> {
        use lsp_types::GotoDefinitionResponse;
        use std::path::PathBuf;

        let response = self.lsp.poll_definition()?;

        // Extract location from response (take first if multiple)
        let (uri, position) = match response {
            GotoDefinitionResponse::Scalar(location) => (location.uri, location.range.start),
            GotoDefinitionResponse::Array(locations) => {
                let loc = locations.into_iter().next()?;
                (loc.uri, loc.range.start)
            }
            GotoDefinitionResponse::Link(links) => {
                let link = links.into_iter().next()?;
                (link.target_uri, link.target_selection_range.start)
            }
        };

        // Convert file:// URI to PathBuf
        let uri_str = uri.as_str();
        if !uri_str.starts_with("file://") {
            return None;
        }
        let path_str = &uri_str[7..]; // Skip "file://"
        #[cfg(unix)]
        let path = PathBuf::from(path_str);
        #[cfg(windows)]
        let path = PathBuf::from(path_str.trim_start_matches('/'));

        // LSP uses 0-based line/column
        let line = position.line as usize;
        let column = position.character as usize;

        Some(PanelEvent::OpenFileAt { path, line, column })
    }

    // =========================================================================
    // LSP Find References
    // =========================================================================

    /// Schedule a find-references request at cursor position (called from handle_key).
    pub fn request_references_at_cursor(&mut self) {
        self.lsp.pending_references_request = Some((self.cursor.line, self.cursor.column));
    }

    /// Check if find-references was requested and clear the flag.
    pub fn take_references_request(&mut self) -> Option<(usize, usize)> {
        self.lsp.pending_references_request.take()
    }

    /// Send find-references request to LSP at specified position.
    pub fn request_references(&mut self, line: usize, column: usize, lsp_manager: &LspManager) {
        if let Some(path) = self.buffer.file_path() {
            self.lsp.request_references(path, line, column, lsp_manager);
        }
    }

    /// Poll for references response (non-blocking).
    ///
    /// Returns `Some(locations)` if a response was received (may be empty if no references found).
    pub fn poll_references(&mut self) -> Option<Vec<lsp_types::Location>> {
        self.lsp.poll_references()
    }

    // =========================================================================
    // LSP Rename Symbol
    // =========================================================================

    /// Schedule a rename-symbol request at cursor position (called from handle_key).
    pub fn request_rename_at_cursor(&mut self) {
        self.lsp.pending_rename_request = Some((self.cursor.line, self.cursor.column));
    }

    /// Check if rename was requested and clear the flag.
    pub fn take_rename_request(&mut self) -> Option<(usize, usize)> {
        self.lsp.pending_rename_request.take()
    }

    /// Get the word at cursor position (for rename modal prefill).
    pub fn get_word_at_cursor(&self) -> String {
        use crate::selection::select_word;
        let Some((sel, _)) = select_word(&self.buffer, &self.cursor) else {
            return String::new();
        };
        let line_text = self
            .buffer
            .line(self.cursor.line)
            .map(|cow| cow.to_string())
            .unwrap_or_default();
        let start = sel.start().column;
        let end = sel.end().column;
        line_text.chars().skip(start).take(end - start).collect()
    }

    /// Send rename request to LSP at specified position.
    pub fn request_rename(
        &mut self,
        line: usize,
        column: usize,
        new_name: String,
        lsp_manager: &LspManager,
    ) {
        if let Some(path) = self.buffer.file_path() {
            self.lsp
                .request_rename(path, line, column, new_name, lsp_manager);
        }
    }

    /// Poll for rename response (non-blocking).
    pub fn poll_rename(&mut self) -> Option<lsp_types::WorkspaceEdit> {
        self.lsp.poll_rename()
    }
}
