//! Rendering cache state for the editor.

use std::collections::HashMap;

use termide_config::Config;
use termide_highlight::{global_highlighter, HighlightCache};
use termide_theme::Theme;

/// Cached rendering state for the editor.
pub(crate) struct RenderingCache {
    /// Syntax highlighting cache.
    pub highlight: HighlightCache,
    /// Cached count of virtual lines (buffer lines + deletion markers).
    pub virtual_line_count: usize,
    /// Cached content width from last render.
    pub content_width: usize,
    /// Cached smart wrap setting from last render.
    pub use_smart_wrap: bool,
    /// Cache of wrap points for each line.
    #[allow(dead_code)]
    pub wrap_points: HashMap<usize, Vec<usize>>,
    /// Cached theme for rendering.
    pub theme: Theme,
    /// Cached config for rendering.
    pub config: Config,
}

impl Default for RenderingCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderingCache {
    /// Create new RenderingCache with defaults.
    pub fn new() -> Self {
        Self {
            highlight: HighlightCache::new(global_highlighter(), false),
            virtual_line_count: 0,
            content_width: 0,
            use_smart_wrap: false,
            wrap_points: HashMap::new(),
            theme: Theme::default(),
            config: Config::default(),
        }
    }

    /// Create RenderingCache with large file optimization.
    pub fn new_large_file() -> Self {
        Self {
            highlight: HighlightCache::new(global_highlighter(), true),
            virtual_line_count: 0,
            content_width: 0,
            use_smart_wrap: false,
            wrap_points: HashMap::new(),
            theme: Theme::default(),
            config: Config::default(),
        }
    }

    /// Update cached theme and config before render.
    pub fn prepare(&mut self, theme: &Theme, config: &Config) {
        self.theme = *theme;
        self.config = config.clone();
    }

    /// Invalidate wrap cache (e.g., when content changes).
    pub fn invalidate_wrap_cache(&mut self) {
        self.wrap_points.clear();
    }
}
