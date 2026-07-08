//! Rich Kotlin extractor: classes/interfaces/objects/enums with properties and
//! function signatures, primary-constructor `val`/`var` properties,
//! superclass/interface relationships, and top-level functions/properties/
//! typealiases in the file box.
//!
//! `class_declaration` is reused by the grammar for classes, interfaces and
//! enum classes (the keyword is an anonymous token / error-recovery node), so
//! the header keyword is recovered by inspecting child tokens and
//! `class_modifier`s. Visibility defaults to public (`+`); an interface body is
//! parsed under an `ERROR` node, so member scanning flattens one `ERROR` level.

use std::path::Path;

use tree_sitter::{Node, Parser};

use super::model::{
    collapse, module_label, module_name, node_text as text, rel, sanitize_ident, Model,
};

pub(crate) fn generate(source: &str, file_path: Option<&Path>) -> Option<String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_kotlin_ng::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(source, None)?;
    let src = source.as_bytes();

    let module = module_name(file_path);
    let mut model = Model::new();
    model.set_module_label(module_label(file_path));
    let mut comps: Vec<(String, String, String)> = Vec::new();
    let mut module_idx: Option<usize> = None;

    let mut c = tree.root_node().walk();
    for item in tree.root_node().named_children(&mut c) {
        match item.kind() {
            "class_declaration" => handle_class(item, src, &mut model, &mut comps),
            "object_declaration" => handle_object(item, src, &mut model, &mut comps),
            "function_declaration" => {
                if let Some(sig) = format_fn(item, src) {
                    let idx = module_box(&mut model, &module, &mut module_idx);
                    model.push_member(idx, sig);
                }
            }
            "property_declaration" => {
                if let Some((line, _)) = property(item, src) {
                    let idx = module_box(&mut model, &module, &mut module_idx);
                    model.push_member(idx, line);
                }
            }
            "type_alias" => {
                if let Some(n) = item.child_by_field_name("type") {
                    let idx = module_box(&mut model, &module, &mut module_idx);
                    model.push_member(idx, format!("+type {}", text(n, src)));
                }
            }
            _ => {}
        }
    }

    model.resolve_compositions(comps);
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
    model.set_header(idx, class_header(item, src, &type_name));
    let owner = sanitize_ident(&type_name);

    let mut c = item.walk();
    for ch in item.named_children(&mut c) {
        match ch.kind() {
            "primary_constructor" => handle_primary_ctor(ch, idx, &owner, src, model, comps),
            "delegation_specifiers" => handle_delegation(ch, &type_name, src, model),
            "class_body" | "enum_class_body" => handle_body(ch, idx, &owner, src, model, comps),
            _ => {}
        }
    }
}

fn handle_object(
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
            "{}object {}",
            vis_keyword(item, src),
            sanitize_ident(&type_name)
        ),
    );
    let owner = sanitize_ident(&type_name);

    let mut c = item.walk();
    for ch in item.named_children(&mut c) {
        match ch.kind() {
            "delegation_specifiers" => handle_delegation(ch, &type_name, src, model),
            "class_body" | "enum_class_body" => handle_body(ch, idx, &owner, src, model, comps),
            _ => {}
        }
    }
}

/// Get-or-create the file-level box (cached in `module_idx`).
fn module_box(model: &mut Model, module: &str, module_idx: &mut Option<usize>) -> usize {
    if let Some(i) = *module_idx {
        return i;
    }
    let i = model.module_box(module).unwrap_or(0);
    *module_idx = Some(i);
    i
}

