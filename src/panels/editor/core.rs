use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use std::path::PathBuf;

use super::super::Panel;
use super::{
    clipboard, config::*, cursor, git, rendering, search, selection, text_editing, word_wrap,
};
use crate::editor::{Cursor, HighlightCache, SearchState, Selection, TextBuffer, Viewport};
use crate::state::AppState;
use crate::state::{ActiveModal, PendingAction};
use crate::syntax_highlighter;
use crate::ui::modal::InputModal;

// EditorConfig and EditorInfo moved to config module

/// Editor panel with syntax highlighting
pub struct Editor {
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
    /// Syntax highlighting cache
    highlight_cache: HighlightCache,
    /// Search state
    search_state: Option<SearchState>,
    /// Last search query (preserved when search is closed)
    last_search_query: Option<String>,
    /// Last replace find query (preserved when replace is closed)
    last_replace_find: Option<String>,
    /// Last replace with text (preserved when replace is closed)
    last_replace_with: Option<String>,
    /// Cached title
    cached_title: String,
    /// Modal window request
    modal_request: Option<(PendingAction, ActiveModal)>,
    /// Updated config after save (for applying in AppState)
    config_update: Option<crate::config::Config>,
    /// Status message to display to user
    status_message: Option<String>,
    /// Git diff cache for this file (if in git repo)
    git_diff_cache: Option<crate::git::GitDiffCache>,
    /// Pending git diff update timestamp (for debounce)
    git_diff_update_pending: Option<std::time::Instant>,
    /// Cached count of virtual lines (buffer lines + deletion markers) for viewport calculations
    /// Updated during render to avoid recomputing during navigation
    cached_virtual_line_count: usize,
    /// Temporary file name for unsaved buffer (if this is a scratch buffer with unsaved content)
    unsaved_buffer_file: Option<String>,
    /// Preferred column for vertical navigation (maintains column across lines)
    preferred_column: Option<usize>,
    /// Cached content width from last render (for visual line navigation)
    cached_content_width: usize,
    /// Cached smart wrap setting from last render (for visual line navigation)
    cached_use_smart_wrap: bool,
    /// File size in bytes (for determining whether to use smart features)
    file_size: u64,
    /// Cache of wrap points for each line (line_idx -> vec of char positions where to wrap)
    /// Used for smart word wrapping to avoid recalculating on every render
    #[allow(dead_code)]
    wrap_cache: std::collections::HashMap<usize, Vec<usize>>,
}
// GitLineInfo and VirtualLine moved to git module

impl Editor {
    /// Create new empty editor with default configuration
    pub fn new() -> Self {
        Self::with_config(EditorConfig::default())
    }

    /// Create new empty editor with specified configuration
    pub fn with_config(config: EditorConfig) -> Self {
        Self {
            config,
            buffer: TextBuffer::new(),
            cursor: Cursor::new(),
            selection: None,
            viewport: Viewport::default(),
            highlight_cache: HighlightCache::new(syntax_highlighter::global_highlighter(), false),
            search_state: None,
            last_search_query: None,
            last_replace_find: None,
            last_replace_with: None,
            cached_title: "Untitled".to_string(),
            modal_request: None,
            config_update: None,
            status_message: None,
            git_diff_cache: None,
            git_diff_update_pending: None,
            cached_virtual_line_count: 0,
            unsaved_buffer_file: None,
            preferred_column: None,
            cached_content_width: 0,
            cached_use_smart_wrap: false,
            file_size: 0,
            wrap_cache: std::collections::HashMap::new(),
        }
    }

    /// Check if smart word wrapping should be used
    ///
    /// Smart wrapping is enabled when:
    /// - Syntax highlighting is enabled
    /// - File has a detected syntax/language
    /// - File size is below the configured threshold
    fn should_use_smart_wrap(&self, config: &crate::config::Config) -> bool {
        // Check if syntax highlighting is enabled
        if !self.config.syntax_highlighting {
            return false;
        }

        // Check if a syntax language is detected
        if !self.highlight_cache.has_syntax() {
            return false;
        }

        // Check file size threshold
        let threshold_bytes = config.large_file_threshold_mb * crate::constants::MEGABYTE;
        if self.file_size > threshold_bytes {
            return false;
        }

        true
    }

    /// Get file path
    pub fn file_path(&self) -> Option<&std::path::Path> {
        self.buffer.file_path()
    }

    /// Get unsaved buffer filename (if this is a temporary unsaved buffer)
    pub fn unsaved_buffer_file(&self) -> Option<&str> {
        self.unsaved_buffer_file.as_deref()
    }

    /// Open file with default configuration
    pub fn open_file(path: PathBuf) -> Result<Self> {
        Self::open_file_with_config(path, EditorConfig::default())
    }

    /// Open file with specified configuration
    pub fn open_file_with_config(path: PathBuf, mut config: EditorConfig) -> Result<Self> {
        // Check file size before loading
        let file_size = if let Ok(metadata) = std::fs::metadata(&path) {
            if metadata.is_file() && metadata.len() > crate::constants::MAX_EDITOR_FILE_SIZE {
                return Err(anyhow::anyhow!(
                    "File is too large to open ({:.1} MB). Maximum allowed size is {} MB.",
                    metadata.len() as f64 / crate::constants::MEGABYTE as f64,
                    crate::constants::MAX_EDITOR_FILE_SIZE / crate::constants::MEGABYTE
                ));
            }
            metadata.len()
        } else {
            crate::logger::warn(format!(
                "File size check skipped (permission denied): {}",
                path.display()
            ));
            0
        };

        let buffer = TextBuffer::from_file(&path)?;

        let cached_title = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Untitled".to_string());

        // Check file access rights for auto-detection of read-only
        if let Ok(metadata) = std::fs::metadata(&path) {
            if metadata.permissions().readonly() {
                crate::logger::warn(format!("File detected as read-only: {}", path.display()));
                config.read_only = true;
            }
        }

        // Create highlighting cache and set syntax by file extension
        let mut highlight_cache =
            HighlightCache::new(syntax_highlighter::global_highlighter(), false);

        // Set syntax only if highlighting is enabled
        if config.syntax_highlighting {
            highlight_cache.set_syntax_from_path(&path);
        }

        // Initialize git diff cache (will be populated later via update_git_diff)
        let mut git_diff_cache = None;

        // Try to create git diff cache if file is in a git repository
        // Note: we can't check config.show_git_diff here as we don't have access to global config
        // This will be initialized properly when render is called with state
        let mut cache = crate::git::GitDiffCache::new(path.clone());
        if cache.update().is_ok() {
            git_diff_cache = Some(cache);
        }

        Ok(Self {
            config,
            buffer,
            cursor: Cursor::new(),
            selection: None,
            viewport: Viewport::default(),
            highlight_cache,
            search_state: None,
            last_search_query: None,
            last_replace_find: None,
            last_replace_with: None,
            cached_title,
            modal_request: None,
            config_update: None,
            status_message: None,
            git_diff_cache,
            git_diff_update_pending: None,
            cached_virtual_line_count: 0,
            unsaved_buffer_file: None,
            preferred_column: None,
            cached_content_width: 0,
            cached_use_smart_wrap: false,
            file_size,
            wrap_cache: std::collections::HashMap::new(),
        })
    }

    /// Create editor with text (for displaying help, etc.)
    pub fn from_text(content: &str, title: String) -> Self {
        use ropey::Rope;

        // Create buffer directly through Rope
        let rope = Rope::from_str(content);

        Self {
            config: EditorConfig::view_only(),
            buffer: TextBuffer::from_rope(rope),
            cursor: Cursor::new(),
            selection: None,
            viewport: Viewport::default(),
            highlight_cache: HighlightCache::new(syntax_highlighter::global_highlighter(), false),
            search_state: None,
            last_search_query: None,
            last_replace_find: None,
            last_replace_with: None,
            cached_title: title,
            modal_request: None,
            git_diff_cache: None,
            config_update: None,
            status_message: None,
            git_diff_update_pending: None,
            cached_virtual_line_count: 0,
            unsaved_buffer_file: None,
            preferred_column: None,
            cached_content_width: 0,
            cached_use_smart_wrap: false,
            file_size: 0,
            wrap_cache: std::collections::HashMap::new(),
        }
    }

