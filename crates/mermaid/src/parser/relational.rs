//! `classDiagram` and `erDiagram` parsing, sharing the relation model.

/// One class: its id, display label (defaults to the id), and member lines
/// (attributes + methods, verbatim).
#[derive(Debug, Clone, Default)]
pub struct ClassEntry {
    pub name: String,
    /// Title shown in the box; set from `class Id["Label"]`, else equals `name`.
    pub label: String,
    pub members: Vec<String>,
}

/// A relationship between two classes, with a human-readable kind/label.
#[derive(Debug, Clone)]
pub struct Relation {
    pub from: String,
    pub to: String,
    pub label: String,
}

#[derive(Debug, Clone, Default)]
pub struct ClassDiagram {
    pub entries: Vec<ClassEntry>,
    pub rels: Vec<Relation>,
}

impl ClassDiagram {
    fn entry(&mut self, name: &str) -> usize {
        if let Some(i) = self.entries.iter().position(|e| e.name == name) {
            return i;
        }
        self.entries.push(ClassEntry {
            name: name.to_string(),
            label: name.to_string(),
            members: Vec::new(),
        });
        self.entries.len() - 1
    }
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('"')
        && s.chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        && !is_class_rel_token(s)
}

/// A class relationship operator token (e.g. `<|--`, `*--`, `-->`, `..>`).
fn is_class_rel_token(s: &str) -> bool {
    s.len() >= 2 && s.chars().all(|c| "<|>*o.-".contains(c)) && (s.contains('-') || s.contains('.'))
}

fn class_kind(op: &str) -> &'static str {
    if op.contains("<|") || op.contains("|>") {
        "inherits"
    } else if op.contains('*') {
        "composes"
    } else if op.contains('o') {
        "aggregates"
    } else if op.contains("..") {
        "uses"
    } else {
        ""
    }
}

/// Parse a `classDiagram`. Members come from `class X { … }` blocks or
/// `X : member` lines; relationships map to labelled edges. Generics/
/// multiplicity annotations are ignored.
pub fn parse_class(src: &str) -> ClassDiagram {
    let mut d = ClassDiagram::default();
    let mut block: Option<String> = None;

    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") || line == "classDiagram" {
            continue;
        }
        if let Some(name) = &block {
            if line == "}" {
                block = None;
            } else {
                let idx = d.entry(&name.clone());
                d.entries[idx].members.push(line.to_string());
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("class ") {
            // `class Id`, `class Id {`, `class Id["Label"]`, `class Id["Label"] {`
            let rest = rest.trim();
            let (decl, opens_block) = match rest.strip_suffix('{') {
                Some(d) => (d.trim(), true),
                None => (rest, false),
            };
            let (id, label) = match decl.find('[') {
                Some(br) => {
                    let id = decl[..br].trim();
                    let label =
                        decl[br..].trim_matches(|c| c == '[' || c == ']' || c == '"' || c == ' ');
                    (id, Some(label.to_string()))
                }
                None => (decl, None),
            };
            let idx = d.entry(id);
            if let Some(label) = label {
                if !label.is_empty() {
                    d.entries[idx].label = label;
                }
            }
            if opens_block {
                block = Some(id.to_string());
            }
            continue;
        }
        // Relationship lines contain an operator token; member lines (`X : m`)
        // do not.
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.iter().any(|p| is_class_rel_token(p)) {
            if let Some(rel) = parse_class_rel(line) {
                d.entry(&rel.from);
                d.entry(&rel.to);
                d.rels.push(rel);
            }
        } else if let Some((n, m)) = line.split_once(':') {
            let idx = d.entry(n.trim());
            d.entries[idx].members.push(m.trim().to_string());
        }
    }
    d
}

