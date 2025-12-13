//! Syntax highlighting for termide using tree-sitter.
//!
//! Provides syntax highlighting capabilities for multiple programming languages.

use ratatui::style::{Color, Modifier, Style};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use tree_sitter_highlight::HighlightConfiguration;

/// Global static highlighter (lazily initialized)
static GLOBAL_HIGHLIGHTER: OnceLock<TreeSitterHighlighter> = OnceLock::new();

/// Get global highlighter
pub fn global_highlighter() -> &'static TreeSitterHighlighter {
    GLOBAL_HIGHLIGHTER.get_or_init(TreeSitterHighlighter::new)
}

/// Standard highlight categories used by tree-sitter.
pub const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "function",
    "function.builtin",
    "function.method",
    "keyword",
    "label",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    "escape",
    "embedded",
];

/// Get style for highlight category (simplified version for basic use).
pub fn style_for_highlight(index: usize, base_fg: Color) -> Style {
    let name = HIGHLIGHT_NAMES.get(index).copied().unwrap_or("");

    let color = match name {
        "comment" => Color::DarkGray,
        "keyword" => Color::Magenta,
        "string" | "string.special" => Color::Green,
        "number" | "constant" | "constant.builtin" => Color::Yellow,
        "function" | "function.builtin" | "function.method" => Color::Blue,
        "type" | "type.builtin" => Color::Cyan,
        "variable.builtin" | "variable.parameter" => Color::Red,
        "operator" => Color::LightMagenta,
        "attribute" => Color::LightYellow,
        "tag" => Color::LightBlue,
        "escape" => Color::LightCyan,
        "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => base_fg,
        _ => base_fg,
    };

    let mut style = Style::default().fg(color);

    // Add modifiers for certain categories
    if name == "keyword" {
        style = style.add_modifier(Modifier::BOLD);
    }

    style
}

