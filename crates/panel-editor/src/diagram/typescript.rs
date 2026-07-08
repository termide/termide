//! Rich TypeScript extractor: classes (fields + methods with accessibility),
//! interfaces, enums, `extends`/`implements` edges, field composition, and
//! module-level functions/consts/type aliases.

use std::path::Path;

use tree_sitter::{Node, Parser};

use super::model::{
    collapse, module_label, module_name, node_text as text, rel, sanitize_ident, Model,
};

pub(crate) fn generate(source: &str, file_path: Option<&Path>, tsx: bool) -> Option<String> {
    // The TSX grammar also parses plain TS, but the TypeScript grammar rejects
    // JSX — so `.tsx`/`.jsx` must use `LANGUAGE_TSX`.
    let language = if tsx {
        tree_sitter_typescript::LANGUAGE_TSX
    } else {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT
    };
    let mut parser = Parser::new();
    parser.set_language(&language.into()).ok()?;
    let tree = parser.parse(source, None)?;
    let src = source.as_bytes();

    let module = module_name(file_path);
    let mut model = Model::new();
    model.set_module_label(module_label(file_path));
    let mut comps: Vec<(String, String, String)> = Vec::new();
    let mut module_idx: Option<usize> = None;

    let mut c = tree.root_node().walk();
    for child in tree.root_node().named_children(&mut c) {
        // `export <decl>` wraps the declaration; unwrap to it.
        let (decl, exported) = if child.kind() == "export_statement" {
            match child.child_by_field_name("declaration") {
                Some(d) => (d, true),
                None => continue,
            }
        } else {
            (child, false)
        };
        dispatch(
            decl,
            exported,
            src,
            &mut model,
            &module,
            &mut module_idx,
            &mut comps,
        );
    }

    model.resolve_compositions(comps);
    model.render()
}

