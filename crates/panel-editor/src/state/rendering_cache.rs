//! Rendering cache state for the editor.

use std::collections::HashMap;
use std::sync::Arc;

use ratatui::style::Color;
use termide_config::Config;
use termide_highlight::{global_highlighter, HighlightCache};
use termide_theme::Theme;
use unicode_segmentation::UnicodeSegmentation;

/// Cached wrap data for a single line.
#[derive(Clone, Debug)]
pub struct CachedWrapData {
    /// Number of visual rows this line occupies.
    pub visual_rows: usize,
    /// Grapheme indices where each new visual line starts.
    pub wrap_points: Vec<usize>,
    /// Total number of grapheme clusters in the line.
    pub grapheme_count: usize,
    /// Content width used to compute this wrap data.
    pub computed_width: usize,
    /// Smart wrap setting used to compute this wrap data.
    pub computed_smart_wrap: bool,
}

/// Cached rendering state for the editor.
pub(crate) struct RenderingCache {
    /// Syntax highlighting cache.
    pub highlight: HighlightCache,
    /// Cached count of virtual lines (buffer lines + deletion markers).
    pub virtual_line_count: usize,
    /// Cached content width from last render.
    pub content_width: usize,
    /// Cached content height from last render.
    pub content_height: usize,
    /// Cached smart wrap setting from last render.
    pub use_smart_wrap: bool,
    /// Cache of wrap points for each line: line_index -> (visual_rows, wrap_points).
    wrap_cache: HashMap<usize, CachedWrapData>,
    /// Cumulative visual row counts: cumulative_rows[i] = total visual rows for lines 0..i.
    /// Used for O(1) lookup of visual row position.
    cumulative_visual_rows: Vec<usize>,
    /// Whether cumulative cache is valid.
    cumulative_valid: bool,
    /// Cached diagnostic rows per buffer line: line -> total diagnostic visual rows.
    diagnostic_rows_cache: HashMap<usize, usize>,
    /// Content width used to compute diagnostic rows cache.
    diagnostic_cache_width: usize,
    /// Whether diagnostic rows cache is valid.
    diagnostic_cache_valid: bool,
    /// Cached theme for rendering.
    pub theme: Theme,
    /// Cached config for rendering (Arc to avoid full clone every frame).
    pub config: Arc<Config>,
}

impl Default for RenderingCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderingCache {
    /// Create new RenderingCache with defaults.
    pub fn new() -> Self {
        let theme = Theme::default();
        Self {
            // Note: is_light_theme and default_fg will be set correctly by prepare_render()
            highlight: HighlightCache::new(global_highlighter(), false, Color::White),
            virtual_line_count: 0,
            content_width: 0,
            content_height: 0,
            use_smart_wrap: false,
            wrap_cache: HashMap::new(),
            cumulative_visual_rows: Vec::new(),
            cumulative_valid: false,
            diagnostic_rows_cache: HashMap::new(),
            diagnostic_cache_width: 0,
            diagnostic_cache_valid: false,
            theme,
            config: Arc::new(Config::default()),
        }
    }

    /// Get cached wrap data for a line, if available and valid for current settings.
    ///
    /// Returns `None` if the cached data was computed with different width or smart_wrap settings.
    pub fn get_wrap_data(
        &self,
        line: usize,
        content_width: usize,
        use_smart_wrap: bool,
    ) -> Option<&CachedWrapData> {
        self.wrap_cache.get(&line).filter(|cached| {
            cached.computed_width == content_width && cached.computed_smart_wrap == use_smart_wrap
        })
    }

    /// Store wrap data for a line in cache.
    pub fn set_wrap_data(
        &mut self,
        line: usize,
        visual_rows: usize,
        wrap_points: Vec<usize>,
        grapheme_count: usize,
        content_width: usize,
        use_smart_wrap: bool,
    ) {
        // Check if visual_rows changed - only then invalidate cumulative cache
        // Only compare if old entry has matching width settings (otherwise it's outdated)
        let old_visual_rows = self
            .wrap_cache
            .get(&line)
            .filter(|old| {
                old.computed_width == content_width && old.computed_smart_wrap == use_smart_wrap
            })
            .map(|old| old.visual_rows);

        self.wrap_cache.insert(
            line,
            CachedWrapData {
                visual_rows,
                wrap_points,
                grapheme_count,
                computed_width: content_width,
                computed_smart_wrap: use_smart_wrap,
            },
        );

        // Only invalidate cumulative cache if:
        // 1. Line existed in cache with matching settings AND visual_rows changed
        // 2. Don't invalidate for new lines beyond cumulative cache - they'll trigger rebuild anyway
        let should_invalidate = match old_visual_rows {
            Some(old_vr) => old_vr != visual_rows, // Existing line changed
            None => {
                // New line - only invalidate if it's within existing cumulative range
                // (lines beyond cumulative cache will trigger rebuild via get_total_visual_rows)
                line < self.cumulative_visual_rows.len()
            }
        };

        if should_invalidate {
            self.cumulative_valid = false;
        }
    }

    /// Invalidate wrap cache for a single line.
    pub fn invalidate_wrap_line(&mut self, line: usize) {
        self.wrap_cache.remove(&line);
        self.cumulative_valid = false;
    }

    /// Invalidate wrap cache for a range of lines (from start_line to end).
    pub fn invalidate_wrap_range(&mut self, start_line: usize) {
        self.wrap_cache.retain(|&line, _| line < start_line);
        self.cumulative_valid = false;
    }

    /// Invalidate entire wrap cache (e.g., when content width changes).
    pub fn invalidate_wrap_cache(&mut self) {
        self.wrap_cache.clear();
        self.cumulative_visual_rows.clear();
        self.cumulative_valid = false;
    }

