//! Rich Python extractor: classes with methods and annotated attributes,
//! base-class inheritance edges, and module-level functions/constants.
//!
//! Visibility follows the naming convention: a leading underscore marks a
//! member private (`-`), everything else public (`+`).

use std::collections::HashSet;
use std::path::Path;

use tree_sitter::{Node, Parser};

use super::model::{collapse, module_name, node_text as text, rel, sanitize_ident, Model};

pub(crate) fn generate(source: &str, file_path: Option<&Path>) -> Option<String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(source, None)?;
    let src = source.as_bytes();

    let module = module_name(file_path);
    let mut model = Model::new();
    let mut comps: Vec<(String, String, String)> = Vec::new();
    let mut module_idx: Option<usize> = None;

    walk(
        tree.root_node(),
        src,
        &mut model,
        &module,
        &mut module_idx,
        &mut comps,
    );

    let local: HashSet<String> = model.local_type_names().into_iter().collect();
    for (owner, base, label) in comps {
        if owner != base && local.contains(&base) {
            model.add_rel(&owner, &base, rel::COMPOSE, &label);
        }
    }
    model.render()
}

fn walk(
    node: Node,
    src: &[u8],
    model: &mut Model,
    module: &str,
    module_idx: &mut Option<usize>,
    comps: &mut Vec<(String, String, String)>,
) {
    let mut c = node.walk();
    for item in node.named_children(&mut c) {
        // Unwrap `@decorator`-wrapped defs to the definition they decorate.
        let item = if item.kind() == "decorated_definition" {
            match item.child_by_field_name("definition") {
                Some(d) => d,
                None => continue,
            }
        } else {
            item
        };
        match item.kind() {
            "class_definition" => handle_class(item, src, model, comps),
            "function_definition" => {
                let name = item.child_by_field_name("name").map(|n| text(n, src));
                if let Some(sig) = format_fn(item, src, vis_of(name.unwrap_or(""))) {
                    let idx = module_box(model, module, module_idx);
                    model.push_member(idx, sig);
                }
            }
            "expression_statement" => {
                if let Some(line) = module_assignment(item, src) {
                    let idx = module_box(model, module, module_idx);
                    model.push_member(idx, line);
                }
            }
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
    let owner = sanitize_ident(&type_name);

    // Base classes -> inheritance edges.
    if let Some(bases) = item.child_by_field_name("superclasses") {
        let mut c = bases.walk();
        for b in bases.named_children(&mut c) {
            if let Some(base) = base_ident(b, src) {
                model.add_rel(&base, &type_name, rel::INHERIT, "");
            }
        }
    }

    let Some(body) = item.child_by_field_name("body") else {
        return;
    };
    let mut c = body.walk();
    for stmt in body.named_children(&mut c) {
        let stmt = if stmt.kind() == "decorated_definition" {
            match stmt.child_by_field_name("definition") {
                Some(d) => d,
                None => continue,
            }
        } else {
            stmt
        };
        match stmt.kind() {
            "function_definition" => {
                let name = stmt.child_by_field_name("name").map(|n| text(n, src));
                if let Some(sig) = format_fn(stmt, src, vis_of(name.unwrap_or(""))) {
                    model.push_member(idx, sig);
                }
            }
            "expression_statement" => {
                if let Some((line, ty)) = class_attribute(stmt, src) {
                    model.push_member(idx, line);
                    if let Some(base) = ty {
                        comps.push((owner.clone(), base, String::new()));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Get-or-create the synthetic `<<module>>` box.
fn module_box(model: &mut Model, module: &str, module_idx: &mut Option<usize>) -> usize {
    if let Some(i) = *module_idx {
        return i;
    }
    let i = model.box_idx(module).unwrap_or(0);
    model.set_stereotype(i, "module");
    *module_idx = Some(i);
    i
}

fn format_fn(f: Node, src: &[u8], marker: &str) -> Option<String> {
    let name = text(f.child_by_field_name("name")?, src);
    let mut params = Vec::new();
    if let Some(ps) = f.child_by_field_name("parameters") {
        let mut c = ps.walk();
        for p in ps.named_children(&mut c) {
            if let Some(repr) = param_repr(p, src) {
                params.push(repr);
            }
        }
    }
    let mut s = format!("{marker}{name}({})", params.join(", "));
    if let Some(ret) = f.child_by_field_name("return_type") {
        s.push(' ');
        s.push_str(&collapse(ret, src));
    }
    Some(s)
}

/// Render a parameter as its type annotation when present, else its name.
/// `self`/`cls` receivers are skipped.
fn param_repr(p: Node, src: &[u8]) -> Option<String> {
    let (name, ty): (&str, Option<Node>) = match p.kind() {
        "identifier" => (text(p, src), None),
        "typed_parameter" => {
            let nm = p.named_child(0).map(|n| text(n, src)).unwrap_or("");
            (nm, p.child_by_field_name("type"))
        }
        "default_parameter" => (
            p.child_by_field_name("name")
                .map(|n| text(n, src))
                .unwrap_or(""),
            None,
        ),
        "typed_default_parameter" => (
            p.child_by_field_name("name")
                .map(|n| text(n, src))
                .unwrap_or(""),
            p.child_by_field_name("type"),
        ),
        "list_splat_pattern" | "dictionary_splat_pattern" => return Some(collapse(p, src)),
        _ => return None,
    };
    if name == "self" || name == "cls" {
        return None;
    }
    Some(
        ty.map(|t| collapse(t, src))
            .unwrap_or_else(|| name.to_string()),
    )
}

/// A module-level annotated/plain assignment as a `<<module>>` member line.
fn module_assignment(stmt: Node, src: &[u8]) -> Option<String> {
    let a = stmt.named_child(0)?;
    if a.kind() != "assignment" {
        return None;
    }
    let left = a.child_by_field_name("left")?;
    if left.kind() != "identifier" {
        return None;
    }
    let name = text(left, src);
    let marker = vis_of(name);
    match a.child_by_field_name("type") {
        Some(ty) => Some(format!("{marker}{name}: {}", collapse(ty, src))),
        None => Some(format!("{marker}{name}")),
    }
}

/// A class-body assignment as `(member line, referenced local type)`.
fn class_attribute(stmt: Node, src: &[u8]) -> Option<(String, Option<String>)> {
    let a = stmt.named_child(0)?;
    if a.kind() != "assignment" {
        return None;
    }
    let left = a.child_by_field_name("left")?;
    if left.kind() != "identifier" {
        return None;
    }
    let name = text(left, src);
    let marker = vis_of(name);
    match a.child_by_field_name("type") {
        Some(ty) => Some((
            format!("{marker}{name}: {}", collapse(ty, src)),
            base_ident(ty, src),
        )),
        None => Some((format!("{marker}{name}"), None)),
    }
}

/// Base identifier of a name/type node: the last dotted segment, sanitized
/// (`pkg.mod.Base` -> `Base`, `list[int]` -> `list`).
fn base_ident(n: Node, src: &[u8]) -> Option<String> {
    let t = text(n, src);
    let seg = t.rsplit('.').next().unwrap_or(t);
    let s = sanitize_ident(seg);
    (!s.is_empty()).then_some(s)
}

fn vis_of(name: &str) -> &'static str {
    if name.starts_with('_') {
        "-"
    } else {
        "+"
    }
}
