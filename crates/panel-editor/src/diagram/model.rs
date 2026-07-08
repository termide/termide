//! Shared diagram model and Mermaid `classDiagram` rendering.
//!
//! Both the rich Rust extractor and the generic symbol-based fallback build a
//! [`Model`] of boxes and relationships, then render it to Mermaid text that
//! `crates/mermaid` lays out as terminal pseudographics.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use tree_sitter::Node;

/// A relationship operator understood by the mermaid class renderer.
pub(crate) mod rel {
    /// Realization: a type implements a trait/interface (`Iface <|.. Type`).
    pub const REALIZE: &str = "<|..";
    /// Inheritance: a type extends a base class (`Base <|-- Derived`).
    pub const INHERIT: &str = "<|--";
    /// Composition (`Owner *-- Part`).
    pub const COMPOSE: &str = "*--";
}

struct DiagramBox {
    /// Mermaid id used for relationship endpoints (a bare identifier).
    name: String,
    /// Declaration title shown in the box (`pub struct Cli`, `enum Color`, the
    /// file name for the module box). `None` renders as a plain `class name`.
    header: Option<String>,
    /// The synthetic file-level box (free functions/consts); excluded as a
    /// composition target.
    is_module: bool,
    /// Pre-formatted, sanitized member lines.
    members: Vec<String>,
}

struct Relation {
    from: String,
    to: String,
    op: &'static str,
    label: String,
}

/// A class diagram under construction.
#[derive(Default)]
pub(crate) struct Model {
    boxes: Vec<DiagramBox>,
    rels: Vec<Relation>,
    index: HashMap<String, usize>,
    /// Title for the file-level box (e.g. `cli.rs`).
    module_label: Option<String>,
}

impl Model {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get or create a box, returning its index. The name is sanitized so that
    /// box names and relationship endpoints match consistently.
    pub fn box_idx(&mut self, name: &str) -> Option<usize> {
        let name = sanitize_ident(name);
        if name.is_empty() {
            return None;
        }
        if let Some(&i) = self.index.get(&name) {
            return Some(i);
        }
        let i = self.boxes.len();
        self.index.insert(name.clone(), i);
        self.boxes.push(DiagramBox {
            name,
            header: None,
            is_module: false,
            members: Vec::new(),
        });
        Some(i)
    }

    /// Set the title used for the file-level (`module`) box.
    pub fn set_module_label(&mut self, label: String) {
        self.module_label = Some(label);
    }

    /// Get-or-create the file-level box holding top-level free items
    /// (functions, constants, aliases). Titled with the file name. Idempotent
    /// by id.
    pub fn module_box(&mut self, id: &str) -> Option<usize> {
        let idx = self.box_idx(id)?;
        self.boxes[idx].is_module = true;
        if self.boxes[idx].header.is_none() {
            let header = self.module_label.clone().unwrap_or_else(|| id.to_string());
            self.boxes[idx].header = Some(header);
        }
        Some(idx)
    }

    /// Set a box's declaration header, keeping the first one assigned.
    pub fn set_header(&mut self, idx: usize, header: impl Into<String>) {
        let b = &mut self.boxes[idx];
        if b.header.is_none() {
            b.header = Some(header.into());
        }
    }

    /// Push a member line into a box, sanitizing and de-duplicating.
    pub fn push_member(&mut self, idx: usize, member: impl AsRef<str>) {
        let m = sanitize_member(member.as_ref());
        if m.is_empty() {
            return;
        }
        let b = &mut self.boxes[idx];
        if !b.members.contains(&m) {
            b.members.push(m);
        }
    }

    /// Record a relationship between two boxes (endpoints sanitized).
    pub fn add_rel(&mut self, from: &str, to: &str, op: &'static str, label: &str) {
        let from = sanitize_ident(from);
        let to = sanitize_ident(to);
        if from.is_empty() || to.is_empty() {
            return;
        }
        if self
            .rels
            .iter()
            .any(|r| r.from == from && r.to == to && r.op == op)
        {
            return;
        }
        self.rels.push(Relation {
            from,
            to,
            op,
            label: label.to_string(),
        });
    }