/// `val`/`var` primary-constructor parameters are properties; plain parameters
/// are not fields and are skipped.
fn handle_primary_ctor(
    pc: Node,
    idx: usize,
    owner: &str,
    src: &[u8],
    model: &mut Model,
    comps: &mut Vec<(String, String, String)>,
) {
    let mut c = pc.walk();
    for params in pc.named_children(&mut c) {
        if params.kind() != "class_parameters" {
            continue;
        }
        let mut pc2 = params.walk();
        for p in params.named_children(&mut pc2) {
            if p.kind() != "class_parameter" || !is_val_or_var(p) {
                continue;
            }
            let Some(name) = p.named_child(0) else {
                continue;
            };
            let ty = named_child_of_kind(p, "user_type");
            let vis = member_vis(p, src);
            let line = match ty {
                Some(t) => format!("{vis}{}: {}", text(name, src), collapse(t, src)),
                None => format!("{vis}{}", text(name, src)),
            };
            model.push_member(idx, line);
            if let Some(base) = ty.and_then(|t| type_base(t, src)) {
                comps.push((owner.to_string(), base, text(name, src).to_string()));
            }
        }
    }
}

/// `: Base(), Iface` — a `constructor_invocation` is a superclass (inherit); a
/// bare `user_type` is an implemented interface (realize).
fn handle_delegation(ds: Node, type_name: &str, src: &[u8], model: &mut Model) {
    let mut c = ds.walk();
    for spec in ds.named_children(&mut c) {
        if spec.kind() != "delegation_specifier" {
            continue;
        }
        let Some(inner) = spec.named_child(0) else {
            continue;
        };
        match inner.kind() {
            "constructor_invocation" => {
                if let Some(base) =
                    named_child_of_kind(inner, "user_type").and_then(|ut| type_base(ut, src))
                {
                    model.add_rel(&base, type_name, rel::INHERIT, "");
                }
            }
            "user_type" => {
                if let Some(iface) = type_base(inner, src) {
                    model.add_rel(&iface, type_name, rel::REALIZE, "");
                }
            }
            _ => {}
        }
    }
}

fn handle_body(
    body: Node,
    idx: usize,
    owner: &str,
    src: &[u8],
    model: &mut Model,
    comps: &mut Vec<(String, String, String)>,
) {
    for m in body_members(body) {
        match m.kind() {
            "property_declaration" => {
                if let Some((line, ty)) = property(m, src) {
                    model.push_member(idx, line);
                    if let Some(base) = ty.and_then(|t| type_base(t, src)) {
                        comps.push((owner.to_string(), base, String::new()));
                    }
                }
            }
            "function_declaration" => {
                if let Some(sig) = format_fn(m, src) {
                    model.push_member(idx, sig);
                }
            }
            "enum_entry" => {
                if let Some(n) = m.named_child(0) {
                    model.push_member(idx, text(n, src));
                }
            }
            _ => {}
        }
    }
}

