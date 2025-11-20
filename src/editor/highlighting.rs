use ratatui::style::Style;
use std::collections::HashMap;
use tree_sitter_highlight::{HighlightEvent, Highlighter};

use crate::syntax_highlighter::TreeSitterHighlighter;

/// Maximum highlight cache size (lines)
const MAX_CACHE_SIZE: usize = 1000;

/// Highlighted lines cache for incremental highlighting
pub struct HighlightCache {
    /// Highlighted lines: line number -> (vector of segments, last access time)
    lines: HashMap<usize, (Vec<(String, Style)>, u64)>,
    /// Current language
    language: Option<String>,
    /// Global SyntaxHighlighter (static)
    syntax_highlighter: &'static TreeSitterHighlighter,
    /// Light or dark theme
    is_light_theme: bool,
    /// Access counter for LRU
    access_counter: u64,
    /// Full text cache for highlighting
    full_text_cache: Option<String>,
}

impl HighlightCache {
    /// Create a new cache
    pub fn new(syntax_highlighter: &'static TreeSitterHighlighter, is_light_theme: bool) -> Self {
        Self {
            lines: HashMap::new(),
            language: None,
            syntax_highlighter,
            is_light_theme,
            access_counter: 0,
            full_text_cache: None,
        }
    }

    /// Set syntax (by language name)
    pub fn set_syntax(&mut self, language_name: &str) {
        if self.language.as_deref() == Some(language_name) {
            return; // Already set
        }

        if self.syntax_highlighter.get_config(language_name).is_some() {
            self.language = Some(language_name.to_string());
            self.invalidate_all();
        }
    }

    /// Set syntax by file extension
    pub fn set_syntax_from_path(&mut self, path: &std::path::Path) {
        if let Some(language) = self.syntax_highlighter.language_for_file(path) {
            self.set_syntax(language);
        }
    }

    /// Update text cache (called when buffer changes)
    pub fn update_text(&mut self, text: String) {
        self.full_text_cache = Some(text);
        // Invalidate all lines when text is updated
        self.invalidate_all();
    }

    /// Get line highlighting (with caching)
    pub fn get_line_segments(&mut self, line_idx: usize, line_text: &str) -> &[(String, Style)] {
        // Increment access counter
        self.access_counter += 1;

        // Check cache and update access time
        if let Some((_, access_time)) = self.lines.get_mut(&line_idx) {
            *access_time = self.access_counter;
        } else {
            // Create new entry
            let segments = self.compute_line_segments(line_idx, line_text);

            // Check cache size and remove old entries if needed
            if self.lines.len() >= MAX_CACHE_SIZE {
                self.evict_lru();
            }

            // Save to cache with access time
            self.lines.insert(line_idx, (segments, self.access_counter));
        }

        // Return reference to cached data
        &self.lines.get(&line_idx).unwrap().0
    }

    /// Compute highlighting for line
    fn compute_line_segments(&mut self, _line_idx: usize, line_text: &str) -> Vec<(String, Style)> {
        let Some(ref language) = self.language else {
            return vec![(line_text.to_string(), Style::default())];
        };

        let Some(config) = self.syntax_highlighter.get_config(language) else {
            return vec![(line_text.to_string(), Style::default())];
        };

        // Highlight one line
        let mut highlighter = Highlighter::new();
        let source = line_text.as_bytes();

        let highlights = match highlighter.highlight(config, source, None, |_| None) {
            Ok(h) => h,
            Err(_) => return vec![(line_text.to_string(), Style::default())],
        };

        let mut segments = Vec::new();
        let mut current_style = Style::default();
        let mut current_text = String::new();
        let mut byte_offset = 0;

        for event in highlights {
            match event {
                Ok(HighlightEvent::Source { start, end }) => {
                    if let Ok(text) = std::str::from_utf8(&source[start..end]) {
                        current_text.push_str(text);
                    }
                    byte_offset = end;
                }
                Ok(HighlightEvent::HighlightStart(highlight)) => {
                    // Save current segment if exists
                    if !current_text.is_empty() {
                        segments.push((current_text.clone(), current_style));
                        current_text.clear();
                    }
                    current_style = self.syntax_highlighter.style_for_highlight(highlight.0, self.is_light_theme);
                }
                Ok(HighlightEvent::HighlightEnd) => {
                    // Save current segment
                    if !current_text.is_empty() {
                        segments.push((current_text.clone(), current_style));
                        current_text.clear();
                    }
                    current_style = Style::default();
                }
                Err(_) => {
                    // Parse error - return text without highlighting
                    return vec![(line_text.to_string(), Style::default())];
                }
            }
        }

        // Add remaining text
        if !current_text.is_empty() {
            segments.push((current_text, current_style));
        }

        // If no segments, return all text without style
        if segments.is_empty() {
            vec![(line_text.to_string(), Style::default())]
        } else {
            segments
        }
    }

    /// Remove oldest entries from cache (LRU)
    fn evict_lru(&mut self) {
        // Remove 20% of oldest entries
        let evict_count = MAX_CACHE_SIZE / 5;

        // Collect all entries with access time
        let mut entries: Vec<(usize, u64)> = self
            .lines
            .iter()
            .map(|(line_idx, (_, access_time))| (*line_idx, *access_time))
            .collect();

        // Sort by access time (oldest first)
        entries.sort_by_key(|(_, access_time)| *access_time);

        // Remove oldest entries
        for (line_idx, _) in entries.iter().take(evict_count) {
            self.lines.remove(line_idx);
        }
    }

    /// Invalidate line (when editing)
    pub fn invalidate_line(&mut self, line_idx: usize) {
        self.lines.remove(&line_idx);
    }

    /// Invalidate line range
    pub fn invalidate_range(&mut self, start_line: usize, end_line: usize) {
        for idx in start_line..=end_line {
            self.lines.remove(&idx);
        }
    }

    /// Invalidate entire cache
    pub fn invalidate_all(&mut self) {
        self.lines.clear();
    }

    /// Change theme (light/dark)
    pub fn set_light_theme(&mut self, is_light: bool) {
        if self.is_light_theme != is_light {
            self.is_light_theme = is_light;
            self.invalidate_all();
        }
    }

    /// Check if syntax is set
    pub fn has_syntax(&self) -> bool {
        self.language.is_some()
    }

    /// Get current syntax
    pub fn current_syntax(&self) -> Option<&str> {
        self.language.as_deref()
    }
}