#[allow(clippy::too_many_arguments)]
fn dispatch(
    decl: Node,
    exported: bool,
    src: &[u8],
    model: &mut Model,
    module: &str,
    module_idx: &mut Option<usize>,
    comps: &mut Vec<(String, String, String)>,
) {
    match decl.kind() {
        "class_declaration" | "abstract_class_declaration" => {
            handle_class(decl, exported, src, model, comps)
        }
        "interface_declaration" => handle_interface(decl, exported, src, model),
        "enum_declaration" => handle_enum(decl, exported, src, model),
        "function_declaration" | "generator_function_declaration" => {
            if let Some(sig) = format_fn(decl, src, "+") {
                let idx = module_box(model, module, module_idx);
                model.push_member(idx, sig);
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            let mut c = decl.walk();
            for d in decl.named_children(&mut c) {
                if d.kind() != "variable_declarator" {
                    continue;
                }
                if let Some(name) = d.child_by_field_name("name") {
                    let idx = module_box(model, module, module_idx);
                    let line = match d.child_by_field_name("type") {
                        Some(ty) => format!("+{}: {}", text(name, src), ann_type(ty, src)),
                        None => format!("+{}", text(name, src)),
                    };
                    model.push_member(idx, line);
                }
            }
        }
        "type_alias_declaration" => {
            if let Some(name) = decl.child_by_field_name("name") {
                let idx = module_box(model, module, module_idx);
                model.push_member(idx, format!("+type {}", text(name, src)));
            }
        }
        _ => {}
    }
}

fn handle_class(
    decl: Node,
    exported: bool,
    src: &[u8],
    model: &mut Model,
    comps: &mut Vec<(String, String, String)>,
) {
    let Some(name_node) = decl.child_by_field_name("name") else {
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
            export_kw(exported),
            sanitize_ident(&type_name)
        ),
    );
    let owner = sanitize_ident(&type_name);

    // Heritage: `extends Base` (inherit), `implements Iface` (realize).
    let mut hc = decl.walk();
    for h in decl.named_children(&mut hc) {
        if h.kind() != "class_heritage" {
            continue;
        }
        let mut c = h.walk();
        for clause in h.named_children(&mut c) {
            match clause.kind() {
                "extends_clause" => {
                    let mut cc = clause.walk();
                    for t in clause.named_children(&mut cc) {
                        if let Some(base) = base_ident(t, src) {
                            model.add_rel(&base, &type_name, rel::INHERIT, "");
                        }
                    }
                }
                "implements_clause" => {
                    let mut cc = clause.walk();
                    for t in clause.named_children(&mut cc) {
                        if let Some(iface) = base_ident(t, src) {
                            model.add_rel(&iface, &type_name, rel::REALIZE, "");
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let Some(body) = decl.child_by_field_name("body") else {
        return;
    };
    let mut c = body.walk();
    for m in body.named_children(&mut c) {
        match m.kind() {
            "public_field_definition" => {
                let Some(name) = m.child_by_field_name("name") else {
                    continue;
                };
                let marker = member_vis(m, name, src);
                let line = match m.child_by_field_name("type") {
                    Some(ty) => format!("{marker}{}: {}", text(name, src), ann_type(ty, src)),
                    None => format!("{marker}{}", text(name, src)),
                };
                model.push_member(idx, line);
                if let Some(ty) = m.child_by_field_name("type") {
                    if let Some(base) = base_ident_from_annotation(ty, src) {
                        comps.push((owner.clone(), base, text(name, src).to_string()));
                    }
                }
            }
            "method_definition" => {
                let name = m.child_by_field_name("name");
                let marker = name.map(|n| member_vis(m, n, src)).unwrap_or("+");
                if let Some(sig) = format_fn(m, src, marker) {
                    model.push_member(idx, sig);
                }
            }
            _ => {}
        }
    }
}

fn handle_interface(decl: Node, exported: bool, src: &[u8], model: &mut Model) {
    let Some(name_node) = decl.child_by_field_name("name") else {
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
            export_kw(exported),
            sanitize_ident(&type_name)
        ),
    );

    // `interface X extends A, B`
    let mut hc = decl.walk();
    for h in decl.named_children(&mut hc) {
        if h.kind() == "extends_type_clause" {
            let mut c = h.walk();
            for t in h.named_children(&mut c) {
                if let Some(base) = base_ident(t, src) {
                    model.add_rel(&base, &type_name, rel::INHERIT, "");
                }
            }
        }
    }

    let Some(body) = decl.child_by_field_name("body") else {
        return;
    };
    let mut c = body.walk();
    for m in body.named_children(&mut c) {
        match m.kind() {
            "method_signature" => {
                if let Some(sig) = format_fn(m, src, "+") {
                    model.push_member(idx, sig);
                }
            }
            "property_signature" => {
                let Some(name) = m.child_by_field_name("name") else {
                    continue;
                };
                let line = match m.child_by_field_name("type") {
                    Some(ty) => format!("+{}: {}", text(name, src), ann_type(ty, src)),
                    None => format!("+{}", text(name, src)),
                };
                model.push_member(idx, line);
            }
            _ => {}
        }
    }
}

fn handle_enum(decl: Node, exported: bool, src: &[u8], model: &mut Model) {
    let Some(name_node) = decl.child_by_field_name("name") else {
        return;
    };
    let type_name = text(name_node, src).to_string();
    let Some(idx) = model.box_idx(&type_name) else {
        return;
    };
    model.set_header(
        idx,
        format!("{}enum {}", export_kw(exported), sanitize_ident(&type_name)),
    );
    let Some(body) = decl.child_by_field_name("body") else {
        return;
    };
    let mut c = body.walk();
    for v in body.named_children(&mut c) {
        match v.kind() {
            "property_identifier" => model.push_member(idx, text(v, src)),
            "enum_assignment" => {
                if let Some(n) = v.child_by_field_name("name") {
                    model.push_member(idx, text(n, src));
                }
            }
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

/// TypeScript `export ` keyword prefix (the type-level visibility signal).
fn export_kw(exported: bool) -> &'static str {
    if exported {
        "export "
    } else {
        ""
    }
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
        s.push_str(&ann_type(ret, src));
    }
    Some(s)
}

/// A parameter as its type annotation when present, else its binding name.
fn param_repr(p: Node, src: &[u8]) -> Option<String> {
    match p.kind() {
        "required_parameter" | "optional_parameter" => {
            if let Some(ty) = p.child_by_field_name("type") {
                Some(ann_type(ty, src))
            } else {
                p.child_by_field_name("pattern")
                    .map(|n| text(n, src).to_string())
            }
        }
        "rest_parameter" => Some(collapse(p, src)),
        _ => None,
    }
}

/// Visibility marker for a class member from its `accessibility_modifier`
/// child, or a `#private` name. Defaults to public.
fn member_vis(member: Node, name: Node, src: &[u8]) -> &'static str {
    if name.kind() == "private_property_identifier" {
        return "-";
    }
    let mut c = member.walk();
    let modifier = member
        .named_children(&mut c)
        .find(|ch| ch.kind() == "accessibility_modifier");
    match modifier.map(|m| text(m, src)) {
        Some("private") => "-",
        Some("protected") => "#",
        _ => "+",
    }
}

/// Text of a `type_annotation` (`: Foo`) with the leading colon stripped.
fn ann_type(annotation: Node, src: &[u8]) -> String {
    collapse(annotation, src)
        .trim_start_matches(':')
        .trim()
        .to_string()
}

/// Base identifier of a type/heritage node, sanitized (`Array<Point>` ->
/// `Array`, `pkg.Base` -> `Base`).
fn base_ident(n: Node, src: &[u8]) -> Option<String> {
    let t = text(n, src);
    let seg = t.rsplit('.').next().unwrap_or(t);
    let s = sanitize_ident(seg);
    (!s.is_empty()).then_some(s)
}

/// Base identifier from a `type_annotation`, stripping the leading colon first.
fn base_ident_from_annotation(annotation: Node, src: &[u8]) -> Option<String> {
    let s = sanitize_ident(ann_type(annotation, src).trim());
    (!s.is_empty()).then_some(s)
}
