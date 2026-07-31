//! LSP integration state for the editor.
//!
//! Tracks LSP-related state for a single editor instance:
//! - Language ID for current file
//! - Document version for sync
//! - Pending LSP requests (completion, hover, definition)
//! - Current diagnostics
//! - Active completion popup

use std::path::Path;
use std::sync::mpsc;

use lsp_types::{
    CodeAction, CodeActionResponse, Command, CompletionResponse, Diagnostic,
    GotoDefinitionResponse, Hover, Location, Position, Range, WorkspaceEdit,
};
use ratatui::layout::Rect;
use termide_lsp::{CompletionTriggerKind, LspManager};

use termide_ui::ColorPreview;

use crate::code_action_popup::CodeActionPopup;
use crate::completion_popup::CompletionPopup;
use crate::hover_popup::HoverPopup;

/// Poll an optional receiver, returning the value on success.
/// Clears the receiver on completion, `None` response, or disconnect.
/// Whether two LSP ranges overlap (touching endpoints count as overlapping),
/// comparing by (line, character) order.
fn ranges_overlap(a: Range, b: Range) -> bool {
    let a_start = (a.start.line, a.start.character);
    let a_end = (a.end.line, a.end.character);
    let b_start = (b.start.line, b.start.character);
    let b_end = (b.end.line, b.end.character);
    a_start <= b_end && b_start <= a_end
}

fn poll_receiver<T>(rx: &mut Option<mpsc::Receiver<Option<T>>>) -> Option<T> {
    let result = match rx.as_ref()?.try_recv() {
        Ok(Some(value)) => {
            *rx = None;
            return Some(value);
        }
        Ok(None) | Err(mpsc::TryRecvError::Disconnected) => None,
        Err(mpsc::TryRecvError::Empty) => return None,
    };
    *rx = None;
    result
}

/// LSP state for a single editor instance.
pub struct LspState {
    /// Language ID for current file (e.g., "rust", "python")
    pub language_id: Option<String>,

    /// Document version for sync (incremented on each change)
    pub document_version: i32,

    /// Pending completion request receiver
    pub completion_rx: Option<mpsc::Receiver<Option<CompletionResponse>>>,

    /// Pending hover request receiver
    pub hover_rx: Option<mpsc::Receiver<Option<Hover>>>,

    /// Pending goto-definition request receiver
    pub definition_rx: Option<mpsc::Receiver<Option<GotoDefinitionResponse>>>,

    /// Pending find-references request receiver
    pub references_rx: Option<mpsc::Receiver<Option<Vec<Location>>>>,

    /// Pending find-references request position - set by F12 handler, consumed by tick
    pub pending_references_request: Option<(usize, usize)>,

    /// Pending rename request receiver
    pub rename_rx: Option<mpsc::Receiver<Option<WorkspaceEdit>>>,

    /// Pending rename request position - set by F2 handler, consumed by key_handler
    pub pending_rename_request: Option<(usize, usize)>,

    /// Pending code-action request receiver
    pub code_action_rx: Option<mpsc::Receiver<Option<CodeActionResponse>>>,

    /// Pending `codeAction/resolve` receiver (fills in a deferred edit)
    pub code_action_resolve_rx: Option<mpsc::Receiver<Option<CodeAction>>>,

    /// Set when the user asked for code actions; consumed by the app layer
    /// which has the LspManager to issue the request.
    pub code_action_requested: bool,

    /// WorkspaceEdit from an accepted code action, waiting for the app layer to
    /// apply it (edits may span files, so the editor can't apply it alone).
    pub pending_code_action_edit: Option<WorkspaceEdit>,

    /// Accepted code action whose edit is deferred — the app layer issues a
    /// `codeAction/resolve` for it (it holds the LspManager).
    pub pending_code_action_resolve: Option<CodeAction>,

    /// Command of an accepted code action — the app layer runs it via
    /// `workspace/executeCommand`; the server then pushes the resulting edit
    /// back through `workspace/applyEdit` (e.g. phpactor "Import class").
    pub pending_code_action_command: Option<Command>,

