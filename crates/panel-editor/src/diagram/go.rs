//! Rich Go extractor: structs (fields), interfaces (methods), methods attached
//! to their receiver type, and module-level functions/consts. Visibility
//! follows Go's exported-name convention (leading uppercase = public).

use std::collections::HashSet;
use std::path::Path;

use tree_sitter::{Node, Parser};

use super::model::{
    collapse, module_label, module_name, node_text as text, rel, sanitize_ident, Model,
};

pub(crate) fn generate(source: &str, file_path: Option<&Path>) -> Option<String> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_go::LANGUAGE.into()).ok()?;
    let tree = parser.parse(source, None)?;
    let src = source.as_bytes();

    let module = module_name(file_path);
    let mut model = Model::new();
    model.set_module_label(module_label(file_path));
    let mut comps: Vec<(String, String, String)> = Vec::new();

    let mut c = tree.root_node().walk();
    for item in tree.root_node().named_children(&mut c) {
        match item.kind() {
            "type_declaration" => handle_type_decl(item, src, &mut model, &module, &mut comps),
            "method_declaration" => handle_method(item, src, &mut model),
            "function_declaration" => {
                if let (Some(sig), Some(idx)) =
                    (format_fn(item, src, "+"), model.module_box(&module))
                {
                    model.push_member(idx, sig);
                }
            }
            "const_declaration" | "var_declaration" => handle_const(item, src, &mut model, &module),
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

fn handle_type_decl(
    item: Node,
    src: &[u8],
    model: &mut Model,
    module: &str,
    comps: &mut Vec<(String, String, String)>,
) {
    let mut c = item.walk();
    for spec in item.named_children(&mut c) {
        if spec.kind() != "type_spec" {
            continue;
        }
        let Some(name_node) = spec.child_by_field_name("name") else {
            continue;
        };
        let type_name = text(name_node, src).to_string();
        let Some(ty) = spec.child_by_field_name("type") else {
            continue;
        };
        match ty.kind() {
            "struct_type" => {
                let Some(idx) = model.box_idx(&type_name) else {
                    continue;
                };
                model.set_header(idx, format!("struct {}", sanitize_ident(&type_name)));
                let owner = sanitize_ident(&type_name);
                if let Some(fields) = ty.named_child(0) {
                    let mut fc = fields.walk();
                    for f in fields.named_children(&mut fc) {
                        if f.kind() != "field_declaration" {
                            continue;
                        }
                        let fty = f.child_by_field_name("type");
                        let fname = f.child_by_field_name("name");
                        if let Some(fname) = fname {
                            let n = text(fname, src);
                            let tt = fty.map(|t| collapse(t, src)).unwrap_or_default();
                            model.push_member(idx, format!("{}{n}: {tt}", vis(n)));
                        }
                        if let Some(fty) = fty {
                            if let Some(base) = base_ident(fty, src) {
                                let label =
                                    fname.map(|n| text(n, src).to_string()).unwrap_or_default();
                                comps.push((owner.clone(), base, label));
                            }
                        }
                    }
                }
            }
            "interface_type" => {
                let Some(idx) = model.box_idx(&type_name) else {
                    continue;
                };
                model.set_header(idx, format!("interface {}", sanitize_ident(&type_name)));
                let mut ic = ty.walk();
                for m in ty.named_children(&mut ic) {
                    if m.kind() == "method_elem" {
                        if let Some(sig) = format_fn(m, src, "+") {
                            model.push_member(idx, sig);
                        }
                    }
                }
            }
            _ => {
                // Type alias / named basic type -> module box entry.
                if let Some(idx) = model.module_box(module) {
                    model.push_member(idx, format!("{}type {type_name}", vis(&type_name)));
                }
            }
        }
    }
}

fn handle_method(item: Node, src: &[u8], model: &mut Model) {
    // Attach to the receiver type: `func (c *Circle) Area()` -> Circle.
    let recv_type = item
        .child_by_field_name("receiver")
        .and_then(|r| {
            let mut c = r.walk();
            let decl = r
                .named_children(&mut c)
                .find(|n| n.kind() == "parameter_declaration");
            decl
        })
        .and_then(|p| p.child_by_field_name("type"))
        .and_then(|t| base_ident(t, src));
    let Some(recv_type) = recv_type else {
        return;
    };
    let Some(idx) = model.box_idx(&recv_type) else {
        return;
    };
    let name = item
        .child_by_field_name("name")
        .map(|n| text(n, src))
        .unwrap_or("");
    if let Some(sig) = format_fn(item, src, vis(name)) {
        model.push_member(idx, sig);
    }
}

fn handle_const(item: Node, src: &[u8], model: &mut Model, module: &str) {
    let mut c = item.walk();
    for spec in item.named_children(&mut c) {
        if !matches!(spec.kind(), "const_spec" | "var_spec") {
            continue;
        }
        if let Some(name) = spec.child_by_field_name("name") {
            let n = text(name, src);
            if let Some(idx) = model.module_box(module) {
                model.push_member(idx, format!("{}{n}", vis(n)));
            }
        }
    }
}

fn format_fn(f: Node, src: &[u8], marker: &str) -> Option<String> {
    let name = text(f.child_by_field_name("name")?, src);
    let mut ptypes = Vec::new();
    if let Some(params) = f.child_by_field_name("parameters") {
        let mut c = params.walk();
        for p in params.named_children(&mut c) {
            if p.kind() == "parameter_declaration" {
                if let Some(t) = p.child_by_field_name("type") {
                    ptypes.push(collapse(t, src));
                }
            }
        }
    }
    let mut s = format!("{marker}{name}({})", ptypes.join(", "));
    if let Some(res) = f.child_by_field_name("result") {
        s.push(' ');
        s.push_str(&collapse(res, src));
    }
    Some(s)
}

/// Base named type, descending through pointers/slices/qualified names.
fn base_ident(n: Node, src: &[u8]) -> Option<String> {
    match n.kind() {
        "type_identifier" => {
            let s = sanitize_ident(text(n, src));
            (!s.is_empty()).then_some(s)
        }
        "qualified_type" => n
            .child_by_field_name("name")
            .and_then(|x| base_ident(x, src)),
        "pointer_type" | "slice_type" | "array_type" | "generic_type" => {
            let mut c = n.walk();
            let found = n.named_children(&mut c).find_map(|ch| base_ident(ch, src));
            found
        }
        _ => None,
    }
}

/// Go visibility: exported (leading uppercase) is public, else private.
fn vis(name: &str) -> &'static str {
    match name.chars().next() {
        Some(c) if c.is_uppercase() => "+",
        _ => "-",
    }
}