/// Shared prefix for the two relation grammars: split off the optional `:`
/// label, tokenise the left side, and locate the relation operator. Returns the
/// tokens, the operator index, and the raw (untrimmed) label text. The two
/// callers diverge afterwards in how they pick `from`/`to` and build the label.
fn split_rel(line: &str, is_op: impl Fn(&str) -> bool) -> Option<(Vec<&str>, usize, &str)> {
    let (main, label) = match line.split_once(':') {
        Some((a, b)) => (a.trim(), b.trim()),
        None => (line.trim(), ""),
    };
    let parts: Vec<&str> = main.split_whitespace().collect();
    let opi = parts.iter().position(|p| is_op(p))?;
    Some((parts, opi, label))
}

fn parse_class_rel(line: &str) -> Option<Relation> {
    let (parts, opi, lbl) = split_rel(line, is_class_rel_token)?;
    let from = parts[..opi].iter().rev().find(|p| is_ident(p))?;
    let to = parts[opi + 1..].iter().find(|p| is_ident(p))?;
    let label = if lbl.is_empty() {
        class_kind(parts[opi]).to_string()
    } else {
        lbl.to_string()
    };
    Some(Relation {
        from: from.trim_matches('"').to_string(),
        to: to.trim_matches('"').to_string(),
        label,
    })
}

#[derive(Debug, Clone, Default)]
pub struct ErEntry {
    pub name: String,
    pub attrs: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ErDiagram {
    pub entries: Vec<ErEntry>,
    pub rels: Vec<Relation>,
}

impl ErDiagram {
    fn entry(&mut self, name: &str) -> usize {
        if let Some(i) = self.entries.iter().position(|e| e.name == name) {
            return i;
        }
        self.entries.push(ErEntry {
            name: name.to_string(),
            attrs: Vec::new(),
        });
        self.entries.len() - 1
    }
}

/// Parse an `erDiagram`. Attributes come from `ENTITY { … }` blocks;
/// relationships carry crow's-foot cardinality plus the optional verb label.
pub fn parse_er(src: &str) -> ErDiagram {
    let mut d = ErDiagram::default();
    let mut block: Option<String> = None;

    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") || line == "erDiagram" {
            continue;
        }
        if let Some(name) = &block {
            if line == "}" {
                block = None;
            } else {
                let idx = d.entry(&name.clone());
                d.entries[idx].attrs.push(line.to_string());
            }
            continue;
        }
        // `ENTITY { ... }` attribute block.
        if let Some(name) = line.strip_suffix('{').map(|s| s.trim()) {
            if !name.is_empty() && !name.contains(' ') {
                d.entry(name);
                block = Some(name.to_string());
                continue;
            }
        }
        // Relationship: `A <card>--<card> B : verb`.
        if let Some(rel) = parse_er_rel(line) {
            d.entry(&rel.from);
            d.entry(&rel.to);
            d.rels.push(rel);
        }
    }
    d
}

fn is_er_rel_token(s: &str) -> bool {
    s.contains("--") && s.chars().all(|c| "|}{o.-".contains(c))
}

fn card_text(card: &str) -> &'static str {
    match card {
        "||" => "1",
        "|o" | "o|" => "0..1",
        "}o" | "o{" => "0..N",
        "}|" | "|{" => "1..N",
        _ => "",
    }
}

fn parse_er_rel(line: &str) -> Option<Relation> {
    let (parts, opi, verb_raw) = split_rel(line, is_er_rel_token)?;
    let verb = verb_raw.trim_matches('"');
    let op = parts[opi];
    let dash = op.find("--")?;
    let left = card_text(&op[..dash]);
    let right = card_text(&op[dash + 2..]);
    let from = parts.get(opi.checked_sub(1)?)?;
    let to = parts.get(opi + 1)?;
    let mut label = format!("{left}–{right}");
    if !verb.is_empty() {
        label = format!("{verb} {label}");
    }
    Some(Relation {
        from: from.trim_matches('"').to_string(),
        to: to.trim_matches('"').to_string(),
        label,
    })
}