/// Detect language from file extension.
pub fn detect_language(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?;

    match ext.to_lowercase().as_str() {
        "rs" => Some("rust"),
        "py" | "pyw" => Some("python"),
        "go" => Some("go"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "jsx" => Some("jsx"),
        "c" | "h" => Some("c"),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => Some("cpp"),
        "java" => Some("java"),
        "rb" => Some("ruby"),
        "php" => Some("php"),
        "hs" => Some("haskell"),
        "nix" => Some("nix"),
        "html" | "htm" => Some("html"),
        "css" => Some("css"),
        "json" => Some("json"),
        "toml" => Some("toml"),
        "yaml" | "yml" => Some("yaml"),
        "sh" | "bash" | "zsh" => Some("bash"),
        "md" | "markdown" => Some("markdown"),
        _ => None,
    }
}

/// Supported languages list.
pub const SUPPORTED_LANGUAGES: &[&str] = &[
    "rust",
    "python",
    "go",
    "javascript",
    "typescript",
    "tsx",
    "jsx",
    "c",
    "cpp",
    "java",
    "ruby",
    "php",
    "haskell",
    "nix",
    "html",
    "css",
    "json",
    "toml",
    "yaml",
    "bash",
    "markdown",
];

/// Check if language is supported.
pub fn is_language_supported(lang: &str) -> bool {
    SUPPORTED_LANGUAGES.contains(&lang)
}

/// Syntax highlighter manager based on tree-sitter
pub struct TreeSitterHighlighter {
    /// Configurations for each supported language
    configs: HashMap<&'static str, HighlightConfiguration>,
    /// Highlight category names for mapping to colors
    highlight_names: Vec<String>,
}

impl TreeSitterHighlighter {
    /// Create new highlighter with support for all languages
    pub fn new() -> Self {
        let mut configs = HashMap::new();

        // Define highlight category names (standard for tree-sitter)
        let highlight_names = HIGHLIGHT_NAMES
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();

        // Load configurations for all supported languages
        // Main programming languages
        Self::load_language_config(
            &mut configs,
            "rust",
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            "",
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "python",
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY,
            "",
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "go",
            tree_sitter_go::LANGUAGE.into(),
            tree_sitter_go::HIGHLIGHTS_QUERY,
            "",
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "javascript",
            tree_sitter_javascript::LANGUAGE.into(),
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::INJECTIONS_QUERY,
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "typescript",
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
            "",
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "tsx",
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
            "",
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "c",
            tree_sitter_c::LANGUAGE.into(),
            tree_sitter_c::HIGHLIGHT_QUERY,
            "",
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "cpp",
            tree_sitter_cpp::LANGUAGE.into(),
            tree_sitter_cpp::HIGHLIGHT_QUERY,
            "",
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "java",
            tree_sitter_java::LANGUAGE.into(),
            tree_sitter_java::HIGHLIGHTS_QUERY,
            "",
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "ruby",
            tree_sitter_ruby::LANGUAGE.into(),
            tree_sitter_ruby::HIGHLIGHTS_QUERY,
            "",
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "php",
            tree_sitter_php::LANGUAGE_PHP.into(),
            tree_sitter_php::HIGHLIGHTS_QUERY,
            tree_sitter_php::INJECTIONS_QUERY,
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "haskell",
            tree_sitter_haskell::LANGUAGE.into(),
            tree_sitter_haskell::HIGHLIGHTS_QUERY,
            tree_sitter_haskell::INJECTIONS_QUERY,
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "nix",
            tree_sitter_nix::LANGUAGE.into(),
            tree_sitter_nix::HIGHLIGHTS_QUERY,
            tree_sitter_nix::INJECTIONS_QUERY,
            &highlight_names,
        );

        // Web technologies
        Self::load_language_config(
            &mut configs,
            "html",
            tree_sitter_html::LANGUAGE.into(),
            tree_sitter_html::HIGHLIGHTS_QUERY,
            tree_sitter_html::INJECTIONS_QUERY,
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "css",
            tree_sitter_css::LANGUAGE.into(),
            tree_sitter_css::HIGHLIGHTS_QUERY,
            "",
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "json",
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY,
            "",
            &highlight_names,
        );

        // Configuration formats
        Self::load_language_config(
            &mut configs,
            "toml",
            tree_sitter_toml_ng::LANGUAGE.into(),
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
            "",
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "yaml",
            tree_sitter_yaml::LANGUAGE.into(),
            tree_sitter_yaml::HIGHLIGHTS_QUERY,
            "",
            &highlight_names,
        );

        Self::load_language_config(
            &mut configs,
            "bash",
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY,
            "",
            &highlight_names,
        );

        // Markdown (has separate block and inline grammars)
        Self::load_language_config(
            &mut configs,
            "markdown",
            tree_sitter_md::LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
            tree_sitter_md::INJECTION_QUERY_BLOCK,
            &highlight_names,
        );

        Self {
            configs,
            highlight_names,
        }
    }

    /// Helper function to load language configuration
    fn load_language_config(
        configs: &mut HashMap<&'static str, HighlightConfiguration>,
        name: &'static str,
        language: tree_sitter::Language,
        highlights_query: &str,
        injections_query: &str,
        highlight_names: &[String],
    ) {
        if let Ok(mut config) =
            HighlightConfiguration::new(language, name, highlights_query, injections_query, "")
        {
            config.configure(highlight_names);
            configs.insert(name, config);
        }
    }

    /// Determine language by file extension
    pub fn language_for_file(&self, path: &Path) -> Option<&'static str> {
        detect_language(path)
    }

    /// Get configuration for language by name
    pub fn get_config(&self, language: &str) -> Option<&HighlightConfiguration> {
        self.configs.get(language)
    }

    /// Convert highlight index to ratatui Style
    pub fn style_for_highlight(&self, highlight_id: usize, is_light_theme: bool) -> Style {
        let highlight_name = self
            .highlight_names
            .get(highlight_id)
            .map(|s| s.as_str())
            .unwrap_or("");

        // Map highlight names to colors
        let (fg, modifiers) = if is_light_theme {
            self.color_for_highlight_light(highlight_name)
        } else {
            self.color_for_highlight_dark(highlight_name)
        };

        let mut style = Style::default().fg(fg);
        for modifier in modifiers {
            style = style.add_modifier(modifier);
        }
        style
    }

    /// Color scheme for dark theme (One Dark inspired)
    fn color_for_highlight_dark(&self, name: &str) -> (Color, Vec<Modifier>) {
        match name {
            "comment" => (Color::Rgb(105, 112, 125), vec![Modifier::ITALIC]),
            "keyword" => (Color::Rgb(199, 146, 234), vec![Modifier::BOLD]),
            "function" | "function.builtin" | "function.method" => {
                (Color::Rgb(130, 170, 255), vec![])
            }
            "string" | "string.special" => (Color::Rgb(152, 195, 121), vec![]),
            "number" => (Color::Rgb(209, 154, 102), vec![]),
            "constant" | "constant.builtin" => (Color::Rgb(229, 192, 123), vec![]),
            "type" | "type.builtin" => (Color::Rgb(86, 182, 194), vec![]),
            "variable" | "variable.parameter" => (Color::Rgb(224, 108, 117), vec![]),
            "variable.builtin" => (Color::Rgb(224, 108, 117), vec![Modifier::ITALIC]),
            "property" => (Color::Rgb(152, 195, 121), vec![]),
            "operator" => (Color::Rgb(198, 120, 221), vec![]),
            "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => {
                (Color::Rgb(171, 178, 191), vec![])
            }
            "punctuation.special" => (Color::Rgb(198, 120, 221), vec![]),
            "constructor" => (Color::Rgb(229, 192, 123), vec![Modifier::BOLD]),
            "tag" => (Color::Rgb(224, 108, 117), vec![]),
            "attribute" => (Color::Rgb(209, 154, 102), vec![]),
            "label" => (Color::Rgb(229, 192, 123), vec![]),
            "escape" => (Color::Rgb(86, 182, 194), vec![]),
            "embedded" => (Color::Rgb(198, 120, 221), vec![]),
            _ => (Color::Rgb(171, 178, 191), vec![]),
        }
    }

    /// Color scheme for light theme (GitHub Light inspired)
    fn color_for_highlight_light(&self, name: &str) -> (Color, Vec<Modifier>) {
        match name {
            "comment" => (Color::Rgb(106, 115, 125), vec![Modifier::ITALIC]),
            "keyword" => (Color::Rgb(215, 58, 73), vec![Modifier::BOLD]),
            "function" | "function.builtin" | "function.method" => {
                (Color::Rgb(111, 66, 193), vec![])
            }
            "string" | "string.special" => (Color::Rgb(3, 102, 214), vec![]),
            "number" => (Color::Rgb(0, 92, 197), vec![]),
            "constant" | "constant.builtin" => (Color::Rgb(0, 92, 197), vec![]),
            "type" | "type.builtin" => (Color::Rgb(215, 58, 73), vec![]),
            "variable" | "variable.parameter" => (Color::Rgb(0, 92, 197), vec![]),
            "variable.builtin" => (Color::Rgb(0, 92, 197), vec![Modifier::ITALIC]),
            "property" => (Color::Rgb(0, 92, 197), vec![]),
            "operator" => (Color::Rgb(215, 58, 73), vec![]),
            "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => {
                (Color::Rgb(36, 41, 46), vec![])
            }
            "punctuation.special" => (Color::Rgb(215, 58, 73), vec![]),
            "constructor" => (Color::Rgb(111, 66, 193), vec![Modifier::BOLD]),
            "tag" => (Color::Rgb(34, 134, 58), vec![]),
            "attribute" => (Color::Rgb(111, 66, 193), vec![]),
            "label" => (Color::Rgb(111, 66, 193), vec![]),
            "escape" => (Color::Rgb(0, 92, 197), vec![]),
            "embedded" => (Color::Rgb(215, 58, 73), vec![]),
            _ => (Color::Rgb(36, 41, 46), vec![]),
        }
    }
}

