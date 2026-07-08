//! Rich Java extractor: classes/interfaces/enums with fields and method
//! signatures, `extends`/`implements` relationships, and access-modifier
//! visibility.

use std::path::Path;

use tree_sitter::{Node, Parser};

use super::model::{collapse, module_name, node_text as text, rel, sanitize_ident, Model};

pub(crate) fn generate(source: &str, file_path: Option<&Path>) -> Option<String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(source, None)?;
    let src = source.as_bytes();

    let _module = module_name(file_path); // Java has no free functions
    let mut model = Model::new();
    let mut comps: Vec<(String, String, String)> = Vec::new();

    walk(tree.root_node(), src, &mut model, &mut comps);

    model.resolve_compositions(comps);
    model.render()
}

fn walk(node: Node, src: &[u8], model: &mut Model, comps: &mut Vec<(String, String, String)>) {
    let mut c = node.walk();
    for item in node.named_children(&mut c) {
        match item.kind() {
            "class_declaration" => handle_class(item, src, model, comps),
            "interface_declaration" => handle_interface(item, src, model),
            "enum_declaration" => handle_enum(item, src, model),
            _ => {}
        }
    }
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
    model.set_header(
        idx,
        format!(
            "{}class {}",
            vis_keyword(item, src),
            sanitize_ident(&type_name)
        ),
    );
    let owner = sanitize_ident(&type_name);

    if let Some(sc) = item.child_by_field_name("superclass") {
        if let Some(base) = first_type_ident(sc, src) {
            model.add_rel(&base, &type_name, rel::INHERIT, "");
        }
    }
    if let Some(ifaces) = item.child_by_field_name("interfaces") {
        for base in type_idents(ifaces, src) {
            model.add_rel(&base, &type_name, rel::REALIZE, "");
        }
    }

    let Some(body) = item.child_by_field_name("body") else {
        return;
    };
    let mut c = body.walk();
    for m in body.named_children(&mut c) {
        match m.kind() {
            "field_declaration" => {
                let ty = m.child_by_field_name("type");
                if let Some(decl) = m.child_by_field_name("declarator") {
                    if let Some(name) = decl.child_by_field_name("name") {
                        let tt = ty.map(|t| collapse(t, src)).unwrap_or_default();
                        model.push_member(idx, format!("{}{}: {tt}", vis(m, src), text(name, src)));
                    }
                }
                if let Some(ty) = ty {
                    if let Some(base) = first_type_ident(ty, src) {
                        let label = m
                            .child_by_field_name("declarator")
                            .and_then(|d| d.child_by_field_name("name"))
                            .map(|n| text(n, src).to_string())
                            .unwrap_or_default();
                        comps.push((owner.clone(), base, label));
                    }
                }
            }
            "method_declaration" | "constructor_declaration" => {
                if let Some(sig) = format_method(m, src, vis(m, src)) {
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
    let type_name = text(name_node, src).to_string();
    let Some(idx) = model.box_idx(&type_name) else {
        return;
    };
    model.set_header(
        idx,
        format!(
            "{}interface {}",
            vis_keyword(item, src),
            sanitize_ident(&type_name)
        ),
    );
    if let Some(body) = item.child_by_field_name("body") {
        let mut c = body.walk();
        for m in body.named_children(&mut c) {
            if m.kind() == "method_declaration" {
                if let Some(sig) = format_method(m, src, "+") {
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
    let type_name = text(name_node, src).to_string();
    let Some(idx) = model.box_idx(&type_name) else {
        return;
    };
    model.set_header(
        idx,
        format!(
            "{}enum {}",
            vis_keyword(item, src),
            sanitize_ident(&type_name)
        ),
    );
    if let Some(body) = item.child_by_field_name("body") {
        let mut c = body.walk();
        for e in body.named_children(&mut c) {
            if e.kind() == "enum_constant" {
                if let Some(name) = e.child_by_field_name("name") {
                    model.push_member(idx, text(name, src));
                }
            }
        }
    }
}

fn format_method(m: Node, src: &[u8], marker: &str) -> Option<String> {
    let name = text(m.child_by_field_name("name")?, src);
    let mut ptypes = Vec::new();
    if let Some(params) = m.child_by_field_name("parameters") {
        let mut c = params.walk();
        for p in params.named_children(&mut c) {
            if matches!(p.kind(), "formal_parameter" | "spread_parameter") {
                if let Some(t) = p.child_by_field_name("type") {
                    ptypes.push(collapse(t, src));
                }
            }
        }
    }
    let mut s = format!("{marker}{name}({})", ptypes.join(", "));
    // Constructors have no return type.
    if let Some(ret) = m.child_by_field_name("type") {
        s.push(' ');
        s.push_str(&collapse(ret, src));
    }
    Some(s)
}

/// All `type_identifier`s under a node (for `implements A, B`).
fn type_idents(node: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    collect_type_idents(node, src, &mut out);
    out
}

fn collect_type_idents(node: Node, src: &[u8], out: &mut Vec<String>) {
    let mut c = node.walk();
    for ch in node.named_children(&mut c) {
        if matches!(ch.kind(), "type_identifier" | "scoped_type_identifier") {
            let s = sanitize_ident(text(ch, src));
            if !s.is_empty() {
                out.push(s);
            }
        } else {
            collect_type_idents(ch, src, out);
        }
    }
}

fn first_type_ident(node: Node, src: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" | "scoped_type_identifier" => {
            let s = sanitize_ident(text(node, src));
            (!s.is_empty()).then_some(s)
        }
        _ => {
            let mut c = node.walk();
            let found = node
                .named_children(&mut c)
                .find_map(|ch| first_type_ident(ch, src));
            found
        }
    }
}

/// Type-level visibility keyword with a trailing space (`public `, `private `,
/// `protected `), or empty for package-private.
fn vis_keyword(item: Node, src: &[u8]) -> &'static str {
    match vis(item, src) {
        "+" => "public ",
        "#" => "protected ",
        "-" => "private ",
        _ => "",
    }
}

/// Visibility from the `modifiers` child; package-private (`~`) by default.
fn vis(member: Node, src: &[u8]) -> &'static str {
    let mut c = member.walk();
    let modifiers = member
        .named_children(&mut c)
        .find(|n| n.kind() == "modifiers");
    match modifiers.map(|m| text(m, src)) {
        Some(t) if t.contains("private") => "-",
        Some(t) if t.contains("protected") => "#",
        Some(t) if t.contains("public") => "+",
        _ => "~",
    }
}