    /// Save file
    pub fn save(&mut self) -> Result<()> {
        use crate::config::Config;

        // Check if this is a config file
        if let Some(path) = self.buffer.file_path() {
            if Config::is_config_file(path) {
                let path_str = path.display().to_string();
                // Validate config before saving
                let content = self.buffer.to_string();
                match Config::validate_content(&content) {
                    Ok(new_config) => {
                        // Save and set config update flag
                        self.buffer.save()?;
                        crate::logger::info(format!("Config file saved: {}", path_str));
                        self.config_update = Some(new_config);
                    }
                    Err(e) => {
                        crate::logger::error(format!(
                            "Save failed - config validation error: {}",
                            e
                        ));
                        return Err(anyhow::anyhow!("Invalid config: {}", e));
                    }
                }
                return Ok(());
            }
        }

        self.buffer.save()?;

        if let Some(path) = self.buffer.file_path() {
            crate::logger::info(format!("File saved: {}", path.display()));
        }

        // Update git diff after successful save
        self.update_git_diff();

        Ok(())
    }

    /// Insert text at the beginning of the buffer (for restoring unsaved buffers)
    pub fn insert_text(&mut self, text: &str) -> Result<()> {
        let cursor_at_start = Cursor::new();
        self.cursor = self.buffer.insert(&cursor_at_start, text)?;
        Ok(())
    }

    /// Set the unsaved buffer filename (for session restoration)
    pub fn set_unsaved_buffer_file(&mut self, filename: Option<String>) {
        self.unsaved_buffer_file = filename;
    }

    /// Update git diff cache for this file
    pub fn update_git_diff(&mut self) {
        let file_path = self.file_path().map(|p| p.to_path_buf());
        git::update_git_diff(&mut self.git_diff_cache, file_path.as_deref());
    }

    /// Schedule git diff update with debounce (300ms delay)
    pub fn schedule_git_diff_update(&mut self) {
        if let Some(instant) = git::schedule_git_diff_update(&self.git_diff_cache) {
            self.git_diff_update_pending = Some(instant);
        }
    }

    /// Check and apply pending git diff update if debounce time has passed
    pub fn check_pending_git_diff_update(&mut self) {
        let (updated, new_pending) = git::check_pending_git_diff_update(
            self.git_diff_update_pending,
            &mut self.git_diff_cache,
            &self.buffer,
        );
        if updated {
            self.git_diff_update_pending = new_pending;
        }
    }

    /// Get updated config (if config file was saved)
    pub fn take_config_update(&mut self) -> Option<crate::config::Config> {
        self.config_update.take()
    }

    /// Get status message (if any)
    pub fn take_status_message(&mut self) -> Option<String> {
        self.status_message.take()
    }

    /// Save file as (Save As)
    pub fn save_file_as(&mut self, path: PathBuf) -> Result<()> {
        self.buffer.save_to(&path)?;
        crate::logger::info(format!("File saved as: {}", path.display()));

        // Update title
        self.cached_title = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Untitled".to_string());

        Ok(())
    }

    /// Check if file has path (not unnamed)
    pub fn has_file_path(&self) -> bool {
        self.buffer.file_path().is_some()
    }

    /// Get editor information for status bar
    pub fn get_editor_info(&self) -> EditorInfo {
        // Determine file type by current syntax
        let file_type = self
            .highlight_cache
            .current_syntax()
            .map(Self::format_language_name)
            .unwrap_or("Plain Text")
            .to_string();

        EditorInfo {
            line: self.cursor.line + 1,     // 1-based
            column: self.cursor.column + 1, // 1-based
            tab_size: self.config.tab_size,
            encoding: "UTF-8".to_string(),
            file_type,
            read_only: self.config.read_only,
            syntax_highlighting: self.config.syntax_highlighting,
        }
    }

    /// Check if visual movement should be used (word wrap enabled and width cached).
    fn should_use_visual_movement(&self) -> bool {
        self.config.word_wrap && self.cached_content_width > 0
    }

    /// Ensure preferred column is set for vertical navigation.
    ///
    /// Sets preferred_column to current column if not already set.
    /// Used by visual movement methods to maintain column across wrapped lines.
    fn ensure_preferred_column(&mut self) {
        if self.preferred_column.is_none() {
            self.preferred_column = Some(self.cursor.column);
        }
    }

    /// Convert syntax language name to human-readable display name.
    fn format_language_name(syntax_name: &str) -> &str {
        match syntax_name {
            "rust" => "Rust",
            "python" => "Python",
            "go" => "Go",
            "javascript" => "JavaScript",
            "typescript" => "TypeScript",
            "tsx" => "TSX",
            "c" => "C",
            "cpp" => "C++",
            "java" => "Java",
            "ruby" => "Ruby",
            "html" => "HTML",
            "css" => "CSS",
            "json" => "JSON",
            "toml" => "TOML",
            "yaml" => "YAML",
            "bash" => "Bash",
            "markdown" => "Markdown",
            _ => syntax_name,
        }
    }

    /// Move cursor up
    fn move_cursor_up(&mut self) {
        let maintain_preferred = cursor::physical::move_up(&mut self.cursor);
        if !maintain_preferred {
            self.preferred_column = None;
        }
        self.clamp_cursor();
    }

    /// Move cursor down
    fn move_cursor_down(&mut self) {
        let maintain_preferred = cursor::physical::move_down(&mut self.cursor, &self.buffer);
        if !maintain_preferred {
            self.preferred_column = None;
        }
        self.clamp_cursor();
    }

    /// Move cursor up by one visual line (accounting for word wrap)
    fn move_cursor_up_visual(&mut self) {
        if self.cached_content_width == 0 {
            // No word wrap or width not set - fall back to physical line movement
            self.move_cursor_up();
            return;
        }

        self.ensure_preferred_column();

        if let Some(new_cursor) = cursor::visual::move_up(
            &self.cursor,
            &self.buffer,
            self.preferred_column,
            self.cached_content_width,
            self.cached_use_smart_wrap,
        ) {
            self.cursor = new_cursor;
        }

        self.clamp_cursor();
    }

    /// Move cursor down by one visual line (accounting for word wrap)
    fn move_cursor_down_visual(&mut self) {
        if self.cached_content_width == 0 {
            // No word wrap or width not set - fall back to physical line movement
            self.move_cursor_down();
            return;
        }

        self.ensure_preferred_column();

        if let Some(new_cursor) = cursor::visual::move_down(
            &self.cursor,
            &self.buffer,
            self.preferred_column,
            self.cached_content_width,
            self.cached_use_smart_wrap,
        ) {
            self.cursor = new_cursor;
        }

        self.clamp_cursor();
    }

    /// Move cursor left
    fn move_cursor_left(&mut self) {
        let maintain_preferred = cursor::physical::move_left(&mut self.cursor, &self.buffer);
        if !maintain_preferred {
            self.preferred_column = None;
        }
    }

    /// Move cursor right
    fn move_cursor_right(&mut self) {
        let maintain_preferred = cursor::physical::move_right(&mut self.cursor, &self.buffer);
        if !maintain_preferred {
            self.preferred_column = None;
        }
        self.clamp_cursor();
    }

    /// Move cursor to start of line
    fn move_to_line_start(&mut self) {
        let maintain_preferred = cursor::physical::move_to_line_start(&mut self.cursor);
        if !maintain_preferred {
            self.preferred_column = None;
        }
    }

    /// Move cursor to end of line
    fn move_to_line_end(&mut self) {
        let maintain_preferred = cursor::physical::move_to_line_end(&mut self.cursor, &self.buffer);
        if !maintain_preferred {
            self.preferred_column = None;
        }
    }

    /// Move cursor to start of visual line (for wrapped lines)
    fn move_to_visual_line_start(&mut self) {
        // Reset preferred column on horizontal movement
        self.preferred_column = None;

        if self.cached_content_width == 0 {
            // No word wrap - fall back to physical line start
            self.move_to_line_start();
            return;
        }

        self.cursor.column = cursor::visual::move_to_visual_line_start(
            &self.cursor,
            &self.buffer,
            self.cached_content_width,
            self.cached_use_smart_wrap,
        );
    }

    /// Move cursor to end of visual line (for wrapped lines)
    fn move_to_visual_line_end(&mut self) {
        // Reset preferred column on horizontal movement
        self.preferred_column = None;

        if self.cached_content_width == 0 {
            // No word wrap - fall back to physical line end
            self.move_to_line_end();
            return;
        }

        self.cursor.column = cursor::visual::move_to_visual_line_end(
            &self.cursor,
            &self.buffer,
            self.cached_content_width,
            self.cached_use_smart_wrap,
        );
    }