impl Default for TreeSitterHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

/// Alias for backward compatibility
pub type SyntaxHighlighter = TreeSitterHighlighter;

// ============================================================================
// HighlightCache - Line-based syntax highlighting with caching
// ============================================================================

use tree_sitter_highlight::{HighlightEvent, Highlighter};

/// Maximum highlight cache size (lines)
const MAX_CACHE_SIZE: usize = 1000;

/// Trait for line-based syntax highlighting.
/// Allows custom highlighters (e.g., for log files) to integrate with Editor.
pub trait LineHighlighter: Send + Sync {
    /// Get highlighted segments for a line (with caching).
    fn get_line_segments(&mut self, line_idx: usize, line_text: &str) -> &[(String, Style)];

    /// Invalidate cache from given line to end (called when text changes).
    fn invalidate_from(&mut self, line: usize);

    /// Invalidate entire cache.
    fn invalidate_all(&mut self);

    /// Check if syntax highlighting is active.
    fn has_syntax(&self) -> bool;
}

/// Highlighted lines cache for incremental highlighting.
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
}

impl HighlightCache {
    /// Create a new cache.
    pub fn new(syntax_highlighter: &'static TreeSitterHighlighter, is_light_theme: bool) -> Self {
        Self {
            lines: HashMap::new(),
            language: None,
            syntax_highlighter,
            is_light_theme,
            access_counter: 0,
        }
    }

    /// Set syntax (by language name).
    pub fn set_syntax(&mut self, language_name: &str) {
        if self.language.as_deref() == Some(language_name) {
            return;
        }

        if self.syntax_highlighter.get_config(language_name).is_some() {
            self.language = Some(language_name.to_string());
            self.invalidate_all();
        }
    }

    /// Set syntax by file extension.
    pub fn set_syntax_from_path(&mut self, path: &Path) {
        if let Some(language) = self.syntax_highlighter.language_for_file(path) {
            self.set_syntax(language);
        }
    }

