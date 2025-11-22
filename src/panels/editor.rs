use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use std::path::PathBuf;

use super::Panel;
use crate::editor::{Cursor, HighlightCache, SearchState, Selection, TextBuffer, Viewport};
use crate::state::AppState;
use crate::state::{ActiveModal, PendingAction};
use crate::syntax_highlighter;

/// Editor mode configuration
#[derive(Debug, Clone)]
pub struct EditorConfig {
    /// Whether syntax highlighting is enabled
    pub syntax_highlighting: bool,
    /// Read-only mode
    pub read_only: bool,
    /// Automatic line wrapping by window width
    pub word_wrap: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            syntax_highlighting: true,
            read_only: false,
            word_wrap: false,
        }
    }
}

impl EditorConfig {
    /// Create configuration for view mode (without editing)
    pub fn view_only() -> Self {
        Self {
            syntax_highlighting: true,
            read_only: true,
            word_wrap: false,
        }
    }

    /// Create configuration without syntax highlighting
    #[allow(dead_code)]
    pub fn no_highlighting() -> Self {
        Self {
            syntax_highlighting: false,
            read_only: false,
            word_wrap: false,
        }
    }

    /// Create configuration with line wrapping enabled
    #[allow(dead_code)]
    pub fn with_word_wrap() -> Self {
        Self {
            syntax_highlighting: true,
            read_only: false,
            word_wrap: true,
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
    /// Cached title
    cached_title: String,
    /// Modal window request
    modal_request: Option<(PendingAction, ActiveModal)>,
    /// Updated config after save (for applying in AppState)
    config_update: Option<crate::config::Config>,
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
            cached_title: "Untitled".to_string(),
            modal_request: None,
            config_update: None,
        }
    }

    /// Check if editor is read-only
    #[allow(dead_code)]
    pub fn is_read_only(&self) -> bool {
        self.config.read_only
    }

    /// Check if syntax highlighting is enabled
    #[allow(dead_code)]
    pub fn has_syntax_highlighting(&self) -> bool {
        self.config.syntax_highlighting
    }

    /// Get file path
    pub fn file_path(&self) -> Option<&std::path::Path> {
        self.buffer.file_path()
    }

    /// Open file with default configuration
    pub fn open_file(path: PathBuf) -> Result<Self> {
        Self::open_file_with_config(path, EditorConfig::default())
    }

