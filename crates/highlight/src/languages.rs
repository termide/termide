//! Language tables: highlight categories, per-language query data, and file
//! extension detection for the tree-sitter highlighter.

use std::path::Path;

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
pub(crate) const KOTLIN_HIGHLIGHTS: &str = r#"
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
pub(crate) fn injection_language_alias(name: &str) -> &str {
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
