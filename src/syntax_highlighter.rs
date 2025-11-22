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
        let highlight_names = vec![
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
        ]
        .into_iter()
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
        let extension = path.extension()?.to_str()?;

        match extension.to_lowercase().as_str() {
            // Main programming languages
            "rs" => Some("rust"),
            "py" | "pyw" | "pyi" => Some("python"),
            "go" => Some("go"),
            "js" | "mjs" | "cjs" | "jsx" => Some("javascript"),
            "ts" | "mts" | "cts" => Some("typescript"),
            "tsx" => Some("tsx"),
            "c" | "h" => Some("c"),
            "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" | "c++" | "h++" => Some("cpp"),
            "java" => Some("java"),
            "rb" | "ruby" | "rake" | "gemspec" => Some("ruby"),
            "php" => Some("php"),
            "hs" | "lhs" => Some("haskell"),
            "nix" => Some("nix"),

            // Web technologies
            "html" | "htm" | "xhtml" => Some("html"),
            "css" | "scss" | "less" => Some("css"),
            "json" | "jsonc" | "json5" => Some("json"),

            // Configuration formats
            "toml" => Some("toml"),
            "yaml" | "yml" => Some("yaml"),
            "sh" | "bash" | "zsh" | "fish" => Some("bash"),
            "md" | "markdown" | "mkd" | "mdx" => Some("markdown"),

            _ => None,
        }
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

        // Map highlight names to colors (dark theme)
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

// Rename for backward compatibility with existing code
#[allow(dead_code)]
pub type SyntaxHighlighter = TreeSitterHighlighter;