    /// Current diagnostics for this file
    pub diagnostics: Vec<Diagnostic>,

    /// Whether LSP is enabled for this file
    pub enabled: bool,

    /// Whether LSP server is still starting (for spinner display)
    pub server_loading: bool,

    /// Flag indicating buffer has changed and LSP needs notification
    pub pending_change: bool,

    /// Active completion popup (if any)
    pub completion_popup: Option<CompletionPopup>,

    /// Active code-action popup (if any)
    pub code_action_popup: Option<CodeActionPopup>,

    /// Flag indicating completion was requested (Ctrl+Space)
    pub completion_requested: bool,

    /// Column position where completion was triggered (start of word)
    /// Used to calculate prefix length when accepting completion
    pub completion_trigger_column: usize,

    /// Last rendered popup rect (for mouse hit testing)
    pub popup_rect: Option<Rect>,

    /// Timer for auto-completion delay (None if not scheduled)
    pub completion_timer: Option<std::time::Instant>,

    /// Last inserted character (for auto-completion scheduling)
    pub last_inserted_char: Option<char>,

    /// Status text to display next to spinner (e.g., "starting", "indexing")
    pub server_status_text: Option<String>,

    // === Hover/Diagnostic popup state ===
    /// Active hover popup (if any)
    pub hover_popup: Option<HoverPopup>,

    /// Rect for hover popup mouse hit testing
    pub hover_popup_rect: Option<Rect>,

    /// Pending Ctrl+Click position (screen x, y) for popup placement
    pub pending_ctrl_click: Option<(u16, u16)>,

    /// Pending hover request (line, column) - set by mouse hover timer, consumed by tick
    pub pending_hover_request: Option<(usize, usize)>,

    /// Pending definition request (line, column) - set by Ctrl+click, consumed by tick
    pub pending_definition_request: Option<(usize, usize)>,

    /// Timer for hover delay (None if not scheduled)
    pub hover_timer: Option<std::time::Instant>,

    /// Mouse position (line, col) for scheduled hover request
    pub hover_scheduled_position: Option<(usize, usize)>,

    /// Last known mouse screen position (for tracking movement)
    pub last_mouse_position: Option<(u16, u16)>,

    /// Active color preview popup (shown while Ctrl+click is held on a hex color)
    pub color_preview: Option<ColorPreview>,
}

impl LspState {
    /// Create new LSP state.
    pub fn new() -> Self {
        Self {
            language_id: None,
            document_version: 0,
            completion_rx: None,
            hover_rx: None,
            definition_rx: None,
            references_rx: None,
            pending_references_request: None,
            rename_rx: None,
            pending_rename_request: None,
            code_action_rx: None,
            code_action_resolve_rx: None,
            code_action_requested: false,
            pending_code_action_edit: None,
            pending_code_action_resolve: None,
            pending_code_action_command: None,
            diagnostics: Vec::new(),
            enabled: false,
            server_loading: false,
            pending_change: false,
            completion_popup: None,
            code_action_popup: None,
            completion_requested: false,
            completion_trigger_column: 0,
            popup_rect: None,
            completion_timer: None,
            last_inserted_char: None,
            server_status_text: None,
            hover_popup: None,
            hover_popup_rect: None,
            pending_ctrl_click: None,
            pending_hover_request: None,
            pending_definition_request: None,
            hover_timer: None,
            hover_scheduled_position: None,
            last_mouse_position: None,
            color_preview: None,
        }
    }

    /// Mark that the buffer has changed and needs LSP notification.
    pub fn mark_changed(&mut self) {
        if self.enabled {
            self.pending_change = true;
        }
    }

    /// Check if there's a pending change that needs LSP notification.
    pub fn has_pending_change(&self) -> bool {
        self.pending_change
    }

    /// Clear the pending change flag (after sending notification).
    pub fn clear_pending_change(&mut self) {
        self.pending_change = false;
    }

