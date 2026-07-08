//! Tree-sitter based symbol extraction.
//!
//! Uses tree-sitter Parser + Query to extract structural symbols from source code.

use std::collections::HashMap;
use std::sync::OnceLock;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor};

use crate::symbols::{SymbolInfo, SymbolKind};

/// Cached query data per language: (Language, Query, capture-name-to-SymbolKind mapping).
type QueryEntry = (Language, Query, Vec<SymbolKind>);

static QUERIES: OnceLock<HashMap<&'static str, QueryEntry>> = OnceLock::new();

/// Get or initialize the global query cache.
fn queries() -> &'static HashMap<&'static str, QueryEntry> {
    QUERIES.get_or_init(build_queries)
}

/// Build all language queries.
fn build_queries() -> HashMap<&'static str, QueryEntry> {
    let mut map = HashMap::new();

    // Rust
    register(
        &mut map,
        "rust",
        tree_sitter_rust::LANGUAGE.into(),
        &[
            (
                "(function_item name: (identifier) @function)",
                SymbolKind::Function,
            ),
            (
                "(struct_item name: (type_identifier) @struct)",
                SymbolKind::Struct,
            ),
            (
                "(enum_item name: (type_identifier) @enum)",
                SymbolKind::Enum,
            ),
            (
                "(trait_item name: (type_identifier) @trait)",
                SymbolKind::Trait,
            ),
            ("(impl_item type: (_) @impl)", SymbolKind::Impl),
            (
                "(type_item name: (type_identifier) @type_alias)",
                SymbolKind::TypeAlias,
            ),
            (
                "(const_item name: (identifier) @constant)",
                SymbolKind::Constant,
            ),
            (
                "(static_item name: (identifier) @constant)",
                SymbolKind::Constant,
            ),
            ("(mod_item name: (identifier) @module)", SymbolKind::Module),
            (
                "(macro_definition name: (identifier) @macro)",
                SymbolKind::Macro,
            ),
        ],
    );

    // Python
    register(
        &mut map,
        "python",
        tree_sitter_python::LANGUAGE.into(),
        &[
            (
                "(function_definition name: (identifier) @function)",
                SymbolKind::Function,
            ),
            (
                "(class_definition name: (identifier) @class)",
                SymbolKind::Class,
            ),
        ],
    );

    // Go
    register(
        &mut map,
        "go",
        tree_sitter_go::LANGUAGE.into(),
        &[
            (
                "(function_declaration name: (identifier) @function)",
                SymbolKind::Function,
            ),
            (
                "(method_declaration name: (field_identifier) @method)",
                SymbolKind::Method,
            ),
            (
                "(type_declaration (type_spec name: (type_identifier) @struct))",
                SymbolKind::Struct,
            ),
        ],
    );

    // JavaScript
    register(
        &mut map,
        "javascript",
        tree_sitter_javascript::LANGUAGE.into(),
        &[
            (
                "(function_declaration name: (identifier) @function)",
                SymbolKind::Function,
            ),
            (
                "(class_declaration name: (identifier) @class)",
                SymbolKind::Class,
            ),
            (
                "(method_definition name: (property_identifier) @method)",
                SymbolKind::Method,
            ),
        ],
    );

    // TypeScript
    register(
        &mut map,
        "typescript",
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        &[
            (
                "(function_declaration name: (identifier) @function)",
                SymbolKind::Function,
            ),
            (
                "(class_declaration name: (type_identifier) @class)",
                SymbolKind::Class,
            ),
            (
                "(method_definition name: (property_identifier) @method)",
                SymbolKind::Method,
            ),
        ],
    );

    // TSX
    register(
        &mut map,
        "tsx",
        tree_sitter_typescript::LANGUAGE_TSX.into(),
        &[
            (
                "(function_declaration name: (identifier) @function)",
                SymbolKind::Function,
            ),
            (
                "(class_declaration name: (type_identifier) @class)",
                SymbolKind::Class,
            ),
            (
                "(method_definition name: (property_identifier) @method)",
                SymbolKind::Method,
            ),
        ],
    );

    // C
    register(
        &mut map,
        "c",
        tree_sitter_c::LANGUAGE.into(),
        &[
            ("(function_definition declarator: (function_declarator declarator: (identifier) @function))", SymbolKind::Function),
            ("(struct_specifier name: (type_identifier) @struct)", SymbolKind::Struct),
            ("(enum_specifier name: (type_identifier) @enum)", SymbolKind::Enum),
        ],
    );

    // C++
    register(
        &mut map,
        "cpp",
        tree_sitter_cpp::LANGUAGE.into(),
        &[
            ("(function_definition declarator: (function_declarator declarator: (identifier) @function))", SymbolKind::Function),
            ("(struct_specifier name: (type_identifier) @struct)", SymbolKind::Struct),
            ("(class_specifier name: (type_identifier) @class)", SymbolKind::Class),
            ("(enum_specifier name: (type_identifier) @enum)", SymbolKind::Enum),
        ],
    );

    // Java
    register(
        &mut map,
        "java",
        tree_sitter_java::LANGUAGE.into(),
        &[
            (
                "(class_declaration name: (identifier) @class)",
                SymbolKind::Class,
            ),
            (
                "(method_declaration name: (identifier) @method)",
                SymbolKind::Method,
            ),
            (
                "(interface_declaration name: (identifier) @trait)",
                SymbolKind::Trait,
            ),
            (
                "(enum_declaration name: (identifier) @enum)",
                SymbolKind::Enum,
            ),
        ],
    );

    // Kotlin. `class_declaration` covers classes, interfaces and enum classes;
    // `object_declaration` covers `object`/`companion object`. All callables use
    // `function_declaration`, so methods and free functions share one pattern.
    register(
        &mut map,
        "kotlin",
        tree_sitter_kotlin_ng::LANGUAGE.into(),
        &[
            (
                "(class_declaration name: (identifier) @class)",
                SymbolKind::Class,
            ),
            (
                "(object_declaration name: (identifier) @class)",
                SymbolKind::Class,
            ),
            (
                "(function_declaration name: (identifier) @function)",
                SymbolKind::Function,
            ),
        ],
    );

    // Ruby
    register(
        &mut map,
        "ruby",
        tree_sitter_ruby::LANGUAGE.into(),
        &[
            ("(method name: (identifier) @method)", SymbolKind::Method),
            ("(class name: (constant) @class)", SymbolKind::Class),
            ("(module name: (constant) @module)", SymbolKind::Module),
        ],
    );

    // PHP
    register(
        &mut map,
        "php",
        tree_sitter_php::LANGUAGE_PHP.into(),
        &[
            (
                "(function_definition name: (name) @function)",
                SymbolKind::Function,
            ),
            ("(class_declaration name: (name) @class)", SymbolKind::Class),
            (
                "(method_declaration name: (name) @method)",
                SymbolKind::Method,
            ),
        ],
    );

    // Bash
    register(
        &mut map,
        "bash",
        tree_sitter_bash::LANGUAGE.into(),
        &[(
            "(function_definition name: (word) @function)",
            SymbolKind::Function,
        )],
    );

    // Markdown - use tree-sitter for headings
    register(
        &mut map,
        "markdown",
        tree_sitter_md::LANGUAGE.into(),
        &[
            (
                "(atx_heading (atx_h1_marker) heading_content: (_) @h1)",
                SymbolKind::Heading1,
            ),
            (
                "(atx_heading (atx_h2_marker) heading_content: (_) @h2)",
                SymbolKind::Heading2,
            ),
            (
                "(atx_heading (atx_h3_marker) heading_content: (_) @h3)",
                SymbolKind::Heading3,
            ),
            (
                "(atx_heading (atx_h4_marker) heading_content: (_) @h4)",
                SymbolKind::Heading4,
            ),
            (
                "(atx_heading (atx_h5_marker) heading_content: (_) @h5)",
                SymbolKind::Heading5,
            ),
            (
                "(atx_heading (atx_h6_marker) heading_content: (_) @h6)",
                SymbolKind::Heading6,
            ),
        ],
    );

    map
}

