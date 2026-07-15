use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

use termide_buffer::{Cursor, LineEnding, Selection, TextBuffer, Viewport};
use termide_config::Config;
use termide_core::HotkeyTable;
use termide_i18n::t;
use termide_modal::{ActiveModal, FindBar, SaveAsModal};
use termide_state::PendingAction;
use termide_vfs::VfsManager;

use crate::{
    config::*,
    constants,
    state::{FileState, GitIntegration, InputState, LspState, RenderingCache, SearchController},
    vim::VimState,
};

// Re-export LspManager for use in app integration
pub use termide_lsp::{CompletionTriggerKind, LspManager, ServerStatus};

// Editor methods are split across separate files for better organization
#[path = "editor_construct.rs"]
mod editor_construct;

#[path = "editor_vim.rs"]
mod editor_vim;

#[path = "editor_hotkeys.rs"]
mod editor_hotkeys;
pub(crate) use editor_hotkeys::build_editor_hotkey_table;

#[path = "editor_lsp.rs"]
mod editor_lsp;

#[path = "editor_lsp_completion.rs"]
mod editor_lsp_completion;

#[path = "editor_lsp_code_action.rs"]
mod editor_lsp_code_action;

#[path = "editor_lsp_nav.rs"]
mod editor_lsp_nav;

#[path = "editor_movement.rs"]
mod editor_movement;

#[path = "editor_text.rs"]
mod editor_text;

#[path = "editor_search.rs"]
mod editor_search;

#[path = "editor_mouse.rs"]
mod editor_mouse;

#[path = "editor_viewport.rs"]
mod editor_viewport;

#[path = "editor_panel.rs"]
mod editor_panel;

#[path = "editor_rendering.rs"]
mod editor_rendering;

#[path = "editor_git.rs"]
mod editor_git;

#[path = "editor_file_io.rs"]
mod editor_file_io;

#[cfg(test)]
#[path = "core_tests.rs"]
mod core_tests;

/// Editor panel with syntax highlighting
pub struct Editor {
    // === Core editing state ===
    /// Editor mode configuration
    config: EditorConfig,
    /// Text buffer with Rope
    buffer: TextBuffer,
    /// Cursor
    cursor: Cursor,
    /// Text selection (if any)
    selection: Option<Selection>,
    /// Viewport for virtual scrolling
    viewport: Viewport,

    // === Grouped state ===
    /// File-related state (mtime, external changes, title)
    pub(crate) file_state: FileState,
    /// Search-related state
    pub(crate) search: SearchController,
    /// Git integration state
    pub(crate) git: GitIntegration,
    /// Rendering cache
    pub(crate) render_cache: RenderingCache,
    /// Input state (clicks, preferred column)
    pub(crate) input: InputState,
    /// LSP integration state
    pub(crate) lsp: LspState,
    /// VFS manager for remote file operations
    pub(crate) vfs_manager: Option<Arc<VfsManager>>,

    // === UI state ===
    /// Inline find/replace bar, docked at the top of the panel while open.
    /// Replaces the floating search/replace modals for the editor.
    pub(crate) find_bar: Option<FindBar>,
    /// While the bar is open, whether focus is in the buffer (navigate matches
    /// with the cursor) rather than the bar's fields. Toggled with Tab.
    pub(crate) find_bar_focus_buffer: bool,
    /// Modal window request
    modal_request: Option<(PendingAction, ActiveModal)>,
    /// Pending upload operation (for regular Ctrl+S saves of remote files)
    /// Contains (temp_path, remote_path, vfs_manager) for app to create upload via OperationManager
    pub(crate) pending_upload: Option<(
        PathBuf,
        termide_vfs::VfsPath,
        std::sync::Arc<termide_vfs::VfsManager>,
    )>,
    /// Pending remote file open operation (for async downloads)
    pub(crate) pending_remote_open: Option<crate::remote::PendingRemoteOpen>,
    /// Updated config after save (for applying in AppState)
    config_update: Option<Config>,
    /// Status message to display to user
    pub(crate) status_message: Option<String>,
    /// When true, viewport follows cursor. When false (after mouse scroll), viewport stays put.
    scroll_follows_cursor: bool,

    // === Vim mode state ===
    /// Vim mode state (None if Vim mode is disabled)
    pub(crate) vim: Option<VimState>,

