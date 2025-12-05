use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use std::path::PathBuf;

use super::Panel;
use crate::editor::{Cursor, HighlightCache, SearchState, Selection, TextBuffer, Viewport};
use crate::logger;
use crate::state::AppState;
use crate::state::{ActiveModal, PendingAction};
use crate::syntax_highlighter;
use crate::ui::modal::InputModal;

/// Editor mode configuration
#[derive(Debug, Clone)]
pub struct EditorConfig {
    /// Whether syntax highlighting is enabled
    pub syntax_highlighting: bool,
    /// Read-only mode
    pub read_only: bool,
    /// Automatic line wrapping by window width
    pub word_wrap: bool,
    /// Tab size (number of spaces)
    pub tab_size: usize,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            syntax_highlighting: true,
            read_only: false,
            word_wrap: true,
            tab_size: 4,
        }
    }
}

impl EditorConfig {
    /// Create configuration for view mode (without editing)
    pub fn view_only() -> Self {
        Self {
            syntax_highlighting: true,
            read_only: true,
            word_wrap: true,
            tab_size: 4,
        }
    }
}

/// Editor information for status bar
#[derive(Debug, Clone)]
pub struct EditorInfo {
    pub line: usize,               // Current line (1-based)
    pub column: usize,             // Current column (1-based)
    pub tab_size: usize,           // Tab size
    pub encoding: String,          // Encoding (UTF-8)
    pub file_type: String,         // File type / syntax language
    pub read_only: bool,           // Read-only mode
    pub syntax_highlighting: bool, // Syntax highlighting enabled
}

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

/// Git line rendering information
struct GitLineInfo {
    status_color: Color,
    status_marker: char,
}

