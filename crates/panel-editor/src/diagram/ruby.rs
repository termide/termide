//! Rich Ruby extractor: classes/modules with methods and superclass
//! inheritance, plus module-level methods. Ruby is dynamically typed and has no
//! field declarations, so members are method names only.

use std::path::Path;

use tree_sitter::{Node, Parser};

use super::model::{module_name, node_text as text, rel, Model};

pub(crate) fn generate(source: &str, file_path: Option<&Path>) -> Option<String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(source, None)?;
    let src = source.as_bytes();

    let module = module_name(file_path);
    let mut model = Model::new();
    walk(tree.root_node(), src, &mut model, &module, None);
    model.render()
}

/// Walk a scope. `enclosing` is the box index of a surrounding class/module, so
/// its top-level methods land in the right box; `None` means file scope.
fn walk(node: Node, src: &[u8], model: &mut Model, module: &str, enclosing: Option<usize>) {
    let mut c = node.walk();
    for item in node.named_children(&mut c) {
        match item.kind() {
            "class" | "module" => handle_type(item, src, model, module),
            "method" | "singleton_method" => {
                let name = item
                    .child_by_field_name("name")
                    .map(|n| text(n, src))
                    .unwrap_or("");
                let sig = format_method(item, src, name);
                let idx = match enclosing {
                    Some(i) => Some(i),
                    None => model.module_box(module),
                };
                if let Some(idx) = idx {
                    model.push_member(idx, sig);
                }
            }
            // Ruby wraps top-level statements in a body_statement.
            "body_statement" => walk(item, src, model, module, enclosing),
            _ => {}
        }
    }
}

fn handle_type(item: Node, src: &[u8], model: &mut Model, module: &str) {
    let Some(name_node) = item.child_by_field_name("name") else {
        return;
    };
    let type_name = text(name_node, src).to_string();
    let Some(idx) = model.box_idx(&type_name) else {
        return;
    };
    if let Some(sc) = item.child_by_field_name("superclass") {
        // `superclass` node is `< Base`; take its constant.
        let mut c = sc.walk();
        let base = sc.named_children(&mut c).find(|n| n.kind() == "constant");
        if let Some(base) = base {
            model.add_rel(text(base, src), &type_name, rel::INHERIT, "");
        }
    }
    if let Some(body) = item.child_by_field_name("body") {
        walk(body, src, model, module, Some(idx));
    }
}

fn format_method(item: Node, src: &[u8], name: &str) -> String {
    let mut params = Vec::new();
    if let Some(ps) = item.child_by_field_name("parameters") {
        let mut c = ps.walk();
        for p in ps.named_children(&mut c) {
            let repr = match p.kind() {
                "identifier" => text(p, src).to_string(),
                _ => p
                    .child_by_field_name("name")
                    .map(|n| text(n, src).to_string())
                    .unwrap_or_else(|| text(p, src).to_string()),
            };
            if !repr.is_empty() {
                params.push(repr);
            }
        }
    }
    // `def self.x` is a class method — mark it as such.
    let prefix = if item.kind() == "singleton_method" {
        "+self."
    } else {
        "+"
    };
    format!("{prefix}{name}({})", params.join(", "))
}
