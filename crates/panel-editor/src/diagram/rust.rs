//! Rich Rust extractor: walks the tree-sitter AST to recover visibility,
//! field types, method signatures, enum variants, module-level items, and
//! trait/composition relationships.

use std::path::Path;

use tree_sitter::{Node, Parser};

use super::model::{
    collapse as type_text, module_label, module_name, node_text as text, rel, sanitize_ident, Model,
};

pub(crate) fn generate(source: &str, file_path: Option<&Path>) -> Option<String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(source, None)?;
    let src = source.as_bytes();

    let module = module_name(file_path);
    let mut model = Model::new();
    model.set_module_label(module_label(file_path));
    // Composition candidates (owner, referenced base type, edge label). Resolved
    // to edges only for base types declared in this file.
    let mut comps: Vec<(String, String, String)> = Vec::new();
    let mut module_idx: Option<usize> = None;

    walk_items(
        tree.root_node(),
        src,
        &mut model,
        &module,
        &mut module_idx,
        &mut comps,
    );

    model.resolve_compositions(comps);

    model.render()
}

fn walk_items(
    node: Node,
    src: &[u8],
    model: &mut Model,
    module: &str,
    module_idx: &mut Option<usize>,
    comps: &mut Vec<(String, String, String)>,
) {
    let mut c = node.walk();
    for item in node.named_children(&mut c) {
        match item.kind() {
            "struct_item" | "union_item" => handle_struct(item, src, model, comps),
            "enum_item" => handle_enum(item, src, model, comps),
            "trait_item" => handle_trait(item, src, model),
            "impl_item" => handle_impl(item, src, model),
            "function_item" => {
                if let Some(sig) = format_fn(item, src, vis_marker(item, src)) {
                    let idx = module_box(model, module, module_idx);
                    model.push_member(idx, sig);
                }
            }
            "const_item" | "static_item" => {
                if let Some(line) = format_const(item, src) {
                    let idx = module_box(model, module, module_idx);
                    model.push_member(idx, line);
                }
            }
            "type_item" => {
                if let Some(name) = item.child_by_field_name("name") {
                    let idx = module_box(model, module, module_idx);
                    model.push_member(
                        idx,
                        format!("{}type {}", vis_marker(item, src), text(name, src)),
                    );
                }
            }
            "macro_definition" => {
                if let Some(name) = item.child_by_field_name("name") {
                    let idx = module_box(model, module, module_idx);
                    model.push_member(idx, format!("+{}!", text(name, src)));
                }
            }
            "mod_item" => {
                if let Some(body) = item.child_by_field_name("body") {
                    walk_items(body, src, model, module, module_idx, comps);
                }
            }
            _ => {}
        }
    }
}

fn handle_struct(
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
    let kw = if item.kind() == "union_item" {
        "union"
    } else {
        "struct"
    };
    model.set_header(
        idx,
        format!(
            "{}{kw} {}",
            vis_keyword(item, src),
            sanitize_ident(&type_name)
        ),
    );
    let owner = sanitize_ident(&type_name);
    let Some(body) = item.child_by_field_name("body") else {
        return;
    };
    match body.kind() {
        "field_declaration_list" => {
            let mut c = body.walk();
            for f in body.named_children(&mut c) {
                if f.kind() != "field_declaration" {
                    continue;
                }
                if let Some(line) = format_field(f, src) {
                    model.push_member(idx, line);
                }
                if let Some(ty) = f.child_by_field_name("type") {
                    if let Some(base) = base_type_ident(ty, src) {
                        let label = f
                            .child_by_field_name("name")
                            .map(|n| text(n, src).to_string())
                            .unwrap_or_default();
                        comps.push((owner.clone(), base, label));
                    }
                }
            }
        }
        "ordered_field_declaration_list" => {
            // Tuple struct: ordered types, optional per-field visibility.
            let mut c = body.walk();
            let mut vis = "-";
            let mut i = 0usize;
            for ch in body.named_children(&mut c) {
                match ch.kind() {
                    "visibility_modifier" => vis = vis_from(ch, src),
                    "attribute_item" => {}
                    _ => {
                        model.push_member(idx, format!("{vis}{i}: {}", type_text(ch, src)));
                        if let Some(base) = base_type_ident(ch, src) {
                            comps.push((owner.clone(), base, String::new()));
                        }
                        i += 1;
                        vis = "-";
                    }
                }
            }
        }
        _ => {}
    }
}