    /// Open file with specified configuration
    pub fn open_file_with_config(path: PathBuf, mut config: EditorConfig) -> Result<Self> {
        let buffer = TextBuffer::from_file(&path)?;

        let cached_title = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Untitled".to_string());

        // Check file access rights for auto-detection of read-only
        if let Ok(metadata) = std::fs::metadata(&path) {
            if metadata.permissions().readonly() {
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

        Ok(Self {
            config,
            buffer,
            cursor: Cursor::new(),
            selection: None,
            viewport: Viewport::default(),
            highlight_cache,
            search_state: None,
            cached_title,
            modal_request: None,
            config_update: None,
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
            cached_title: title,
            modal_request: None,
            config_update: None,
        }
    }

    /// Save file
    pub fn save(&mut self) -> Result<()> {
        use crate::config::Config;

        // Check if this is a config file
        if let Some(path) = self.buffer.file_path() {
            if Config::is_config_file(path) {
                // Validate config before saving
                let content = self.buffer.to_string();
                match Config::validate_content(&content) {
                    Ok(new_config) => {
                        // Save and set config update flag
                        self.buffer.save()?;
                        self.config_update = Some(new_config);
                    }
                    Err(e) => {
                        return Err(anyhow::anyhow!("Invalid config: {}", e));
                    }
                }
                return Ok(());
            }
        }

        self.buffer.save()?;
        Ok(())
    }

    /// Get updated config (if config file was saved)
    pub fn take_config_update(&mut self) -> Option<crate::config::Config> {
        self.config_update.take()
    }

    /// Save file as (Save As)
    pub fn save_file_as(&mut self, path: PathBuf) -> Result<()> {
        self.buffer.save_to(&path)?;

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
            tab_size: 4,                    // TODO: get from settings
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

    /// Move cursor left
    fn move_cursor_left(&mut self) {
        self.cursor.move_left(1);

        // Clamp cursor to line length
        if self.cursor.line < self.buffer.line_count() {
            let line_len = self.buffer.line_len_graphemes(self.cursor.line);
            self.cursor.clamp_column(line_len);
        }
    }

    /// Move cursor right
    fn move_cursor_right(&mut self) {
        let line_len = self.buffer.line_len_graphemes(self.cursor.line);
        let max_line = self.buffer.line_count().saturating_sub(1);
        self.cursor.move_right(1, line_len, max_line);
        self.clamp_cursor();
    }

    /// Move cursor to start of line
    fn move_to_line_start(&mut self) {
        self.cursor.column = 0;
    }

    /// Move cursor to end of line
    fn move_to_line_end(&mut self) {
        let line_len = self.buffer.line_len_graphemes(self.cursor.line);
        self.cursor.column = line_len;
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
        self.viewport
            .scroll_down(page_size, self.buffer.line_count());
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
        self.viewport.scroll_to_bottom(self.buffer.line_count());
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
                        let chars: Vec<char> = line.chars().collect();
                        let selected: String = chars[start.column..end.column.min(chars.len())]
                            .iter()
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

                        let chars: Vec<char> = line.chars().collect();
                        if i == start.line {
                            result.push_str(&chars[start.column..].iter().collect::<String>());
                            result.push('\n');
                        } else if i == end.line {
                            result.push_str(
                                &chars[..end.column.min(chars.len())]
                                    .iter()
                                    .collect::<String>(),
                            );
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
            }
        }
        Ok(())
    }

    /// Copy selected text to clipboard
    fn copy_to_clipboard(&mut self) -> Result<()> {
        if let Some(text) = self.get_selected_text() {
            crate::clipboard::copy(text);
        }
        Ok(())
    }

    /// Cut selected text to clipboard
    fn cut_to_clipboard(&mut self) -> Result<()> {
        if let Some(text) = self.get_selected_text() {
            crate::clipboard::copy(text);
            self.delete_selection()?;
        }
        Ok(())
    }

    /// Paste from clipboard
    fn paste_from_clipboard(&mut self) -> Result<()> {
        // Delete selected text before pasting
        self.delete_selection()?;

        let (text, _mode) = crate::clipboard::paste();
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
        }
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
        // Delete selected text before insertion
        self.delete_selection()?;

        let text = ch.to_string();
        let new_cursor = self.buffer.insert(&self.cursor, &text)?;
        self.cursor = new_cursor;
        self.clamp_cursor();

        // Invalidate highlighting cache for changed line
        self.highlight_cache.invalidate_line(self.cursor.line);

        Ok(())
    }

    /// Insert newline
    fn insert_newline(&mut self) -> Result<()> {
        let old_line = self.cursor.line;

        // Delete selected text before insertion
        self.delete_selection()?;

        let new_cursor = self.buffer.insert(&self.cursor, "\n")?;
        self.cursor = new_cursor;
        self.clamp_cursor();

        // Invalidate all lines after inserting new line
        self.highlight_cache
            .invalidate_range(old_line, self.buffer.line_count());

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
        }
        Ok(())
    }

    /// Render editor content
    fn render_content(&mut self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        // Update viewport size (subtract space for line numbers)
        let line_number_width = 5; // "  123 "
        let content_width = area.width.saturating_sub(line_number_width) as usize;
        let content_height = area.height as usize;

        self.viewport.resize(content_width, content_height);

        // Ensure cursor is visible
        self.viewport
            .ensure_cursor_visible(&self.cursor, self.buffer.line_count());

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

        // Variable to track cursor position in word wrap mode
        let mut cursor_viewport_pos: Option<(usize, usize)> = None;

        if self.config.word_wrap && content_width > 0 {
            // Word wrap mode
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

                    while char_offset <= line_len && visual_row < content_height {
                        let chunk_end = (char_offset + content_width).min(line_len);

                        // Номер строки (только на первой визуальной строке)
                        if is_first_visual_row {
                            let line_num = format!("{:>4} ", line_idx + 1);
                            for (i, ch) in line_num.chars().enumerate() {
                                if i < line_number_width as usize {
                                    let x = area.x + i as u16;
                                    let y = area.y + visual_row as u16;
                                    if let Some(cell) = buf.cell_mut((x, y)) {
                                        cell.set_char(ch);
                                        cell.set_style(line_number_style);
                                    }
                                }
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
                                if segment_char_idx >= char_offset && segment_char_idx < chunk_end {
                                    let x = area.x + line_number_width + visual_col as u16;
                                    let y = area.y + visual_row as u16;

                                    if x < area.x + area.width && y < area.y + area.height {
                                        if let Some(cell) = buf.cell_mut((x, y)) {
                                            cell.set_char(ch);

                                            // Проверить, является ли это совпадением поиска
                                            let match_idx = search_matches.iter().position(
                                                |(m_line, m_col, m_len)| {
                                                    *m_line == line_idx
                                                        && segment_char_idx >= *m_col
                                                        && segment_char_idx < m_col + m_len
                                                },
                                            );

                                            // Проверить, находится ли символ в выделении
                                            let is_selected = if let Some((sel_start, sel_end)) =
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
                                    if is_cursor_line && self.cursor.column == segment_char_idx {
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

                        // Если строка пустая, выйти после одной итерации
                        if line_len == 0 {
                            break;
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
                        cell.set_style(
                            Style::default()
                                .bg(theme.selected_bg)
                                .fg(theme.selected_fg)
                                .add_modifier(Modifier::BOLD),
                        );
                    }
                }
            }
        } else {
            // Обычный режим (без word wrap)
            // Отрисовать видимые строки
            for row in 0..content_height {
                let line_idx = self.viewport.top_line + row;

                if line_idx >= self.buffer.line_count() {
                    break;
                }

                let is_cursor_line = line_idx == self.cursor.line;
                let style = if is_cursor_line {
                    cursor_line_style
                } else {
                    text_style
                };

                // Номер строки
                let line_num = format!("{:>4} ", line_idx + 1);
                for (i, ch) in line_num.chars().enumerate() {
                    if i < line_number_width as usize {
                        let x = area.x + i as u16;
                        let y = area.y + row as u16;
                        if let Some(cell) = buf.cell_mut((x, y)) {
                            cell.set_char(ch);
                            cell.set_style(line_number_style);
                        }
                    }
                }

                // Содержимое строки с подсветкой синтаксиса
                if let Some(line_text) = self.buffer.line(line_idx) {
                    // Убрать перевод строки в конце
                    let line_text = line_text.trim_end_matches('\n');

                    // Получить подсветку синтаксиса для строки (без клонирования)
                    // Учитываем config.syntax_highlighting для отключения подсветки
                    let segments =
                        if self.config.syntax_highlighting && self.highlight_cache.has_syntax() {
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

                                        // Проверить, является ли это совпадением поиска
                                        let match_idx = search_matches.iter().position(
                                            |(m_line, m_col, m_len)| {
                                                *m_line == line_idx
                                                    && col_offset >= *m_col
                                                    && col_offset < m_col + m_len
                                            },
                                        );

                                        // Проверить, находится ли символ в выделении
                                        let is_selected =
                                            if let Some((sel_start, sel_end)) = &selection_range {
                                                let pos =
                                                    crate::editor::Cursor::at(line_idx, col_offset);
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

            // Отрисовать курсор
            if let Some((viewport_row, viewport_col)) =
                self.viewport.cursor_to_viewport_pos(&self.cursor)
            {
                let cursor_x = area.x + line_number_width + viewport_col as u16;
                let cursor_y = area.y + viewport_row as u16;

                if cursor_x < area.x + area.width && cursor_y < area.y + area.height {
                    if let Some(cell) = buf.cell_mut((cursor_x, cursor_y)) {
                        cell.set_style(
                            Style::default()
                                .bg(theme.selected_bg)
                                .fg(theme.selected_fg)
                                .add_modifier(Modifier::BOLD),
                        );
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

        // Move cursor to first match
        if let Some(match_cursor) = search.current_match_cursor() {
            self.cursor = *match_cursor;
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
                self.cursor = *match_cursor;
            }
        }
    }

    /// Go to previous match
    pub fn search_prev(&mut self) {
        if let Some(ref mut search) = self.search_state {
            search.prev_match();
            if let Some(match_cursor) = search.current_match_cursor() {
                self.cursor = *match_cursor;
            }
        }
    }

    /// Close search
    pub fn close_search(&mut self) {
        self.search_state = None;
    }

    /// Start search with replace
    pub fn start_replace(&mut self, query: String, replace_with: String, case_sensitive: bool) {
        let mut search = SearchState::new_with_replace(query, replace_with, case_sensitive);

        // Perform search throughout document
        self.perform_search(&mut search);

        // Find closest match to current cursor
        search.find_closest_match(&self.cursor);

        // Move cursor to first match
        if let Some(match_cursor) = search.current_match_cursor() {
            self.cursor = *match_cursor;
        }

        self.search_state = Some(search);
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

                // Update current match index
                if search.matches.is_empty() {
                    search.current_match = None;
                } else if idx >= search.matches.len() {
                    search.current_match = Some(search.matches.len() - 1);
                }

                // Move cursor to next match
                if let Some(match_cursor) = search.current_match_cursor() {
                    self.cursor = *match_cursor;
                }
            }
        }

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
            }
        }

        Ok(count)
    }
}

impl Panel for Editor {
    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        is_focused: bool,
        _panel_index: usize,
        state: &AppState,
    ) {
        // Отрисовать рамку
        let border_color = if is_focused {
            state.theme.accented_fg
        } else {
            state.theme.disabled
        };

        // Рисуем простую рамку
        let border_style = Style::default().fg(border_color);

        // Верхняя граница с заголовком
        if let Some(cell) = buf.cell_mut((area.x, area.y)) {
            cell.set_char('┌');
            cell.set_style(border_style);
        }

        let title = format!(" {} ", self.title());
        let title_width = title.len() as u16;
        for (i, ch) in title.chars().enumerate() {
            if let Some(cell) = buf.cell_mut((area.x + 1 + i as u16, area.y)) {
                cell.set_char(ch);
                cell.set_style(border_style.add_modifier(Modifier::BOLD));
            }
        }

        for x in (area.x + 1 + title_width)..(area.x + area.width - 1) {
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char('─');
                cell.set_style(border_style);
            }
        }

        if let Some(cell) = buf.cell_mut((area.x + area.width - 1, area.y)) {
            cell.set_char('┐');
            cell.set_style(border_style);
        }

        // Боковые границы
        for y in (area.y + 1)..(area.y + area.height - 1) {
            if let Some(cell) = buf.cell_mut((area.x, y)) {
                cell.set_char('│');
                cell.set_style(border_style);
            }
            if let Some(cell) = buf.cell_mut((area.x + area.width - 1, y)) {
                cell.set_char('│');
                cell.set_style(border_style);
            }
        }

        // Нижняя граница
        if let Some(cell) = buf.cell_mut((area.x, area.y + area.height - 1)) {
            cell.set_char('└');
            cell.set_style(border_style);
        }
        for x in (area.x + 1)..(area.x + area.width - 1) {
            if let Some(cell) = buf.cell_mut((x, area.y + area.height - 1)) {
                cell.set_char('─');
                cell.set_style(border_style);
            }
        }
        if let Some(cell) = buf.cell_mut((area.x + area.width - 1, area.y + area.height - 1)) {
            cell.set_char('┘');
            cell.set_style(border_style);
        }

        // Отрисовать содержимое внутри рамки
        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };

        self.render_content(inner, buf, state.theme);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Translate Cyrillic to Latin for hotkeys
        let key = crate::keyboard::translate_hotkey(key);

        match (key.code, key.modifiers) {
            // Navigation (clears selection)
            (KeyCode::Up, KeyModifiers::NONE) => {
                self.selection = None;
                self.move_cursor_up();
            }
            (KeyCode::Down, KeyModifiers::NONE) => {
                self.selection = None;
                self.move_cursor_down();
            }
            (KeyCode::Left, KeyModifiers::NONE) => {
                self.selection = None;
                self.move_cursor_left();
            }
            (KeyCode::Right, KeyModifiers::NONE) => {
                self.selection = None;
                self.move_cursor_right();
            }
            (KeyCode::Home, KeyModifiers::NONE) => {
                self.selection = None;
                self.move_to_line_start();
            }
            (KeyCode::End, KeyModifiers::NONE) => {
                self.selection = None;
                self.move_to_line_end();
            }
            (KeyCode::PageUp, KeyModifiers::NONE) => {
                self.selection = None;
                self.page_up();
            }
            (KeyCode::PageDown, KeyModifiers::NONE) => {
                self.selection = None;
                self.page_down();
            }
            (KeyCode::Home, KeyModifiers::CONTROL) => {
                self.selection = None;
                self.move_to_document_start();
            }
            (KeyCode::End, KeyModifiers::CONTROL) => {
                self.selection = None;
                self.move_to_document_end();
            }

            // Navigation with selection (Shift)
            (KeyCode::Up, KeyModifiers::SHIFT) => {
                self.start_or_extend_selection();
                self.move_cursor_up();
                self.update_selection_active();
            }
            (KeyCode::Down, KeyModifiers::SHIFT) => {
                self.start_or_extend_selection();
                self.move_cursor_down();
                self.update_selection_active();
            }
            (KeyCode::Left, KeyModifiers::SHIFT) => {
                self.start_or_extend_selection();
                self.move_cursor_left();
                self.update_selection_active();
            }
            (KeyCode::Right, KeyModifiers::SHIFT) => {
                self.start_or_extend_selection();
                self.move_cursor_right();
                self.update_selection_active();
            }
            (KeyCode::Home, KeyModifiers::SHIFT) => {
                self.start_or_extend_selection();
                self.move_to_line_start();
                self.update_selection_active();
            }
            (KeyCode::End, KeyModifiers::SHIFT) => {
                self.start_or_extend_selection();
                self.move_to_line_end();
                self.update_selection_active();
            }
            (KeyCode::PageUp, KeyModifiers::SHIFT) => {
                self.start_or_extend_selection();
                self.page_up();
                self.update_selection_active();
            }
            (KeyCode::PageDown, KeyModifiers::SHIFT) => {
                self.start_or_extend_selection();
                self.page_down();
                self.update_selection_active();
            }
            // Shift+Ctrl+Home/End - select to start/end of document
            (KeyCode::Home, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    && modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.start_or_extend_selection();
                self.move_to_document_start();
                self.update_selection_active();
            }
            (KeyCode::End, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    && modifiers.contains(KeyModifiers::CONTROL) =>
            {
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
                if !self.config.read_only && self.buffer.file_path().is_some() {
                    self.save()?;
                }
            }

            // Ctrl+Shift+Z - redo (only if not read-only)
            // With Shift the character becomes uppercase 'Z'
            (KeyCode::Char('Z'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                if !self.config.read_only {
                    if let Some(new_cursor) = self.buffer.redo()? {
                        self.cursor = new_cursor;
                        self.clamp_cursor();
                        // Invalidate entire highlighting cache after redo
                        self.highlight_cache
                            .invalidate_range(0, self.buffer.line_count());
                    }
                }
            }

            // Ctrl+Z - undo (only if not read-only)
            (KeyCode::Char('z'), KeyModifiers::CONTROL) => {
                if !self.config.read_only {
                    if let Some(new_cursor) = self.buffer.undo()? {
                        self.cursor = new_cursor;
                        self.clamp_cursor();
                        // Invalidate entire highlighting cache after undo
                        self.highlight_cache
                            .invalidate_range(0, self.buffer.line_count());
                    }
                }
            }

            // Ctrl+Y - redo (only if not read-only)
            (KeyCode::Char('y'), KeyModifiers::CONTROL) => {
                if !self.config.read_only {
                    if let Some(new_cursor) = self.buffer.redo()? {
                        self.cursor = new_cursor;
                        self.clamp_cursor();
                        // Invalidate entire highlighting cache after redo
                        self.highlight_cache
                            .invalidate_range(0, self.buffer.line_count());
                    }
                }
            }

            // Ctrl+F - search (show modal window)
            (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                use crate::ui::modal::InputModal;
                let t = crate::i18n::t();
                let input = InputModal::new(t.editor_search_title(), t.editor_search_prompt());
                self.modal_request =
                    Some((PendingAction::Search, ActiveModal::Input(Box::new(input))));
            }

            // F3 - next match
            (KeyCode::F(3), KeyModifiers::NONE) => {
                self.search_next();
            }

            // Shift+F3 - previous match
            (KeyCode::F(3), KeyModifiers::SHIFT) => {
                self.search_prev();
            }

            // Esc - close search
            (KeyCode::Esc, KeyModifiers::NONE) => {
                if self.search_state.is_some() {
                    self.close_search();
                }
            }

            // Ctrl+H - text replacement (only if not read-only)
            (KeyCode::Char('h'), KeyModifiers::CONTROL) => {
                if !self.config.read_only {
                    use crate::ui::modal::InputModal;
                    let t = crate::i18n::t();
                    let input =
                        InputModal::new(t.editor_replace_title(), t.editor_replace_prompt());
                    self.modal_request =
                        Some((PendingAction::Replace, ActiveModal::Input(Box::new(input))));
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
                        // TODO: show message to user about number of replacements
                        eprintln!("Replaced {} occurrences", count);
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

            // Ctrl+Insert - copy
            (KeyCode::Insert, KeyModifiers::CONTROL) => {
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

            // Shift+Insert - paste (only if not read-only)
            (KeyCode::Insert, KeyModifiers::SHIFT) => {
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
                self.viewport.scroll_down(3, self.buffer.line_count());
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
        let line_number_width = 5u16;
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

        let buffer_line = self.viewport.top_line + rel_y;
        let buffer_col = self.viewport.left_column + rel_x;

        // Clamp position to valid values
        let max_line = self.buffer.line_count().saturating_sub(1);
        let target_line = buffer_line.min(max_line);
        let line_len = self.buffer.line_len_graphemes(target_line);
        let target_col = buffer_col.min(line_len);

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
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
                // Ensure cursor is visible during dragging
                self.viewport
                    .ensure_cursor_visible(&self.cursor, self.buffer.line_count());
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
}