    /// Schedule auto-completion to trigger after delay.
    pub fn schedule_completion(&mut self) {
        if self.enabled && !self.server_loading {
            self.completion_timer = Some(std::time::Instant::now());
        }
    }

    /// Cancel scheduled auto-completion.
    pub fn cancel_completion_timer(&mut self) {
        self.completion_timer = None;
    }

    /// Check if auto-completion timer has expired and should trigger.
    /// Returns true if completion should be triggered now.
    pub fn check_completion_timer(&mut self, delay_ms: u64) -> bool {
        if let Some(start) = self.completion_timer {
            if start.elapsed().as_millis() >= delay_ms as u128 {
                self.completion_timer = None;
                return true;
            }
        }
        false
    }

    /// Schedule hover request to trigger after delay.
    pub fn schedule_hover(&mut self, line: usize, col: usize, screen_x: u16, screen_y: u16) {
        if self.enabled && !self.server_loading {
            self.hover_timer = Some(std::time::Instant::now());
            self.hover_scheduled_position = Some((line, col));
            self.last_mouse_position = Some((screen_x, screen_y));
        }
    }

    /// Cancel scheduled hover request.
    pub fn cancel_hover_timer(&mut self) {
        self.hover_timer = None;
        self.hover_scheduled_position = None;
    }

    /// Check if hover timer has expired and should trigger.
    /// Returns Some((line, col)) if hover should be requested now.
    pub fn check_hover_timer(&mut self, delay_ms: u64) -> Option<(usize, usize)> {
        if let Some(start) = self.hover_timer {
            if start.elapsed().as_millis() >= delay_ms as u128 {
                self.hover_timer = None;
                return self.hover_scheduled_position.take();
            }
        }
        None
    }

    /// Initialize LSP for a file path.
    ///
    /// Detects language and enables LSP if a server is configured.
    pub fn init_for_file(&mut self, file_path: &Path, lsp_manager: &mut LspManager) {
        // Detect language from file extension
        self.language_id = LspManager::detect_language(file_path);

        if let Some(ref lang) = self.language_id {
            // Try to ensure server is running
            match lsp_manager.ensure_server(lang, file_path) {
                Ok(()) => {
                    self.enabled = true;
                    self.server_loading = true; // Server is starting, will be set to false when ready
                    log::info!("LSP enabled for {} ({:?})", lang, file_path);
                }
                Err(_e) => {
                    self.enabled = false;
                    self.server_loading = false;
                }
            }
        }
    }

    /// Send didOpen notification when file is opened.
    pub fn did_open(&mut self, file_path: &Path, content: &str, lsp_manager: &LspManager) {
        if !self.enabled {
            return;
        }

        if let Some(ref lang) = self.language_id {
            self.document_version = 1;
            lsp_manager.did_open(lang, file_path, content);
        }
    }

    /// Send didChange notification when content changes.
    pub fn did_change(&mut self, file_path: &Path, content: &str, lsp_manager: &LspManager) {
        if !self.enabled {
            return;
        }

        if let Some(ref lang) = self.language_id {
            self.document_version += 1;
            lsp_manager.did_change(lang, file_path, self.document_version, content);
        }
    }

    /// Send didClose notification when file is closed.
    pub fn did_close(&self, file_path: &Path, lsp_manager: &LspManager) {
        if !self.enabled {
            return;
        }

        if let Some(ref lang) = self.language_id {
            lsp_manager.did_close(lang, file_path);
        }
    }

    /// Request completion at position.
    pub fn request_completion(
        &mut self,
        file_path: &Path,
        line: usize,
        column: usize,
        trigger_kind: CompletionTriggerKind,
        trigger_character: Option<String>,
        lsp_manager: &LspManager,
    ) {
        if !self.enabled {
            return;
        }

        if let Some(ref lang) = self.language_id {
            let position = Position::new(line as u32, column as u32);
            self.completion_rx =
                lsp_manager.completion(lang, file_path, position, trigger_kind, trigger_character);
        }
    }

