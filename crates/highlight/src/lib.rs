//! Syntax highlighting for termide using tree-sitter.
//!
//! Provides syntax highlighting capabilities for multiple programming languages.

use ratatui::style::{Color, Modifier, Style};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
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

/// Hand-written highlights query for Kotlin.
///
/// `tree-sitter-kotlin-ng` ships no bundled highlights query, so this covers the
/// common node kinds (validated against the grammar's actual AST). Keep captures
/// to names present in [`HIGHLIGHT_NAMES`]. Literal keyword tokens must be
/// anonymous tokens the grammar actually exposes (`true`/`false`/`null` are
/// literal nodes here, not keyword tokens, so they are excluded).
const KOTLIN_HIGHLIGHTS: &str = r#"
(line_comment) @comment
(block_comment) @comment
(string_literal) @string
(character_literal) @string
(number_literal) @number
(float_literal) @number
(function_declaration (identifier) @function)
(call_expression (identifier) @function)
(class_declaration (identifier) @type)
(object_declaration (identifier) @type)
(user_type (identifier) @type)
(parameter (identifier) @variable.parameter)
(class_parameter (identifier) @variable.parameter)
["class" "interface" "object" "fun" "val" "var" "return" "if" "else" "when" "for" "while" "import" "package" "is" "as" "override" "private" "public" "protected" "internal" "open" "abstract" "sealed" "data" "enum" "const" "companion" "typealias" "this" "super" "throw" "do" "constructor"] @keyword
"#;