/// Register a language with multiple query patterns.
///
/// Combines all patterns into a single query string. Each pattern must have
/// exactly one capture, and `kinds` must be in the same order as the patterns.
fn register(
    map: &mut HashMap<&'static str, QueryEntry>,
    name: &'static str,
    language: Language,
    patterns: &[(&str, SymbolKind)],
) {
    // Build combined query string
    let combined: String = patterns
        .iter()
        .map(|(p, _)| *p)
        .collect::<Vec<_>>()
        .join("\n");
    let kinds: Vec<SymbolKind> = patterns.iter().map(|(_, k)| *k).collect();

    match Query::new(&language, &combined) {
        Ok(query) => {
            // Build a capture-index → SymbolKind map.
            // Each pattern contributes one capture; the capture names in order correspond to kinds.
            let mut capture_kinds = vec![SymbolKind::Function; query.capture_names().len()];
            for (cap_idx, cap_name) in query.capture_names().iter().enumerate() {
                // Find the kind for this capture by matching the pattern index.
                // tree-sitter assigns captures sequentially across patterns.
                // We iterate patterns and find which capture belongs to which pattern.
                for (pattern_idx, kind) in kinds.iter().enumerate() {
                    // Check if this capture belongs to this pattern
                    let pat_start = query.start_byte_for_pattern(pattern_idx);
                    let pat_end = if pattern_idx + 1 < query.pattern_count() {
                        query.start_byte_for_pattern(pattern_idx + 1)
                    } else {
                        combined.len()
                    };
                    let pattern_text = &combined[pat_start..pat_end];
                    let expected_name = format!("@{}", cap_name);
                    if pattern_text.contains(&expected_name) {
                        capture_kinds[cap_idx] = *kind;
                        break;
                    }
                }
            }
            map.insert(name, (language, query, capture_kinds));
        }
        Err(e) => {
            log::warn!("Failed to compile outline query for {}: {}", name, e);
        }
    }
}