    /// Request hover info at position.
    pub fn request_hover(
        &mut self,
        file_path: &Path,
        line: usize,
        column: usize,
        lsp_manager: &LspManager,
    ) {
        if !self.enabled {
            return;
        }

        if let Some(ref lang) = self.language_id {
            let position = Position::new(line as u32, column as u32);
            self.hover_rx = lsp_manager.hover(lang, file_path, position);
        }
    }

    /// Request find-references at position.
    pub fn request_references(
        &mut self,
        file_path: &Path,
        line: usize,
        column: usize,
        lsp_manager: &LspManager,
    ) {
        if !self.enabled {
            return;
        }

        if let Some(ref lang) = self.language_id {
            let position = Position::new(line as u32, column as u32);
            self.references_rx = lsp_manager.references(lang, file_path, position, true);
        }
    }

    /// Poll for references response (non-blocking).
    pub fn poll_references(&mut self) -> Option<Vec<Location>> {
        let result = poll_receiver(&mut self.references_rx);
        if result.is_some() {
            self.server_loading = false;
        }
        result
    }

    /// Request rename symbol at position.
    pub fn request_rename(
        &mut self,
        file_path: &std::path::Path,
        line: usize,
        column: usize,
        new_name: String,
        lsp_manager: &LspManager,
    ) {
        if !self.enabled {
            return;
        }

        if let Some(ref lang) = self.language_id {
            let position = Position::new(line as u32, column as u32);
            self.rename_rx = lsp_manager.rename(lang, file_path, position, new_name);
        }
    }

    /// Poll for rename response (non-blocking).
    pub fn poll_rename(&mut self) -> Option<WorkspaceEdit> {
        let result = poll_receiver(&mut self.rename_rx);
        if result.is_some() {
            self.server_loading = false;
        }
        result
    }

    /// Request code actions for `range`, passing the diagnostics overlapping it
    /// as context (so quick-fixes like "Import class" are offered).
    pub fn request_code_action(
        &mut self,
        file_path: &Path,
        range: Range,
        lsp_manager: &LspManager,
    ) {
        if !self.enabled {
            return;
        }
        if let Some(ref lang) = self.language_id {
            let context_diagnostics: Vec<Diagnostic> = self
                .diagnostics
                .iter()
                .filter(|d| ranges_overlap(d.range, range))
                .cloned()
                .collect();
            self.code_action_rx =
                lsp_manager.code_action(lang, file_path, range, context_diagnostics);
        }
    }

    /// Poll for a code-action response (non-blocking).
    pub fn poll_code_action(&mut self) -> Option<CodeActionResponse> {
        let result = poll_receiver(&mut self.code_action_rx);
        if result.is_some() {
            self.server_loading = false;
        }
        result
    }

    /// Resolve a code action (fetch its deferred `edit`) — only when the server
    /// actually resolves lazily, to avoid sending an unsupported request.
    pub fn request_code_action_resolve(
        &mut self,
        file_path: &Path,
        action: CodeAction,
        lsp_manager: &LspManager,
    ) {
        if !self.enabled {
            return;
        }
        if let Some(ref lang) = self.language_id {
            if lsp_manager.supports_code_action_resolve(lang, file_path) {
                self.code_action_resolve_rx =
                    lsp_manager.code_action_resolve(lang, file_path, action);
            }
        }
    }

    /// Run a command-based code action via `workspace/executeCommand`. The
    /// server applies the change itself and pushes the edit back through a
    /// `workspace/applyEdit` request, polled at the app layer.
    pub fn request_execute_command(
        &mut self,
        file_path: &Path,
        command: Command,
        lsp_manager: &LspManager,
    ) {
        if !self.enabled {
            return;
        }
        if let Some(ref lang) = self.language_id {
            lsp_manager.execute_command(
                lang,
                file_path,
                command.command,
                command.arguments.unwrap_or_default(),
            );
        }
    }