    /// Move cursor page up
    fn page_up(&mut self) {
        let page_size = self.viewport.height;
        let (should_scroll, scroll_amount) = cursor::jump::page_up(&mut self.cursor, page_size);
        self.clamp_cursor();
        if should_scroll {
            self.viewport.scroll_up(scroll_amount);
        }
    }

    /// Move cursor page down
    fn page_down(&mut self) {
        let page_size = self.viewport.height;
        let (should_scroll, scroll_amount) =
            cursor::jump::page_down(&mut self.cursor, &self.buffer, page_size);
        self.clamp_cursor();
        if should_scroll {
            // Use cached virtual line count for viewport scroll (accounts for deletion markers)
            self.viewport
                .scroll_down(scroll_amount, self.cached_virtual_line_count);
        }
    }

    /// Move cursor page up by visual lines (accounting for word wrap)
    fn page_up_visual(&mut self) {
        if self.cached_content_width == 0 {
            // No word wrap - fall back to physical line movement
            self.page_up();
            return;
        }

        self.ensure_preferred_column();

        let page_size = self.viewport.height;
        self.cursor = cursor::visual::page_up(
            &self.cursor,
            &self.buffer,
            self.preferred_column,
            self.cached_content_width,
            self.cached_use_smart_wrap,
            page_size,
        );

        // Don't manually scroll viewport - let ensure_cursor_visible() handle it during rendering
        // This is correct because the viewport needs to track visual rows, not buffer lines
    }

    /// Move cursor page down by visual lines (accounting for word wrap)
    fn page_down_visual(&mut self) {
        if self.cached_content_width == 0 {
            // No word wrap - fall back to physical line movement
            self.page_down();
            return;
        }

        self.ensure_preferred_column();

        let page_size = self.viewport.height;
        self.cursor = cursor::visual::page_down(
            &self.cursor,
            &self.buffer,
            self.preferred_column,
            self.cached_content_width,
            self.cached_use_smart_wrap,
            page_size,
        );

        // Don't manually scroll viewport - let ensure_cursor_visible() handle it during rendering
        // This is correct because the viewport needs to track visual rows, not buffer lines
    }

    /// Move cursor to start of document
    fn move_to_document_start(&mut self) {
        let (new_cursor, should_scroll) = cursor::physical::move_to_document_start();
        self.cursor = new_cursor;
        if should_scroll {
            self.viewport.scroll_to_top();
        }
    }

    /// Move cursor to end of document
    fn move_to_document_end(&mut self) {
        let (new_cursor, should_scroll) = cursor::physical::move_to_document_end(&self.buffer);
        self.cursor = new_cursor;
        if should_scroll {
            // Use cached virtual line count for viewport scroll
            self.viewport
                .scroll_to_bottom(self.cached_virtual_line_count);
        }
    }

    /// Select all
    fn select_all(&mut self) {
        let (new_selection, new_cursor) = selection::select_all(&self.buffer);
        self.selection = Some(new_selection);
        self.cursor = new_cursor;
    }

    /// Start new selection or continue existing
    fn start_or_extend_selection(&mut self) {
        if let Some(new_selection) =
            selection::start_or_extend_selection(self.selection.as_ref(), self.cursor)
        {
            self.selection = Some(new_selection);
        }
    }

    /// Update active point of selection (after cursor movement)
    fn update_selection_active(&mut self) {
        selection::update_selection_active(&mut self.selection, self.cursor);
    }

    /// Get selected text
    fn get_selected_text(&self) -> Option<String> {
        selection::get_selected_text(&self.buffer, self.selection.as_ref())
    }

    /// Delete selected text
    fn delete_selection(&mut self) -> Result<()> {
        if let Some(new_cursor) =
            selection::delete_selection(&mut self.buffer, self.selection.as_ref())?
        {
            self.cursor = new_cursor;
            self.selection = None;

            // Invalidate highlighting cache
            selection::invalidate_cache_after_deletion(
                &mut self.highlight_cache,
                new_cursor.line,
                self.buffer.line_count(),
            );

            // Schedule git diff update
            self.schedule_git_diff_update();
        }
        Ok(())
    }

    /// Copy selected text to clipboard
    fn copy_to_clipboard(&mut self) -> Result<()> {
        let selected_text = self.get_selected_text();
        let result = clipboard::copy_to_clipboard(selected_text);
        self.status_message = Some(result.status_message);
        Ok(())
    }

    /// Cut selected text to clipboard
    fn cut_to_clipboard(&mut self) -> Result<()> {
        let selected_text = self.get_selected_text();
        let (result, should_delete) = clipboard::cut_to_clipboard(selected_text);
        self.status_message = Some(result.status_message);

        if should_delete {
            self.delete_selection()?;
        }
        Ok(())
    }

    /// Paste from clipboard
    fn paste_from_clipboard(&mut self) -> Result<()> {
        // Close search mode when editing begins
        self.close_search();

        // Delete selected text before pasting
        self.delete_selection()?;

        // Paste from clipboard using clipboard module
        if let Some((new_cursor, start_line, is_multiline)) =
            clipboard::paste_from_clipboard(&mut self.buffer, &self.cursor)?
        {
            self.cursor = new_cursor;
            self.clamp_cursor();

            // Invalidate highlighting cache and schedule git update
            self.invalidate_cache_after_edit(start_line, is_multiline);
        }
        Ok(())
    }

    /// Duplicate current line or selected lines
    fn duplicate_line(&mut self) -> Result<()> {
        let result =
            text_editing::duplicate_line(&mut self.buffer, &self.cursor, self.selection.as_ref())?;

        self.cursor = result.new_cursor;
        self.clamp_cursor();

        // Clear selection
        self.selection = None;

        // Invalidate highlighting cache and schedule git update
        self.invalidate_cache_after_edit(result.start_line, result.is_multiline);

        Ok(())
    }

    /// Clamp cursor position to valid values
    fn clamp_cursor(&mut self) {
        cursor::physical::clamp_cursor(&mut self.cursor, &self.buffer);
    }

    /// Insert character at cursor position
    fn insert_char(&mut self, ch: char) -> Result<()> {
        // Close search mode when editing begins
        self.close_search();

        // Delete selected text before insertion
        self.delete_selection()?;

        let result = text_editing::insert_char(&mut self.buffer, &self.cursor, ch)?;
        self.cursor = result.new_cursor;
        self.clamp_cursor();

        // Invalidate highlighting cache and schedule git update
        self.invalidate_cache_after_edit(result.start_line, result.is_multiline);

        Ok(())
    }

    /// Insert newline
    fn insert_newline(&mut self) -> Result<()> {
        // Close search mode when editing begins
        self.close_search();

        // Delete selected text before insertion
        self.delete_selection()?;

        let result = text_editing::insert_newline(&mut self.buffer, &self.cursor)?;
        self.cursor = result.new_cursor;
        self.clamp_cursor();

        // Invalidate highlighting cache and schedule git update
        self.invalidate_cache_after_edit(result.start_line, result.is_multiline);

        Ok(())
    }

    /// Delete character (backspace)
    fn backspace(&mut self) -> Result<()> {
        if let Some(result) = text_editing::backspace(&mut self.buffer, &self.cursor)? {
            self.cursor = result.new_cursor;
            self.clamp_cursor();

            // Invalidate highlighting cache and schedule git update
            self.invalidate_cache_after_edit(result.start_line, result.is_multiline);
        }
        Ok(())
    }

    /// Delete character (delete)
    fn delete(&mut self) -> Result<()> {
        if let Some(result) = text_editing::delete_char(&mut self.buffer, &self.cursor)? {
            // Invalidate highlighting cache and schedule git update
            self.invalidate_cache_after_edit(result.start_line, result.is_multiline);
        }
        Ok(())
    }

    // Git methods moved to git module