/// Extract symbols from source using tree-sitter queries.
///
/// Accepts a `&mut Parser` so callers can reuse it across invocations.
pub(crate) fn extract_symbols_treesitter(
    source: &str,
    language: &str,
    parser: &mut Parser,
) -> Vec<SymbolInfo> {
    let queries = queries();
    let (ts_language, query, capture_kinds) = match queries.get(language) {
        Some(entry) => entry,
        None => return Vec::new(),
    };

    if parser.set_language(ts_language).is_err() {
        return Vec::new();
    }

    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return Vec::new(),
    };

    let mut cursor = QueryCursor::new();
    let source_bytes = source.as_bytes();
    let mut matches = cursor.matches(query, tree.root_node(), source_bytes);

    let mut symbols = Vec::new();

    while let Some(m) = matches.next() {
        for capture in m.captures {
            let node = capture.node;
            let kind = capture_kinds
                .get(capture.index as usize)
                .copied()
                .unwrap_or(SymbolKind::Function);

            let name = node
                .utf8_text(source_bytes)
                .unwrap_or("")
                .trim()
                .to_string();
            if name.is_empty() {
                continue;
            }

            let line = node.start_position().row;
            let column = node.start_position().column;

            // Heading kinds map directly to depth; AST walk is meaningless for
            // markdown because all atx_heading nodes are siblings at the top level.
            let depth = match kind {
                SymbolKind::Heading1 => 0,
                SymbolKind::Heading2 => 1,
                SymbolKind::Heading3 => 2,
                SymbolKind::Heading4 => 3,
                SymbolKind::Heading5 => 4,
                SymbolKind::Heading6 => 5,
                _ => compute_depth(&node, query, capture_kinds, source_bytes),
            };

            symbols.push(SymbolInfo {
                name,
                full_name: None,
                kind,
                line,
                column,
                depth,
            });
        }
    }

    // Sort by line number, deduplicate by (line, name)
    symbols.sort_by_key(|s| (s.line, s.column));
    symbols.dedup_by(|a, b| a.line == b.line && a.name == b.name);

    symbols
}