fn handle_enum(
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
            "{}enum {}",
            vis_keyword(item, src),
            sanitize_ident(&type_name)
        ),
    );
    let owner = sanitize_ident(&type_name);
    let Some(body) = item.child_by_field_name("body") else {
        return;
    };
    let mut c = body.walk();
    for v in body.named_children(&mut c) {
        if v.kind() != "enum_variant" {
            continue;
        }
        let Some(vn) = v.child_by_field_name("name") else {
            continue;
        };
        let vname = text(vn, src);
        let mut line = vname.to_string();
        if let Some(vbody) = v.child_by_field_name("body") {
            let mut parts = Vec::new();
            let mut cc = vbody.walk();
            match vbody.kind() {
                "ordered_field_declaration_list" => {
                    for n in vbody.named_children(&mut cc) {
                        if matches!(n.kind(), "visibility_modifier" | "attribute_item") {
                            continue;
                        }
                        parts.push(type_text(n, src));
                        if let Some(base) = base_type_ident(n, src) {
                            comps.push((owner.clone(), base, vname.to_string()));
                        }
                    }
                }
                "field_declaration_list" => {
                    for f in vbody.named_children(&mut cc) {
                        if f.kind() != "field_declaration" {
                            continue;
                        }
                        let fname = f
                            .child_by_field_name("name")
                            .map(|n| text(n, src))
                            .unwrap_or("");
                        let fty = f
                            .child_by_field_name("type")
                            .map(|t| type_text(t, src))
                            .unwrap_or_default();
                        parts.push(format!("{fname}: {fty}"));
                        if let Some(ty) = f.child_by_field_name("type") {
                            if let Some(base) = base_type_ident(ty, src) {
                                comps.push((owner.clone(), base, vname.to_string()));
                            }
                        }
                    }
                }
                _ => {}
            }
            line = format!("{vname}({})", parts.join(", "));
        }
        model.push_member(idx, line);
    }
}

fn handle_trait(item: Node, src: &[u8], model: &mut Model) {
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
            "{}trait {}",
            vis_keyword(item, src),
            sanitize_ident(&type_name)
        ),
    );
    if let Some(body) = item.child_by_field_name("body") {
        let mut c = body.walk();
        for m in body.named_children(&mut c) {
            if matches!(m.kind(), "function_item" | "function_signature_item") {
                if let Some(sig) = format_fn(m, src, "+") {
                    model.push_member(idx, sig);
                }
            }
        }
    }
}

fn handle_impl(item: Node, src: &[u8], model: &mut Model) {
    let Some(type_node) = item.child_by_field_name("type") else {
        return;
    };
    let Some(type_name) = base_type_ident(type_node, src) else {
        return;
    };
    let Some(idx) = model.box_idx(&type_name) else {
        return;
    };
    let trait_impl = item.child_by_field_name("trait");
    if let Some(tr) = trait_impl {
        if let Some(trait_name) = base_type_ident(tr, src) {
            model.add_rel(&trait_name, &type_name, rel::REALIZE, "impl");
        }
    }
    if let Some(body) = item.child_by_field_name("body") {
        let mut c = body.walk();
        for m in body.named_children(&mut c) {
            if m.kind() != "function_item" {
                continue;
            }
            // Trait-impl methods are public through the trait; inherent methods
            // keep their own visibility.
            let marker = if trait_impl.is_some() {
                "+"
            } else {
                vis_marker(m, src)
            };
            if let Some(sig) = format_fn(m, src, marker) {
                model.push_member(idx, sig);
            }
        }
    }
}

