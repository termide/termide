//! Fallback symbol extraction for text formats and languages without a query.

use regex::Regex;
use std::sync::LazyLock;

use crate::symbols::{SymbolInfo, SymbolKind};

static MD_HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(#{1,6})\s+(.+)$").expect("valid regex"));

static HTML_HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<h([1-6])[^>]*>([^<]+)").expect("valid regex"));

// Matches [section], [section.sub]; only bare TOML keys: alphanumeric, '_', '-', joined by '.'
static TOML_SECTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*\[(\[?)\s*([A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)*)\s*\]?\]").expect("valid regex")
});

static YAML_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)([a-zA-Z0-9_][a-zA-Z0-9_ .-]*)\s*:").expect("valid regex"));

static XML_OPEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<([a-zA-Z][a-zA-Z0-9_:-]*)[>\s/]").expect("valid regex"));

static XML_CLOSE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"</([a-zA-Z][a-zA-Z0-9_:-]*)\s*>").expect("valid regex"));

static XML_SELF_CLOSE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<([a-zA-Z][a-zA-Z0-9_:-]*)[^>]*/\s*>").expect("valid regex"));

/// Extract symbols using regex patterns for languages without tree-sitter support.
pub(crate) fn extract_symbols_regex(source: &str, language: &str) -> Vec<SymbolInfo> {
    match language {
        "alatyr" => crate::alatyr::extract_symbols(source),
        "markdown" => extract_markdown_headings(source),
        "html" => extract_html_headings(source),
        "toml" => extract_toml_sections(source),
        "yaml" => extract_yaml_keys(source),
        "xml" => extract_xml_elements(source),
        _ => Vec::new(),
    }
}

fn extract_markdown_headings(source: &str) -> Vec<SymbolInfo> {
    let mut symbols = Vec::new();

    for (line_idx, line) in source.lines().enumerate() {
        if let Some(caps) = MD_HEADING_RE.captures(line) {
            let hashes = caps.get(1).map(|m| m.as_str().len()).unwrap_or(1);
            let name = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let kind = match hashes {
                1 => SymbolKind::Heading1,
                2 => SymbolKind::Heading2,
                3 => SymbolKind::Heading3,
                4 => SymbolKind::Heading4,
                5 => SymbolKind::Heading5,
                _ => SymbolKind::Heading6,
            };
            symbols.push(SymbolInfo {
                name: name.to_string(),
                full_name: None,
                kind,
                line: line_idx,
                column: 0,
                depth: hashes.saturating_sub(1),
            });
        }
    }

    symbols
}

fn extract_toml_sections(source: &str) -> Vec<SymbolInfo> {
    let mut symbols = Vec::new();
    let mut emitted: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for (line_idx, line) in source.lines().enumerate() {
        if let Some(caps) = TOML_SECTION_RE.captures(line) {
            let is_array = caps.get(1).is_some_and(|m| !m.as_str().is_empty());
            if is_array {
                continue;
            }
            let full_name = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            if full_name.is_empty() {
                continue;
            }

            // Find the longest emitted ancestor prefix
            let segments: Vec<&str> = full_name.split('.').collect();
            let mut ancestor_len = 0;
            let mut ancestor_depth = 0;
            for i in (1..segments.len()).rev() {
                let prefix = segments[..i].join(".");
                if let Some(&d) = emitted.get(&prefix) {
                    ancestor_len = i;
                    ancestor_depth = d;
                    break;
                }
            }

            let depth = if ancestor_len > 0 {
                ancestor_depth + 1
            } else {
                0
            };
            let display_name = segments[ancestor_len..].join(".");

            emitted.insert(full_name.to_string(), depth);
            symbols.push(SymbolInfo {
                name: display_name,
                full_name: if full_name.contains('.') {
                    Some(full_name.to_string())
                } else {
                    None
                },
                kind: SymbolKind::Section,
                line: line_idx,
                column: 0,
                depth,
            });
        }
    }

    symbols
}

fn extract_html_headings(source: &str) -> Vec<SymbolInfo> {
    let mut symbols = Vec::new();

    for (line_idx, line) in source.lines().enumerate() {
        for caps in HTML_HEADING_RE.captures_iter(line) {
            let level: usize = caps
                .get(1)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(1);
            let name = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let kind = match level {
                1 => SymbolKind::Heading1,
                2 => SymbolKind::Heading2,
                3 => SymbolKind::Heading3,
                4 => SymbolKind::Heading4,
                5 => SymbolKind::Heading5,
                _ => SymbolKind::Heading6,
            };
            symbols.push(SymbolInfo {
                name: name.to_string(),
                full_name: None,
                kind,
                line: line_idx,
                column: 0,
                depth: level.saturating_sub(1),
            });
        }
    }

    symbols
}