    /// Get line highlighting (with caching).
    pub fn get_line_segments(&mut self, line_idx: usize, line_text: &str) -> &[(String, Style)] {
        self.access_counter += 1;

        if let Some((_, access_time)) = self.lines.get_mut(&line_idx) {
            *access_time = self.access_counter;
        } else {
            let segments = self.compute_line_segments(line_text);

            if self.lines.len() >= MAX_CACHE_SIZE {
                self.evict_lru();
            }

            self.lines.insert(line_idx, (segments, self.access_counter));
        }

        &self
            .lines
            .get(&line_idx)
            .expect("line was just inserted or updated above")
            .0
    }

    /// Compute highlighting for line.
    fn compute_line_segments(&self, line_text: &str) -> Vec<(String, Style)> {
        let Some(ref language) = self.language else {
            return vec![(line_text.to_string(), Style::default())];
        };

        let Some(config) = self.syntax_highlighter.get_config(language) else {
            return vec![(line_text.to_string(), Style::default())];
        };

        let mut highlighter = Highlighter::new();
        let source = line_text.as_bytes();

        let highlights = match highlighter.highlight(config, source, None, |_| None) {
            Ok(h) => h,
            Err(_) => return vec![(line_text.to_string(), Style::default())],
        };

        let mut segments = Vec::new();
        let mut current_style = Style::default();
        let mut current_text = String::new();

        for event in highlights {
            match event {
                Ok(HighlightEvent::Source { start, end }) => {
                    if let Ok(text) = std::str::from_utf8(&source[start..end]) {
                        current_text.push_str(text);
                    }
                }
                Ok(HighlightEvent::HighlightStart(highlight)) => {
                    if !current_text.is_empty() {
                        segments.push((current_text.clone(), current_style));
                        current_text.clear();
                    }
                    current_style = self
                        .syntax_highlighter
                        .style_for_highlight(highlight.0, self.is_light_theme);
                }
                Ok(HighlightEvent::HighlightEnd) => {
                    if !current_text.is_empty() {
                        segments.push((current_text.clone(), current_style));
                        current_text.clear();
                    }
                    current_style = Style::default();
                }
                Err(_) => {
                    return vec![(line_text.to_string(), Style::default())];
                }
            }
        }

        if !current_text.is_empty() {
            segments.push((current_text, current_style));
        }

        if segments.is_empty() {
            vec![(line_text.to_string(), Style::default())]
        } else {
            segments
        }
    }

    /// Remove oldest entries from cache (LRU).
    fn evict_lru(&mut self) {
        let evict_count = MAX_CACHE_SIZE / 5;

        let mut entries: Vec<(usize, u64)> = self
            .lines
            .iter()
            .map(|(line_idx, (_, access_time))| (*line_idx, *access_time))
            .collect();

        entries.sort_by_key(|(_, access_time)| *access_time);

        for (line_idx, _) in entries.iter().take(evict_count) {
            self.lines.remove(line_idx);
        }
    }

    /// Invalidate line (when editing).
    pub fn invalidate_line(&mut self, line_idx: usize) {
        self.lines.remove(&line_idx);
    }

    /// Invalidate line range.
    pub fn invalidate_range(&mut self, start_line: usize, end_line: usize) {
        for idx in start_line..=end_line {
            self.lines.remove(&idx);
        }
    }

    /// Invalidate entire cache.
    pub fn invalidate_all(&mut self) {
        self.lines.clear();
    }

    /// Change theme (light/dark).
    pub fn set_light_theme(&mut self, is_light: bool) {
        if self.is_light_theme != is_light {
            self.is_light_theme = is_light;
            self.invalidate_all();
        }
    }

    /// Check if syntax is set.
    pub fn has_syntax(&self) -> bool {
        self.language.is_some()
    }

    /// Get current syntax.
    pub fn current_syntax(&self) -> Option<&str> {
        self.language.as_deref()
    }
}

impl LineHighlighter for HighlightCache {
    fn get_line_segments(&mut self, line_idx: usize, line_text: &str) -> &[(String, Style)] {
        HighlightCache::get_line_segments(self, line_idx, line_text)
    }

    fn invalidate_from(&mut self, line: usize) {
        let lines_to_remove: Vec<usize> =
            self.lines.keys().filter(|&&l| l >= line).copied().collect();
        for line_idx in lines_to_remove {
            self.lines.remove(&line_idx);
        }
    }

    fn invalidate_all(&mut self) {
        HighlightCache::invalidate_all(self);
    }

    fn has_syntax(&self) -> bool {
        HighlightCache::has_syntax(self)
    }
}