    /// Check if wrap settings match current parameters (for cache invalidation on settings change).
    pub fn wrap_settings_match(&self, content_width: usize, use_smart_wrap: bool) -> bool {
        self.content_width == content_width && self.use_smart_wrap == use_smart_wrap
    }

    /// Update wrap settings and invalidate cache if they changed.
    pub fn update_wrap_settings(&mut self, content_width: usize, use_smart_wrap: bool) {
        if !self.wrap_settings_match(content_width, use_smart_wrap) {
            self.invalidate_wrap_cache();
            self.content_width = content_width;
            self.use_smart_wrap = use_smart_wrap;
        }
    }

    // =========================================================================
    // Cumulative Visual Rows Cache
    // =========================================================================

    /// Build cumulative visual rows cache for the entire buffer.
    ///
    /// After calling this, `cumulative_visual_rows[i]` contains the total
    /// number of visual rows for lines 0..=i.
    ///
    /// This enables O(1) lookup for:
    /// - Total visual rows in the buffer
    /// - Visual row offset for any given buffer line
    pub fn build_cumulative_cache(&mut self, buffer: &termide_buffer::TextBuffer) {
        let line_count = buffer.line_count();
        self.cumulative_visual_rows.clear();
        self.cumulative_visual_rows.reserve(line_count);

        let mut cumulative = 0usize;

        let content_width = self.content_width;
        let use_smart_wrap = self.use_smart_wrap;

        for line_idx in 0..line_count {
            // Get visual rows from wrap cache, or compute and cache if missing
            // Validate cached data matches current width settings
            let visual_rows = if let Some(cached) = self.wrap_cache.get(&line_idx).filter(|c| {
                c.computed_width == content_width && c.computed_smart_wrap == use_smart_wrap
            }) {
                cached.visual_rows
            } else if let Some(line_text) = buffer.line(line_idx) {
                let line_text = line_text.trim_end_matches('\n');
                let grapheme_count = line_text.graphemes(true).count();
                let (visual_rows, wrap_points) = crate::word_wrap::get_line_wrap_points(
                    line_text,
                    content_width,
                    use_smart_wrap,
                );
                self.wrap_cache.insert(
                    line_idx,
                    CachedWrapData {
                        visual_rows,
                        wrap_points,
                        grapheme_count,
                        computed_width: content_width,
                        computed_smart_wrap: use_smart_wrap,
                    },
                );
                visual_rows
            } else {
                1 // Empty/non-existent line = 1 visual row
            };

            cumulative += visual_rows;
            self.cumulative_visual_rows.push(cumulative);
        }

        self.cumulative_valid = true;
    }

    /// Get total visual rows in the buffer (O(1) if cumulative cache is valid).
    ///
    /// Returns `None` if the cumulative cache is not valid.
    pub fn get_total_visual_rows(&self) -> Option<usize> {
        if self.cumulative_valid {
            self.cumulative_visual_rows.last().copied()
        } else {
            None
        }
    }

    /// Get the cumulative visual row count up to and including a given line.
    ///
    /// Returns the total number of visual rows for lines 0..=line.
    /// Returns `None` if the cumulative cache is not valid or line is out of bounds.
    pub fn get_cumulative_visual_rows(&self, line: usize) -> Option<usize> {
        if self.cumulative_valid && line < self.cumulative_visual_rows.len() {
            Some(self.cumulative_visual_rows[line])
        } else {
            None
        }
    }

    /// Get the starting visual row for a given buffer line (O(1) lookup).
    ///
    /// Returns the visual row index where the given buffer line starts.
    /// This is cumulative_visual_rows[line-1] for line > 0, or 0 for line 0.
    pub fn get_visual_row_for_line(&self, line: usize) -> Option<usize> {
        if !self.cumulative_valid {
            return None;
        }

        if line == 0 {
            Some(0)
        } else if line <= self.cumulative_visual_rows.len() {
            Some(self.cumulative_visual_rows[line - 1])
        } else {
            None
        }
    }

    /// Check if cumulative cache is valid.
    pub fn is_cumulative_valid(&self) -> bool {
        self.cumulative_valid
    }

    /// Check if cumulative cache covers the given number of buffer lines.
    pub fn cumulative_covers_line_count(&self, line_count: usize) -> bool {
        self.cumulative_valid && self.cumulative_visual_rows.len() >= line_count
    }

    // =========================================================================
    // Diagnostic Rows Cache
    // =========================================================================

    /// Check if diagnostic rows cache is valid for given content width.
    pub fn is_diagnostic_cache_valid(&self, content_width: usize) -> bool {
        self.diagnostic_cache_valid && self.diagnostic_cache_width == content_width
    }

    /// Store computed diagnostic rows cache.
    pub fn set_diagnostic_rows_cache(
        &mut self,
        cache: HashMap<usize, usize>,
        content_width: usize,
    ) {
        self.diagnostic_rows_cache = cache;
        self.diagnostic_cache_width = content_width;
        self.diagnostic_cache_valid = true;
    }

    /// Get cached diagnostic rows for a specific buffer line.
    ///
    /// Returns 0 if cache is invalid or line has no diagnostics.
    pub fn diagnostic_rows_for_line(&self, line: usize) -> usize {
        if self.diagnostic_cache_valid {
            self.diagnostic_rows_cache.get(&line).copied().unwrap_or(0)
        } else {
            0
        }
    }

    /// Invalidate diagnostic rows cache (e.g., when diagnostics change).
    pub fn invalidate_diagnostic_cache(&mut self) {
        self.diagnostic_cache_valid = false;
    }
}