/// Get-or-create the file-level box for free items (cached in `module_idx`).
fn module_box(model: &mut Model, module: &str, module_idx: &mut Option<usize>) -> usize {
    if let Some(i) = *module_idx {
        return i;
    }
    let i = model.module_box(module).unwrap_or(0);
    *module_idx = Some(i);
    i
}

/// Rust type-level visibility keyword with a trailing space (`pub `,
/// `pub(crate) `), or empty for private.
fn vis_keyword(item: Node, src: &[u8]) -> String {
    let mut c = item.walk();
    let v = item
        .named_children(&mut c)
        .find(|n| n.kind() == "visibility_modifier");
    match v {
        Some(v) => format!("{} ", type_text(v, src)),
        None => String::new(),
    }
}

fn format_fn(f: Node, src: &[u8], marker: &str) -> Option<String> {
    let name = text(f.child_by_field_name("name")?, src);
    let mut ptypes = Vec::new();
    if let Some(params) = f.child_by_field_name("parameters") {
        let mut c = params.walk();
        for p in params.named_children(&mut c) {
            match p.kind() {
                "parameter" => {
                    if let Some(ty) = p.child_by_field_name("type") {
                        ptypes.push(type_text(ty, src));
                    }
                }
                "variadic_parameter" => ptypes.push("...".to_string()),
                _ => {} // self_parameter and attributes skipped
            }
        }
    }
    let mut s = format!("{marker}{name}({})", ptypes.join(", "));
    if let Some(ret) = f.child_by_field_name("return_type") {
        s.push(' ');
        s.push_str(&type_text(ret, src));
    }
    Some(s)
}

fn format_field(f: Node, src: &[u8]) -> Option<String> {
    let name = text(f.child_by_field_name("name")?, src);
    let ty = f
        .child_by_field_name("type")
        .map(|t| type_text(t, src))
        .unwrap_or_default();
    Some(format!("{}{name}: {ty}", vis_marker(f, src)))
}

fn format_const(item: Node, src: &[u8]) -> Option<String> {
    let name = text(item.child_by_field_name("name")?, src);
    let ty = item
        .child_by_field_name("type")
        .map(|t| type_text(t, src))
        .unwrap_or_default();
    let kw = if item.kind() == "static_item" {
        "static"
    } else {
        "const"
    };
    Some(format!("{}{kw} {name}: {ty}", vis_marker(item, src)))
}

/// The base named type of a type node, descending through references and
/// generics (`&mut Vec<Point>` -> `Vec`). Returns `None` for primitives,
/// tuples, and other anonymous types.
fn base_type_ident(n: Node, src: &[u8]) -> Option<String> {
    match n.kind() {
        "type_identifier" | "scoped_type_identifier" => {
            let s = sanitize_ident(text(n, src));
            (!s.is_empty()).then_some(s)
        }
        "generic_type" => n
            .child_by_field_name("type")
            .and_then(|t| base_type_ident(t, src)),
        "reference_type" | "pointer_type" => {
            let mut c = n.walk();
            let found = n
                .named_children(&mut c)
                .find_map(|ch| base_type_ident(ch, src));
            found
        }
        _ => None,
    }
}

/// Visibility marker for an item: `+` public, `~` restricted (`pub(crate)` …),
/// `-` private.
fn vis_marker(item: Node, src: &[u8]) -> &'static str {
    let mut c = item.walk();
    let vis = item
        .named_children(&mut c)
        .find(|ch| ch.kind() == "visibility_modifier");
    vis.map(|v| vis_from(v, src)).unwrap_or("-")
}

fn vis_from(vis_node: Node, src: &[u8]) -> &'static str {
    if text(vis_node, src) == "pub" {
        "+"
    } else {
        "~"
    }
}
