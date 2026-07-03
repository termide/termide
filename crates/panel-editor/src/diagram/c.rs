//! Rich C extractor: structs/unions (fields), enums, and module-level
//! functions. C has no visibility or methods, so all members are public and
//! functions live in the `<<module>>` box.

use std::collections::HashSet;
use std::path::Path;

use tree_sitter::{Node, Parser};

use super::model::{
    collapse, module_label, module_name, node_text as text, rel, sanitize_ident, Model,
};

pub(crate) fn generate(source: &str, file_path: Option<&Path>) -> Option<String> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_c::LANGUAGE.into()).ok()?;
    let tree = parser.parse(source, None)?;
    let src = source.as_bytes();
    let module = module_name(file_path);
    let mut model = Model::new();
    model.set_module_label(module_label(file_path));
    let mut comps: Vec<(String, String, String)> = Vec::new();

    let mut c = tree.root_node().walk();
    for item in tree.root_node().named_children(&mut c) {
        match item.kind() {
            "struct_specifier" | "union_specifier" => {
                handle_record(item, None, src, &mut model, &mut comps)
            }
            "enum_specifier" => handle_enum(item, None, src, &mut model),
            "type_definition" => handle_typedef(item, src, &mut model, &mut comps),
            "function_definition" => {
                if let (Some(sig), Some(idx)) = (format_fn(item, src), model.module_box(&module)) {
                    model.push_member(idx, sig);
                }
            }
            _ => {}
        }
    }

    let local: HashSet<String> = model.local_type_names().into_iter().collect();
    for (owner, base, label) in comps {
        if owner != base && local.contains(&base) {
            model.add_rel(&owner, &base, rel::COMPOSE, &label);
        }
    }
    model.render()
}

/// `typedef struct {...} Name;` — name the record after the typedef alias when
/// the record itself is anonymous.
fn handle_typedef(
    item: Node,
    src: &[u8],
    model: &mut Model,
    comps: &mut Vec<(String, String, String)>,
) {
    let alias = item
        .child_by_field_name("declarator")
        .map(|d| innermost_ident(d, src));
    let mut c = item.walk();
    for ch in item.named_children(&mut c) {
        match ch.kind() {
            "struct_specifier" | "union_specifier" => {
                handle_record(ch, alias.as_deref(), src, model, comps)
            }
            "enum_specifier" => handle_enum(ch, alias.as_deref(), src, model),
            _ => {}
        }
    }
}

fn handle_record(
    item: Node,
    alias: Option<&str>,
    src: &[u8],
    model: &mut Model,
    comps: &mut Vec<(String, String, String)>,
) {
    let name = item
        .child_by_field_name("name")
        .map(|n| text(n, src).to_string())
        .or_else(|| alias.map(str::to_string));
    let Some(name) = name else {
        return;
    };
    let Some(idx) = model.box_idx(&name) else {
        return;
    };
    let kw = if item.kind() == "union_specifier" {
        "union"
    } else {
        "struct"
    };
    model.set_header(idx, format!("{kw} {}", sanitize_ident(&name)));
    let owner = sanitize_ident(&name);
    let Some(body) = item.child_by_field_name("body") else {
        return;
    };
    let mut c = body.walk();
    for f in body.named_children(&mut c) {
        if f.kind() != "field_declaration" {
            continue;
        }
        let ty = f.child_by_field_name("type");
        if let Some(decl) = f.child_by_field_name("declarator") {
            let fname = innermost_ident(decl, src);
            let tt = ty.map(|t| collapse(t, src)).unwrap_or_default();
            model.push_member(idx, format!("+{fname}: {tt}"));
        }
        if let Some(ty) = ty {
            if let Some(base) = base_ident(ty, src) {
                comps.push((owner.clone(), base, String::new()));
            }
        }
    }
}

fn handle_enum(item: Node, alias: Option<&str>, src: &[u8], model: &mut Model) {
    let name = item
        .child_by_field_name("name")
        .map(|n| text(n, src).to_string())
        .or_else(|| alias.map(str::to_string));
    let Some(name) = name else {
        return;
    };
    let Some(idx) = model.box_idx(&name) else {
        return;
    };
    model.set_header(idx, format!("enum {}", sanitize_ident(&name)));
    if let Some(body) = item.child_by_field_name("body") {
        let mut c = body.walk();
        for e in body.named_children(&mut c) {
            if e.kind() == "enumerator" {
                if let Some(n) = e.child_by_field_name("name") {
                    model.push_member(idx, text(n, src));
                }
            }
        }
    }
}

fn format_fn(item: Node, src: &[u8]) -> Option<String> {
    let decl = item.child_by_field_name("declarator")?;
    // decl is a function_declarator (possibly wrapped in a pointer_declarator).
    let fdecl = find_function_declarator(decl)?;
    let name = fdecl
        .child_by_field_name("declarator")
        .map(|d| innermost_ident(d, src))
        .unwrap_or_default();
    if name.is_empty() {
        return None;
    }
    let mut ptypes = Vec::new();
    if let Some(params) = fdecl.child_by_field_name("parameters") {
        let mut c = params.walk();
        for p in params.named_children(&mut c) {
            if p.kind() == "parameter_declaration" {
                if let Some(t) = p.child_by_field_name("type") {
                    ptypes.push(collapse(t, src));
                }
            }
        }
    }
    let ret = item
        .child_by_field_name("type")
        .map(|t| collapse(t, src))
        .unwrap_or_default();
    let mut s = format!("+{name}({})", ptypes.join(", "));
    if !ret.is_empty() {
        s.push(' ');
        s.push_str(&ret);
    }
    Some(s)
}

fn find_function_declarator(node: Node) -> Option<Node> {
    if node.kind() == "function_declarator" {
        return Some(node);
    }
    node.child_by_field_name("declarator")
        .and_then(find_function_declarator)
}

/// Innermost identifier of a (possibly pointer/array) declarator.
fn innermost_ident(node: Node, src: &[u8]) -> String {
    match node.kind() {
        "identifier" | "field_identifier" | "type_identifier" => text(node, src).to_string(),
        _ => node
            .child_by_field_name("declarator")
            .map(|d| innermost_ident(d, src))
            .unwrap_or_default(),
    }
}

fn base_ident(n: Node, src: &[u8]) -> Option<String> {
    match n.kind() {
        "type_identifier" => {
            let s = sanitize_ident(text(n, src));
            (!s.is_empty()).then_some(s)
        }
        "struct_specifier" | "union_specifier" | "enum_specifier" => n
            .child_by_field_name("name")
            .and_then(|x| base_ident(x, src)),
        _ => None,
    }
}
