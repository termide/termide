//! Rich PHP extractor: classes/interfaces with typed properties and method
//! signatures, `extends`/`implements` relationships, and module-level
//! functions/constants.

use std::collections::HashSet;
use std::path::Path;

use tree_sitter::{Node, Parser};

use super::model::{collapse, module_name, node_text as text, rel, sanitize_ident, Model};

pub(crate) fn generate(source: &str, file_path: Option<&Path>) -> Option<String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_php::LANGUAGE_PHP.into())
        .ok()?;
    let tree = parser.parse(source, None)?;
    let src = source.as_bytes();

    let module = module_name(file_path);
    let mut model = Model::new();
    let mut comps: Vec<(String, String, String)> = Vec::new();

    let mut c = tree.root_node().walk();
    for item in tree.root_node().named_children(&mut c) {
        match item.kind() {
            "class_declaration" => handle_class(item, src, &mut model, &mut comps),
            "interface_declaration" => handle_interface(item, src, &mut model),
            "enum_declaration" => handle_enum(item, src, &mut model),
            "function_definition" => {
                if let (Some(sig), Some(idx)) =
                    (format_fn(item, src, "+"), model.module_box(&module))
                {
                    model.push_member(idx, sig);
                }
            }
            "const_declaration" => {
                let mut cc = item.walk();
                for el in item.named_children(&mut cc) {
                    if el.kind() == "const_element" {
                        if let Some(n) = el.named_child(0) {
                            if let Some(idx) = model.module_box(&module) {
                                model.push_member(idx, format!("+{}", text(n, src)));
                            }
                        }
                    }
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

fn handle_class(
    item: Node,
    src: &[u8],
    model: &mut Model,
    comps: &mut Vec<(String, String, String)>,
) {
    let Some(name_node) = item.child_by_field_name("name") else {
        return;
    };
    let type_name = text(name_node, src).to_string();
    let Some(idx) = model.box_idx(&type_name) else {
        return;
    };
    let owner = sanitize_ident(&type_name);

    let mut hc = item.walk();
    for ch in item.named_children(&mut hc) {
        match ch.kind() {
            "base_clause" => {
                for base in names(ch, src) {
                    model.add_rel(&base, &type_name, rel::INHERIT, "");
                }
            }
            "class_interface_clause" => {
                for iface in names(ch, src) {
                    model.add_rel(&iface, &type_name, rel::REALIZE, "");
                }
            }
            _ => {}
        }
    }

    let Some(body) = item.child_by_field_name("body") else {
        return;
    };
    let mut c = body.walk();
    for m in body.named_children(&mut c) {
        match m.kind() {
            "property_declaration" => {
                let ty = m.child_by_field_name("type");
                let vis = vis(m, src);
                let mut pc = m.walk();
                for el in m.named_children(&mut pc) {
                    if el.kind() == "property_element" {
                        if let Some(name) = el.child_by_field_name("name") {
                            let tt = ty.map(|t| collapse(t, src)).unwrap_or_default();
                            let n = text(name, src);
                            model.push_member(idx, format!("{vis}{n}: {tt}"));
                        }
                    }
                }
                if let Some(ty) = ty {
                    if let Some(base) = base_ident(ty, src) {
                        comps.push((owner.clone(), base, String::new()));
                    }
                }
            }
            "method_declaration" => {
                if let Some(sig) = format_fn(m, src, vis(m, src)) {
                    model.push_member(idx, sig);
                }
            }
            _ => {}
        }
    }
}

fn handle_interface(item: Node, src: &[u8], model: &mut Model) {
    let Some(name_node) = item.child_by_field_name("name") else {
        return;
    };
    let Some(idx) = model.box_idx(text(name_node, src)) else {
        return;
    };
    model.set_stereotype(idx, "interface");
    if let Some(body) = item.child_by_field_name("body") {
        let mut c = body.walk();
        for m in body.named_children(&mut c) {
            if m.kind() == "method_declaration" {
                if let Some(sig) = format_fn(m, src, "+") {
                    model.push_member(idx, sig);
                }
            }
        }
    }
}

fn handle_enum(item: Node, src: &[u8], model: &mut Model) {
    let Some(name_node) = item.child_by_field_name("name") else {
        return;
    };
    let Some(idx) = model.box_idx(text(name_node, src)) else {
        return;
    };
    model.set_stereotype(idx, "enum");
    if let Some(body) = item.child_by_field_name("body") {
        let mut c = body.walk();
        for e in body.named_children(&mut c) {
            if e.kind() == "enum_case" {
                if let Some(n) = e.child_by_field_name("name") {
                    model.push_member(idx, text(n, src));
                }
            }
        }
    }
}

fn format_fn(item: Node, src: &[u8], marker: &str) -> Option<String> {
    let name = text(item.child_by_field_name("name")?, src);
    let mut params = Vec::new();
    if let Some(ps) = item.child_by_field_name("parameters") {
        let mut c = ps.walk();
        for p in ps.named_children(&mut c) {
            if matches!(
                p.kind(),
                "simple_parameter" | "variadic_parameter" | "property_promotion_parameter"
            ) {
                let repr = match p.child_by_field_name("type") {
                    Some(t) => collapse(t, src),
                    None => p
                        .child_by_field_name("name")
                        .map(|n| text(n, src).to_string())
                        .unwrap_or_default(),
                };
                if !repr.is_empty() {
                    params.push(repr);
                }
            }
        }
    }
    let mut s = format!("{marker}{name}({})", params.join(", "));
    if let Some(ret) = item.child_by_field_name("return_type") {
        s.push(' ');
        s.push_str(&collapse(ret, src));
    }
    Some(s)
}

/// `name` nodes directly under a clause (`extends A`, `implements A, B`).
fn names(clause: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut c = clause.walk();
    for ch in clause.named_children(&mut c) {
        if matches!(ch.kind(), "name" | "qualified_name") {
            let s = sanitize_ident(text(ch, src));
            if !s.is_empty() {
                out.push(s);
            }
        }
    }
    out
}

fn base_ident(n: Node, src: &[u8]) -> Option<String> {
    match n.kind() {
        "named_type" | "name" | "qualified_name" | "type_identifier" => {
            let s = sanitize_ident(text(n, src));
            (!s.is_empty()).then_some(s)
        }
        _ => {
            let mut c = n.walk();
            let found = n.named_children(&mut c).find_map(|ch| base_ident(ch, src));
            found
        }
    }
}

fn vis(member: Node, src: &[u8]) -> &'static str {
    let mut c = member.walk();
    let modifier = member
        .named_children(&mut c)
        .find(|n| n.kind() == "visibility_modifier");
    match modifier.map(|m| text(m, src)) {
        Some(t) if t.contains("private") => "-",
        Some(t) if t.contains("protected") => "#",
        _ => "+",
    }
}