    // === Outline symbol navigation ===
    /// Sorted line positions of structural symbols (from outline) for Ctrl+Up/Down navigation.
    /// When empty, paragraph navigation falls back to blank lines.
    symbol_lines: Vec<usize>,

    // === Stale-on-collapse optimization ===
    /// Whether panel is stale (collapsed, skipping background work)
    is_stale: bool,

    /// Hotkey table for configurable keyboard shortcuts
    hotkeys: HotkeyTable,
    /// Pointer of the last Arc<Config> used to build hotkeys (skip rebuild when unchanged)
    last_config_ptr: usize,

    /// Per-editor tab_size override set at runtime (e.g. from the status bar
    /// Tab indicator modal). When `Some`, it wins over `config.editor.tab_size`
    /// on every `prepare_render` so the global config resync doesn't clobber
    /// it. `None` means "follow the global setting".
    tab_size_override: Option<usize>,

    /// Open syntax-highlighting language picker (dropdown), if any.
    pub(crate) syntax_picker: Option<crate::syntax_picker::SyntaxPicker>,
}

impl Editor {
    /// Check if smart word wrapping should be used
    ///
    /// Smart wrapping is enabled when:
    /// - File size is below the configured threshold
    ///
    /// Smart wrap works for both code files (with syntax) and plain text files.
    fn should_use_smart_wrap(&self, config: &Config) -> bool {
        // Check file size threshold (for performance)
        let threshold_bytes = config.editor.large_file_threshold_mb * constants::MEGABYTE;
        if self.file_state.size > threshold_bytes {
            return false;
        }

        true
    }

    /// Get file path
    pub fn file_path(&self) -> Option<&std::path::Path> {
        self.buffer.file_path()
    }

    /// Open the syntax-highlighting language picker (dropdown).
    pub fn open_language_picker(&mut self) {
        let custom: Vec<String> = self
            .render_cache
            .config
            .highlight
            .custom_languages
            .iter()
            .map(|c| c.name.clone())
            .collect();
        let current = self.render_cache.highlight.current_syntax();
        self.syntax_picker = Some(crate::syntax_picker::SyntaxPicker::new(
            termide_highlight::SUPPORTED_LANGUAGES,
            &custom,
            current,
        ));
    }

    /// Apply a language chosen in the picker (`Auto-detect` re-detects by path).
    pub(crate) fn apply_syntax(&mut self, language: &str) {
        if language == crate::syntax_picker::AUTO_DETECT {
            if let Some(path) = self.buffer.file_path().map(|p| p.to_path_buf()) {
                self.render_cache.highlight.set_syntax_from_path(&path);
            }
        } else {
            self.render_cache.highlight.set_syntax(language);
        }
        self.syntax_picker = None;
    }

    /// Check if Vim mode is enabled
    pub fn vim_mode_enabled(&self) -> bool {
        self.vim.is_some()
    }

