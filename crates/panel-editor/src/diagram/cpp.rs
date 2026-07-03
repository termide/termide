//! Rich C++ extractor: classes/structs with fields and methods (tracking
//! `public:`/`private:`/`protected:` sections), base-class inheritance, enums,
//! and free functions.

use std::collections::HashSet;
use std::path::Path;

use tree_sitter::{Node, Parser};

use super::model::{
    collapse, module_label, module_name, node_text as text, rel, sanitize_ident, Model,
};

pub(crate) fn generate(source: &str, file_path: Option<&Path>) -> Option<String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(source, None)?;
    let src = source.as_bytes();

    let module = module_name(file_path);
    let mut model = Model::new();
    model.set_module_label(module_label(file_path));
    let mut comps: Vec<(String, String, String)> = Vec::new();

    let mut c = tree.root_node().walk();
    for item in tree.root_node().named_children(&mut c) {
        match item.kind() {
            "class_specifier" | "struct_specifier" | "union_specifier" => {
                handle_record(item, src, &mut model, &mut comps)
            }
            "enum_specifier" => handle_enum(item, src, &mut model),
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

fn handle_record(
    item: Node,
    src: &[u8],
    model: &mut Model,
    comps: &mut Vec<(String, String, String)>,
) {
    let Some(name_node) = item.child_by_field_name("name") else {
        return; // anonymous record
    };
    let type_name = text(name_node, src).to_string();
    let Some(idx) = model.box_idx(&type_name) else {
        return;
    };
    let kw = match item.kind() {
        "struct_specifier" => "struct",
        "union_specifier" => "union",
        _ => "class",
    };
    model.set_header(idx, format!("{kw} {}", sanitize_ident(&type_name)));
    let owner = sanitize_ident(&type_name);

    // Base classes: `class C : public Base { ... }`.
    let mut hc = item.walk();
    for ch in item.named_children(&mut hc) {
        if ch.kind() == "base_class_clause" {
            for base in type_idents(ch, src) {
                model.add_rel(&base, &type_name, rel::INHERIT, "");
            }
        }
    }

    let Some(body) = item.child_by_field_name("body") else {
        return;
    };
    // Structs default to public, classes to private.
    let mut current_vis = if item.kind() == "class_specifier" {
        "-"
    } else {
        "+"
    };
    let mut c = body.walk();
    for m in body.named_children(&mut c) {
        match m.kind() {
            "access_specifier" => current_vis = access_vis(m, src),
            "field_declaration" => {
                let ty = m.child_by_field_name("type");
                let decl = m.child_by_field_name("declarator");
                if let Some(decl) = decl {
                    if let Some(fdecl) = as_function_declarator(decl) {
                        // Member function declaration.
                        let name = fdecl
                            .child_by_field_name("declarator")
                            .map(|d| innermost_ident(d, src))
                            .unwrap_or_default();
                        let ptypes = param_types(fdecl, src);
                        let ret = ty.map(|t| collapse(t, src)).unwrap_or_default();
                        let mut s = format!("{current_vis}{name}({})", ptypes.join(", "));
                        if !ret.is_empty() {
                            s.push(' ');
                            s.push_str(&ret);
                        }
                        model.push_member(idx, s);
                    } else {
                        // Data member.
                        let name = innermost_ident(decl, src);
                        let tt = ty.map(|t| collapse(t, src)).unwrap_or_default();
                        model.push_member(idx, format!("{current_vis}{name}: {tt}"));
                        if let Some(ty) = ty {
                            if let Some(base) = base_ident(ty, src) {
                                comps.push((owner.clone(), base, name));
                            }
                        }
                    }
                }
            }
            _ => {}
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
    model.set_header(idx, format!("enum {}", sanitize_ident(&type_name)));
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
    let fdecl = as_function_declarator(decl)?;
    let name = fdecl
        .child_by_field_name("declarator")
        .map(|d| innermost_ident(d, src))
        .unwrap_or_default();
    if name.is_empty() {
        return None;
    }
    let ret = item
        .child_by_field_name("type")
        .map(|t| collapse(t, src))
        .unwrap_or_default();
    let mut s = format!("+{name}({})", param_types(fdecl, src).join(", "));
    if !ret.is_empty() {
        s.push(' ');
        s.push_str(&ret);
    }
    Some(s)
}

fn param_types(fdecl: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(params) = fdecl.child_by_field_name("parameters") {
        let mut c = params.walk();
        for p in params.named_children(&mut c) {
            if p.kind() == "parameter_declaration" {
                if let Some(t) = p.child_by_field_name("type") {
                    out.push(collapse(t, src));
                }
            }
        }
    }
    out
}

fn as_function_declarator(node: Node) -> Option<Node> {
    if node.kind() == "function_declarator" {
        return Some(node);
    }
    // Peel reference/pointer declarators.
    node.child_by_field_name("declarator")
        .and_then(as_function_declarator)
}

fn innermost_ident(node: Node, src: &[u8]) -> String {
    match node.kind() {
        "identifier" | "field_identifier" | "type_identifier" => text(node, src).to_string(),
        _ => node
            .child_by_field_name("declarator")
            .map(|d| innermost_ident(d, src))
            .unwrap_or_default(),
    }
}

fn type_idents(node: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut c = node.walk();
    for ch in node.named_children(&mut c) {
        if matches!(ch.kind(), "type_identifier" | "qualified_identifier") {
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
        "type_identifier" => {
            let s = sanitize_ident(text(n, src));
            (!s.is_empty()).then_some(s)
        }
        "struct_specifier" | "union_specifier" | "class_specifier" => n
            .child_by_field_name("name")
            .and_then(|x| base_ident(x, src)),
        _ => None,
    }
}

fn access_vis(node: Node, src: &[u8]) -> &'static str {
    match text(node, src) {
        t if t.contains("private") => "-",
        t if t.contains("protected") => "#",
        _ => "+",
    }
}