/// Member nodes of a class/enum body, flattening one level of `ERROR` (an
/// interface body parses as `enum_class_body(ERROR(function_declaration …))`).
fn body_members(body: Node) -> Vec<Node> {
    let mut out = Vec::new();
    let mut c = body.walk();
    for ch in body.named_children(&mut c) {
        if ch.kind() == "ERROR" {
            let mut c2 = ch.walk();
            for g in ch.named_children(&mut c2) {
                out.push(g);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// `(name: Type)` for a property declaration, plus its type node for
/// composition. `None` when the declaration has no binding name.
fn property<'a>(prop: Node<'a>, src: &[u8]) -> Option<(String, Option<Node<'a>>)> {
    let vd = named_child_of_kind(prop, "variable_declaration")?;
    let name = vd.named_child(0)?;
    let ty = named_child_of_kind(vd, "user_type");
    let vis = member_vis(prop, src);
    let line = match ty {
        Some(t) => format!("{vis}{}: {}", text(name, src), collapse(t, src)),
        None => format!("{vis}{}", text(name, src)),
    };
    Some((line, ty))
}

fn format_fn(func: Node, src: &[u8]) -> Option<String> {
    let name = text(func.child_by_field_name("name")?, src);
    let vis = member_vis(func, src);
    let mut ptypes = Vec::new();
    if let Some(params) = named_child_of_kind(func, "function_value_parameters") {
        let mut c = params.walk();
        for p in params.named_children(&mut c) {
            if p.kind() == "parameter" {
                match named_child_of_kind(p, "user_type") {
                    Some(t) => ptypes.push(collapse(t, src)),
                    None => {
                        if let Some(n) = p.named_child(0) {
                            ptypes.push(text(n, src).to_string());
                        }
                    }
                }
            }
        }
    }
    let mut s = format!("{vis}{name}({})", ptypes.join(", "));
    // The return type is a `user_type` directly under the declaration (parameter
    // types are nested inside `function_value_parameters`, so they don't match).
    if let Some(ret) = named_child_of_kind(func, "user_type") {
        s.push(' ');
        s.push_str(&collapse(ret, src));
    }
    Some(s)
}

/// Declaration header keyword(s): `interface`, `enum class`, `data class`,
/// `sealed class`, `annotation class`, or plain `class`.
fn class_header(item: Node, src: &[u8], type_name: &str) -> String {
    let vis = vis_keyword(item, src);
    let kw = class_keyword(item);
    format!("{vis}{kw} {}", sanitize_ident(type_name))
}

fn class_keyword(item: Node) -> String {
    // The `interface` keyword is an anonymous direct child token.
    for i in 0..item.child_count() {
        if item.child(i).map(|c| c.kind()) == Some("interface") {
            return "interface".to_string();
        }
    }
    let prefix = class_modifier_kind(item)
        .map(|m| match m {
            "enum" => "enum ",
            "data" => "data ",
            "sealed" => "sealed ",
            "annotation" => "annotation ",
            _ => "",
        })
        .unwrap_or("");
    format!("{prefix}class")
}

/// The kind of a `class_modifier` token (`enum`/`data`/`sealed`/…) if present.
fn class_modifier_kind(item: Node) -> Option<&'static str> {
    let modifiers = named_child_of_kind(item, "modifiers")?;
    let cm = named_child_of_kind(modifiers, "class_modifier")?;
    let tok = cm.child(0)?;
    match tok.kind() {
        "enum" => Some("enum"),
        "data" => Some("data"),
        "sealed" => Some("sealed"),
        "annotation" => Some("annotation"),
        _ => None,
    }
}

/// Whether a `class_parameter` is a `val`/`var` (property) parameter.
fn is_val_or_var(p: Node) -> bool {
    (0..p.child_count()).any(|i| matches!(p.child(i).map(|c| c.kind()), Some("val") | Some("var")))
}

/// First direct named child of a given kind.
fn named_child_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut c = node.walk();
    let found = node.named_children(&mut c).find(|ch| ch.kind() == kind);
    found
}

/// Base identifier of a type node: its first `identifier` descendant, sanitized
/// (`List<Point>` -> `List`).
fn type_base(node: Node, src: &[u8]) -> Option<String> {
    if node.kind() == "identifier" {
        let s = sanitize_ident(text(node, src));
        return (!s.is_empty()).then_some(s);
    }
    let mut c = node.walk();
    let found = node
        .named_children(&mut c)
        .find_map(|ch| type_base(ch, src));
    found
}

/// Type-level visibility keyword with a trailing space; empty for the (default)
/// public visibility, which Kotlin omits.
fn vis_keyword(item: Node, src: &[u8]) -> &'static str {
    match member_vis(item, src) {
        "-" => "private ",
        "#" => "protected ",
        "~" => "internal ",
        _ => "",
    }
}

/// Visibility marker from a `modifiers > visibility_modifier`; public (`+`) by
/// default (Kotlin's implicit visibility).
fn member_vis(node: Node, src: &[u8]) -> &'static str {
    let Some(modifiers) = named_child_of_kind(node, "modifiers") else {
        return "+";
    };
    let t = text(modifiers, src);
    if t.contains("private") {
        "-"
    } else if t.contains("protected") {
        "#"
    } else if t.contains("internal") {
        "~"
    } else {
        "+"
    }
}