/// Virtual line representation for rendering
/// Allows inserting visual-only lines (like deletion markers) between real buffer lines
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VirtualLine {
    /// Real line from the buffer at given index (0-based)
    Real(usize),
    /// Visual deletion indicator after the given buffer line index
    /// Parameters: (after_line_idx, deletion_count)
    /// This is a visual-only line showing where content was deleted and how many lines
    DeletionMarker(usize, usize),
}

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
        if let Some(ref mut cache) = self.git_diff_cache {
            let _ = cache.update();
        } else if let Some(file_path) = self.file_path() {
            // Try to create cache if file is now in git repo
            let mut cache = crate::git::GitDiffCache::new(file_path.to_path_buf());
            if cache.update().is_ok() {
                self.git_diff_cache = Some(cache);
            }
        }
    }

    /// Schedule git diff update with debounce (300ms delay)
    pub fn schedule_git_diff_update(&mut self) {
        // Only schedule if we have a git diff cache
        if self.git_diff_cache.is_some() {
            self.git_diff_update_pending = Some(std::time::Instant::now());
        }
    }

    /// Check and apply pending git diff update if debounce time has passed
    pub fn check_pending_git_diff_update(&mut self) {
        const DEBOUNCE_MS: u64 = 300;

        if let Some(pending_time) = self.git_diff_update_pending {
            if pending_time.elapsed().as_millis() >= DEBOUNCE_MS as u128 {
                // Time has passed, perform update
                self.git_diff_update_pending = None;

                // Get current buffer content
                let content = self.buffer.to_string();

                // Update diff cache with current buffer
                if let Some(ref mut cache) = self.git_diff_cache {
                    let _ = cache.update_from_buffer(&content);
                }
            }
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
            .map(|s| {
                // Convert language name to readable format
                match s {
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
                    _ => s,
                }
            })
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

    /// Move cursor up
    fn move_cursor_up(&mut self) {
        self.cursor.move_up(1);
        self.clamp_cursor();
    }

    /// Move cursor down
    fn move_cursor_down(&mut self) {
        let max_line = self.buffer.line_count().saturating_sub(1);
        self.cursor.move_down(1, max_line);
        self.clamp_cursor();
    }

    /// Move cursor up by one visual line (accounting for word wrap)
    fn move_cursor_up_visual(&mut self) {
        if self.cached_content_width == 0 {
            // No word wrap or width not set - fall back to physical line movement
            self.move_cursor_up();
            return;
        }

        // Save preferred column on first vertical movement
        if self.preferred_column.is_none() {
            self.preferred_column = Some(self.cursor.column);
        }

        let content_width = self.cached_content_width;

        // Get current line text
        if let Some(line_text) = self.buffer.line(self.cursor.line) {
            let line_text = line_text.trim_end_matches('\n');
            let chars: Vec<char> = line_text.chars().collect();
            let line_len = chars.len();

            // Clamp cursor column to line length
            let cursor_col = self.cursor.column.min(line_len);

            // Get wrap points for this line
            let (_visual_rows, wrap_points) = self.get_line_wrap_points(line_text, content_width);

            // Find which visual row the cursor is on
            let current_visual_row = wrap_points.iter().filter(|&&wp| wp <= cursor_col).count();

            if current_visual_row > 0 {
                // We can move up within the same physical line
                let target_visual_row = current_visual_row - 1;

                // Calculate the visual row boundaries using actual wrap points
                let visual_row_start = if target_visual_row == 0 {
                    0
                } else {
                    wrap_points[target_visual_row - 1]
                };
                let visual_row_end = if target_visual_row < wrap_points.len() {
                    wrap_points[target_visual_row]
                } else {
                    line_len
                };

                // Position cursor at preferred column within the target visual row
                let target_col = self.preferred_column.unwrap_or(self.cursor.column);
                // Clamp target column to be within the target visual row
                let new_col = if target_col < visual_row_start {
                    visual_row_start
                } else if target_col >= visual_row_end {
                    visual_row_end.saturating_sub(1).max(visual_row_start)
                } else {
                    target_col
                };

                self.cursor.column = new_col;
                return;
            }
        }

        // Need to move to previous physical line
        if self.cursor.line > 0 {
            self.cursor.line -= 1;

            // Position at the last visual row of the previous line
            if let Some(line_text) = self.buffer.line(self.cursor.line) {
                let line_text = line_text.trim_end_matches('\n');
                let chars: Vec<char> = line_text.chars().collect();
                let line_len = chars.len();

                if line_len > 0 {
                    // Get wrap points for the previous line
                    let (visual_rows, wrap_points) =
                        self.get_line_wrap_points(line_text, content_width);
                    let last_visual_row = visual_rows - 1;

                    // Calculate the last visual row boundaries
                    let visual_row_start = if last_visual_row == 0 {
                        0
                    } else if last_visual_row - 1 < wrap_points.len() {
                        wrap_points[last_visual_row - 1]
                    } else {
                        0
                    };
                    let visual_row_end = line_len;

                    // Position cursor at preferred column within the last visual row
                    let target_col = self.preferred_column.unwrap_or(self.cursor.column);
                    // Clamp target column to be within the last visual row
                    let new_col = if target_col < visual_row_start {
                        visual_row_start
                    } else if target_col >= visual_row_end {
                        visual_row_end.saturating_sub(1).max(visual_row_start)
                    } else {
                        target_col
                    };

                    self.cursor.column = new_col;
                } else {
                    self.cursor.column = 0;
                }
            } else {
                self.cursor.column = 0;
            }
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

        // Save preferred column on first vertical movement
        if self.preferred_column.is_none() {
            self.preferred_column = Some(self.cursor.column);
        }

        let content_width = self.cached_content_width;

        // Get current line text
        if let Some(line_text) = self.buffer.line(self.cursor.line) {
            let line_text = line_text.trim_end_matches('\n');
            let chars: Vec<char> = line_text.chars().collect();
            let line_len = chars.len();

            // Clamp cursor column to line length
            let cursor_col = self.cursor.column.min(line_len);

            // Get wrap points for this line
            let (total_visual_rows, wrap_points) =
                self.get_line_wrap_points(line_text, content_width);

            // Find which visual row the cursor is on
            let current_visual_row = wrap_points.iter().filter(|&&wp| wp <= cursor_col).count();

            if current_visual_row + 1 < total_visual_rows {
                // We can move down within the same physical line
                let target_visual_row = current_visual_row + 1;

                // Calculate the visual row boundaries using actual wrap points
                let visual_row_start = if target_visual_row == 0 {
                    0
                } else if target_visual_row - 1 < wrap_points.len() {
                    wrap_points[target_visual_row - 1]
                } else {
                    0
                };
                let visual_row_end = if target_visual_row < wrap_points.len() {
                    wrap_points[target_visual_row]
                } else {
                    line_len
                };

                // Position cursor at preferred column within the target visual row
                let target_col = self.preferred_column.unwrap_or(self.cursor.column);
                // Clamp target column to be within the target visual row
                let new_col = if target_col < visual_row_start {
                    visual_row_start
                } else if target_col >= visual_row_end {
                    visual_row_end.saturating_sub(1).max(visual_row_start)
                } else {
                    target_col
                };

                self.cursor.column = new_col;
                return;
            }
        }

        // Need to move to next physical line
        let max_line = self.buffer.line_count().saturating_sub(1);
        if self.cursor.line < max_line {
            self.cursor.line += 1;

            // Position at the first visual row of the next line
            if let Some(line_text) = self.buffer.line(self.cursor.line) {
                let line_text = line_text.trim_end_matches('\n');
                let chars: Vec<char> = line_text.chars().collect();
                let line_len = chars.len();

                if line_len > 0 {
                    // Get wrap points for the next line
                    let (_visual_rows, wrap_points) =
                        self.get_line_wrap_points(line_text, content_width);

                    // First visual row: from 0 to first wrap point (or line_len if no wrapping)
                    let visual_row_start = 0;
                    let visual_row_end = if !wrap_points.is_empty() {
                        wrap_points[0]
                    } else {
                        line_len
                    };

                    // Position cursor at preferred column within the first visual row
                    let target_col = self.preferred_column.unwrap_or(self.cursor.column);
                    // Clamp target column to be within the first visual row
                    let new_col = if target_col < visual_row_start {
                        visual_row_start
                    } else if target_col >= visual_row_end {
                        visual_row_end.saturating_sub(1).max(visual_row_start)
                    } else {
                        target_col
                    };

                    self.cursor.column = new_col;
                } else {
                    self.cursor.column = 0;
                }
            } else {
                self.cursor.column = 0;
            }
        }

        self.clamp_cursor();
    }

    /// Move cursor left
    fn move_cursor_left(&mut self) {
        // Reset preferred column on horizontal movement
        self.preferred_column = None;

        self.cursor.move_left(1);

        // Clamp cursor to line length
        if self.cursor.line < self.buffer.line_count() {
            let line_len = self.buffer.line_len_graphemes(self.cursor.line);
            self.cursor.clamp_column(line_len);
        }
    }

    /// Move cursor right
    fn move_cursor_right(&mut self) {
        // Reset preferred column on horizontal movement
        self.preferred_column = None;

        let line_len = self.buffer.line_len_graphemes(self.cursor.line);
        let max_line = self.buffer.line_count().saturating_sub(1);
        self.cursor.move_right(1, line_len, max_line);
        self.clamp_cursor();
    }

    /// Move cursor to start of line
    fn move_to_line_start(&mut self) {
        // Reset preferred column on horizontal movement
        self.preferred_column = None;
        self.cursor.column = 0;
    }

    /// Move cursor to end of line
    fn move_to_line_end(&mut self) {
        // Reset preferred column on horizontal movement
        self.preferred_column = None;
        let line_len = self.buffer.line_len_graphemes(self.cursor.line);
        self.cursor.column = line_len;
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

        let content_width = self.cached_content_width;

        // Get current line text
        if let Some(line_text) = self.buffer.line(self.cursor.line) {
            let line_text = line_text.trim_end_matches('\n');
            let chars: Vec<char> = line_text.chars().collect();
            let line_len = chars.len();

            // Clamp cursor column to line length
            let cursor_col = self.cursor.column.min(line_len);

            // Get wrap points for this line
            let (_visual_rows, wrap_points) = self.get_line_wrap_points(line_text, content_width);

            // Find which visual row the cursor is on
            let current_visual_row = wrap_points.iter().filter(|&&wp| wp <= cursor_col).count();

            // Move to start of current visual row
            let visual_row_start = if current_visual_row == 0 {
                0
            } else if current_visual_row - 1 < wrap_points.len() {
                wrap_points[current_visual_row - 1]
            } else {
                0
            };

            self.cursor.column = visual_row_start;
        } else {
            self.cursor.column = 0;
        }
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

        let content_width = self.cached_content_width;

        // Get current line text
        if let Some(line_text) = self.buffer.line(self.cursor.line) {
            let line_text = line_text.trim_end_matches('\n');
            let chars: Vec<char> = line_text.chars().collect();
            let line_len = chars.len();

            // Clamp cursor column to line length
            let cursor_col = self.cursor.column.min(line_len);

            // Get wrap points for this line
            let (_visual_rows, wrap_points) = self.get_line_wrap_points(line_text, content_width);

            // Find which visual row the cursor is on
            let current_visual_row = wrap_points.iter().filter(|&&wp| wp <= cursor_col).count();

            // Calculate end of current visual row
            let visual_row_end = if current_visual_row < wrap_points.len() {
                wrap_points[current_visual_row]
            } else {
                line_len
            };

            // Move to end of current visual row
            self.cursor.column = visual_row_end;
        } else {
            self.cursor.column = 0;
        }
    }

    /// Move cursor page up
    fn page_up(&mut self) {
        let page_size = self.viewport.height;
        self.cursor.move_up(page_size);
        self.clamp_cursor();
        self.viewport.scroll_up(page_size);
    }

    /// Move cursor page down
    fn page_down(&mut self) {
        let page_size = self.viewport.height;
        let max_line = self.buffer.line_count().saturating_sub(1);
        self.cursor.move_down(page_size, max_line);
        self.clamp_cursor();
        // Use cached virtual line count for viewport scroll (accounts for deletion markers)
        self.viewport
            .scroll_down(page_size, self.cached_virtual_line_count);
    }

    /// Move cursor page up by visual lines (accounting for word wrap)
    fn page_up_visual(&mut self) {
        if self.cached_content_width == 0 {
            // No word wrap - fall back to physical line movement
            self.page_up();
            return;
        }

        let page_size = self.viewport.height;

        // Move up by page_size visual lines
        for _ in 0..page_size {
            let prev_line = self.cursor.line;
            let prev_col = self.cursor.column;

            self.move_cursor_up_visual();

            // Stop if we haven't moved (at top of document)
            if self.cursor.line == prev_line && self.cursor.column == prev_col {
                break;
            }
        }

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

        let page_size = self.viewport.height;
        let max_line = self.buffer.line_count().saturating_sub(1);

        // Move down by page_size visual lines
        for _ in 0..page_size {
            let prev_line = self.cursor.line;
            let prev_col = self.cursor.column;

            self.move_cursor_down_visual();

            // Stop if we haven't moved (at bottom of document)
            if self.cursor.line == prev_line && self.cursor.column == prev_col {
                break;
            }

            // Stop if we reached the last line
            if self.cursor.line >= max_line {
                break;
            }
        }

        // Don't manually scroll viewport - let ensure_cursor_visible() handle it during rendering
        // This is correct because the viewport needs to track visual rows, not buffer lines
    }

    /// Move cursor to start of document
    fn move_to_document_start(&mut self) {
        self.cursor = Cursor::at(0, 0);
        self.viewport.scroll_to_top();
    }

    /// Move cursor to end of document
    fn move_to_document_end(&mut self) {
        let max_line = self.buffer.line_count().saturating_sub(1);
        let line_len = self.buffer.line_len_graphemes(max_line);
        self.cursor = Cursor::at(max_line, line_len);
        // Use cached virtual line count for viewport scroll
        self.viewport
            .scroll_to_bottom(self.cached_virtual_line_count);
    }

    /// Select all
    fn select_all(&mut self) {
        let start = Cursor::at(0, 0);
        let max_line = self.buffer.line_count().saturating_sub(1);
        let line_len = self.buffer.line_len_graphemes(max_line);
        let end = Cursor::at(max_line, line_len);
        self.selection = Some(Selection::new(start, end));
        self.cursor = end;
    }

    /// Start new selection or continue existing
    fn start_or_extend_selection(&mut self) {
        if self.selection.is_none() {
            self.selection = Some(Selection::new(self.cursor, self.cursor));
        }
    }

    /// Update active point of selection (after cursor movement)
    fn update_selection_active(&mut self) {
        if let Some(ref mut selection) = self.selection {
            selection.active = self.cursor;
        }
    }

    /// Get selected text
    fn get_selected_text(&self) -> Option<String> {
        if let Some(ref selection) = self.selection {
            if !selection.is_empty() {
                let start = selection.start();
                let end = selection.end();

                // Simple implementation - get all text and cut the needed fragment
                // TODO: optimize for large selections
                let full_text = self.buffer.text();
                let lines: Vec<&str> = full_text.lines().collect();

                if start.line == end.line {
                    // Single line
                    if let Some(line) = lines.get(start.line) {
                        // Extract substring by character indices without allocating Vec<char>
                        let selected: String = line
                            .chars()
                            .skip(start.column)
                            .take(end.column.saturating_sub(start.column))
                            .collect();
                        return Some(selected);
                    }
                } else {
                    // Multiple lines
                    let mut result = String::new();
                    for (i, line) in lines.iter().enumerate() {
                        if i < start.line || i > end.line {
                            continue;
                        }

                        if i == start.line {
                            // Extract from start.column to end without Vec<char>
                            for ch in line.chars().skip(start.column) {
                                result.push(ch);
                            }
                            result.push('\n');
                        } else if i == end.line {
                            // Extract from beginning to end.column without Vec<char>
                            for ch in line.chars().take(end.column) {
                                result.push(ch);
                            }
                        } else {
                            result.push_str(line);
                            result.push('\n');
                        }
                    }
                    return Some(result);
                }
            }
        }
        None
    }

    /// Delete selected text
    fn delete_selection(&mut self) -> Result<()> {
        if let Some(ref selection) = self.selection {
            if !selection.is_empty() {
                let start = selection.start();
                let end = selection.end();
                self.buffer.delete_range(&start, &end)?;
                self.cursor = start;
                self.selection = None;

                // Invalidate highlighting cache
                // When deleting multiline selection, need to invalidate all lines after
                self.highlight_cache
                    .invalidate_range(start.line, self.buffer.line_count());

                // Schedule git diff update
                self.schedule_git_diff_update();
            }
        }
        Ok(())
    }

    /// Copy selected text to clipboard
    fn copy_to_clipboard(&mut self) -> Result<()> {
        match self.get_selected_text() {
            Some(text) => {
                // Debug: show what we're trying to copy
                let char_count = text.chars().count();
                let preview = if char_count > 50 {
                    let preview_text: String = text.chars().take(50).collect();
                    format!("{}...", preview_text)
                } else {
                    text.clone()
                };

                match crate::clipboard::copy(text) {
                    Ok(()) => {
                        self.status_message = Some(format!(
                            "Copied to clipboard: {:?} ({} chars)",
                            preview, char_count
                        ));
                    }
                    Err(e) => {
                        self.status_message = Some(format!("Clipboard error: {}", e));
                    }
                }
            }
            None => {
                self.status_message = Some("Nothing selected to copy".to_string());
            }
        }
        Ok(())
    }

    /// Cut selected text to clipboard
    fn cut_to_clipboard(&mut self) -> Result<()> {
        match self.get_selected_text() {
            Some(text) => {
                let char_count = text.chars().count();
                let preview = if char_count > 50 {
                    let preview_text: String = text.chars().take(50).collect();
                    format!("{}...", preview_text)
                } else {
                    text.clone()
                };

                match crate::clipboard::copy(text) {
                    Ok(()) => {
                        self.delete_selection()?;
                        self.status_message = Some(format!(
                            "Cut to clipboard: {:?} ({} chars)",
                            preview, char_count
                        ));
                    }
                    Err(e) => {
                        self.status_message = Some(format!("Clipboard error: {}", e));
                    }
                }
            }
            None => {
                self.status_message = Some("Nothing selected to cut".to_string());
            }
        }
        Ok(())
    }

    /// Paste from clipboard
    fn paste_from_clipboard(&mut self) -> Result<()> {
        // Close search mode when editing begins
        self.close_search();

        // Delete selected text before pasting
        self.delete_selection()?;

        // Read from system clipboard via arboard
        if let Some(text) = crate::clipboard::paste() {
            if !text.is_empty() {
                let start_line = self.cursor.line;
                let new_cursor = self.buffer.insert(&self.cursor, &text)?;
                self.cursor = new_cursor;
                self.clamp_cursor();

                // Invalidate highlighting cache
                if text.contains('\n') {
                    // Multiline paste
                    self.highlight_cache
                        .invalidate_range(start_line, self.buffer.line_count());
                } else {
                    // Single line paste
                    self.highlight_cache.invalidate_line(start_line);
                }

                // Schedule git diff update
                self.schedule_git_diff_update();
            }
        }
        Ok(())
    }

    /// Duplicate current line or selected lines
    fn duplicate_line(&mut self) -> Result<()> {
        // Determine which lines to duplicate
        let (start_line, end_line) = if let Some(ref selection) = self.selection {
            let start = selection.start();
            let end = selection.end();
            (start.line, end.line)
        } else {
            (self.cursor.line, self.cursor.line)
        };

        // Get all text and extract the lines to duplicate
        let full_text = self.buffer.text();
        let lines: Vec<&str> = full_text.lines().collect();

        // Build text to duplicate
        let mut text_to_duplicate = String::new();
        for line_idx in start_line..=end_line {
            if let Some(line) = lines.get(line_idx) {
                text_to_duplicate.push_str(line);
                if line_idx < end_line {
                    text_to_duplicate.push('\n');
                }
            }
        }

        // Insert newline and duplicated text after the last line
        text_to_duplicate.insert(0, '\n');

        // Move cursor to end of last line to duplicate
        let last_line_len = self.buffer.line_len_graphemes(end_line);
        let insert_cursor = Cursor {
            line: end_line,
            column: last_line_len,
        };

        self.buffer.insert(&insert_cursor, &text_to_duplicate)?;

        // Move cursor to the beginning of the first duplicated line
        self.cursor = Cursor {
            line: end_line + 1,
            column: 0,
        };
        self.clamp_cursor();

        // Clear selection
        self.selection = None;

        // Invalidate highlighting cache
        self.highlight_cache
            .invalidate_range(start_line, self.buffer.line_count());

        // Schedule git diff update
        self.schedule_git_diff_update();

        Ok(())
    }

    /// Clamp cursor position to valid values
    fn clamp_cursor(&mut self) {
        let max_line = self.buffer.line_count().saturating_sub(1);
        if self.cursor.line > max_line {
            self.cursor.line = max_line;
        }

        let line_len = self.buffer.line_len_graphemes(self.cursor.line);
        self.cursor.clamp_column(line_len);
    }

    /// Insert character at cursor position
    fn insert_char(&mut self, ch: char) -> Result<()> {
        // Close search mode when editing begins
        self.close_search();

        // Delete selected text before insertion
        self.delete_selection()?;

        let text = ch.to_string();
        let new_cursor = self.buffer.insert(&self.cursor, &text)?;
        self.cursor = new_cursor;
        self.clamp_cursor();

        // Invalidate highlighting cache for changed line
        self.highlight_cache.invalidate_line(self.cursor.line);

        // Schedule git diff update
        self.schedule_git_diff_update();

        Ok(())
    }

    /// Insert newline
    fn insert_newline(&mut self) -> Result<()> {
        // Close search mode when editing begins
        self.close_search();

        let old_line = self.cursor.line;

        // Delete selected text before insertion
        self.delete_selection()?;

        let new_cursor = self.buffer.insert(&self.cursor, "\n")?;
        self.cursor = new_cursor;
        self.clamp_cursor();

        // Invalidate all lines after inserting new line
        self.highlight_cache
            .invalidate_range(old_line, self.buffer.line_count());

        // Schedule git diff update
        self.schedule_git_diff_update();

        Ok(())
    }

    /// Delete character (backspace)
    fn backspace(&mut self) -> Result<()> {
        let old_line = self.cursor.line;
        let was_at_line_start = self.cursor.column == 0;

        if let Some(new_cursor) = self.buffer.backspace(&self.cursor)? {
            self.cursor = new_cursor;
            self.clamp_cursor();

            // Invalidate highlighting cache
            if was_at_line_start && old_line > 0 {
                // Deleted newline - need to invalidate all lines after
                self.highlight_cache
                    .invalidate_range(new_cursor.line, self.buffer.line_count());
            } else {
                // Regular character deletion
                self.highlight_cache.invalidate_line(new_cursor.line);
            }

            // Schedule git diff update
            self.schedule_git_diff_update();
        }
        Ok(())
    }

    /// Delete character (delete)
    fn delete(&mut self) -> Result<()> {
        let line_len = self.buffer.line_len_graphemes(self.cursor.line);
        let was_at_line_end = self.cursor.column >= line_len;

        if self.buffer.delete_char(&self.cursor)? {
            // Invalidate highlighting cache
            if was_at_line_end {
                // Deleted newline - need to invalidate all lines after
                self.highlight_cache
                    .invalidate_range(self.cursor.line, self.buffer.line_count());
            } else {
                // Regular character deletion
                self.highlight_cache.invalidate_line(self.cursor.line);
            }

            // Schedule git diff update
            self.schedule_git_diff_update();
        }
        Ok(())
    }

    /// Get git diff information for a line (cached helper)
    fn get_git_line_info(
        &self,
        line_idx: usize,
        config: &crate::config::Config,
        theme: &crate::theme::Theme,
    ) -> GitLineInfo {
        if !config.show_git_diff {
            return GitLineInfo {
                status_color: theme.disabled,
                status_marker: ' ',
            };
        }

        self.git_diff_cache
            .as_ref()
            .map(|cache| {
                let status = cache.get_line_status(line_idx);

                // Status marker and color
                let (status_color, status_marker) = match status {
                    crate::git::LineStatus::Added => (theme.success, ' '),
                    crate::git::LineStatus::Modified => (theme.warning, ' '),
                    crate::git::LineStatus::Unchanged => (theme.disabled, ' '),
                    crate::git::LineStatus::DeletedAfter => (theme.disabled, ' '),
                };

                GitLineInfo {
                    status_color,
                    status_marker,
                }
            })
            .unwrap_or(GitLineInfo {
                status_color: theme.disabled,
                status_marker: ' ',
            })
    }

    /// Build list of virtual lines (real buffer lines + deletion marker lines)
    /// Returns a Vec mapping visual row index to VirtualLine
    fn build_virtual_lines(&self, config: &crate::config::Config) -> Vec<VirtualLine> {
        let mut virtual_lines = Vec::new();
        let buffer_line_count = self.buffer.line_count();

        // If git diff is disabled or not available, just return real lines
        if !config.show_git_diff || self.git_diff_cache.is_none() {
            for line_idx in 0..buffer_line_count {
                virtual_lines.push(VirtualLine::Real(line_idx));
            }
            return virtual_lines;
        }

        let git_diff = self.git_diff_cache.as_ref().unwrap();

        // Interleave real lines with deletion markers
        for line_idx in 0..buffer_line_count {
            virtual_lines.push(VirtualLine::Real(line_idx));

            // Check if there's a deletion marker after this line
            if git_diff.has_deletion_marker(line_idx) {
                let deletion_count = git_diff.get_deletion_count(line_idx);
                virtual_lines.push(VirtualLine::DeletionMarker(line_idx, deletion_count));
            }
        }

        virtual_lines
    }

    /// Get the total count of virtual lines (real buffer lines + deletion marker lines + word wrap)
    /// This is used for viewport calculations to account for deletion markers and word wrapping
    fn virtual_line_count(&self, config: &crate::config::Config) -> usize {
        // If word wrap is enabled, count visual rows instead of buffer lines
        if self.config.word_wrap && self.cached_content_width > 0 {
            // Use calculate_total_visual_rows which accounts for word wrapping
            let total_visual_rows = self.calculate_total_visual_rows(self.cached_content_width);

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
        let line_number_width = 6; // "  123  " (line number + 2 git markers)
        let content_width = area.width.saturating_sub(line_number_width) as usize;
        let content_height = area.height as usize;

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

        // Pre-extract selection information
        let selection_range = self.selection.as_ref().map(|s| (s.start(), s.end()));

        // Pre-extract match information to avoid borrow checker issues
        let search_matches: Vec<(usize, usize, usize)> = if let Some(ref search) = self.search_state
        {
            search
                .matches
                .iter()
                .map(|c| (c.line, c.column, search.query.len()))
                .collect()
        } else {
            Vec::new()
        };
        let current_match_idx = self.search_state.as_ref().and_then(|s| s.current_match);

        // Performance optimization: Create HashMap for O(1) search match lookups
        // Maps (line, column) to match index for fast character-by-character highlighting
        let mut search_match_map: std::collections::HashMap<(usize, usize), usize> =
            std::collections::HashMap::with_capacity(search_matches.len() * 10);
        for (idx, &(m_line, m_col, m_len)) in search_matches.iter().enumerate() {
            for col in m_col..(m_col + m_len) {
                search_match_map.insert((m_line, col), idx);
            }
        }

        // Variable to track cursor position in word wrap mode
        let mut cursor_viewport_pos: Option<(usize, usize)> = None;

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
                        let git_info = self.get_git_line_info(line_idx, config, theme);

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
                            cursor_viewport_pos = Some((visual_row, 0));
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
                                let git_info = self.get_git_line_info(line_idx, config, theme);

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

                                                // Проверить, является ли это совпадением поиска (O(1) HashMap lookup)
                                                let match_idx = search_match_map
                                                    .get(&(line_idx, segment_char_idx))
                                                    .copied();

                                                // Проверить, находится ли символ в выделении
                                                let is_selected =
                                                    if let Some((sel_start, sel_end)) =
                                                        &selection_range
                                                    {
                                                        let pos = crate::editor::Cursor::at(
                                                            line_idx,
                                                            segment_char_idx,
                                                        );
                                                        (pos.line > sel_start.line
                                                            || (pos.line == sel_start.line
                                                                && pos.column >= sel_start.column))
                                                            && (pos.line < sel_end.line
                                                                || (pos.line == sel_end.line
                                                                    && pos.column < sel_end.column))
                                                    } else {
                                                        false
                                                    };

                                                // Определить финальный стиль
                                                let final_style = if let Some(idx) = match_idx {
                                                    if Some(idx) == current_match_idx {
                                                        current_match_style
                                                    } else {
                                                        search_match_style
                                                    }
                                                } else if is_selected {
                                                    selection_style
                                                } else if is_cursor_line {
                                                    segment_style.bg(theme.accented_bg)
                                                } else {
                                                    *segment_style
                                                };
                                                cell.set_style(final_style);
                                            }
                                        }

                                        // Проверить позицию курсора
                                        if is_cursor_line && self.cursor.column == segment_char_idx
                                        {
                                            cursor_viewport_pos = Some((visual_row, visual_col));
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
                                cursor_viewport_pos =
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

                            // Отрисовать виртуальную строку с deletion marker, линией и текстом

                            // Пустое место для номера строки (4 пробела)
                            for i in 0..4 {
                                let x = area.x + i as u16;
                                let y = area.y + visual_row as u16;
                                if let Some(cell) = buf.cell_mut((x, y)) {
                                    cell.set_char(' ');
                                    cell.set_style(Style::default().fg(theme.disabled));
                                }
                            }

                            // Красный маркер ▶ (показывает что здесь было удаление)
                            let marker_style = Style::default().fg(theme.error);
                            let x = area.x + 4; // Позиция после пробелов
                            let y = area.y + visual_row as u16;
                            if let Some(cell) = buf.cell_mut((x, y)) {
                                cell.set_char('▶');
                                cell.set_style(marker_style);
                            }

                            // Пустое место после маркера
                            let x = area.x + 5;
                            let y = area.y + visual_row as u16;
                            if let Some(cell) = buf.cell_mut((x, y)) {
                                cell.set_char(' ');
                                cell.set_style(Style::default().fg(theme.disabled));
                            }

                            // Content area - псевдографическая линия с текстом по центру
                            let line_style = Style::default().fg(theme.disabled);
                            let deletion_text = format!(
                                " {} ",
                                crate::i18n::t().editor_deletion_marker(deletion_count)
                            );
                            // Performance optimization: Convert to Vec<char> once for O(1) indexing
                            let deletion_chars: Vec<char> = deletion_text.chars().collect();
                            let text_len = deletion_chars.len();

                            // Вычислить позицию для центрирования текста
                            let text_start_col = if content_width > text_len {
                                (content_width - text_len) / 2
                            } else {
                                0
                            };

                            for col in 0..content_width {
                                let x = area.x + line_number_width + col as u16;
                                let y = area.y + visual_row as u16;
                                if x < area.x + area.width && y < area.y + area.height {
                                    if let Some(cell) = buf.cell_mut((x, y)) {
                                        // Проверить, находится ли эта позиция в области текста
                                        if col >= text_start_col && col < text_start_col + text_len
                                        {
                                            // Отрисовать символ из текста (O(1) indexing)
                                            let text_idx = col - text_start_col;
                                            let ch = deletion_chars
                                                .get(text_idx)
                                                .copied()
                                                .unwrap_or('─');
                                            cell.set_char(ch);
                                        } else {
                                            // Отрисовать линию
                                            cell.set_char('─');
                                        }
                                        cell.set_style(line_style);
                                    }
                                }
                            }

                            visual_row += 1;
                        }
                    }
                }

                line_idx += 1;
            }

            // Отрисовать курсор в режиме word wrap
            if let Some((row, col)) = cursor_viewport_pos {
                let cursor_x = area.x + line_number_width + col as u16;
                let cursor_y = area.y + row as u16;

                if cursor_x < area.x + area.width && cursor_y < area.y + area.height {
                    if let Some(cell) = buf.cell_mut((cursor_x, cursor_y)) {
                        // Инверсия: swap fg и bg с fallback к theme цветам
                        let current_fg = match cell.fg {
                            Color::Reset => theme.fg,
                            color => color,
                        };
                        let current_bg = match cell.bg {
                            Color::Reset => theme.bg,
                            color => color,
                        };
                        cell.set_style(
                            Style::default()
                                .bg(current_fg)
                                .fg(current_bg)
                                .add_modifier(Modifier::BOLD),
                        );
                    }
                }
            }
        } else {
            // Обычный режим (без word wrap) - используем виртуальные строки
            // Построить список виртуальных строк (real buffer lines + deletion markers)
            let virtual_lines = self.build_virtual_lines(config);

            // Найти индекс первой виртуальной строки для viewport.top_line (buffer line index)
            // viewport.top_line это buffer line index, нужно преобразовать в virtual line index
            let start_virtual_idx = virtual_lines
                .iter()
                .position(|vline| matches!(vline, VirtualLine::Real(idx) if *idx >= self.viewport.top_line))
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
                    VirtualLine::Real(line_idx) => {
                        // Обычная строка из буфера
                        let is_cursor_line = line_idx == self.cursor.line;
                        let style = if is_cursor_line {
                            cursor_line_style
                        } else {
                            text_style
                        };

                        // Номер строки с git diff статусом
                        let git_info = self.get_git_line_info(line_idx, config, theme);

                        // Отрисовать номер строки (4 символа) + status marker (1 символ)
                        let line_num_style = Style::default().fg(git_info.status_color);
                        let line_num_part =
                            format!("{:>4}{}", line_idx + 1, git_info.status_marker);

                        for (i, ch) in line_num_part.chars().enumerate() {
                            let x = area.x + i as u16;
                            let y = area.y + row as u16;
                            if let Some(cell) = buf.cell_mut((x, y)) {
                                cell.set_char(ch);
                                cell.set_style(line_num_style);
                            }
                        }

                        // НЕ рисуем deletion marker здесь - он теперь на отдельной виртуальной строке
                        // Рисуем пробел вместо старого deletion marker
                        let x = area.x + 5;
                        let y = area.y + row as u16;
                        if let Some(cell) = buf.cell_mut((x, y)) {
                            cell.set_char(' ');
                            cell.set_style(line_num_style);
                        }

                        // Содержимое строки с подсветкой синтаксиса
                        if let Some(line_text) = self.buffer.line(line_idx) {
                            // Убрать перевод строки в конце
                            let line_text = line_text.trim_end_matches('\n');

                            // Получить подсветку синтаксиса для строки (без клонирования)
                            // Учитываем config.syntax_highlighting для отключения подсветки
                            let segments = if self.config.syntax_highlighting
                                && self.highlight_cache.has_syntax()
                            {
                                self.highlight_cache.get_line_segments(line_idx, line_text)
                            } else {
                                // Для текста без подсветки используем временный массив
                                &[(line_text.to_string(), style)][..]
                            };

                            // Отрисовать сегменты с подсветкой
                            let mut col_offset = 0;
                            for (segment_text, segment_style) in segments {
                                for ch in segment_text.chars() {
                                    if col_offset >= self.viewport.left_column
                                        && col_offset < self.viewport.left_column + content_width
                                    {
                                        let x = area.x
                                            + line_number_width
                                            + (col_offset - self.viewport.left_column) as u16;
                                        let y = area.y + row as u16;

                                        if x < area.x + area.width && y < area.y + area.height {
                                            if let Some(cell) = buf.cell_mut((x, y)) {
                                                cell.set_char(ch);

                                                // Проверить, является ли это совпадением поиска (O(1) HashMap lookup)
                                                let match_idx = search_match_map
                                                    .get(&(line_idx, col_offset))
                                                    .copied();

                                                // Проверить, находится ли символ в выделении
                                                let is_selected =
                                                    if let Some((sel_start, sel_end)) =
                                                        &selection_range
                                                    {
                                                        let pos = crate::editor::Cursor::at(
                                                            line_idx, col_offset,
                                                        );
                                                        (pos.line > sel_start.line
                                                            || (pos.line == sel_start.line
                                                                && pos.column >= sel_start.column))
                                                            && (pos.line < sel_end.line
                                                                || (pos.line == sel_end.line
                                                                    && pos.column < sel_end.column))
                                                    } else {
                                                        false
                                                    };

                                                // Определить финальный стиль с учетом подсветки, выделения, курсорной линии и совпадений
                                                let final_style = if let Some(idx) = match_idx {
                                                    // Это совпадение поиска
                                                    if Some(idx) == current_match_idx {
                                                        // Текущее активное совпадение
                                                        current_match_style
                                                    } else {
                                                        // Обычное совпадение
                                                        search_match_style
                                                    }
                                                } else if is_selected {
                                                    // Выделенный текст
                                                    selection_style
                                                } else if is_cursor_line {
                                                    // Курсорная линия (но не совпадение и не выделение)
                                                    segment_style.bg(theme.accented_bg)
                                                } else {
                                                    // Обычный текст
                                                    *segment_style
                                                };
                                                cell.set_style(final_style);
                                            }
                                        }
                                    }
                                    col_offset += 1;
                                }
                            }

                            // Заполнить остаток строки фоном (для курсорной линии)
                            if is_cursor_line {
                                let line_len = line_text.chars().count();
                                for col in line_len..content_width {
                                    if col >= self.viewport.left_column {
                                        let x = area.x
                                            + line_number_width
                                            + (col - self.viewport.left_column) as u16;
                                        let y = area.y + row as u16;

                                        if x < area.x + area.width && y < area.y + area.height {
                                            if let Some(cell) = buf.cell_mut((x, y)) {
                                                cell.set_char(' ');
                                                cell.set_style(cursor_line_style);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    VirtualLine::DeletionMarker(_after_line_idx, deletion_count) => {
                        // Виртуальная строка - deletion marker
                        // Отрисовать gutter с красным маркером ▶ и content с линией и текстом

                        // Пустое место для номера строки (4 пробела)
                        for i in 0..4 {
                            let x = area.x + i as u16;
                            let y = area.y + row as u16;
                            if let Some(cell) = buf.cell_mut((x, y)) {
                                cell.set_char(' ');
                                cell.set_style(Style::default().fg(theme.disabled));
                            }
                        }

                        // Красный маркер ▶ (показывает что здесь было удаление)
                        let marker_style = Style::default().fg(theme.error);
                        let x = area.x + 4; // Позиция после пробелов
                        let y = area.y + row as u16;
                        if let Some(cell) = buf.cell_mut((x, y)) {
                            cell.set_char('▶');
                            cell.set_style(marker_style);
                        }

                        // Пустое место после маркера
                        let x = area.x + 5;
                        let y = area.y + row as u16;
                        if let Some(cell) = buf.cell_mut((x, y)) {
                            cell.set_char(' ');
                            cell.set_style(Style::default().fg(theme.disabled));
                        }

                        // Content area - псевдографическая линия с текстом по центру
                        let line_style = Style::default().fg(theme.disabled);
                        let deletion_text = format!(
                            " {} ",
                            crate::i18n::t().editor_deletion_marker(deletion_count)
                        );
                        // Performance optimization: Convert to Vec<char> once for O(1) indexing
                        let deletion_chars: Vec<char> = deletion_text.chars().collect();
                        let text_len = deletion_chars.len();

                        // Вычислить позицию для центрирования текста
                        let text_start_col = if content_width > text_len {
                            (content_width - text_len) / 2
                        } else {
                            0
                        };

                        for col in 0..content_width {
                            let x = area.x + line_number_width + col as u16;
                            let y = area.y + row as u16;
                            if x < area.x + area.width && y < area.y + area.height {
                                if let Some(cell) = buf.cell_mut((x, y)) {
                                    // Проверить, находится ли эта позиция в области текста
                                    if col >= text_start_col && col < text_start_col + text_len {
                                        // Отрисовать символ из текста (O(1) indexing)
                                        let text_idx = col - text_start_col;
                                        let ch =
                                            deletion_chars.get(text_idx).copied().unwrap_or('─');
                                        cell.set_char(ch);
                                    } else {
                                        // Отрисовать линию
                                        cell.set_char('─');
                                    }
                                    cell.set_style(line_style);
                                }
                            }
                        }

                        // Пропускаем рендеринг контента для deletion marker
                        continue;
                    }
                }
            }

            // Отрисовать курсор с учетом виртуальных строк
            // Найти virtual line index для cursor.line
            let cursor_virtual_idx = virtual_lines.iter().position(
                |vline| matches!(vline, VirtualLine::Real(idx) if *idx == self.cursor.line),
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

                        if cursor_x < area.x + area.width
                            && cursor_y < area.y + area.height
                            && viewport_col < content_width
                        {
                            if let Some(cell) = buf.cell_mut((cursor_x, cursor_y)) {
                                // Инверсия: swap fg и bg с fallback к theme цветам
                                let current_fg = match cell.fg {
                                    Color::Reset => theme.fg,
                                    color => color,
                                };
                                let current_bg = match cell.bg {
                                    Color::Reset => theme.bg,
                                    color => color,
                                };
                                cell.set_style(
                                    Style::default()
                                        .bg(current_fg)
                                        .fg(current_bg)
                                        .add_modifier(Modifier::BOLD),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// Start search
    pub fn start_search(&mut self, query: String, case_sensitive: bool) {
        let mut search = SearchState::new(query, case_sensitive);

        // Perform search throughout document
        self.perform_search(&mut search);

        // Find closest match to current cursor
        search.find_closest_match(&self.cursor);

        // Move cursor to end of match and create selection
        if let Some(match_cursor) = search.current_match_cursor() {
            let query_len = search.query.chars().count();
            let end_cursor = Cursor::at(match_cursor.line, match_cursor.column + query_len);
            self.cursor = end_cursor;
            self.selection = Some(Selection::new(*match_cursor, end_cursor));
        }

        self.search_state = Some(search);
    }

    /// Perform search in document
    fn perform_search(&self, search: &mut SearchState) {
        search.matches.clear();

        if search.query.is_empty() {
            return;
        }

        let query = if search.case_sensitive {
            search.query.clone()
        } else {
            search.query.to_lowercase()
        };

        // Search through all lines
        for line_idx in 0..self.buffer.line_count() {
            if let Some(line_text) = self.buffer.line(line_idx) {
                let search_text = if search.case_sensitive {
                    line_text.to_string()
                } else {
                    line_text.to_lowercase()
                };

                // Find all occurrences in line
                let mut col = 0;
                while let Some(pos) = search_text[col..].find(&query) {
                    let match_col = col + pos;
                    search.matches.push(Cursor {
                        line: line_idx,
                        column: match_col,
                    });
                    col = match_col + 1;
                }
            }
        }
    }

    /// Go to next match
    pub fn search_next(&mut self) {
        if let Some(ref mut search) = self.search_state {
            search.next_match();
            if let Some(match_cursor) = search.current_match_cursor() {
                let query_len = search.query.chars().count();
                let end_cursor = Cursor::at(match_cursor.line, match_cursor.column + query_len);
                self.cursor = end_cursor;
                self.selection = Some(Selection::new(*match_cursor, end_cursor));
            }
        }
    }

    /// Go to previous match
    pub fn search_prev(&mut self) {
        if let Some(ref mut search) = self.search_state {
            search.prev_match();
            if let Some(match_cursor) = search.current_match_cursor() {
                let query_len = search.query.chars().count();
                let end_cursor = Cursor::at(match_cursor.line, match_cursor.column + query_len);
                self.cursor = end_cursor;
                self.selection = Some(Selection::new(*match_cursor, end_cursor));
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
        let mut search = SearchState::new_with_replace(query, replace_with, case_sensitive);

        // Perform search throughout document
        self.perform_search(&mut search);

        // Find closest match to current cursor
        search.find_closest_match(&self.cursor);

        // Move cursor to first match and create selection
        if let Some(match_cursor) = search.current_match_cursor() {
            let query_len = search.query.chars().count();
            let end_cursor = Cursor::at(match_cursor.line, match_cursor.column + query_len);
            self.cursor = end_cursor;
            self.selection = Some(Selection::new(*match_cursor, end_cursor));
        }

        self.search_state = Some(search);
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
        let (match_cursor, replace_with, query_len) = if let Some(ref search) = self.search_state {
            if let (Some(replace_with), Some(idx)) = (&search.replace_with, search.current_match) {
                if let Some(match_cursor) = search.matches.get(idx).cloned() {
                    (match_cursor, replace_with.clone(), search.query.len())
                } else {
                    return Ok(());
                }
            } else {
                return Ok(());
            }
        } else {
            return Ok(());
        };

        // Delete old text
        let end_cursor = Cursor {
            line: match_cursor.line,
            column: match_cursor.column + query_len,
        };

        // Select found text
        self.selection = Some(Selection {
            anchor: match_cursor,
            active: end_cursor,
        });

        // Delete selected text
        self.delete_selection()?;

        // Insert new text
        self.cursor = match_cursor;
        self.buffer.insert(&self.cursor, &replace_with)?;
        self.cursor.column += replace_with.len();

        // Invalidate highlighting cache for changed line
        self.highlight_cache.invalidate_line(match_cursor.line);

        // Update search_state
        if let Some(ref mut search) = self.search_state {
            if let Some(idx) = search.current_match {
                // Remove this match from list
                search.matches.remove(idx);

                // Update positions of remaining matches on the same line after replacement point
                let replacement_offset = replace_with.len() as isize - query_len as isize;
                if replacement_offset != 0 {
                    for match_pos in search.matches.iter_mut() {
                        // Only update matches on same line that come after the replacement
                        if match_pos.line == match_cursor.line
                            && match_pos.column > match_cursor.column
                        {
                            // Adjust column position by the length difference
                            match_pos.column =
                                (match_pos.column as isize + replacement_offset).max(0) as usize;
                        }
                    }
                }

                // Update current match index
                if search.matches.is_empty() {
                    search.current_match = None;
                } else if idx >= search.matches.len() {
                    search.current_match = Some(search.matches.len() - 1);
                }

                // Move cursor to next match and create selection
                if let Some(match_cursor) = search.current_match_cursor() {
                    let query_len = search.query.chars().count();
                    let end_cursor = Cursor::at(match_cursor.line, match_cursor.column + query_len);
                    self.cursor = end_cursor;
                    self.selection = Some(Selection::new(*match_cursor, end_cursor));
                }
            }
        }

        // Schedule git diff update
        self.schedule_git_diff_update();

        Ok(())
    }

    /// Replace all matches
    pub fn replace_all(&mut self) -> Result<usize> {
        let mut count = 0;

        if let Some(ref search) = self.search_state.clone() {
            if let Some(replace_with) = &search.replace_with {
                // Replace in reverse order to avoid position shifts
                for match_cursor in search.matches.iter().rev() {
                    // Delete old text
                    let end_cursor = Cursor {
                        line: match_cursor.line,
                        column: match_cursor.column + search.query.len(),
                    };

                    // Select found text
                    self.selection = Some(Selection {
                        anchor: *match_cursor,
                        active: end_cursor,
                    });

                    // Delete selected text
                    self.delete_selection()?;

                    // Insert new text
                    self.cursor = *match_cursor;
                    self.buffer.insert(&self.cursor, replace_with)?;

                    // Invalidate highlighting cache
                    self.highlight_cache.invalidate_line(match_cursor.line);

                    count += 1;
                }

                // Clear search state
                self.search_state = None;

                // Schedule git diff update
                self.schedule_git_diff_update();
            }
        }

        Ok(count)
    }

    /// Get wrap points for a line, accounting for smart wrapping setting
    /// Returns (number of visual rows, vector of wrap points)
    /// Wrap points are character positions where new visual lines start
    fn get_line_wrap_points(&self, line_text: &str, content_width: usize) -> (usize, Vec<usize>) {
        if content_width == 0 {
            return (1, Vec::new());
        }

        let chars: Vec<char> = line_text.chars().collect();
        let line_len = chars.len();

        if line_len == 0 {
            return (1, Vec::new());
        }

        if line_len <= content_width {
            return (1, Vec::new()); // No wrapping needed
        }

        if self.cached_use_smart_wrap {
            // Use smart wrapping from wrap.rs module
            let wrap_points =
                crate::editor::calculate_wrap_points_for_line(line_text, content_width);
            let visual_rows = wrap_points.len() + 1; // +1 for the first line
            (visual_rows, wrap_points)
        } else {
            // Use simple wrapping (hard break at content_width)
            let visual_rows = line_len.div_ceil(content_width);
            let mut wrap_points = Vec::new();
            for i in 1..visual_rows {
                wrap_points.push(i * content_width);
            }
            (visual_rows, wrap_points)
        }
    }

    /// Calculate the visual row index for the cursor position
    /// Returns the visual row index from viewport.top_line
    /// This accounts for word wrapping - a single buffer line may span multiple visual rows
    #[allow(dead_code)]
    fn calculate_visual_row_for_cursor(&self, content_width: usize) -> usize {
        if content_width == 0 || !self.config.word_wrap {
            // No word wrap - visual row is just buffer line offset from top
            return self.cursor.line.saturating_sub(self.viewport.top_line);
        }

        let mut visual_row = 0;
        let mut line_idx = self.viewport.top_line;

        // Count visual rows from viewport top to cursor line
        while line_idx < self.cursor.line && line_idx < self.buffer.line_count() {
            if let Some(line_text) = self.buffer.line(line_idx) {
                let line_text = line_text.trim_end_matches('\n');
                let (line_visual_rows, _) = self.get_line_wrap_points(line_text, content_width);
                visual_row += line_visual_rows;
            } else {
                visual_row += 1; // Empty line = 1 visual row
            }
            line_idx += 1;
        }

        // Now add the visual row within the cursor's line
        if let Some(line_text) = self.buffer.line(self.cursor.line) {
            let line_text = line_text.trim_end_matches('\n');
            let (_line_visual_rows, wrap_points) =
                self.get_line_wrap_points(line_text, content_width);

            // Find which visual row within this line the cursor is on
            let cursor_col = self.cursor.column.min(line_text.chars().count());
            let row_within_line = wrap_points.iter().filter(|&&wp| wp <= cursor_col).count();
            visual_row += row_within_line;
        }

        visual_row
    }

    /// Calculate total number of visual rows in the entire buffer
    /// This accounts for word wrapping - returns total visual rows across all lines
    fn calculate_total_visual_rows(&self, content_width: usize) -> usize {
        if content_width == 0 || !self.config.word_wrap {
            // No word wrap - just return buffer line count
            return self.buffer.line_count();
        }

        let mut total_visual_rows = 0;

        for line_idx in 0..self.buffer.line_count() {
            if let Some(line_text) = self.buffer.line(line_idx) {
                let line_text = line_text.trim_end_matches('\n');
                let (line_visual_rows, _) = self.get_line_wrap_points(line_text, content_width);
                total_visual_rows += line_visual_rows;
            } else {
                total_visual_rows += 1; // Empty line = 1 visual row
            }
        }

        total_visual_rows
    }

    /// Convert visual row to buffer position accounting for word wrap
    /// Returns (buffer_line, column_offset) for the given visual row
    fn visual_row_to_buffer_position(
        &self,
        visual_row: usize,
        content_width: usize,
    ) -> (usize, usize) {
        if content_width == 0 {
            return (self.viewport.top_line + visual_row, 0);
        }

        let mut current_visual_row = 0;
        let mut line_idx = self.viewport.top_line;

        while line_idx < self.buffer.line_count() {
            if let Some(line_text) = self.buffer.line(line_idx) {
                let line_text = line_text.trim_end_matches('\n');

                // Calculate how many visual rows this line occupies using actual wrap points
                let (visual_rows_for_line, wrap_points) =
                    self.get_line_wrap_points(line_text, content_width);

                // Check if target visual row is in this buffer line
                if current_visual_row + visual_rows_for_line > visual_row {
                    // Found the buffer line containing the target visual row
                    let row_within_line = visual_row - current_visual_row;

                    // Calculate column offset using actual wrap points
                    let column_offset = if row_within_line == 0 {
                        0 // First visual line starts at column 0
                    } else if row_within_line - 1 < wrap_points.len() {
                        wrap_points[row_within_line - 1] // Use actual wrap point
                    } else {
                        0 // Shouldn't happen, but safe fallback
                    };

                    return (line_idx, column_offset);
                }

                current_visual_row += visual_rows_for_line;
            } else {
                // If line doesn't exist, treat as empty (1 visual row)
                if current_visual_row >= visual_row {
                    return (line_idx, 0);
                }
                current_visual_row += 1;
            }

            line_idx += 1;
        }

        // If we've exhausted all lines, return the last line
        (self.buffer.line_count().saturating_sub(1), 0)
    }
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
                self.close_search();
                self.selection = None;
                if self.config.word_wrap && self.cached_content_width > 0 {
                    self.move_cursor_up_visual();
                } else {
                    self.move_cursor_up();
                }
            }
            (KeyCode::Down, KeyModifiers::NONE) => {
                self.close_search();
                self.selection = None;
                if self.config.word_wrap && self.cached_content_width > 0 {
                    self.move_cursor_down_visual();
                } else {
                    self.move_cursor_down();
                }
            }
            (KeyCode::Left, KeyModifiers::NONE) => {
                self.close_search();
                self.selection = None;
                self.move_cursor_left();
            }
            (KeyCode::Right, KeyModifiers::NONE) => {
                self.close_search();
                self.selection = None;
                self.move_cursor_right();
            }
            (KeyCode::Home, KeyModifiers::NONE) => {
                self.close_search();
                self.selection = None;
                if self.config.word_wrap && self.cached_content_width > 0 {
                    self.move_to_visual_line_start();
                } else {
                    self.move_to_line_start();
                }
            }
            (KeyCode::End, KeyModifiers::NONE) => {
                self.close_search();
                self.selection = None;
                if self.config.word_wrap && self.cached_content_width > 0 {
                    self.move_to_visual_line_end();
                } else {
                    self.move_to_line_end();
                }
            }
            (KeyCode::PageUp, KeyModifiers::NONE) => {
                self.close_search();
                self.selection = None;
                if self.config.word_wrap && self.cached_content_width > 0 {
                    self.page_up_visual();
                } else {
                    self.page_up();
                }
            }
            (KeyCode::PageDown, KeyModifiers::NONE) => {
                self.close_search();
                self.selection = None;
                if self.config.word_wrap && self.cached_content_width > 0 {
                    self.page_down_visual();
                } else {
                    self.page_down();
                }
            }
            (KeyCode::Home, KeyModifiers::CONTROL) => {
                self.close_search();
                self.selection = None;
                self.move_to_document_start();
            }
            (KeyCode::End, KeyModifiers::CONTROL) => {
                self.close_search();
                self.selection = None;
                self.move_to_document_end();
            }

            // Navigation with selection (Shift) - closes search
            (KeyCode::Up, KeyModifiers::SHIFT) => {
                self.close_search();
                self.start_or_extend_selection();
                if self.config.word_wrap && self.cached_content_width > 0 {
                    self.move_cursor_up_visual();
                } else {
                    self.move_cursor_up();
                }
                self.update_selection_active();
            }
            (KeyCode::Down, KeyModifiers::SHIFT) => {
                self.close_search();
                self.start_or_extend_selection();
                if self.config.word_wrap && self.cached_content_width > 0 {
                    self.move_cursor_down_visual();
                } else {
                    self.move_cursor_down();
                }
                self.update_selection_active();
            }
            (KeyCode::Left, KeyModifiers::SHIFT) => {
                self.close_search();
                self.start_or_extend_selection();
                self.move_cursor_left();
                self.update_selection_active();
            }
            (KeyCode::Right, KeyModifiers::SHIFT) => {
                self.close_search();
                self.start_or_extend_selection();
                self.move_cursor_right();
                self.update_selection_active();
            }
            (KeyCode::Home, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    && !modifiers.contains(KeyModifiers::CONTROL) =>
            {
                logger::debug("Shift+Home pressed - moving to line start with selection");
                self.close_search();
                self.start_or_extend_selection();
                if self.config.word_wrap && self.cached_content_width > 0 {
                    logger::debug("Using visual line start");
                    self.move_to_visual_line_start();
                } else {
                    logger::debug("Using physical line start");
                    self.move_to_line_start();
                }
                self.update_selection_active();
            }
            (KeyCode::End, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    && !modifiers.contains(KeyModifiers::CONTROL) =>
            {
                logger::debug("Shift+End pressed - moving to line end with selection");
                self.close_search();
                self.start_or_extend_selection();
                if self.config.word_wrap && self.cached_content_width > 0 {
                    logger::debug("Using visual line end");
                    self.move_to_visual_line_end();
                } else {
                    logger::debug("Using physical line end");
                    self.move_to_line_end();
                }
                self.update_selection_active();
            }
            (KeyCode::PageUp, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    && !modifiers.contains(KeyModifiers::CONTROL) =>
            {
                logger::debug("Shift+PageUp pressed - paging up with selection");
                self.close_search();
                self.start_or_extend_selection();
                if self.config.word_wrap && self.cached_content_width > 0 {
                    logger::debug("Using visual page up");
                    self.page_up_visual();
                } else {
                    logger::debug("Using physical page up");
                    self.page_up();
                }
                self.update_selection_active();
            }
            (KeyCode::PageDown, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    && !modifiers.contains(KeyModifiers::CONTROL) =>
            {
                logger::debug("Shift+PageDown pressed - paging down with selection");
                self.close_search();
                self.start_or_extend_selection();
                if self.config.word_wrap && self.cached_content_width > 0 {
                    logger::debug("Using visual page down");
                    self.page_down_visual();
                } else {
                    logger::debug("Using physical page down");
                    self.page_down();
                }
                self.update_selection_active();
            }
            // Shift+Ctrl+Home/End - select to start/end of document - closes search
            (KeyCode::Home, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    && modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.close_search();
                self.start_or_extend_selection();
                self.move_to_document_start();
                self.update_selection_active();
            }
            (KeyCode::End, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    && modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.close_search();
                self.start_or_extend_selection();
                self.move_to_document_end();
                self.update_selection_active();
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
                    // Close search mode when editing begins
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
                        self.backspace()?;
                    }
                }
            }
            (KeyCode::Delete, KeyModifiers::NONE) => {
                if !self.config.read_only {
                    // Close search mode when editing begins
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
                        self.delete()?;
                    }
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
                    // Close search mode when editing begins
                    self.close_search();

                    if let Some(new_cursor) = self.buffer.undo()? {
                        self.cursor = new_cursor;
                        self.clamp_cursor();
                        // Invalidate entire highlighting cache after undo
                        self.highlight_cache
                            .invalidate_range(0, self.buffer.line_count());
                        // Schedule git diff update
                        self.schedule_git_diff_update();
                    }
                }
            }

            // Ctrl+Y - redo (only if not read-only)
            (KeyCode::Char('y'), KeyModifiers::CONTROL) => {
                if !self.config.read_only {
                    // Close search mode when editing begins
                    self.close_search();

                    if let Some(new_cursor) = self.buffer.redo()? {
                        self.cursor = new_cursor;
                        self.clamp_cursor();
                        // Invalidate entire highlighting cache after redo
                        self.highlight_cache
                            .invalidate_range(0, self.buffer.line_count());
                        // Schedule git diff update
                        self.schedule_git_diff_update();
                    }
                }
            }

            // Ctrl+F - search (show interactive search modal)
            (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                use crate::ui::modal::SearchModal;
                let mut search_modal = SearchModal::new("");

                // Restore previous search text and match info if search is already active
                if let Some(ref search_state) = self.search_state {
                    search_modal.set_input(search_state.query.clone());
                    if let Some((current, total)) = self.get_search_match_info() {
                        search_modal.set_match_info(current, total);
                    }
                }
                // If there's a saved query but no active search - restore and execute search
                else if let Some(ref query) = self.last_search_query {
                    search_modal.set_input(query.clone());

                    // Execute search immediately
                    self.start_search(query.clone(), false);

                    // Update match info in modal
                    if let Some((current, total)) = self.get_search_match_info() {
                        search_modal.set_match_info(current, total);
                    }
                }

                self.modal_request = Some((
                    PendingAction::Search,
                    ActiveModal::Search(Box::new(search_modal)),
                ));
            }

            // F3 - next match (or open search if no active search)
            (KeyCode::F(3), KeyModifiers::NONE) => {
                if self.search_state.is_some() {
                    self.search_next();
                } else {
                    // Open search modal if no active search, restoring last query
                    use crate::ui::modal::SearchModal;

                    if let Some(ref query) = self.last_search_query {
                        // Restore last query and immediately trigger search
                        let mut search_modal = SearchModal::new("");
                        search_modal.set_input(query.clone());

                        // Execute search immediately
                        self.start_search(query.clone(), false);

                        // Update match info in modal
                        if let Some((current, total)) = self.get_search_match_info() {
                            search_modal.set_match_info(current, total);
                        }

                        self.modal_request = Some((
                            PendingAction::Search,
                            ActiveModal::Search(Box::new(search_modal)),
                        ));
                    } else {
                        // No saved query - just open empty modal
                        let search_modal = SearchModal::new("");
                        self.modal_request = Some((
                            PendingAction::Search,
                            ActiveModal::Search(Box::new(search_modal)),
                        ));
                    }
                }
            }

            // Shift+F3 - previous match (or open search if no active search)
            (KeyCode::F(3), KeyModifiers::SHIFT) => {
                if self.search_state.is_some() {
                    self.search_prev();
                } else {
                    // Open search modal if no active search, restoring last query
                    use crate::ui::modal::SearchModal;

                    if let Some(ref query) = self.last_search_query {
                        // Restore last query and immediately trigger search
                        let mut search_modal = SearchModal::new("");
                        search_modal.set_input(query.clone());

                        // Execute search immediately
                        self.start_search(query.clone(), false);

                        // Update match info in modal
                        if let Some((current, total)) = self.get_search_match_info() {
                            search_modal.set_match_info(current, total);
                        }

                        self.modal_request = Some((
                            PendingAction::Search,
                            ActiveModal::Search(Box::new(search_modal)),
                        ));
                    } else {
                        // No saved query - just open empty modal
                        let search_modal = SearchModal::new("");
                        self.modal_request = Some((
                            PendingAction::Search,
                            ActiveModal::Search(Box::new(search_modal)),
                        ));
                    }
                }
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
        let line_number_width = 6u16;
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
            self.visual_row_to_buffer_position(rel_y, content_width as usize)
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