    /// Sanitized ids of boxes that denote a concrete local type (everything
    /// except the file-level box). Used to filter composition edges to types
    /// actually declared in this file.
    pub fn local_type_names(&self) -> Vec<String> {
        self.boxes
            .iter()
            .filter(|b| !b.is_module)
            .map(|b| b.name.clone())
            .collect()
    }

    /// Turn composition candidates `(owner, base, label)` into `*--` edges,
    /// keeping only those whose `base` is a type declared in this file (and not
    /// the owner itself). Shared by every language extractor.
    pub fn resolve_compositions(&mut self, comps: Vec<(String, String, String)>) {
        let local: HashSet<String> = self.local_type_names().into_iter().collect();
        for (owner, base, label) in comps {
            if owner != base && local.contains(&base) {
                self.add_rel(&owner, &base, rel::COMPOSE, &label);
            }
        }
    }

    /// Render to Mermaid `classDiagram` source, or `None` if there is nothing
    /// to draw.
    pub fn render(&self) -> Option<String> {
        if self.boxes.is_empty() {
            return None;
        }
        let mut s = String::from("classDiagram\n");
        for b in &self.boxes {
            // `class Id` or `class Id["Header"]`.
            s.push_str("    class ");
            s.push_str(&b.name);
            if let Some(h) = &b.header {
                s.push('[');
                s.push('"');
                s.push_str(&sanitize_header(h));
                s.push('"');
                s.push(']');
            }
            if b.members.is_empty() {
                s.push('\n');
                continue;
            }
            s.push_str(" {\n");
            for m in &b.members {
                s.push_str("        ");
                s.push_str(m);
                s.push('\n');
            }
            s.push_str("    }\n");
        }
        for r in &self.rels {
            s.push_str("    ");
            s.push_str(&r.from);
            s.push(' ');
            s.push_str(r.op);
            s.push(' ');
            s.push_str(&r.to);
            if !r.label.is_empty() {
                s.push_str(" : ");
                s.push_str(&r.label);
            }
            s.push('\n');
        }
        Some(s)
    }
}

/// Reduce a raw type/box name to a Mermaid-safe identifier.
///
/// Keeps the last `::`-separated path segment and its leading run of identifier
/// characters, dropping generic parameters and other punctuation
/// (`crate::Wrap<T>` -> `Wrap`). Returns an empty string when nothing usable
/// remains.
pub(crate) fn sanitize_ident(raw: &str) -> String {
    let base = raw.rsplit("::").next().unwrap_or(raw).trim();
    base.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// Sanitize a member line: drop braces (they would prematurely close a
/// `class X { ... }` block in the mermaid parser) and collapse whitespace.
fn sanitize_member(raw: &str) -> String {
    let cleaned: String = raw.chars().filter(|c| *c != '{' && *c != '}').collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Sanitize a box header: drop characters that would break the
/// `class Id["Header"]` syntax, and collapse whitespace.
fn sanitize_header(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | '"' | '{' | '}'))
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// UTF-8 text of a node (empty on malformed bytes).
pub(crate) fn node_text<'a>(n: Node, src: &'a [u8]) -> &'a str {
    n.utf8_text(src).unwrap_or("")
}

/// Node text with internal whitespace runs collapsed to single spaces — used
/// for type text that may span lines (generic bounds, where-clauses).
pub(crate) fn collapse(n: Node, src: &[u8]) -> String {
    node_text(n, src)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The file-level box id: the sanitized file stem, or `module` when unknown.
pub(crate) fn module_name(file_path: Option<&Path>) -> String {
    file_path
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .map(sanitize_ident)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "module".to_string())
}

/// The file-level box title: the full file name (e.g. `cli.rs`), or `module`.
pub(crate) fn module_label(file_path: Option<&Path>) -> String {
    file_path
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| "module".to_string())
}