    /// Get Vim mode display string for status bar (e.g., "NORMAL", "INSERT")
    /// Returns None if Vim mode is disabled
    pub fn vim_mode_display(&self) -> Option<&'static str> {
        self.vim.as_ref().map(|v| v.mode.display())
    }

    /// Get mutable reference to Vim state
    pub fn vim_state_mut(&mut self) -> Option<&mut VimState> {
        self.vim.as_mut()
    }

    /// Get reference to Vim state
    pub fn vim_state(&self) -> Option<&VimState> {
        self.vim.as_ref()
    }

    /// Get unsaved buffer filename (if this is a temporary unsaved buffer)
    pub fn unsaved_buffer_file(&self) -> Option<&str> {
        self.file_state.unsaved_buffer_file.as_deref()
    }

    /// Set the file state (for remote file handling)
    pub fn set_file_state(&mut self, file_state: FileState) {
        self.file_state = file_state;
    }

    /// Set the VFS manager (for remote file saves)
    pub fn set_vfs_manager(&mut self, vfs_manager: Arc<VfsManager>) {
        self.vfs_manager = Some(vfs_manager);
    }

    /// Set outline symbol line positions for Ctrl+Up/Down navigation.
    pub fn set_symbol_lines(&mut self, lines: Vec<usize>) {
        self.symbol_lines = lines;
    }

    /// Insert text at the beginning of the buffer (for restoring unsaved buffers)
    pub fn insert_text(&mut self, text: &str) -> Result<()> {
        let cursor_at_start = Cursor::new();
        self.cursor = self.buffer.insert(&cursor_at_start, text)?;
        self.invalidate_cache_after_edit(0, text.contains('\n'));
        Ok(())
    }

    /// Set the unsaved buffer filename (for session restoration)
    pub fn set_unsaved_buffer_file(&mut self, filename: Option<String>) {
        self.file_state.unsaved_buffer_file = filename;
    }

    /// Assign a filename to this unsaved buffer if it doesn't have one yet.
    /// Called before session save so that to_session() has a stable name.
    pub fn ensure_unsaved_buffer_file(&mut self) {
        if self.file_path().is_none()
            && self.buffer_is_modified()
            && self.file_state.unsaved_buffer_file.is_none()
        {
            self.file_state.unsaved_buffer_file =
                Some(termide_session::generate_unsaved_filename());
        }
    }

    /// Check if buffer has unsaved modifications
    pub fn buffer_is_modified(&self) -> bool {
        self.buffer.is_modified()
    }

    /// Get updated config (if config file was saved)
    pub fn take_config_update(&mut self) -> Option<Config> {
        self.config_update.take()
    }

    /// Check if file has path (not unnamed)
    pub fn has_file_path(&self) -> bool {
        self.buffer.file_path().is_some()
    }

    /// Get editor information for status bar
    pub fn get_editor_info(&self) -> EditorInfo {
        // Determine file type by current syntax
        let file_type = self
            .render_cache
            .highlight
            .current_syntax()
            .map(Self::format_language_name)
            .unwrap_or("Plain Text")
            .to_string();

        EditorInfo {
            line: self.cursor.line + 1,     // 1-based
            column: self.cursor.column + 1, // 1-based
            tab_size: self.config.tab_size,
            encoding: "UTF-8".to_string(),
            line_ending: match self.buffer.line_ending() {
                LineEnding::LF => "LF".to_string(),
                LineEnding::CRLF => "CRLF".to_string(),
            },
            file_type,
            read_only: self.config.read_only,
            syntax_highlighting: self.config.syntax_highlighting,
            vim_mode: self.vim_mode_display(),
        }
    }

    /// Get disk space information for the file's storage device.
    pub fn get_disk_space_info(&self) -> Option<termide_system_monitor::DiskSpaceInfo> {
        self.file_path()
            .and_then(termide_system_monitor::get_disk_space_info)
    }

    // ===== LogViewer support methods =====

    /// Get current cursor line (0-based).
    pub fn cursor_line(&self) -> usize {
        self.cursor.line
    }

    /// Get all buffer text as a string.
    pub fn content_string(&self) -> String {
        self.buffer.text()
    }

    /// Monotonic edit version counter (delegates to buffer).
    pub fn edit_version(&self) -> u64 {
        self.buffer.edit_version()
    }

    /// Get immutable reference to buffer.
    pub fn buffer(&self) -> &TextBuffer {
        &self.buffer
    }

    /// Get mutable reference to buffer.
    pub fn buffer_mut(&mut self) -> &mut TextBuffer {
        &mut self.buffer
    }

    /// Get immutable reference to viewport.
    pub fn viewport(&self) -> &Viewport {
        &self.viewport
    }

    /// Get mutable reference to viewport.
    pub fn viewport_mut(&mut self) -> &mut Viewport {
        &mut self.viewport
    }

    /// Set cursor to specific line (for log viewer scroll-to-end).
    pub fn set_cursor_line(&mut self, line: usize) {
        self.cursor.line = line.min(self.buffer.line_count().saturating_sub(1));
        self.cursor.column = 0;
    }

    /// Scroll to end of document (word-wrap aware).
    /// Used by JournalPanel for auto-scroll functionality.
    pub fn scroll_to_document_end(&mut self) {
        let last_line = self.buffer.line_count().saturating_sub(1);
        self.cursor.line = last_line;
        self.cursor.column = 0;
        self.scroll_follows_cursor = true;
    }

    /// Go to specific position (for go-to-definition, outline navigation, etc.).
    /// Places the target line at the top of the viewport.
    pub fn goto_position(&mut self, line: usize, column: usize) {
        let max_line = self.buffer.line_count().saturating_sub(1);
        let target_line = line.min(max_line);

        let line_len = self.buffer.line_len_graphemes(target_line);
        let target_col = column.min(line_len);

        self.cursor = Cursor::at(target_line, target_col);
        self.selection = None;

        // Place the target line at the top of the viewport
        self.viewport.top_line = target_line;
        self.viewport.top_visual_row_offset = 0;
        self.scroll_follows_cursor = true;
    }

    /// Handle backspace/delete key with selection awareness.
    ///
    /// If selection exists and is not empty, deletes the selection.
    /// Otherwise, clears selection and performs the specified delete operation.
    pub(crate) fn handle_delete_key<F>(&mut self, delete_fn: F) -> Result<()>
    where
        F: FnOnce(&mut Self) -> Result<()>,
    {
        self.close_search();

        if self
            .selection
            .as_ref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
        {
            self.delete_selection()?;
        } else {
            self.selection = None;
            delete_fn(self)?;
        }
        Ok(())
    }

    /// Invalidate syntax highlighting and wrap caches after text edit and schedule git diff update.
    ///
    /// If the edit is multiline, invalidates all lines from start_line to end of buffer.
    /// Otherwise, invalidates only the single changed line.
    pub(crate) fn invalidate_cache_after_edit(&mut self, start_line: usize, is_multiline: bool) {
        if is_multiline {
            self.render_cache
                .highlight
                .invalidate_range(start_line, self.buffer.line_count());
            // Invalidate wrap cache for all lines from start_line onwards
            self.render_cache.invalidate_wrap_range(start_line);
        } else {
            self.render_cache.highlight.invalidate_line(start_line);
            // Invalidate wrap cache for just this line
            self.render_cache.invalidate_wrap_line(start_line);
        }
        self.schedule_git_diff_update();
        // Mark for LSP notification
        self.mark_lsp_changed();
    }

    /// Handle undo/redo operation with unified logic.
    ///
    /// Performs the specified buffer operation (undo or redo), updates cursor position,
    /// invalidates cache, and schedules git diff update.
    pub(crate) fn handle_undo_redo<F>(&mut self, operation: F) -> Result<()>
    where
        F: FnOnce(&mut TextBuffer) -> Result<Option<Cursor>>,
    {
        self.close_search();

        if let Some(new_cursor) = operation(&mut self.buffer)? {
            self.cursor = new_cursor;
            self.clamp_cursor();
            // Invalidate entire highlighting cache after undo/redo
            self.render_cache
                .highlight
                .invalidate_range(0, self.buffer.line_count());
            // Invalidate wrap cache - undo/redo can affect any lines
            self.render_cache.invalidate_wrap_cache();
            // Schedule git diff update
            self.schedule_git_diff_update();
            // Mark for LSP notification
            self.mark_lsp_changed();
        }
        Ok(())
    }

    /// Open the inline find bar (find-only). The `_execute_search` flag is
    /// retained for its callers; the bar always runs the seeded query.
    pub(crate) fn open_search_modal(&mut self, _execute_search: bool) {
        self.open_find_bar(false);
    }

    /// Execute navigation with visual/physical mode selection.
    ///
    /// Prepares for navigation, then calls visual_fn if word wrap is enabled,
    /// otherwise calls physical_fn.
    pub(crate) fn navigate<FV, FP>(&mut self, visual_fn: FV, physical_fn: FP)
    where
        FV: FnOnce(&mut Self),
        FP: FnOnce(&mut Self),
    {
        self.prepare_for_navigation();
        if self.should_use_visual_movement() {
            visual_fn(self);
        } else {
            physical_fn(self);
        }
    }

    /// Execute navigation with selection, using visual/physical mode.
    ///
    /// Prepares for navigation with selection, calls visual_fn if word wrap enabled,
    /// otherwise calls physical_fn, then updates selection.
    pub(crate) fn navigate_with_selection<FV, FP>(&mut self, visual_fn: FV, physical_fn: FP)
    where
        FV: FnOnce(&mut Self),
        FP: FnOnce(&mut Self),
    {
        self.prepare_for_navigation_with_selection();
        if self.should_use_visual_movement() {
            visual_fn(self);
        } else {
            physical_fn(self);
        }
        self.update_selection_active();
    }

    /// Execute simple navigation (no visual/physical choice).
    ///
    /// Prepares for navigation and calls the movement function.
    /// Use for movements that don't have visual/physical variants (e.g., Left, Right).
    pub(crate) fn navigate_simple<F>(&mut self, movement_fn: F)
    where
        F: FnOnce(&mut Self),
    {
        self.prepare_for_navigation();
        movement_fn(self);
    }

    /// Execute simple navigation with selection (no visual/physical choice).
    ///
    /// Prepares for navigation with selection, calls movement function, then updates selection.
    /// Use for movements that don't have visual/physical variants (e.g., Shift+Left, Shift+Right).
    pub(crate) fn navigate_with_selection_simple<F>(&mut self, movement_fn: F)
    where
        F: FnOnce(&mut Self),
    {
        self.prepare_for_navigation_with_selection();
        movement_fn(self);
        self.update_selection_active();
    }

    /// Go to next search match, or open search modal if no active search.
    pub(crate) fn search_next_or_open(&mut self) {
        if self.search.state.is_some() {
            self.search_next();
        } else {
            self.open_search_modal(true);
        }
    }

    /// Go to previous search match, or open search modal if no active search.
    pub(crate) fn search_prev_or_open(&mut self) {
        if self.search.state.is_some() {
            self.search_prev();
        } else {
            self.open_search_modal(true);
        }
    }

    /// Handle save command - either save to existing path or open "Save As" modal
    /// Returns Some((temp_path, remote_path, vfs_manager)) for remote files (async upload via OperationManager), None for local files
    pub(crate) fn handle_save(
        &mut self,
    ) -> Result<
        Option<(
            PathBuf,
            termide_vfs::VfsPath,
            std::sync::Arc<termide_vfs::VfsManager>,
        )>,
    > {
        if self.buffer.file_path().is_some() {
            // File has path - save normally
            self.save()
        } else {
            // File has no path - open "Save As" dialog
            self.handle_save_as()?;
            Ok(None)
        }
    }

    /// Open "Save As" modal for saving file with a new name
    pub(crate) fn handle_save_as(&mut self) -> Result<()> {
        // Priority: initial_directory > file_path parent > CWD > home
        let directory = self
            .file_state
            .initial_directory
            .clone()
            .or_else(|| {
                self.file_path()
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            })
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")));

        // Предложить полный путь с именем файла по умолчанию
        let default_path = directory.join("untitled.txt");
        let default_value = default_path.display().to_string();

        let modal = SaveAsModal::new(t().modal_save_as_title(), default_value);
        let action = PendingAction::SaveFileAs { directory };
        self.modal_request = Some((action, ActiveModal::SaveAs(Box::new(modal))));
        Ok(())
    }

    /// Open the inline find/replace bar (find + replace fields).
    pub(crate) fn handle_start_replace(&mut self) {
        self.open_find_bar(true);
    }
}