    /// Get the total count of virtual lines (real buffer lines + deletion marker lines + word wrap)
    /// This is used for viewport calculations to account for deletion markers and word wrapping
    fn virtual_line_count(&self, config: &crate::config::Config) -> usize {
        // If word wrap is enabled, count visual rows instead of buffer lines
        if self.should_use_visual_movement() {
            // Use calculate_total_visual_rows which accounts for word wrapping
            let total_visual_rows = word_wrap::calculate_total_visual_rows(
                &self.buffer,
                self.cached_content_width,
                self.config.word_wrap,
                self.cached_use_smart_wrap,
            );

            // Add deletion markers if git diff is shown
            if config.show_git_diff {
                if let Some(git_diff) = &self.git_diff_cache {
                    let buffer_line_count = self.buffer.line_count();
                    let deletion_marker_count = (0..buffer_line_count)
                        .filter(|&idx| git_diff.has_deletion_marker(idx))
                        .count();
                    return total_visual_rows + deletion_marker_count;
                }
            }

            return total_visual_rows;
        }

        // No word wrap - use old logic with buffer lines + deletion markers
        if !config.show_git_diff || self.git_diff_cache.is_none() {
            return self.buffer.line_count();
        }

        let buffer_line_count = self.buffer.line_count();
        let deletion_marker_count = self
            .git_diff_cache
            .as_ref()
            .map(|cache| {
                (0..buffer_line_count)
                    .filter(|&idx| cache.has_deletion_marker(idx))
                    .count()
            })
            .unwrap_or(0);

        buffer_line_count + deletion_marker_count
    }

    /// Render editor content
    fn render_content(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &crate::theme::Theme,
        config: &crate::config::Config,
    ) {
        // Update viewport size (subtract space for line numbers)
        let line_number_width = rendering::LINE_NUMBER_WIDTH as u16;
        let (content_width, content_height) =
            rendering::calculate_content_dimensions(area.width, area.height);

        // Cache content width for visual line navigation
        self.cached_content_width = if self.config.word_wrap {
            content_width
        } else {
            0 // Set to 0 when word wrap is disabled to trigger fallback behavior
        };

        // Initially set smart wrap to false (will be updated later if word wrap is enabled)
        self.cached_use_smart_wrap = false;

        self.viewport.resize(content_width, content_height);

        // Compute and cache virtual line count for viewport calculations
        let virtual_lines_total = self.virtual_line_count(config);
        self.cached_virtual_line_count = virtual_lines_total;

        // Ensure cursor is visible (using virtual line count to account for deletion markers)
        self.viewport
            .ensure_cursor_visible(&self.cursor, virtual_lines_total);

        let text_style = Style::default().fg(theme.fg);
        let line_number_style = Style::default().fg(theme.disabled);
        let cursor_line_style = Style::default().bg(theme.accented_bg).fg(theme.fg);

        // Style for found matches (using theme colors)
        let search_match_style = Style::default()
            .bg(theme.warning) // Warning color for search matches
            .fg(theme.bg); // Contrasting text

        let current_match_style = Style::default()
            .bg(theme.accented_fg) // Accent color for current match
            .fg(theme.bg) // Contrasting text
            .add_modifier(Modifier::BOLD);

        // Style for selected text
        let selection_style = Style::default().bg(theme.selected_bg).fg(theme.selected_fg);

        // Prepare rendering context (search matches, selection, etc.)
        let mut render_context =
            rendering::context::RenderContext::prepare(&self.search_state, &self.selection);

        if self.config.word_wrap && content_width > 0 {
            // Word wrap mode

            // Check if smart wrapping should be used
            let use_smart_wrap = self.should_use_smart_wrap(config);
            // Cache it for cursor navigation methods
            self.cached_use_smart_wrap = use_smart_wrap;

            let mut visual_row = 0;
            let mut line_idx = self.viewport.top_line;

            while visual_row < content_height && line_idx < self.buffer.line_count() {
                let is_cursor_line = line_idx == self.cursor.line;
                let style = if is_cursor_line {
                    cursor_line_style
                } else {
                    text_style
                };

                if let Some(line_text) = self.buffer.line(line_idx) {
                    let line_text = line_text.trim_end_matches('\n');
                    let chars: Vec<char> = line_text.chars().collect();
                    let line_len = chars.len();

                    // Разбить строку на визуальные строки по content_width
                    let mut char_offset = 0;
                    let mut is_first_visual_row = true;

                    // Специальная обработка пустых строк
                    if line_len == 0 {
                        // Получить git информацию для этой строки
                        let git_info = git::get_git_line_info(
                            line_idx,
                            &self.git_diff_cache,
                            config.show_git_diff,
                            theme,
                        );

                        // Отрисовать номер строки (4 символа) + status marker (1 символ)
                        let line_num_style = Style::default().fg(git_info.status_color);
                        let line_num_part =
                            format!("{:>4}{}", line_idx + 1, git_info.status_marker);

                        for (i, ch) in line_num_part.chars().enumerate() {
                            let x = area.x + i as u16;
                            let y = area.y + visual_row as u16;
                            if let Some(cell) = buf.cell_mut((x, y)) {
                                cell.set_char(ch);
                                cell.set_style(line_num_style);
                            }
                        }

                        // НЕ рисуем deletion marker здесь - он теперь на отдельной виртуальной строке
                        // Рисуем пробел вместо старого deletion marker
                        let x = area.x + 5;
                        let y = area.y + visual_row as u16;
                        if let Some(cell) = buf.cell_mut((x, y)) {
                            cell.set_char(' ');
                            cell.set_style(line_num_style);
                        }

                        // Заполнить остальную часть строки фоном (для курсора)
                        for col in 0..content_width {
                            let x = area.x + line_number_width + col as u16;
                            let y = area.y + visual_row as u16;

                            if x < area.x + area.width && y < area.y + area.height {
                                if let Some(cell) = buf.cell_mut((x, y)) {
                                    cell.set_char(' ');
                                    cell.set_style(style); // Использует cursor_line_style если это строка курсора
                                }
                            }
                        }

                        // Проверить, находится ли курсор на этой пустой строке в колонке 0
                        if is_cursor_line && self.cursor.column == 0 {
                            render_context.cursor_viewport_pos = Some((visual_row, 0));
                        }

                        visual_row += 1;
                        // Перейти к следующей строке
                    } else {
                        // Обработка непустых строк
                        while char_offset < line_len && visual_row < content_height {
                            // Calculate wrap point (smart or simple)
                            let chunk_end = if use_smart_wrap {
                                // Smart wrapping: respect word boundaries
                                crate::editor::calculate_wrap_point(
                                    &chars,
                                    char_offset,
                                    content_width,
                                    line_len,
                                )
                            } else {
                                // Simple wrapping: hard break at content_width
                                (char_offset + content_width).min(line_len)
                            };

                            // Номер строки (только на первой визуальной строке)
                            if is_first_visual_row {
                                // Получить git информацию для этой строки
                                let git_info = git::get_git_line_info(
                                    line_idx,
                                    &self.git_diff_cache,
                                    config.show_git_diff,
                                    theme,
                                );

                                // Отрисовать номер строки (4 символа) + status marker (1 символ)
                                let line_num_style = Style::default().fg(git_info.status_color);
                                let line_num_part =
                                    format!("{:>4}{}", line_idx + 1, git_info.status_marker);

                                for (i, ch) in line_num_part.chars().enumerate() {
                                    let x = area.x + i as u16;
                                    let y = area.y + visual_row as u16;
                                    if let Some(cell) = buf.cell_mut((x, y)) {
                                        cell.set_char(ch);
                                        cell.set_style(line_num_style);
                                    }
                                }

                                // НЕ рисуем deletion marker здесь - он теперь на отдельной виртуальной строке
                                // Рисуем пробел вместо старого deletion marker
                                let x = area.x + 5;
                                let y = area.y + visual_row as u16;
                                if let Some(cell) = buf.cell_mut((x, y)) {
                                    cell.set_char(' ');
                                    cell.set_style(line_num_style);
                                }
                            } else {
                                // Пустое место вместо номера строки для продолжения
                                for i in 0..line_number_width as usize {
                                    let x = area.x + i as u16;
                                    let y = area.y + visual_row as u16;
                                    if let Some(cell) = buf.cell_mut((x, y)) {
                                        cell.set_char(' ');
                                        cell.set_style(line_number_style);
                                    }
                                }
                            }

                            // Получить сегменты подсветки
                            let segments = if self.config.syntax_highlighting
                                && self.highlight_cache.has_syntax()
                            {
                                self.highlight_cache.get_line_segments(line_idx, line_text)
                            } else {
                                &[(line_text.to_string(), style)][..]
                            };

                            // Отрисовать символы этой визуальной строки
                            let mut segment_char_idx = 0;
                            let mut visual_col = 0;

                            for (segment_text, segment_style) in segments {
                                for ch in segment_text.chars() {
                                    // Проверить, попадает ли символ в текущий чанк
                                    if segment_char_idx >= char_offset
                                        && segment_char_idx < chunk_end
                                    {
                                        let x = area.x + line_number_width + visual_col as u16;
                                        let y = area.y + visual_row as u16;

                                        if x < area.x + area.width && y < area.y + area.height {
                                            if let Some(cell) = buf.cell_mut((x, y)) {
                                                cell.set_char(ch);

                                                // Determine final style using highlight renderer
                                                let final_style = rendering::highlight_renderer::determine_cell_style(
                                                    line_idx,
                                                    segment_char_idx,
                                                    *segment_style,
                                                    is_cursor_line,
                                                    &render_context,
                                                    search_match_style,
                                                    current_match_style,
                                                    selection_style,
                                                    theme.accented_bg,
                                                );
                                                cell.set_style(final_style);
                                            }
                                        }

                                        // Проверить позицию курсора
                                        if is_cursor_line && self.cursor.column == segment_char_idx
                                        {
                                            render_context.cursor_viewport_pos =
                                                Some((visual_row, visual_col));
                                        }

                                        visual_col += 1;
                                    }
                                    segment_char_idx += 1;
                                }
                            }

                            // Проверить курсор в конце строки
                            if is_cursor_line
                                && self.cursor.column >= char_offset
                                && self.cursor.column <= chunk_end
                                && (self.cursor.column == chunk_end
                                    || (chunk_end == line_len && self.cursor.column >= line_len))
                            {
                                render_context.cursor_viewport_pos =
                                    Some((visual_row, self.cursor.column - char_offset));
                            }

                            // Заполнить остаток строки фоном (для курсорной линии)
                            if is_cursor_line {
                                for col in visual_col..content_width {
                                    let x = area.x + line_number_width + col as u16;
                                    let y = area.y + visual_row as u16;

                                    if x < area.x + area.width && y < area.y + area.height {
                                        if let Some(cell) = buf.cell_mut((x, y)) {
                                            cell.set_char(' ');
                                            cell.set_style(cursor_line_style);
                                        }
                                    }
                                }
                            }

                            is_first_visual_row = false;
                            char_offset = chunk_end;
                            visual_row += 1;
                        }
                    }
                }

                // Проверить, есть ли deletion marker после этой строки
                if config.show_git_diff && visual_row < content_height {
                    if let Some(git_diff) = &self.git_diff_cache {
                        if git_diff.has_deletion_marker(line_idx) {
                            let deletion_count = git_diff.get_deletion_count(line_idx);

                            rendering::deletion_markers::render_deletion_marker(
                                buf,
                                area,
                                visual_row,
                                deletion_count,
                                theme,
                                content_width,
                                line_number_width,
                            );

                            visual_row += 1;
                        }
                    }
                }

                line_idx += 1;
            }

            // Отрисовать курсор в режиме word wrap
            if let Some((row, col)) = render_context.cursor_viewport_pos {
                let cursor_x = area.x + line_number_width + col as u16;
                let cursor_y = area.y + row as u16;
                rendering::cursor_renderer::render_cursor_at(buf, cursor_x, cursor_y, area, theme);
            }
        } else {
            // Обычный режим (без word wrap) - используем виртуальные строки
            // Построить список виртуальных строк (real buffer lines + deletion markers)
            let virtual_lines =
                git::build_virtual_lines(&self.buffer, &self.git_diff_cache, config.show_git_diff);

            // Найти индекс первой виртуальной строки для viewport.top_line (buffer line index)
            // viewport.top_line это buffer line index, нужно преобразовать в virtual line index
            let start_virtual_idx = virtual_lines
                .iter()
                .position(|vline| matches!(vline, git::VirtualLine::Real(idx) if *idx >= self.viewport.top_line))
                .unwrap_or(virtual_lines.len());

            // Отрисовать видимые виртуальные строки
            for row in 0..content_height {
                let virtual_idx = start_virtual_idx + row;

                if virtual_idx >= virtual_lines.len() {
                    break;
                }

                let virtual_line = virtual_lines[virtual_idx];

                // Обработка в зависимости от типа виртуальной строки
                match virtual_line {
                    git::VirtualLine::Real(line_idx) => {
                        // Render real line in no-wrap mode
                        if let Some(line_text) = self.buffer.line(line_idx) {
                            let line_text = line_text.trim_end_matches('\n');
                            let is_cursor_line = line_idx == self.cursor.line;

                            rendering::line_rendering::render_line_no_wrap(
                                buf,
                                area,
                                row,
                                line_idx,
                                line_text,
                                is_cursor_line,
                                text_style,
                                cursor_line_style,
                                &self.git_diff_cache,
                                config.show_git_diff,
                                theme,
                                line_number_width,
                                content_width,
                                self.viewport.left_column,
                                self.config.syntax_highlighting,
                                &mut self.highlight_cache,
                                &render_context,
                                search_match_style,
                                current_match_style,
                                selection_style,
                            );
                        }
                    }
                    git::VirtualLine::DeletionMarker(_after_line_idx, deletion_count) => {
                        // Виртуальная строка - deletion marker
                        rendering::deletion_markers::render_deletion_marker(
                            buf,
                            area,
                            row,
                            deletion_count,
                            theme,
                            content_width,
                            line_number_width,
                        );

                        // Пропускаем рендеринг контента для deletion marker
                        continue;
                    }
                }
            }

            // Отрисовать курсор с учетом виртуальных строк
            // Найти virtual line index для cursor.line
            let cursor_virtual_idx = virtual_lines.iter().position(
                |vline| matches!(vline, git::VirtualLine::Real(idx) if *idx == self.cursor.line),
            );

            if let Some(cursor_virtual_idx) = cursor_virtual_idx {
                // Вычислить viewport row с учетом deletion marker строк
                if cursor_virtual_idx >= start_virtual_idx {
                    let viewport_row = cursor_virtual_idx - start_virtual_idx;

                    // Вычислить viewport col (с учетом horizontal scrolling)
                    if self.cursor.column >= self.viewport.left_column {
                        let viewport_col = self.cursor.column - self.viewport.left_column;

                        let cursor_x = area.x + line_number_width + viewport_col as u16;
                        let cursor_y = area.y + viewport_row as u16;

                        if viewport_col < content_width {
                            rendering::cursor_renderer::render_cursor_at(
                                buf, cursor_x, cursor_y, area, theme,
                            );
                        }
                    }
                }
            }
        }
    }