    /// Poll for a `codeAction/resolve` response (non-blocking).
    pub fn poll_code_action_resolve(&mut self) -> Option<CodeAction> {
        let result = poll_receiver(&mut self.code_action_resolve_rx);
        if result.is_some() {
            self.server_loading = false;
        }
        result
    }

    /// Request goto-definition at position.
    pub fn request_definition(
        &mut self,
        file_path: &Path,
        line: usize,
        column: usize,
        lsp_manager: &LspManager,
    ) {
        if !self.enabled {
            return;
        }

        if let Some(ref lang) = self.language_id {
            let position = Position::new(line as u32, column as u32);
            self.definition_rx = lsp_manager.goto_definition(lang, file_path, position);
        }
    }

    /// Poll for completion response (non-blocking).
    pub fn poll_completion(&mut self) -> Option<CompletionResponse> {
        let result = poll_receiver(&mut self.completion_rx);
        if result.is_some() {
            self.server_loading = false;
        }
        result
    }

    /// Poll for hover response (non-blocking).
    pub fn poll_hover(&mut self) -> Option<Hover> {
        let result = poll_receiver(&mut self.hover_rx);
        if result.is_some() {
            self.server_loading = false;
        }
        result
    }

    /// Poll for definition response (non-blocking).
    pub fn poll_definition(&mut self) -> Option<GotoDefinitionResponse> {
        let result = poll_receiver(&mut self.definition_rx);
        if result.is_some() {
            self.server_loading = false;
        }
        result
    }

    /// Update diagnostics for this file.
    pub fn update_diagnostics(&mut self, diagnostics: Vec<Diagnostic>) {
        self.diagnostics = diagnostics;
    }

    /// Get error count.
    pub fn error_count(&self) -> usize {
        use lsp_types::DiagnosticSeverity;
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
            .count()
    }

    /// Get warning count.
    pub fn warning_count(&self) -> usize {
        use lsp_types::DiagnosticSeverity;
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Some(DiagnosticSeverity::WARNING))
            .count()
    }

    /// Get the most severe diagnostic severity at a specific line.
    ///
    /// Returns the most severe (ERROR > WARNING > INFO > HINT) diagnostic
    /// severity for the given line, used for gutter marker display.
    pub fn diagnostic_severity_at_line(
        &self,
        line: usize,
    ) -> Option<lsp_types::DiagnosticSeverity> {
        use lsp_types::DiagnosticSeverity;

        self.diagnostics
            .iter()
            .filter(|d| d.range.start.line as usize == line)
            .filter_map(|d| d.severity)
            .min_by_key(|s| match *s {
                DiagnosticSeverity::ERROR => 0,
                DiagnosticSeverity::WARNING => 1,
                DiagnosticSeverity::INFORMATION => 2,
                DiagnosticSeverity::HINT => 3,
                _ => 4,
            })
    }

    /// Get all diagnostics that overlap with a specific position.
    ///
    /// Used for showing diagnostic popup on Ctrl+click.
    pub fn diagnostics_at_position(&self, line: usize, column: usize) -> Vec<&Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| {
                let range = &d.range;
                let start_line = range.start.line as usize;
                let end_line = range.end.line as usize;
                let start_col = range.start.character as usize;
                let end_col = range.end.character as usize;

                if line < start_line || line > end_line {
                    return false;
                }

                if start_line == end_line {
                    // Single line diagnostic
                    // Handle zero-width ranges (start == end) by expanding to at least 1 character
                    let effective_end_col = if end_col <= start_col {
                        start_col + 1
                    } else {
                        end_col
                    };
                    column >= start_col && column < effective_end_col
                } else if line == start_line {
                    column >= start_col
                } else if line == end_line {
                    column < end_col
                } else {
                    // Middle line of multi-line diagnostic
                    true
                }
            })
            .collect()
    }
}

impl Default for LspState {
    fn default() -> Self {
        Self::new()
    }
}
