//! Name-based fallback extractor for languages without a dedicated rich
//! extractor.
//!
//! Uses the outline's flat symbol list (names + kinds + nesting depth). It can
//! only recover type boxes with their methods and a `<<module>>` box of
//! top-level functions — no field types, signatures, or relationships — but it
//! gives every tree-sitter language (Go, C, Java, Ruby, …) at least a usable
//! diagram until it gets a dedicated extractor like `rust.rs`.

use std::path::Path;

use termide_panel_outline::symbols::{SymbolInfo, SymbolKind};

use super::model::{module_name, Model};

pub(crate) fn generate(
    source: &str,
    language: Option<&str>,
    file_path: Option<&Path>,
) -> Option<String> {
    let mut parser = tree_sitter::Parser::new();
    let symbols =
        termide_panel_outline::symbols::extract_symbols(source, language, file_path, &mut parser);

    let mut model = Model::new();
    build(&symbols, &module_name(file_path), &mut model);
    model.render()
}

/// Whether a type kind is a container that directly holds methods.
///
/// In the outline's tree-sitter model a member's nesting `depth` equals the
/// depth of its container's *name* node, while a sibling in the enclosing scope
/// is one level shallower. Only classes, interfaces (`Trait`), and Rust `impl`
/// blocks wrap their captured functions.
fn is_container_kind(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class | SymbolKind::Trait | SymbolKind::Impl
    )
}

fn build(symbols: &[SymbolInfo], module: &str, model: &mut Model) {
    // (name depth, box index) of the most recent container-capable type.
    let mut current: Option<(usize, usize)> = None;
    let mut module_idx: Option<usize> = None;

    for sym in symbols {
        match sym.kind {
            SymbolKind::Struct
            | SymbolKind::Class
            | SymbolKind::Enum
            | SymbolKind::Trait
            | SymbolKind::Impl => {
                if let Some(idx) = model.box_idx(&sym.name) {
                    match sym.kind {
                        SymbolKind::Enum => model.set_stereotype(idx, "enum"),
                        SymbolKind::Trait => model.set_stereotype(idx, "trait"),
                        _ => {}
                    }
                    if is_container_kind(sym.kind) {
                        current = Some((sym.depth, idx));
                    }
                }
            }
            SymbolKind::Function | SymbolKind::Method => {
                // A member sits at the same depth as its container's name.
                // Functions not nested in a captured type are module-level and
                // go into a synthetic `<<module>>` box.
                match current {
                    Some((depth, idx)) if sym.depth == depth => {
                        model.push_member(idx, format!("+{}()", sym.name));
                    }
                    _ => {
                        let idx = module_box(model, module, &mut module_idx);
                        model.push_member(idx, format!("+{}()", sym.name));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Get-or-create the synthetic `<<module>>` box for top-level functions.
fn module_box(model: &mut Model, module: &str, module_idx: &mut Option<usize>) -> usize {
    if let Some(i) = *module_idx {
        return i;
    }
    let i = model.box_idx(module).unwrap_or(0);
    model.set_stereotype(i, "module");
    *module_idx = Some(i);
    i
}