    /// Start search
    pub fn start_search(&mut self, query: String, case_sensitive: bool) {
        let mut search_state = SearchState::new(query, case_sensitive);

        // Perform search throughout document
        self.perform_search(&mut search_state);

        // Find closest match to current cursor
        search_state.find_closest_match(&self.cursor);

        // Move cursor to end of match and create selection
        if let Some(match_cursor) = search_state.current_match_cursor() {
            let query_len = search_state.query.chars().count();
            let (selection, end_cursor) = search::get_match_selection(match_cursor, query_len);
            self.cursor = end_cursor;
            self.selection = Some(selection);
        }

        self.search_state = Some(search_state);
    }

    /// Perform search in document
    fn perform_search(&self, search_state: &mut SearchState) {
        search::perform_search(&self.buffer, search_state);
    }

    /// Go to next match
    pub fn search_next(&mut self) {
        if let Some(ref mut search_state) = self.search_state {
            search_state.next_match();
            if let Some(match_cursor) = search_state.current_match_cursor() {
                let query_len = search_state.query.chars().count();
                let (selection, end_cursor) = search::get_match_selection(match_cursor, query_len);
                self.cursor = end_cursor;
                self.selection = Some(selection);
            }
        }
    }

    /// Go to previous match
    pub fn search_prev(&mut self) {
        if let Some(ref mut search_state) = self.search_state {
            search_state.prev_match();
            if let Some(match_cursor) = search_state.current_match_cursor() {
                let query_len = search_state.query.chars().count();
                let (selection, end_cursor) = search::get_match_selection(match_cursor, query_len);
                self.cursor = end_cursor;
                self.selection = Some(selection);
            }
        }
    }

    /// Close search
    pub fn close_search(&mut self) {
        // Preserve the last search/replace query before closing
        if let Some(ref search) = self.search_state {
            if let Some(ref replace_with) = search.replace_with {
                // This is a replace operation - save to replace history
                self.last_replace_find = Some(search.query.clone());
                self.last_replace_with = Some(replace_with.clone());
            } else {
                // This is a search operation - save to search history
                self.last_search_query = Some(search.query.clone());
            }
        }
        self.search_state = None;
    }

    /// Get search match information (current index, total count)
    pub fn get_search_match_info(&self) -> Option<(usize, usize)> {
        if let Some(ref search) = self.search_state {
            let current = search.current_match.unwrap_or(0);
            let total = search.matches.len();
            Some((current, total))
        } else {
            None
        }
    }