fn extract_yaml_keys(source: &str) -> Vec<SymbolInfo> {
    let mut symbols = Vec::new();

    for (line_idx, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        // Skip comments and document markers
        if trimmed.starts_with('#') || trimmed.starts_with("---") || trimmed.starts_with("...") {
            continue;
        }

        if let Some(caps) = YAML_KEY_RE.captures(line) {
            let indent = caps.get(1).map(|m| m.as_str().len()).unwrap_or(0);
            let depth = indent / 2;
            if depth > 1 {
                continue;
            }
            let name = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            symbols.push(SymbolInfo {
                name: name.to_string(),
                full_name: None,
                kind: SymbolKind::Section,
                line: line_idx,
                column: indent,
                depth,
            });
        }
    }

    symbols
}

fn extract_xml_elements(source: &str) -> Vec<SymbolInfo> {
    let mut symbols = Vec::new();
    let mut stack: Vec<String> = Vec::new();

    for (line_idx, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        // Skip processing instructions, comments, and DOCTYPE
        if trimmed.starts_with("<?") || trimmed.starts_with("<!--") || trimmed.starts_with("<!") {
            continue;
        }

        // Process all tags on the line by scanning from left to right
        let mut pos = 0;
        while pos < line.len() {
            let remaining = &line[pos..];

            // Find the next '<' character
            let Some(lt) = remaining.find('<') else {
                break;
            };
            let tag_start = pos + lt;
            let tag_rest = &line[tag_start..];

            // Skip comments/PI that appear mid-line
            if tag_rest.starts_with("<?")
                || tag_rest.starts_with("<!--")
                || tag_rest.starts_with("<!")
            {
                pos = tag_start + 1;
                continue;
            }

            // Closing tag (must start with </)
            if tag_rest.starts_with("</") {
                if let Some(caps) = XML_CLOSE_RE.captures(tag_rest) {
                    let m = caps.get(0).unwrap();
                    let tag = caps.get(1).map(|c| c.as_str()).unwrap_or("");
                    if let Some(p) = stack.iter().rposition(|t| t == tag) {
                        stack.truncate(p);
                    }
                    pos = tag_start + m.end();
                    continue;
                }
                pos = tag_start + 1;
                continue;
            }

            // Self-closing tag (check before open since both start with <tag)
            if let Some(caps) = XML_SELF_CLOSE_RE.captures(tag_rest) {
                let m = caps.get(0).unwrap();
                // Ensure the match starts at position 0 in tag_rest
                if m.start() == 0 {
                    let tag = caps.get(1).map(|c| c.as_str()).unwrap_or("");
                    if !tag.is_empty() && stack.len() <= 1 {
                        symbols.push(SymbolInfo {
                            name: tag.to_string(),
                            full_name: None,
                            kind: SymbolKind::Section,
                            line: line_idx,
                            column: tag_start,
                            depth: stack.len(),
                        });
                    }
                    pos = tag_start + m.end();
                    continue;
                }
            }

            // Opening tag
            if let Some(caps) = XML_OPEN_RE.captures(tag_rest) {
                let m = caps.get(0).unwrap();
                if m.start() == 0 {
                    let tag = caps.get(1).map(|c| c.as_str()).unwrap_or("");
                    if !tag.is_empty() {
                        if stack.len() <= 1 {
                            symbols.push(SymbolInfo {
                                name: tag.to_string(),
                                full_name: None,
                                kind: SymbolKind::Section,
                                line: line_idx,
                                column: tag_start,
                                depth: stack.len(),
                            });
                        }
                        stack.push(tag.to_string());
                    }
                    pos = tag_start + m.end();
                    continue;
                }
            }

            // No tag pattern matched, advance past '<'
            pos = tag_start + 1;
        }
    }

    symbols
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toml_sections_skip_arrays() {
        let source = "[package]\nname = \"foo\"\n\n[[bin]]\nname = \"bar\"\n\n[dependencies]\n";
        let symbols = extract_toml_sections(source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["package", "dependencies"]);
    }

    #[test]
    fn test_toml_nested_sections() {
        let source = "[a]\n[a.b]\n[[a.b.c]]\n";
        let symbols = extract_toml_sections(source);
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "a");
        assert_eq!(symbols[0].depth, 0);
        assert_eq!(symbols[1].name, "b");
        assert_eq!(symbols[1].depth, 1);
    }

    #[test]
    fn test_toml_skipped_ancestors_show_relative_path() {
        // [package] exists, but [package.metadata] and [package.metadata.docs] do not
        let source = "[package]\n[package.metadata.docs.rs]\n[package.metadata.deb]\n";
        let symbols = extract_toml_sections(source);
        assert_eq!(symbols.len(), 3);
        assert_eq!(symbols[0].name, "package");
        assert_eq!(symbols[0].depth, 0);
        // metadata.docs.rs collapsed under package
        assert_eq!(symbols[1].name, "metadata.docs.rs");
        assert_eq!(symbols[1].depth, 1);
        // metadata.deb — now package.metadata is emitted, so deb is under it
        assert_eq!(symbols[2].name, "metadata.deb");
        assert_eq!(symbols[2].depth, 1);
    }

    #[test]
    fn test_toml_inline_arrays_not_matched() {
        // Inline array values like ["target/release/termide", "usr/bin/", "755"]
        // must not be matched as section headers
        let source = "[package]\nassets = [\n  [\"target/release/termide\", \"usr/bin/\", \"755\"],\n]\n[dependencies]\n";
        let symbols = extract_toml_sections(source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["package", "dependencies"]);
    }

    #[test]
    fn test_toml_full_name_for_dotted_sections() {
        let source = "[package]\n[package.metadata.docs.rs]\n[dependencies]\n";
        let symbols = extract_toml_sections(source);
        assert!(symbols[0].full_name.is_none()); // "package" — no dots
        assert_eq!(
            symbols[1].full_name.as_deref(),
            Some("package.metadata.docs.rs")
        );
        assert!(symbols[2].full_name.is_none()); // "dependencies" — no dots
    }

    #[test]
    fn test_toml_deep_chain_with_intermediate() {
        // [package.metadata.docs.rs] then [package.metadata.generate-rpm.requires]
        let source = "[package]\n[package.metadata.docs.rs]\n[package.metadata.generate-rpm]\n[package.metadata.generate-rpm.requires]\n";
        let symbols = extract_toml_sections(source);
        assert_eq!(symbols.len(), 4);
        assert_eq!(symbols[2].name, "metadata.generate-rpm");
        assert_eq!(symbols[2].depth, 1);
        // requires is a child of the emitted generate-rpm
        assert_eq!(symbols[3].name, "requires");
        assert_eq!(symbols[3].depth, 2);
    }

    #[test]
    fn test_yaml_top_level_keys() {
        let source = "name: my-app\nversion: 1.0\ndependencies:\n  foo: ^1.0\n  bar: ^2.0\n    baz: nested\n";
        let symbols = extract_yaml_keys(source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["name", "version", "dependencies", "foo", "bar"]);
        assert_eq!(symbols[0].depth, 0);
        assert_eq!(symbols[3].depth, 1);
        // "baz" at 4 spaces (depth 2) should be excluded
    }

    #[test]
    fn test_yaml_comments_skipped() {
        let source =
            "# This is a comment\nname: value\n  # indented comment\n  key: val\n---\nnext: doc\n";
        let symbols = extract_yaml_keys(source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["name", "key", "next"]);
    }

    #[test]
    fn test_xml_nested_elements() {
        let source = "<root>\n  <child>\n    <deep>text</deep>\n  </child>\n  <other>text</other>\n</root>\n";
        let symbols = extract_xml_elements(source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["root", "child", "other"]);
        assert_eq!(symbols[0].depth, 0);
        assert_eq!(symbols[1].depth, 1);
        assert_eq!(symbols[2].depth, 1);
    }

    #[test]
    fn test_xml_self_closing() {
        let source = "<?xml version=\"1.0\"?>\n<root>\n  <item name=\"a\" />\n  <item name=\"b\" />\n</root>\n";
        let symbols = extract_xml_elements(source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["root", "item", "item"]);
        assert_eq!(symbols[0].depth, 0);
        assert_eq!(symbols[1].depth, 1);
        assert_eq!(symbols[2].depth, 1);
    }
}
