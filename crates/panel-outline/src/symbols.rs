//! Symbol data model for the outline panel.

/// Kind of symbol extracted from source code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Class,
    Trait,
    Enum,
    EnumVariant,
    Constant,
    Module,
    Impl,
    TypeAlias,
    Macro,
    Heading1,
    Heading2,
    Heading3,
    Heading4,
    Heading5,
    Heading6,
    Section,
}

impl SymbolKind {
    /// Single-character icon for display.
    pub fn icon(self) -> char {
        match self {
            SymbolKind::Function | SymbolKind::Method => 'f',
            SymbolKind::Struct => 'S',
            SymbolKind::Class => 'C',
            SymbolKind::Trait => 'T',
            SymbolKind::Enum | SymbolKind::EnumVariant => 'E',
            SymbolKind::Constant => 'c',
            SymbolKind::Module => 'M',
            SymbolKind::Impl => 'I',
            SymbolKind::TypeAlias => 't',
            SymbolKind::Macro => '!',
            SymbolKind::Heading1
            | SymbolKind::Heading2
            | SymbolKind::Heading3
            | SymbolKind::Heading4
            | SymbolKind::Heading5
            | SymbolKind::Heading6 => '#',
            SymbolKind::Section => '§',
        }
    }

    /// Whether this symbol comes from source code (as opposed to text headings/sections).
    pub fn is_code(self) -> bool {
        !matches!(
            self,
            SymbolKind::Heading1
                | SymbolKind::Heading2
                | SymbolKind::Heading3
                | SymbolKind::Heading4
                | SymbolKind::Heading5
                | SymbolKind::Heading6
                | SymbolKind::Section
        )
    }
}

/// Information about a single symbol.
#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub name: String,
    /// Full qualified name for flat-mode display (e.g. `package.metadata.docs.rs`).
    /// When `None`, flat mode falls back to `name`.
    pub full_name: Option<String>,
    pub kind: SymbolKind,
    /// 0-indexed line number.
    pub line: usize,
    /// 0-indexed column number.
    pub column: usize,
    /// Nesting depth for indentation.
    pub depth: usize,
}

/// Extract symbols from source code, dispatching to tree-sitter or a language
/// specific fallback extractor.
///
/// Accepts a `&mut Parser` so the caller can reuse it across invocations.
pub fn extract_symbols(
    source: &str,
    language: Option<&str>,
    file_path: Option<&std::path::Path>,
    parser: &mut tree_sitter::Parser,
) -> Vec<SymbolInfo> {
    let lang = match language {
        Some(lang) if lang.eq_ignore_ascii_case("alatyr") || lang.eq_ignore_ascii_case("al") => {
            Some("alatyr")
        }
        Some(lang) => Some(lang),
        None => file_path.and_then(detect_outline_language),
    };

    let lang = match lang {
        Some(l) => l,
        None => return Vec::new(),
    };

    // Try tree-sitter extraction first
    let symbols = crate::treesitter::extract_symbols_treesitter(source, lang, parser);
    if !symbols.is_empty() {
        return symbols;
    }

    // Fall back to lightweight extractors for languages without a tree-sitter
    // query (including Alatyr).
    crate::regex_fallback::extract_symbols_regex(source, lang)
}

fn detect_outline_language(path: &std::path::Path) -> Option<&'static str> {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("al"))
    {
        Some("alatyr")
    } else {
        termide_highlight::detect_language(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn detects_alatyr_from_al_extension_without_affecting_highlight_detection() {
        let mut parser = tree_sitter::Parser::new();
        let symbols = extract_symbols(
            "main := fn() -> u64 { 0 }\n",
            None,
            Some(Path::new("main.al")),
            &mut parser,
        );

        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "main");
        assert_eq!(symbols[0].kind, SymbolKind::Function);
    }
}