    /// Start search with replace
    pub fn start_replace(&mut self, query: String, replace_with: String, case_sensitive: bool) {
        let mut search_state = SearchState::new_with_replace(query, replace_with, case_sensitive);

        // Perform search throughout document
        self.perform_search(&mut search_state);

        // Find closest match to current cursor
        search_state.find_closest_match(&self.cursor);

        // Move cursor to first match and create selection
        if let Some(match_cursor) = search_state.current_match_cursor() {
            let query_len = search_state.query.chars().count();
            let (selection, end_cursor) = search::get_match_selection(match_cursor, query_len);
            self.cursor = end_cursor;
            self.selection = Some(selection);
        }

        self.search_state = Some(search_state);
    }

    /// Update replace_with value in active search state without rebuilding search
    pub fn update_replace_with(&mut self, replace_with: String) {
        if let Some(ref mut search) = self.search_state {
            search.replace_with = Some(replace_with);
        }
    }

    /// Replace current match
    pub fn replace_current(&mut self) -> Result<()> {
        // Collect data from search_state
        let (match_cursor, replace_with, query_len) =
            if let Some(ref search_state) = self.search_state {
                if let (Some(replace_with), Some(idx)) =
                    (&search_state.replace_with, search_state.current_match)
                {
                    if let Some(match_cursor) = search_state.matches.get(idx).cloned() {
                        (match_cursor, replace_with.clone(), search_state.query.len())
                    } else {
                        return Ok(());
                    }
                } else {
                    return Ok(());
                }
            } else {
                return Ok(());
            };

        // Perform replacement
        let result =
            search::replace_at_position(&mut self.buffer, &match_cursor, query_len, &replace_with)?;
        self.cursor = result.new_cursor;

        // Invalidate highlighting cache for changed line
        self.highlight_cache.invalidate_line(result.start_line);

        // Update search_state
        if let Some(ref mut search_state) = self.search_state {
            if let Some(idx) = search_state.current_match {
                // Remove this match from list
                search_state.matches.remove(idx);

                // Update positions of remaining matches on the same line after replacement point
                search::update_match_positions_after_replace(
                    &mut search_state.matches,
                    &match_cursor,
                    query_len,
                    replace_with.len(),
                );

                // Update current match index
                if search_state.matches.is_empty() {
                    search_state.current_match = None;
                } else if idx >= search_state.matches.len() {
                    search_state.current_match = Some(search_state.matches.len() - 1);
                }

                // Move cursor to next match and create selection
                if let Some(match_cursor) = search_state.current_match_cursor() {
                    let query_len = search_state.query.chars().count();
                    let (selection, end_cursor) =
                        search::get_match_selection(match_cursor, query_len);
                    self.cursor = end_cursor;
                    self.selection = Some(selection);
                }
            }
        }

        // Schedule git diff update
        self.schedule_git_diff_update();

        Ok(())
    }

    /// Replace all matches
    pub fn replace_all(&mut self) -> Result<usize> {
        let count = if let Some(ref search_state) = self.search_state.clone() {
            if let Some(replace_with) = &search_state.replace_with {
                // Perform all replacements
                let count = search::replace_all_matches(
                    &mut self.buffer,
                    &search_state.matches,
                    search_state.query.len(),
                    replace_with,
                )?;

                // Invalidate highlighting cache for all affected lines
                for match_cursor in &search_state.matches {
                    self.highlight_cache.invalidate_line(match_cursor.line);
                }

                // Clear search state
                self.search_state = None;

                // Schedule git diff update
                self.schedule_git_diff_update();

                count
            } else {
                0
            }
        } else {
            0
        };

        Ok(count)
    }

    /// Prepare for navigation: close search and clear selection.
    fn prepare_for_navigation(&mut self) {
        self.close_search();
        self.selection = None;
    }

    /// Prepare for navigation with selection: close search and start/extend selection.
    fn prepare_for_navigation_with_selection(&mut self) {
        self.close_search();
        self.start_or_extend_selection();
    }

    /// Handle backspace/delete key with selection awareness.
    ///
    /// If selection exists and is not empty, deletes the selection.
    /// Otherwise, clears selection and performs the specified delete operation.
    fn handle_delete_key<F>(&mut self, delete_fn: F) -> Result<()>
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

    /// Invalidate syntax highlighting cache after text edit and schedule git diff update.
    ///
    /// If the edit is multiline, invalidates all lines from start_line to end of buffer.
    /// Otherwise, invalidates only the single changed line.
    fn invalidate_cache_after_edit(&mut self, start_line: usize, is_multiline: bool) {
        if is_multiline {
            self.highlight_cache
                .invalidate_range(start_line, self.buffer.line_count());
        } else {
            self.highlight_cache.invalidate_line(start_line);
        }
        self.schedule_git_diff_update();
    }

    /// Handle undo/redo operation with unified logic.
    ///
    /// Performs the specified buffer operation (undo or redo), updates cursor position,
    /// invalidates cache, and schedules git diff update.
    fn handle_undo_redo<F>(&mut self, operation: F) -> Result<()>
    where
        F: FnOnce(&mut TextBuffer) -> Result<Option<Cursor>>,
    {
        self.close_search();

        if let Some(new_cursor) = operation(&mut self.buffer)? {
            self.cursor = new_cursor;
            self.clamp_cursor();
            // Invalidate entire highlighting cache after undo/redo
            self.highlight_cache
                .invalidate_range(0, self.buffer.line_count());
            // Schedule git diff update
            self.schedule_git_diff_update();
        }
        Ok(())
    }

    /// Open search modal, optionally restoring and executing previous query.
    ///
    /// If active search exists, restores its state. Otherwise, if a previous query
    /// exists and execute_search is true, executes it immediately.
    fn open_search_modal(&mut self, execute_search: bool) {
        use crate::ui::modal::SearchModal;
        let mut search_modal = SearchModal::new("");

        // Restore active search state if it exists
        if let Some(ref search_state) = self.search_state {
            search_modal.set_input(search_state.query.clone());
            if let Some((current, total)) = self.get_search_match_info() {
                search_modal.set_match_info(current, total);
            }
        }
        // If there's a saved query but no active search
        else if let Some(ref query) = self.last_search_query {
            search_modal.set_input(query.clone());

            if execute_search {
                // Execute search immediately
                self.start_search(query.clone(), false);

                // Update match info in modal
                if let Some((current, total)) = self.get_search_match_info() {
                    search_modal.set_match_info(current, total);
                }
            }
        }

        self.modal_request = Some((
            PendingAction::Search,
            ActiveModal::Search(Box::new(search_modal)),
        ));
    }

    /// Execute navigation with visual/physical mode selection.
    ///
    /// Prepares for navigation, then calls visual_fn if word wrap is enabled,
    /// otherwise calls physical_fn.
    fn navigate<FV, FP>(&mut self, visual_fn: FV, physical_fn: FP)
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
    fn navigate_with_selection<FV, FP>(&mut self, visual_fn: FV, physical_fn: FP)
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
    fn navigate_simple<F>(&mut self, movement_fn: F)
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
    fn navigate_with_selection_simple<F>(&mut self, movement_fn: F)
    where
        F: FnOnce(&mut Self),
    {
        self.prepare_for_navigation_with_selection();
        movement_fn(self);
        self.update_selection_active();
    }

    /// Go to next search match, or open search modal if no active search.
    fn search_next_or_open(&mut self) {
        if self.search_state.is_some() {
            self.search_next();
        } else {
            self.open_search_modal(true);
        }
    }

    /// Go to previous search match, or open search modal if no active search.
    fn search_prev_or_open(&mut self) {
        if self.search_state.is_some() {
            self.search_prev();
        } else {
            self.open_search_modal(true);
        }
    }

    // Word wrap methods moved to word_wrap module
}