// Additional methods used by app layer (not part of Panel trait)
impl Editor {
    /// Take modal window request (if any).
    pub fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)> {
        self.modal_request.take()
    }

    /// Take pending upload operation (if any).
    /// Returns (temp_path, remote_path, vfs_manager) for app to create upload via OperationManager
    pub fn take_pending_upload(
        &mut self,
    ) -> Option<(
        PathBuf,
        termide_vfs::VfsPath,
        std::sync::Arc<termide_vfs::VfsManager>,
    )> {
        self.pending_upload.take()
    }

    /// Set pending upload operation (called by keyboard handler).
    pub(crate) fn set_pending_upload(
        &mut self,
        upload: (
            PathBuf,
            termide_vfs::VfsPath,
            std::sync::Arc<termide_vfs::VfsManager>,
        ),
    ) {
        self.pending_upload = Some(upload);
    }

    /// Take pending remote open operation (if any).
    pub fn take_pending_remote_open(&mut self) -> Option<crate::remote::PendingRemoteOpen> {
        self.pending_remote_open.take()
    }

    /// Set pending remote open operation.
    pub fn set_pending_remote_open(&mut self, pending: crate::remote::PendingRemoteOpen) {
        self.pending_remote_open = Some(pending);
    }

    /// Get the per-editor tab_size override, if any.
    pub fn tab_size_override(&self) -> Option<usize> {
        self.tab_size_override
    }

    /// Set (or clear with `None`) the per-editor tab_size override.
    /// Applied on the next `prepare_render`.
    pub fn set_tab_size_override(&mut self, v: Option<usize>) {
        self.tab_size_override = v;
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        // Cleanup remote temp file if present
        if let Some(temp_path) = self.file_state.temp_local_path() {
            // Safety check - only cleanup files in our temp directory
            if let Some(parent) = temp_path.parent() {
                if parent.ends_with("termide-remote-edit") && temp_path.exists() {
                    if let Err(e) = std::fs::remove_file(temp_path) {
                        log::warn!("Failed to cleanup temp file {}: {}", temp_path.display(), e);
                    }
                }
            }
        }
    }
}