/// Compute nesting depth by walking up the tree and counting symbol-producing ancestors.
fn compute_depth(
    node: &tree_sitter::Node,
    query: &Query,
    capture_kinds: &[SymbolKind],
    source: &[u8],
) -> usize {
    let mut depth = 0;
    let mut current = node.parent();

    // Set of node kinds that are "symbol containers"
    while let Some(parent) = current {
        // Check if this parent node would produce a symbol match
        if is_symbol_container(&parent, query, capture_kinds, source) {
            depth += 1;
        }
        current = parent.parent();
    }

    depth
}

/// Check if a node is a symbol container (impl_item, class, struct body, etc.)
fn is_symbol_container(
    node: &tree_sitter::Node,
    _query: &Query,
    _capture_kinds: &[SymbolKind],
    _source: &[u8],
) -> bool {
    let kind = node.kind();
    matches!(
        kind,
        "impl_item"
            | "class_definition"
            | "class_declaration"
            | "class_specifier"
            | "object_declaration"
            | "module"
            | "module_definition"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_symbols() {
        let source = r#"
fn main() {}
struct Config {}
enum Color { Red, Blue }
trait Panel {}
"#;
        let mut parser = Parser::new();
        let symbols = extract_symbols_treesitter(source, "rust", &mut parser);
        assert!(!symbols.is_empty(), "Should find Rust symbols");
    }

    #[test]
    fn test_kotlin_symbols() {
        let source = "package p\n\n\
                      class Animal {\n    fun speak() {}\n}\n\n\
                      interface Draw {\n    fun draw()\n}\n\n\
                      object Registry {\n    fun get(): Int = 1\n}\n\n\
                      enum class Color { RED, GREEN }\n\n\
                      fun topLevel() {}\n";
        let mut parser = Parser::new();
        let symbols = extract_symbols_treesitter(source, "kotlin", &mut parser);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Animal"), "classes: {names:?}");
        assert!(names.contains(&"Draw"), "interfaces: {names:?}");
        assert!(names.contains(&"Registry"), "objects: {names:?}");
        assert!(names.contains(&"Color"), "enums: {names:?}");
        assert!(names.contains(&"speak"), "methods: {names:?}");
        assert!(names.contains(&"topLevel"), "free functions: {names:?}");
    }

    #[test]
    fn test_markdown_symbols() {
        let source = "# Hello\n\n## World\n\nSome text\n\n### Sub heading\n";
        let mut parser = Parser::new();
        let symbols = extract_symbols_treesitter(source, "markdown", &mut parser);
        assert_eq!(symbols.len(), 3);
        assert_eq!(symbols[0].kind, SymbolKind::Heading1);
        assert_eq!(symbols[0].depth, 0);
        assert_eq!(symbols[1].kind, SymbolKind::Heading2);
        assert_eq!(symbols[1].depth, 1);
        assert_eq!(symbols[2].kind, SymbolKind::Heading3);
        assert_eq!(symbols[2].depth, 2);
    }

    #[test]
    fn test_markdown_query_compilation() {
        let queries = queries();
        assert!(
            queries.contains_key("markdown"),
            "Markdown query should be registered"
        );
    }

    #[test]
    fn test_markdown_tree() {
        let source = "# Hello\n\n## World\n";
        let language: Language = tree_sitter_md::LANGUAGE.into();
        let mut parser = Parser::new();
        parser.set_language(&language).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let _ = tree;
    }
}