impl Panel for Editor {
    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        _is_focused: bool,
        _panel_index: usize,
        state: &AppState,
    ) {
        // Render editor content directly (accordion already drew border with title/buttons)
        self.render_content(area, buf, state.theme, &state.config);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Translate Cyrillic to Latin for hotkeys
        let key = crate::keyboard::translate_hotkey(key);

        match (key.code, key.modifiers) {
            // Navigation (clears selection and closes search)
            (KeyCode::Up, KeyModifiers::NONE) => {
                self.navigate(Self::move_cursor_up_visual, Self::move_cursor_up);
            }
            (KeyCode::Down, KeyModifiers::NONE) => {
                self.navigate(Self::move_cursor_down_visual, Self::move_cursor_down);
            }
            (KeyCode::Left, KeyModifiers::NONE) => {
                self.navigate_simple(Self::move_cursor_left);
            }
            (KeyCode::Right, KeyModifiers::NONE) => {
                self.navigate_simple(Self::move_cursor_right);
            }
            (KeyCode::Home, KeyModifiers::NONE) => {
                self.navigate(Self::move_to_visual_line_start, Self::move_to_line_start);
            }
            (KeyCode::End, KeyModifiers::NONE) => {
                self.navigate(Self::move_to_visual_line_end, Self::move_to_line_end);
            }
            (KeyCode::PageUp, KeyModifiers::NONE) => {
                self.navigate(Self::page_up_visual, Self::page_up);
            }
            (KeyCode::PageDown, KeyModifiers::NONE) => {
                self.navigate(Self::page_down_visual, Self::page_down);
            }
            (KeyCode::Home, KeyModifiers::CONTROL) => {
                self.navigate_simple(Self::move_to_document_start);
            }
            (KeyCode::End, KeyModifiers::CONTROL) => {
                self.navigate_simple(Self::move_to_document_end);
            }

            // Navigation with selection (Shift) - closes search
            (KeyCode::Up, KeyModifiers::SHIFT) => {
                self.navigate_with_selection(Self::move_cursor_up_visual, Self::move_cursor_up);
            }
            (KeyCode::Down, KeyModifiers::SHIFT) => {
                self.navigate_with_selection(Self::move_cursor_down_visual, Self::move_cursor_down);
            }
            (KeyCode::Left, KeyModifiers::SHIFT) => {
                self.navigate_with_selection_simple(Self::move_cursor_left);
            }
            (KeyCode::Right, KeyModifiers::SHIFT) => {
                self.navigate_with_selection_simple(Self::move_cursor_right);
            }
            (KeyCode::Home, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    && !modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.navigate_with_selection(
                    Self::move_to_visual_line_start,
                    Self::move_to_line_start,
                );
            }
            (KeyCode::End, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    && !modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.navigate_with_selection(Self::move_to_visual_line_end, Self::move_to_line_end);
            }
            (KeyCode::PageUp, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    && !modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.navigate_with_selection(Self::page_up_visual, Self::page_up);
            }
            (KeyCode::PageDown, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    && !modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.navigate_with_selection(Self::page_down_visual, Self::page_down);
            }
            // Shift+Ctrl+Home/End - select to start/end of document - closes search
            (KeyCode::Home, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    && modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.navigate_with_selection_simple(Self::move_to_document_start);
            }
            (KeyCode::End, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    && modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.navigate_with_selection_simple(Self::move_to_document_end);
            }

            // Editing (only if not read-only)
            (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                if !self.config.read_only {
                    self.insert_char(ch)?;
                }
            }
            (KeyCode::Enter, KeyModifiers::NONE) => {
                if !self.config.read_only {
                    self.insert_newline()?;
                }
            }
            (KeyCode::Backspace, KeyModifiers::NONE) => {
                if !self.config.read_only {
                    self.handle_delete_key(|editor| editor.backspace())?;
                }
            }
            (KeyCode::Delete, KeyModifiers::NONE) => {
                if !self.config.read_only {
                    self.handle_delete_key(|editor| editor.delete())?;
                }
            }

            // Ctrl+S - save (only if not read-only)
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
                if !self.config.read_only {
                    if self.buffer.file_path().is_some() {
                        // File has path - save normally
                        self.save()?;
                    } else {
                        // File has no path - open "Save As" dialog
                        let directory = std::env::current_dir().unwrap_or_else(|_| {
                            dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
                        });

                        let modal = InputModal::new("Save File As", "untitled.txt");
                        let action = PendingAction::SaveFileAs {
                            panel_index: 0, // will be updated in app.rs
                            directory,
                        };
                        self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
                    }
                }
            }

            // Ctrl+Z - undo (only if not read-only)
            (KeyCode::Char('z'), KeyModifiers::CONTROL) => {
                if !self.config.read_only {
                    self.handle_undo_redo(|buffer| buffer.undo())?;
                }
            }

            // Ctrl+Y - redo (only if not read-only)
            (KeyCode::Char('y'), KeyModifiers::CONTROL) => {
                if !self.config.read_only {
                    self.handle_undo_redo(|buffer| buffer.redo())?;
                }
            }

            // Ctrl+F - search (show interactive search modal)
            (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                self.open_search_modal(true);
            }

            // F3 - next match (or open search if no active search)
            (KeyCode::F(3), KeyModifiers::NONE) => {
                self.search_next_or_open();
            }

            // Shift+F3 - previous match (or open search if no active search)
            (KeyCode::F(3), KeyModifiers::SHIFT) => {
                self.search_prev_or_open();
            }

            // Esc - close search
            (KeyCode::Esc, KeyModifiers::NONE) => {
                if self.search_state.is_some() {
                    self.close_search();
                }
            }

            // Tab - next match (synonym for F3 when search is active)
            (KeyCode::Tab, KeyModifiers::NONE) => {
                if self.search_state.is_some() {
                    self.search_next();
                }
            }

            // Shift+Tab - previous match (synonym for Shift+F3 when search is active)
            (KeyCode::BackTab, _) => {
                if self.search_state.is_some() {
                    self.search_prev();
                }
            }

            // Ctrl+H - text replacement (only if not read-only)
            (KeyCode::Char('h'), KeyModifiers::CONTROL) => {
                if !self.config.read_only {
                    use crate::ui::modal::ReplaceModal;
                    let mut replace_modal = ReplaceModal::new();

                    // Restore previous find/replace text if available
                    if let Some(ref find) = self.last_replace_find {
                        replace_modal.set_find_input(find.clone());
                    }
                    if let Some(ref replace) = self.last_replace_with {
                        replace_modal.set_replace_input(replace.clone());
                    }

                    // If there's saved find text - execute search immediately
                    if let Some(ref find) = self.last_replace_find {
                        let replace_with = self.last_replace_with.clone().unwrap_or_default();
                        self.start_replace(find.clone(), replace_with, false);

                        // Update match info in modal
                        if let Some((current, total)) = self.get_search_match_info() {
                            replace_modal.set_match_info(current, total);
                        }
                    }

                    self.modal_request = Some((
                        PendingAction::Replace,
                        ActiveModal::Replace(Box::new(replace_modal)),
                    ));
                }
            }

            // Ctrl+Alt+R - replace all matches (only if not read-only)
            // Must be BEFORE Ctrl+R for correct pattern matching
            (KeyCode::Char('r'), modifiers)
                if modifiers.contains(KeyModifiers::CONTROL)
                    && modifiers.contains(KeyModifiers::ALT) =>
            {
                if !self.config.read_only {
                    if let Ok(count) = self.replace_all() {
                        self.status_message = Some(format!(
                            "Replaced {} occurrence{}",
                            count,
                            if count == 1 { "" } else { "s" }
                        ));
                    }
                }
            }

            // Ctrl+R - replace current match (only if not read-only)
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                if !self.config.read_only {
                    if let Err(e) = self.replace_current() {
                        eprintln!("Replace error: {}", e);
                    }
                }
            }

            // Ctrl+A - select all
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                self.select_all();
            }

            // Ctrl+C - copy
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.copy_to_clipboard()?;
            }

            // Ctrl+D - duplicate line
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                if !self.config.read_only {
                    self.duplicate_line()?;
                }
            }

            // Ctrl+Insert - copy
            (KeyCode::Insert, KeyModifiers::CONTROL) => {
                self.copy_to_clipboard()?;
            }

            // NOTE: Ctrl+Shift+C/V and Ctrl+Insert may be intercepted by terminal emulators
            // (gnome-terminal, konsole) before reaching the application. This is because:
            // - Terminal emulators intercept these keys at the terminal layer
            // - They copy their own selection buffer, not the application's selection
            //
            // Users have two options:
            // 1. Use Ctrl+C/V (always works in application, copies to PRIMARY + CLIPBOARD)
            // 2. Use Shift+Mouse to select text at terminal layer, then Ctrl+Shift+C
            //
            // These handlers work in terminals that don't intercept (alacritty, some configs).
            // On Linux, we write to both CLIPBOARD and PRIMARY selections for compatibility.

            // Ctrl+Shift+C - copy (terminal shortcut)
            (KeyCode::Char('c'), mods)
                if mods.contains(KeyModifiers::CONTROL) && mods.contains(KeyModifiers::SHIFT) =>
            {
                self.copy_to_clipboard()?;
            }

            // Ctrl+Shift+C - uppercase variant
            (KeyCode::Char('C'), mods)
                if mods.contains(KeyModifiers::CONTROL) && mods.contains(KeyModifiers::SHIFT) =>
            {
                self.copy_to_clipboard()?;
            }

            // Ctrl+X - cut (only if not read-only)
            (KeyCode::Char('x'), KeyModifiers::CONTROL) => {
                if !self.config.read_only {
                    self.cut_to_clipboard()?;
                }
            }

            // Shift+Delete - cut (only if not read-only)
            (KeyCode::Delete, KeyModifiers::SHIFT) => {
                if !self.config.read_only {
                    self.cut_to_clipboard()?;
                }
            }

            // Ctrl+V - paste (only if not read-only)
            (KeyCode::Char('v'), KeyModifiers::CONTROL) => {
                if !self.config.read_only {
                    self.paste_from_clipboard()?;
                }
            }

            // NOTE: Shift+Insert may be intercepted by terminal emulators (gnome-terminal)
            // for terminal-layer paste from PRIMARY selection. This handler works in terminals
            // that don't intercept. Users can always use Ctrl+V which works at app level.
            (KeyCode::Insert, KeyModifiers::SHIFT) => {
                if !self.config.read_only {
                    self.paste_from_clipboard()?;
                }
            }

            // Ctrl+Shift+V - paste (terminal shortcut)
            (KeyCode::Char('v'), mods)
                if mods.contains(KeyModifiers::CONTROL) && mods.contains(KeyModifiers::SHIFT) =>
            {
                if !self.config.read_only {
                    self.paste_from_clipboard()?;
                }
            }

            // Ctrl+Shift+V - uppercase variant
            (KeyCode::Char('V'), mods)
                if mods.contains(KeyModifiers::CONTROL) && mods.contains(KeyModifiers::SHIFT) =>
            {
                if !self.config.read_only {
                    self.paste_from_clipboard()?;
                }
            }

            _ => {}
        }

        Ok(())
    }

    fn title(&self) -> String {
        let modified = if self.buffer.is_modified() { "*" } else { "" };

        // Add search information if active
        let search_info = if let Some(ref search) = self.search_state {
            if search.is_active() {
                let t = crate::i18n::t();
                let current = search.current_match.map(|i| i + 1).unwrap_or(0);
                let total = search.match_count();
                if total > 0 {
                    format!(" [{}]", t.editor_search_match_info(current, total))
                } else {
                    format!(" [{}]", t.editor_search_no_matches())
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        format!("{}{}{}", self.cached_title, modified, search_info)
    }

    fn needs_close_confirmation(&self) -> Option<String> {
        if self.buffer.is_modified() {
            Some("File has unsaved changes. Close anyway?".to_string())
        } else {
            None
        }
    }

    fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)> {
        self.modal_request.take()
    }

    fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        panel_area: Rect,
    ) -> Result<()> {
        use crossterm::event::{MouseButton, MouseEventKind};

        // Handle scroll first (works anywhere in panel)
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.viewport.scroll_up(3);
                // Keep cursor in visible area so render doesn't reset scroll
                if self.cursor.line >= self.viewport.bottom_line() {
                    self.cursor.line = self.viewport.bottom_line().saturating_sub(1);
                    self.clamp_cursor();
                }
                return Ok(());
            }
            MouseEventKind::ScrollDown => {
                // Use cached virtual line count for scroll
                self.viewport.scroll_down(3, self.cached_virtual_line_count);
                // Keep cursor in visible area so render doesn't reset scroll
                if self.cursor.line < self.viewport.top_line {
                    self.cursor.line = self.viewport.top_line;
                    self.clamp_cursor();
                }
                return Ok(());
            }
            _ => {}
        }

        // Calculate inner area (without border)
        let inner = Rect {
            x: panel_area.x + 1,
            y: panel_area.y + 1,
            width: panel_area.width.saturating_sub(2),
            height: panel_area.height.saturating_sub(2),
        };

        // Check that event is inside content area
        let line_number_width = rendering::LINE_NUMBER_WIDTH as u16;
        let content_x = inner.x + line_number_width;
        let content_y = inner.y;
        let content_width = inner.width.saturating_sub(line_number_width);
        let content_height = inner.height;

        // Check that mouse is inside content area
        if mouse.column < content_x || mouse.column >= content_x + content_width {
            return Ok(());
        }
        if mouse.row < content_y || mouse.row >= content_y + content_height {
            return Ok(());
        }

        // Convert mouse coordinates to position in buffer
        let rel_x = (mouse.column - content_x) as usize;
        let rel_y = (mouse.row - content_y) as usize;

        // In word wrap mode, visual rows don't correspond 1:1 with buffer lines
        let (buffer_line, wrapped_offset) = if self.config.word_wrap {
            word_wrap::visual_row_to_buffer_position(
                &self.buffer,
                rel_y,
                self.viewport.top_line,
                content_width as usize,
                self.cached_use_smart_wrap,
            )
        } else {
            (self.viewport.top_line + rel_y, 0)
        };

        let buffer_col = if self.config.word_wrap {
            wrapped_offset + rel_x
        } else {
            self.viewport.left_column + rel_x
        };

        // Clamp position to valid values
        let max_line = self.buffer.line_count().saturating_sub(1);
        let target_line = buffer_line.min(max_line);
        let line_len = self.buffer.line_len_graphemes(target_line);
        let target_col = buffer_col.min(line_len);

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Close search mode on click
                self.close_search();
                // Start selection - set cursor and begin selection
                self.cursor = Cursor::at(target_line, target_col);
                self.selection = Some(Selection::new(self.cursor, self.cursor));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Continue selection - update active point
                self.cursor = Cursor::at(target_line, target_col);
                if let Some(ref mut selection) = self.selection {
                    selection.active = self.cursor;
                }
                // Ensure cursor is visible during dragging (use cached virtual line count)
                self.viewport
                    .ensure_cursor_visible(&self.cursor, self.cached_virtual_line_count);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // Finish selection
                self.cursor = Cursor::at(target_line, target_col);
                if let Some(ref mut selection) = self.selection {
                    selection.active = self.cursor;
                    // If selection is empty, remove it
                    if selection.is_empty() {
                        self.selection = None;
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn get_working_directory(&self) -> Option<std::path::PathBuf> {
        // Return parent directory of the file if it's saved
        self.file_path()
            .and_then(|p| p.parent().map(|parent| parent.to_path_buf()))
    }

    fn captures_escape(&self) -> bool {
        // Capture Escape when search is active to prevent panel closure
        self.search_state.is_some()
    }

    fn to_session_panel(
        &mut self,
        session_dir: &std::path::Path,
    ) -> Option<crate::session::SessionPanel> {
        let path = self.file_path().map(|p| p.to_path_buf());

        // If buffer was unsaved but now has a path, clean up old temporary file
        if path.is_some() && self.unsaved_buffer_file.is_some() {
            if let Some(ref old_filename) = self.unsaved_buffer_file {
                let _ = crate::session::cleanup_unsaved_buffer(session_dir, old_filename);
            }
            self.unsaved_buffer_file = None;
        }

        // For unsaved buffers without a file path, save content to temporary file
        let unsaved_buffer_file = if path.is_none() {
            // Get buffer content
            let content = self.buffer.to_string();

            // Don't save empty buffers - return None to skip this panel
            if content.trim().is_empty() {
                // Clean up temporary file if one existed
                if let Some(ref old_filename) = self.unsaved_buffer_file {
                    let _ = crate::session::cleanup_unsaved_buffer(session_dir, old_filename);
                    self.unsaved_buffer_file = None;
                }
                return None;
            }

            // Reuse existing filename if available, generate new one only if needed
            let filename = if let Some(ref existing) = self.unsaved_buffer_file {
                existing.clone()
            } else {
                crate::session::generate_unsaved_filename()
            };

            // Save/update temporary file
            if let Err(e) = crate::session::save_unsaved_buffer(session_dir, &filename, &content) {
                eprintln!("Warning: Failed to save unsaved buffer: {}", e);
                None
            } else {
                // Store filename for future reuse
                self.unsaved_buffer_file = Some(filename.clone());
                Some(filename)
            }
        } else {
            None
        };

        Some(crate::session::SessionPanel::Editor {
            path,
            unsaved_buffer_file,
        })
    }
}
