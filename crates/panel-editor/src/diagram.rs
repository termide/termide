//! Generate Mermaid class diagrams from source code symbols.

use std::path::Path;

use termide_panel_outline::symbols::SymbolKind;

/// Generate a Mermaid classDiagram from source code.
///
/// Returns None if the language is unsupported or no symbols are found.
#[allow(dead_code)]
pub fn generate_class_diagram(
    source: &str,
    language: Option<&str>,
    file_path: Option<&Path>,
) -> Option<String> {
    let mut parser = tree_sitter::Parser::new();
    let symbols =
        termide_panel_outline::symbols::extract_symbols(source, language, file_path, &mut parser);

    if symbols.is_empty() {
        return None;
    }

    let mut mermaid = String::from("classDiagram\n");

    for sym in &symbols {
        match sym.kind {
            SymbolKind::Struct | SymbolKind::Class | SymbolKind::Enum | SymbolKind::Trait => {
                mermaid.push_str(&format!("    class {} {{\n", sym.name));
                mermaid.push_str("    }\n");
            }
            _ => {}
        }
    }

    Some(mermaid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_empty_diagram_for_no_symbols() {
        let result = generate_class_diagram("", Some("rust"), None);
        assert!(result.is_none());
    }

    #[test]
    fn generates_struct_class() {
        let source = "struct Dog { name: String }";
        let result = generate_class_diagram(source, Some("rust"), None).unwrap();
        assert!(result.contains("classDiagram"));
        assert!(result.contains("class Dog"));
    }

    #[test]
    fn generates_trait_class() {
        let source = "trait Animal { fn speak(&self); }";
        let result = generate_class_diagram(source, Some("rust"), None).unwrap();
        assert!(result.contains("class Animal"));
    }
}