/// Map an injection language name (as it appears in a grammar's injections
/// query or a markdown code fence) to the key under which its config is loaded.
/// Unknown names pass through unchanged and resolve to no config.
fn injection_language_alias(name: &str) -> &str {
    match name {
        "js" => "javascript",
        "ts" => "typescript",
        "rs" => "rust",
        "py" => "python",
        "rb" => "ruby",
        "sh" | "shell" | "zsh" => "bash",
        "yml" => "yaml",
        "c++" => "cpp",
        "md" => "markdown",
        other => other,
    }
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
        "kt" | "kts" => Some("kotlin"),
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
    "kotlin",
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

        // JSX is parsed by the JavaScript grammar; its highlights are the base
        // JavaScript query plus the JSX-specific additions.
        let jsx_query = format!(
            "{}\n{}",
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
        );
        Self::load_language_config(
            &mut configs,
            "jsx",
            tree_sitter_javascript::LANGUAGE.into(),
            &jsx_query,
            tree_sitter_javascript::INJECTIONS_QUERY,
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
            "kotlin",
            tree_sitter_kotlin_ng::LANGUAGE.into(),
            KOTLIN_HIGHLIGHTS,
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

        // LANGUAGE_PHP (the full template grammar, not PHP_ONLY): real .php files
        // interleave HTML with `<?php … ?>` blocks. This grammar tracks the
        // HTML↔PHP mode; the markup between PHP tags is parsed into `text` nodes.
        // The bundled injections query only covers phpdoc/heredoc, so we append
        // a rule routing those `text` nodes to the HTML grammar — that is what
        // colours the HTML in a template. It only pays off under whole-document
        // highlighting (see `HighlightCache::set_document`), which keeps the
        // cross-line mode state the per-line path cannot.
        let php_injections = format!(
            "{}\n((text) @injection.content (#set! injection.language \"html\"))",
            tree_sitter_php::INJECTIONS_QUERY
        );
        Self::load_language_config(
            &mut configs,
            "php",
            tree_sitter_php::LANGUAGE_PHP.into(),
            tree_sitter_php::HIGHLIGHTS_QUERY,
            &php_injections,
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
        match HighlightConfiguration::new(language, name, highlights_query, injections_query, "") {
            Ok(mut config) => {
                config.configure(highlight_names);
                configs.insert(name, config);
            }
            Err(e) => {
                // A failed grammar must not silently disable highlighting for the
                // language; surface it so version/ABI regressions are diagnosable.
                log::error!("Failed to load syntax highlighting for {name}: {e:?}");
            }
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

    /// Style for a highlight category name (e.g. "keyword", "comment"). Used by
    /// the lightweight keyword highlighter so its colors match the tree-sitter
    /// palette / theme.
    pub fn style_for_name(&self, name: &str, is_light_theme: bool) -> Style {
        let (fg, modifiers) = if is_light_theme {
            self.color_for_highlight_light(name)
        } else {
            self.color_for_highlight_dark(name)
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
// Keyword highlighter — a lightweight, config-driven line tokenizer for
// languages that don't (yet) have a tree-sitter grammar.
// ============================================================================

/// A user-defined language for the keyword highlighter. Built by the host from
/// configuration; this crate stays config-format-agnostic.
#[derive(Debug, Clone)]
pub struct KeywordSyntax {
    /// Display name (e.g. shown in the status bar).
    pub name: String,
    /// Line-comment lead-in (e.g. `##`, `//`, `#`). Everything from here to the
    /// end of the line is a comment.
    pub line_comment: Option<String>,
    /// Block-comment delimiters `(open, close)`. Only matched within a single
    /// line (multi-line blocks need cross-line context the line tokenizer lacks).
    pub block_comment: Option<(String, String)>,
    /// Words coloured as keywords.
    pub keywords: HashSet<String>,
    /// Words coloured as types.
    pub types: HashSet<String>,
}

impl KeywordSyntax {
    /// Build from primitive fields (keyword/type lists become sets).
    pub fn new(
        name: String,
        line_comment: Option<String>,
        block_comment: Option<(String, String)>,
        keywords: impl IntoIterator<Item = String>,
        types: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            name,
            line_comment: line_comment.filter(|s| !s.is_empty()),
            block_comment: block_comment.filter(|(o, c)| !o.is_empty() && !c.is_empty()),
            keywords: keywords.into_iter().collect(),
            types: types.into_iter().collect(),
        }
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}
fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Tokenize one line with `syntax`, mapping each token category to a style via
/// `style`. Segments concatenate back to exactly `line`. Line-based: comments,
/// strings and numbers do not span lines (block comments only match in-line).
fn keyword_line_segments(
    line: &str,
    syntax: &KeywordSyntax,
    style: &impl Fn(&str) -> Style,
) -> Vec<(Cow<'static, str>, Style)> {
    let default = style("");
    let mut out: Vec<(Cow<'static, str>, Style)> = Vec::new();
    let mut push = |text: &str, st: Style| {
        if !text.is_empty() {
            out.push((Cow::Owned(text.to_string()), st));
        }
    };

    let bytes = line.as_bytes();
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];

        // Line comment → rest of the line.
        if let Some(lc) = &syntax.line_comment {
            if rest.starts_with(lc.as_str()) {
                push(rest, style("comment"));
                break;
            }
        }
        // Single-line block comment.
        if let Some((open, close)) = &syntax.block_comment {
            if rest.starts_with(open.as_str()) {
                let end = rest[open.len()..]
                    .find(close.as_str())
                    .map(|p| i + open.len() + p + close.len())
                    .unwrap_or(line.len());
                push(&line[i..end], style("comment"));
                i = end;
                continue;
            }
        }

        let c = rest.chars().next().unwrap();

        // String literal (double or single quote), with `\` escapes.
        if c == '"' || c == '\'' {
            let quote = c;
            let mut j = i + c.len_utf8();
            while j < line.len() {
                let ch = line[j..].chars().next().unwrap();
                j += ch.len_utf8();
                if ch == '\\' && j < line.len() {
                    j += line[j..].chars().next().unwrap().len_utf8();
                } else if ch == quote {
                    break;
                }
            }
            push(&line[i..j], style("string"));
            i = j;
            continue;
        }

        // Number literal.
        if c.is_ascii_digit() {
            let mut j = i;
            while j < line.len() {
                let ch = line[j..].chars().next().unwrap();
                if ch.is_alphanumeric() || ch == '.' || ch == '_' {
                    j += ch.len_utf8();
                } else {
                    break;
                }
            }
            push(&line[i..j], style("number"));
            i = j;
            continue;
        }

        // Identifier / keyword / type.
        if is_ident_start(c) {
            let mut j = i;
            while j < line.len() {
                let ch = line[j..].chars().next().unwrap();
                if is_ident_continue(ch) {
                    j += ch.len_utf8();
                } else {
                    break;
                }
            }
            let word = &line[i..j];
            let st = if syntax.keywords.contains(word) {
                style("keyword")
            } else if syntax.types.contains(word) {
                style("type")
            } else {
                default
            };
            push(word, st);
            i = j;
            continue;
        }

        // Operator/punctuation run (ASCII punctuation that isn't a delimiter we
        // already handle); everything else (whitespace) is default.
        if c.is_ascii_punctuation() {
            let mut j = i;
            while j < line.len() {
                let ch = line[j..].chars().next().unwrap();
                // Stop so the next iteration can re-detect comments/strings.
                if ch.is_ascii_punctuation() && ch != '"' && ch != '\'' && ch != '_' {
                    j += ch.len_utf8();
                } else {
                    break;
                }
            }
            // Avoid swallowing a comment lead-in that begins mid-run.
            push(&line[i..j], style("operator"));
            i = j;
            continue;
        }

        // Whitespace / other: emit a default run up to the next interesting char.
        let mut j = i;
        while j < line.len() {
            let ch = line[j..].chars().next().unwrap();
            if ch.is_ascii_punctuation() || is_ident_start(ch) || ch.is_ascii_digit() {
                break;
            }
            j += ch.len_utf8();
        }
        // Guard against zero-progress on an unexpected byte.
        if j == i {
            j += bytes[i..].iter().next().map(|_| c.len_utf8()).unwrap_or(1);
        }
        push(&line[i..j], default);
        i = j;
    }
    out
}

// ============================================================================
// HighlightCache - Line-based syntax highlighting with caching
// ============================================================================

use tree_sitter_highlight::{HighlightEvent, Highlighter};

/// Maximum highlight cache size (lines)
const MAX_CACHE_SIZE: usize = 1000;

/// Upper bound (in bytes) for whole-document syntax highlighting.
///
/// Whole-document highlighting re-parses the entire buffer on every edit, which
/// is the only way to resolve cross-line context (PHP's HTML/PHP mode switches,
/// multi-line strings and comments). Past this size the cost per keystroke is no
/// longer worth it, so callers fall back to the per-line path.
pub const WHOLE_DOCUMENT_BYTE_LIMIT: usize = 1024 * 1024;

/// Trait for line-based syntax highlighting.
/// Allows custom highlighters (e.g., for log files) to integrate with Editor.
pub trait LineHighlighter: Send + Sync {
    /// Get highlighted segments for a line (with caching).
    ///
    /// Segment text is returned as `Cow<str>` so callers that don't need
    /// highlighting (fallback path) can avoid per-frame `String` allocations
    /// by passing a borrowed slice of `line_text` directly.
    fn get_line_segments<'a>(
        &'a mut self,
        line_idx: usize,
        line_text: &'a str,
    ) -> &'a [(Cow<'a, str>, Style)];

    /// Invalidate cache from given line to end (called when text changes).
    fn invalidate_from(&mut self, line: usize);

    /// Invalidate entire cache.
    fn invalidate_all(&mut self);

    /// Check if syntax highlighting is active.
    fn has_syntax(&self) -> bool;

    /// Whether a whole-document highlight pass is pending and applicable.
    ///
    /// When `true`, the caller should hand the full buffer text to
    /// [`LineHighlighter::set_document`] before requesting line segments so
    /// cross-line context resolves correctly. Default: never needed.
    fn needs_document(&self) -> bool {
        false
    }

    /// Provide the full buffer text for a context-aware whole-document highlight.
    ///
    /// Implementations that highlight per line ignore this. Callers must gate
    /// invocation on buffer size (see [`WHOLE_DOCUMENT_BYTE_LIMIT`]) to keep the
    /// per-edit cost bounded. Default: no-op.
    fn set_document(&mut self, _text: &str) {}
}

/// Highlighted lines cache for incremental highlighting.
pub struct HighlightCache {
    /// Highlighted lines: line number -> (vector of segments, last access time)
    #[allow(clippy::type_complexity)]
    lines: HashMap<usize, (Vec<(Cow<'static, str>, Style)>, u64)>,
    /// Current language
    language: Option<String>,
    /// Global SyntaxHighlighter (static)
    syntax_highlighter: &'static TreeSitterHighlighter,
    /// Light or dark theme
    is_light_theme: bool,
    /// Access counter for LRU
    access_counter: u64,
    /// Default foreground color for unstyled text (from theme.fg)
    default_fg: Color,
    /// Per-line segments from the last whole-document highlight pass, indexed by
    /// line number. Populated by [`HighlightCache::set_document`]; empty when the
    /// document path is unused (no syntax, oversized buffer, or stale).
    #[allow(clippy::type_complexity)]
    doc_segments: Vec<Vec<(Cow<'static, str>, Style)>>,
    /// Whether `doc_segments` reflects the current buffer/syntax/theme.
    doc_valid: bool,
    /// Active config-driven keyword highlighter (set for extensions without a
    /// tree-sitter grammar). Mutually exclusive with `language`.
    custom: Option<KeywordSyntax>,
}

impl HighlightCache {
    /// Create a new cache.
    pub fn new(
        syntax_highlighter: &'static TreeSitterHighlighter,
        is_light_theme: bool,
        default_fg: Color,
    ) -> Self {
        Self {
            lines: HashMap::new(),
            language: None,
            syntax_highlighter,
            is_light_theme,
            access_counter: 0,
            default_fg,
            doc_segments: Vec::new(),
            doc_valid: false,
            custom: None,
        }
    }

    /// Set (or clear) the config-driven keyword highlighter. Used for file
    /// extensions that have no tree-sitter grammar. Clears the tree-sitter
    /// language so the two never fight.
    pub fn set_custom_syntax(&mut self, syntax: Option<KeywordSyntax>) {
        let changed = match (&self.custom, &syntax) {
            (Some(a), Some(b)) => a.name != b.name,
            (None, None) => false,
            _ => true,
        };
        if !changed {
            return;
        }
        if syntax.is_some() {
            self.language = None;
        }
        self.custom = syntax;
        self.invalidate_all();
        self.invalidate_document();
    }

    /// Drop any cached whole-document highlight. Called whenever the buffer,
    /// syntax or theme changes so the next render rebuilds it.
    fn invalidate_document(&mut self) {
        self.doc_valid = false;
        self.doc_segments.clear();
    }

    /// Set syntax (by language name).
    pub fn set_syntax(&mut self, language_name: &str) {
        if self.language.as_deref() == Some(language_name) {
            return;
        }

        if self.syntax_highlighter.get_config(language_name).is_some() {
            self.language = Some(language_name.to_string());
            self.custom = None; // tree-sitter wins over the keyword highlighter
            self.invalidate_all();
            self.invalidate_document();
        }
    }

    /// Set syntax by file extension.
    pub fn set_syntax_from_path(&mut self, path: &Path) {
        if let Some(language) = self.syntax_highlighter.language_for_file(path) {
            self.set_syntax(language);
        }
    }

    /// Get line highlighting (with caching).
    pub fn get_line_segments<'a>(
        &'a mut self,
        line_idx: usize,
        line_text: &'a str,
    ) -> &'a [(Cow<'a, str>, Style)] {
        // Whole-document fast path: when a context-aware pass is current and its
        // cached line still reconstructs this exact text, serve it directly. The
        // text check guards against any line-index drift (CRLF, trailing
        // newline, inline-diff rows) by falling back to the per-line path.
        // The condition is evaluated to a bool first so the immutable borrow is
        // released before the conditional re-borrow on `return` (NLL).
        let doc_hit = self.doc_valid
            && self
                .doc_segments
                .get(line_idx)
                .is_some_and(|segments| Self::segments_match(segments, line_text));
        if doc_hit {
            return &self.doc_segments[line_idx];
        }

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

    /// Whether a whole-document highlight pass should be (re)built before the
    /// next render. True only when a syntax is active and the cached pass is
    /// stale; the size guard lives at the call site (see
    /// [`WHOLE_DOCUMENT_BYTE_LIMIT`]).
    pub fn needs_document(&self) -> bool {
        self.language.is_some() && !self.doc_valid
    }

    /// Highlight the entire buffer in one context-aware pass and cache the
    /// result per line.
    ///
    /// The per-line path parses each line in isolation, which cannot resolve
    /// tokens whose meaning spans lines: PHP files switch between HTML and PHP
    /// at `<?php`/`?>`, and strings/comments routinely run across line breaks.
    /// A single whole-buffer parse keeps that context, so each line is coloured
    /// according to the real parse state at that point.
    ///
    /// No-op when no syntax is set or the grammar/parse fails (the per-line
    /// path then serves plain text). Callers must gate on buffer size.
    pub fn set_document(&mut self, text: &str) {
        self.doc_segments.clear();
        self.doc_valid = false;

        let Some(ref language) = self.language else {
            return;
        };
        let Some(config) = self.syntax_highlighter.get_config(language) else {
            return;
        };

        let default_style = Style::default().fg(self.default_fg);
        let mut highlighter = Highlighter::new();
        let source = text.as_bytes();

        // Resolve embedded languages (e.g. the HTML in a PHP template, or CSS/JS
        // inside HTML) to their loaded configs so injected regions are coloured
        // too. The highlighter is `'static`, so the borrowed configs outlive the
        // pass. Unknown injection languages simply stay unhighlighted.
        let highlighter_ref = self.syntax_highlighter;
        let events = match highlighter.highlight(config, source, None, |name| {
            highlighter_ref.get_config(injection_language_alias(name))
        }) {
            Ok(events) => events,
            Err(_) => return,
        };

        let mut doc: Vec<Vec<(Cow<'static, str>, Style)>> = Vec::new();
        let mut current_line: Vec<(Cow<'static, str>, Style)> = Vec::new();
        let mut current_style = default_style;

        for event in events {
            match event {
                Ok(HighlightEvent::Source { start, end }) => {
                    let Ok(chunk) = std::str::from_utf8(&source[start..end]) else {
                        continue;
                    };
                    // tree-sitter emits Source for every byte in order, so the
                    // chunks concatenate back to the whole document. Split on
                    // '\n' to distribute each chunk across the lines it spans;
                    // a style that straddles a newline (e.g. a block comment)
                    // is carried onto the continuation line.
                    let mut rest = chunk;
                    while let Some(nl) = rest.find('\n') {
                        let piece = &rest[..nl];
                        if !piece.is_empty() {
                            current_line.push((Cow::Owned(piece.to_string()), current_style));
                        }
                        doc.push(std::mem::take(&mut current_line));
                        rest = &rest[nl + 1..];
                    }
                    if !rest.is_empty() {
                        current_line.push((Cow::Owned(rest.to_string()), current_style));
                    }
                }
                Ok(HighlightEvent::HighlightStart(highlight)) => {
                    current_style = self
                        .syntax_highlighter
                        .style_for_highlight(highlight.0, self.is_light_theme);
                }
                Ok(HighlightEvent::HighlightEnd) => {
                    current_style = default_style;
                }
                Err(_) => {
                    self.doc_segments.clear();
                    return;
                }
            }
        }
        // The text after the final newline (or the whole buffer if it has none).
        doc.push(current_line);

        self.doc_segments = doc;
        self.doc_valid = true;
    }

    /// True when `segments` concatenate to exactly `line_text`. Used to confirm
    /// a cached whole-document line still matches what the renderer is drawing
    /// before serving it.
    fn segments_match(segments: &[(Cow<'_, str>, Style)], line_text: &str) -> bool {
        let mut rest = line_text;
        for (text, _) in segments {
            match rest.strip_prefix(text.as_ref()) {
                Some(remainder) => rest = remainder,
                None => return false,
            }
        }
        rest.is_empty()
    }

    /// Compute highlighting for line.
    fn compute_line_segments(&self, line_text: &str) -> Vec<(Cow<'static, str>, Style)> {
        let default_style = Style::default().fg(self.default_fg);

        // Config-driven keyword highlighter (extensions without a grammar).
        if let Some(ref syntax) = self.custom {
            let default_fg = self.default_fg;
            let is_light = self.is_light_theme;
            let hl = self.syntax_highlighter;
            let style = |name: &str| {
                if name.is_empty() {
                    Style::default().fg(default_fg)
                } else {
                    hl.style_for_name(name, is_light)
                }
            };
            let segs = keyword_line_segments(line_text, syntax, &style);
            return if segs.is_empty() {
                vec![(Cow::Owned(line_text.to_string()), default_style)]
            } else {
                segs
            };
        }

        let Some(ref language) = self.language else {
            return vec![(Cow::Owned(line_text.to_string()), default_style)];
        };

        let Some(config) = self.syntax_highlighter.get_config(language) else {
            return vec![(Cow::Owned(line_text.to_string()), default_style)];
        };

        let mut highlighter = Highlighter::new();
        let source = line_text.as_bytes();

        let highlights = match highlighter.highlight(config, source, None, |_| None) {
            Ok(h) => h,
            Err(_) => return vec![(Cow::Owned(line_text.to_string()), default_style)],
        };

        let mut segments = Vec::new();
        let mut current_style = default_style;
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
                        // Use take() instead of clone() + clear() to avoid allocation
                        segments
                            .push((Cow::Owned(std::mem::take(&mut current_text)), current_style));
                    }
                    current_style = self
                        .syntax_highlighter
                        .style_for_highlight(highlight.0, self.is_light_theme);
                }
                Ok(HighlightEvent::HighlightEnd) => {
                    if !current_text.is_empty() {
                        // Use take() instead of clone() + clear() to avoid allocation
                        segments
                            .push((Cow::Owned(std::mem::take(&mut current_text)), current_style));
                    }
                    current_style = default_style;
                }
                Err(_) => {
                    return vec![(Cow::Owned(line_text.to_string()), default_style)];
                }
            }
        }

        if !current_text.is_empty() {
            segments.push((Cow::Owned(current_text), current_style));
        }

        if segments.is_empty() {
            vec![(Cow::Owned(line_text.to_string()), default_style)]
        } else {
            segments
        }
    }

    /// Remove oldest entries from cache (LRU).
    ///
    /// Uses partial sort (select_nth_unstable) for O(n) performance instead of O(n log n).
    fn evict_lru(&mut self) {
        let evict_count = MAX_CACHE_SIZE / 5;

        let mut entries: Vec<(usize, u64)> = self
            .lines
            .iter()
            .map(|(line_idx, (_, access_time))| (*line_idx, *access_time))
            .collect();

        if entries.len() <= evict_count {
            return;
        }

        // Partial sort: elements before evict_count are the smallest (oldest)
        // This is O(n) on average vs O(n log n) for full sort
        entries.select_nth_unstable_by_key(evict_count, |(_, access_time)| *access_time);

        // Remove the oldest entries (those before the partition point)
        for (line_idx, _) in entries.iter().take(evict_count) {
            self.lines.remove(line_idx);
        }
    }

    /// Invalidate line (when editing).
    pub fn invalidate_line(&mut self, line_idx: usize) {
        self.lines.remove(&line_idx);
        self.invalidate_document();
    }

    /// Invalidate line range.
    pub fn invalidate_range(&mut self, start_line: usize, end_line: usize) {
        for idx in start_line..=end_line {
            self.lines.remove(&idx);
        }
        self.invalidate_document();
    }

    /// Invalidate entire cache.
    pub fn invalidate_all(&mut self) {
        self.lines.clear();
        self.invalidate_document();
    }

    /// Change theme (light/dark).
    pub fn set_light_theme(&mut self, is_light: bool) {
        if self.is_light_theme != is_light {
            self.is_light_theme = is_light;
            self.invalidate_all();
        }
    }

    /// Set default foreground color for unstyled text.
    /// This color is used instead of Style::default() to ensure text is visible
    /// on both light and dark theme backgrounds.
    pub fn set_default_fg(&mut self, fg: Color) {
        if self.default_fg != fg {
            self.default_fg = fg;
            self.invalidate_all();
        }
    }

    /// Check if syntax is set (tree-sitter grammar or keyword highlighter).
    pub fn has_syntax(&self) -> bool {
        self.language.is_some() || self.custom.is_some()
    }

    /// Get current syntax name.
    pub fn current_syntax(&self) -> Option<&str> {
        self.custom
            .as_ref()
            .map(|c| c.name.as_str())
            .or(self.language.as_deref())
    }
}

impl LineHighlighter for HighlightCache {
    fn get_line_segments<'a>(
        &'a mut self,
        line_idx: usize,
        line_text: &'a str,
    ) -> &'a [(Cow<'a, str>, Style)] {
        HighlightCache::get_line_segments(self, line_idx, line_text)
    }

    fn invalidate_from(&mut self, line: usize) {
        let lines_to_remove: Vec<usize> =
            self.lines.keys().filter(|&&l| l >= line).copied().collect();
        for line_idx in lines_to_remove {
            self.lines.remove(&line_idx);
        }
        self.invalidate_document();
    }

    fn invalidate_all(&mut self) {
        HighlightCache::invalidate_all(self);
    }

    fn has_syntax(&self) -> bool {
        HighlightCache::has_syntax(self)
    }

    fn needs_document(&self) -> bool {
        HighlightCache::needs_document(self)
    }

    fn set_document(&mut self, text: &str) {
        HighlightCache::set_document(self, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_highlighter_categorizes_and_is_lossless() {
        let syntax = KeywordSyntax::new(
            "test".to_string(),
            Some("##".to_string()),
            None,
            ["pub", "struct"].iter().map(|s| s.to_string()),
            ["u8"].iter().map(|s| s.to_string()),
        );
        let style = |name: &str| {
            Style::default().fg(match name {
                "keyword" => Color::Red,
                "type" => Color::Green,
                "comment" => Color::Blue,
                _ => Color::White,
            })
        };
        let line = "pub x := u8  ## note";
        let segs = keyword_line_segments(line, &syntax, &style);

        // Lossless: segments concatenate back to the original line.
        let joined: String = segs.iter().map(|(t, _)| t.as_ref()).collect();
        assert_eq!(joined, line);

        let cat = |word: &str| segs.iter().find(|(t, _)| t == word).map(|(_, s)| s.fg);
        assert_eq!(cat("pub"), Some(Some(Color::Red)), "keyword");
        assert_eq!(cat("u8"), Some(Some(Color::Green)), "type");
        assert!(
            segs.iter()
                .any(|(t, s)| t.as_ref().contains("note") && s.fg == Some(Color::Blue)),
            "## starts a comment"
        );
    }

    /// Highlight a single line and report how many distinct styled segments it
    /// produces. A line that is recognized by the grammar yields more than one
    /// segment; an unhighlighted (plain-text) line yields exactly one.
    fn segment_count(language: &str, line: &str) -> usize {
        let mut cache = HighlightCache::new(global_highlighter(), false, Color::White);
        cache.set_syntax(language);
        assert_eq!(
            cache.current_syntax(),
            Some(language),
            "language {language} should have a loaded config"
        );
        cache.get_line_segments(0, line).len()
    }

    #[test]
    fn every_supported_language_has_a_config() {
        let h = global_highlighter();
        for lang in SUPPORTED_LANGUAGES {
            assert!(
                h.get_config(lang).is_some(),
                "SUPPORTED_LANGUAGES lists {lang} but no grammar config is loaded"
            );
        }
    }

    #[test]
    fn detected_languages_are_all_loaded() {
        // Every language detect_language can return must have a loaded config,
        // otherwise the file silently falls back to plain text.
        let h = global_highlighter();
        let samples = [
            "a.rs", "a.py", "a.go", "a.js", "a.ts", "a.tsx", "a.jsx", "a.c", "a.cpp", "a.java",
            "a.kt", "a.rb", "a.php", "a.hs", "a.nix", "a.html", "a.css", "a.json", "a.toml",
            "a.yaml", "a.sh", "a.md",
        ];
        for sample in samples {
            let lang = detect_language(Path::new(sample))
                .unwrap_or_else(|| panic!("{sample} should detect a language"));
            assert!(
                h.get_config(lang).is_some(),
                "{sample} detected as {lang} but no config is loaded"
            );
        }
    }

    #[test]
    fn php_is_highlighted_in_document() {
        // Regression: PHP was disabled by an ABI-incompatible grammar. PHP uses
        // the template grammar (HTML↔PHP), so a statement only resolves with the
        // surrounding context the whole-document pass provides.
        let text = "<?php\n$count = 1; // comment\n";
        let mut cache = HighlightCache::new(global_highlighter(), false, Color::White);
        cache.set_syntax("php");
        cache.set_document(text);
        assert!(
            styled_on_line(&mut cache, 1, "$count = 1; // comment") > 0,
            "PHP statement should be highlighted in a document"
        );
    }

    #[test]
    fn kotlin_line_is_highlighted() {
        // Kotlin ships no bundled highlights query; this guards the hand-written
        // KOTLIN_HIGHLIGHTS against a grammar/ABI regression silently disabling it.
        assert!(
            segment_count("kotlin", "fun area(r: Double): Double = 0.0") > 1,
            "Kotlin line should produce multiple highlighted segments"
        );
    }

    #[test]
    fn jsx_line_is_highlighted() {
        // Regression: jsx was advertised but never loaded.
        assert!(
            segment_count("jsx", "const x = <Foo bar={1} />;") > 1,
            "JSX line should produce multiple highlighted segments"
        );
    }

    /// Count segments whose style differs from the default foreground — i.e.
    /// genuinely highlighted spans on a given line of the cached document.
    fn styled_on_line(cache: &mut HighlightCache, line_idx: usize, line_text: &str) -> usize {
        cache
            .get_line_segments(line_idx, line_text)
            .iter()
            .filter(|(_, style)| style.fg != Some(Color::White))
            .count()
    }

    #[test]
    fn php_document_highlights_both_html_and_php() {
        // Regression: a mixed HTML/PHP template (the common .php file) only
        // highlighted its PHP lines under the per-line path. The whole-document
        // pass must colour the surrounding HTML too.
        let lines = [
            "<!DOCTYPE html>",
            "<html lang=\"ru\">",
            "<body>",
            "    <h1>Title</h1>",
            "    <?php",
            "        $name = \"Ivan\";",
            "        echo \"<p>Hi, $name</p>\";",
            "    ?>",
            "</body>",
            "</html>",
        ];
        let text = lines.join("\n");

        let mut cache = HighlightCache::new(global_highlighter(), false, Color::White);
        cache.set_syntax("php");
        assert!(
            cache.needs_document(),
            "fresh syntax should request a document pass"
        );
        cache.set_document(&text);
        assert!(
            !cache.needs_document(),
            "document pass should satisfy the request"
        );

        // HTML tag line — highlighted only by the whole-document pass.
        assert!(
            styled_on_line(&mut cache, 1, lines[1]) > 0,
            "HTML line should be highlighted in a mixed PHP document"
        );
        // PHP statement inside the <?php block.
        assert!(
            styled_on_line(&mut cache, 5, lines[5]) > 0,
            "PHP line should be highlighted in a mixed PHP document"
        );
    }

    #[test]
    fn document_line_text_mismatch_falls_back() {
        // The whole-document cache must never render stale text: if the line the
        // renderer asks for no longer matches the cached segments, the per-line
        // path serves the correct text instead.
        let text = "<?php\n$a = 1;\n";
        let mut cache = HighlightCache::new(global_highlighter(), false, Color::White);
        cache.set_syntax("php");
        cache.set_document(text);

        // Ask for a line whose text differs from the cached document line.
        let segs = cache.get_line_segments(1, "$totally = different;");
        let rebuilt: String = segs.iter().map(|(t, _)| t.as_ref()).collect();
        assert_eq!(rebuilt, "$totally = different;");
    }

    #[test]
    fn editing_invalidates_document_cache() {
        let mut cache = HighlightCache::new(global_highlighter(), false, Color::White);
        cache.set_syntax("php");
        cache.set_document("<?php\n$a = 1;\n");
        assert!(!cache.needs_document());
        cache.invalidate_line(1);
        assert!(
            cache.needs_document(),
            "an edit must trigger a fresh document pass"
        );
    }
}
