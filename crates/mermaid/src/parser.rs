//! Minimal Mermaid parser.
//!
//! Parses `sequenceDiagram`, `flowchart`/`graph`, `stateDiagram`, `pie`,
//! `classDiagram`, and `erDiagram`. Other kinds are detected (so the viewer can
//! show an informative placeholder) but not yet parsed.

/// Which Mermaid diagram a source describes (from its header keyword).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramKind {
    Sequence,
    Flowchart,
    State,
    Pie,
    Class,
    Er,
    Gantt,
    Journey,
    Mindmap,
    Timeline,
    GitGraph,
    Quadrant,
    /// A recognized-but-not-yet-rendered kind (requirement, C4, …).
    Other,
    /// No recognizable Mermaid header.
    Unknown,
}

/// Detect the diagram kind from the first meaningful line.
#[must_use]
pub fn detect_kind(src: &str) -> DiagramKind {
    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue; // blank or comment
        }
        let head = line.split_whitespace().next().unwrap_or("");
        return match head {
            "sequenceDiagram" => DiagramKind::Sequence,
            "flowchart" | "graph" => DiagramKind::Flowchart,
            "stateDiagram" | "stateDiagram-v2" => DiagramKind::State,
            "pie" => DiagramKind::Pie,
            "classDiagram" => DiagramKind::Class,
            "erDiagram" => DiagramKind::Er,
            "gantt" => DiagramKind::Gantt,
            "journey" => DiagramKind::Journey,
            "mindmap" => DiagramKind::Mindmap,
            "timeline" => DiagramKind::Timeline,
            "gitGraph" => DiagramKind::GitGraph,
            "quadrantChart" => DiagramKind::Quadrant,
            "requirementDiagram" | "C4Context" => DiagramKind::Other,
            _ => DiagramKind::Unknown,
        };
    }
    DiagramKind::Unknown
}

/// A sequence-diagram participant (in left-to-right declaration/use order).
#[derive(Debug, Clone)]
pub struct Participant {
    pub id: String,
    pub label: String,
    /// `actor` renders with a stick-figure marker; `participant` is a box.
    pub actor: bool,
}

/// Arrow line style + head, derived from the Mermaid arrow token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arrow {
    pub dashed: bool,
    /// `>` filled head, `x` cross, `o`/`)` open — collapsed to a head glyph.
    pub head: ArrowHead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowHead {
    Filled,
    Open,
    Cross,
    None,
}

/// Where a note sits relative to its participant(s).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotePlacement {
    LeftOf,
    RightOf,
    Over,
}

/// One step of a sequence diagram.
#[derive(Debug, Clone)]
pub enum SeqEvent {
    Message {
        from: usize,
        to: usize,
        text: String,
        arrow: Arrow,
    },
    Note {
        placement: NotePlacement,
        /// Indices into `participants` (one for left/right, one or two for over).
        targets: Vec<usize>,
        text: String,
    },
}

/// A parsed sequence diagram.
#[derive(Debug, Clone, Default)]
pub struct Sequence {
    pub participants: Vec<Participant>,
    pub events: Vec<SeqEvent>,
}

impl Sequence {
    fn participant_index(&mut self, id: &str) -> usize {
        if let Some(i) = self.participants.iter().position(|p| p.id == id) {
            return i;
        }
        self.participants.push(Participant {
            id: id.to_string(),
            label: id.to_string(),
            actor: false,
        });
        self.participants.len() - 1
    }
}

/// Known sequence arrow tokens, longest first so `-->>` wins over `->>`.
const ARROWS: &[(&str, Arrow)] = &[
    (
        "-->>",
        Arrow {
            dashed: true,
            head: ArrowHead::Filled,
        },
    ),
    (
        "->>",
        Arrow {
            dashed: false,
            head: ArrowHead::Filled,
        },
    ),
    (
        "--x",
        Arrow {
            dashed: true,
            head: ArrowHead::Cross,
        },
    ),
    (
        "-x",
        Arrow {
            dashed: false,
            head: ArrowHead::Cross,
        },
    ),
    (
        "--)",
        Arrow {
            dashed: true,
            head: ArrowHead::Open,
        },
    ),
    (
        "-)",
        Arrow {
            dashed: false,
            head: ArrowHead::Open,
        },
    ),
    (
        "-->",
        Arrow {
            dashed: true,
            head: ArrowHead::None,
        },
    ),
    (
        "->",
        Arrow {
            dashed: false,
            head: ArrowHead::None,
        },
    ),
];

/// Parse a `sequenceDiagram` source into a [`Sequence`]. Unrecognized lines
/// (control blocks, activations, etc.) are skipped, so partial input still
/// renders something useful.
pub fn parse_sequence(src: &str) -> Sequence {
    let mut seq = Sequence::default();

    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") || line == "sequenceDiagram" {
            continue;
        }

        // Declarations: `participant A as Alice` / `actor Bob`.
        if let Some(rest) = line
            .strip_prefix("participant ")
            .or_else(|| line.strip_prefix("actor "))
        {
            let actor = line.starts_with("actor ");
            let (id, label) = match rest.split_once(" as ") {
                Some((id, label)) => (id.trim().to_string(), label.trim().to_string()),
                None => (rest.trim().to_string(), rest.trim().to_string()),
            };
            let idx = seq.participant_index(&id);
            seq.participants[idx].label = label;
            seq.participants[idx].actor = actor;
            continue;
        }

        // Notes: `Note left of A: text`, `Note over A,B: text`.
        if let Some(rest) = line
            .strip_prefix("Note ")
            .or_else(|| line.strip_prefix("note "))
        {
            if let Some(ev) = parse_note(&mut seq, rest) {
                seq.events.push(ev);
            }
            continue;
        }

        // Messages: `A->>B: text`.
        if let Some(ev) = parse_message(&mut seq, line) {
            seq.events.push(ev);
        }
    }

    seq
}

fn parse_note(seq: &mut Sequence, rest: &str) -> Option<SeqEvent> {
    let (placement, after) = if let Some(a) = rest.strip_prefix("left of ") {
        (NotePlacement::LeftOf, a)
    } else if let Some(a) = rest.strip_prefix("right of ") {
        (NotePlacement::RightOf, a)
    } else if let Some(a) = rest.strip_prefix("over ") {
        (NotePlacement::Over, a)
    } else {
        return None;
    };
    let (names, text) = after.split_once(':')?;
    let targets: Vec<usize> = names
        .split(',')
        .map(|n| seq.participant_index(n.trim()))
        .collect();
    if targets.is_empty() {
        return None;
    }
    Some(SeqEvent::Note {
        placement,
        targets,
        text: text.trim().to_string(),
    })
}

fn parse_message(seq: &mut Sequence, line: &str) -> Option<SeqEvent> {
    // Find the first arrow token and split the line around it.
    let (tok, arrow, at) = ARROWS
        .iter()
        .filter_map(|(tok, arrow)| line.find(tok).map(|at| (*tok, *arrow, at)))
        .min_by_key(|&(_, _, at)| at)?;

    let left = line[..at].trim();
    let after = &line[at + tok.len()..];
    let (right_raw, text) = match after.split_once(':') {
        Some((r, t)) => (r.trim(), t.trim().to_string()),
        None => (after.trim(), String::new()),
    };
    if left.is_empty() || right_raw.is_empty() {
        return None;
    }
    // Strip activation markers (`+`/`-`) that may prefix the target.
    let right = right_raw.trim_start_matches(['+', '-']).trim();
    let from = seq.participant_index(left);
    let to = seq.participant_index(right);
    Some(SeqEvent::Message {
        from,
        to,
        text,
        arrow,
    })
}

// ===========================================================================
// Flowchart
// ===========================================================================

/// Flowchart layout direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Top-to-bottom (`TD`/`TB`).
    Down,
    /// Bottom-to-top (`BT`).
    Up,
    /// Left-to-right (`LR`).
    Right,
    /// Right-to-left (`RL`).
    Left,
}

impl Direction {
    /// True for the vertical orientations (ranks stacked as rows).
    pub fn vertical(self) -> bool {
        matches!(self, Direction::Down | Direction::Up)
    }
}

/// Node outline hint (affects the corner glyphs used when drawing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeShape {
    Rect,
    Round,
    Stadium,
    Cylinder,
    Circle,
    Diamond,
    Hexagon,
}

#[derive(Debug, Clone)]
pub struct FlowNode {
    pub id: String,
    pub label: String,
    pub shape: NodeShape,
    /// Extra compartment lines shown below the title (class members, ER
    /// attributes). Empty for plain flowchart/state nodes.
    pub body: Vec<String>,
}

/// Edge line style (from the connector token).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeLine {
    Solid,
    Dotted,
    Thick,
}

#[derive(Debug, Clone)]
pub struct FlowEdge {
    pub from: usize,
    pub to: usize,
    pub label: String,
    pub line: EdgeLine,
    /// Whether the edge ends in an arrowhead (vs. an open `---` line).
    pub arrow: bool,
}

#[derive(Debug, Clone)]
pub struct Flowchart {
    pub direction: Direction,
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
}

impl Flowchart {
    fn node_index(&mut self, id: &str) -> usize {
        if let Some(i) = self.nodes.iter().position(|n| n.id == id) {
            return i;
        }
        self.nodes.push(FlowNode {
            id: id.to_string(),
            label: id.to_string(),
            shape: NodeShape::Rect,
            body: Vec::new(),
        });
        self.nodes.len() - 1
    }
}

/// A scanned connector: its end index, style, head, and optional inline label.
struct Connector {
    end: usize,
    line: EdgeLine,
    arrow: bool,
    label: String,
}

/// Parse a `flowchart`/`graph` source. Supports common node shapes, plain and
/// `|label|` edges, chains (`A --> B --> C`), and solid/dotted/thick lines.
/// Subgraphs and inline `-- label -->` edges are not parsed yet.
pub fn parse_flowchart(src: &str) -> Flowchart {
    let mut fc = Flowchart {
        direction: Direction::Down,
        nodes: Vec::new(),
        edges: Vec::new(),
    };

    let mut first = true;
    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if first {
            first = false;
            let mut parts = line.split_whitespace();
            let head = parts.next().unwrap_or("");
            if head == "flowchart" || head == "graph" {
                fc.direction = match parts.next().unwrap_or("TD") {
                    "LR" => Direction::Right,
                    "RL" => Direction::Left,
                    "BT" => Direction::Up,
                    _ => Direction::Down,
                };
                continue;
            }
        }
        let kw = line.split_whitespace().next().unwrap_or("");
        if matches!(
            kw,
            "subgraph" | "end" | "direction" | "classDef" | "class" | "style" | "click"
        ) {
            continue;
        }
        parse_statement(&mut fc, line);
    }

    fc
}

fn parse_statement(fc: &mut Flowchart, line: &str) {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut prev: Option<usize> = None;

    while i < chars.len() {
        let (node_text, next) = read_node_chunk(&chars, i);
        let node_idx = if node_text.trim().is_empty() {
            None
        } else {
            Some(register_node(fc, node_text.trim()))
        };
        i = next;

        if let Some(conn) = scan_connector(&chars, i) {
            let from = node_idx.or(prev);
            i = conn.end;
            let (target_text, after) = read_node_chunk(&chars, i);
            let to = if target_text.trim().is_empty() {
                None
            } else {
                Some(register_node(fc, target_text.trim()))
            };
            if let (Some(f), Some(t)) = (from, to) {
                fc.edges.push(FlowEdge {
                    from: f,
                    to: t,
                    label: conn.label,
                    line: conn.line,
                    arrow: conn.arrow,
                });
            }
            prev = to;
            i = after;
        } else {
            prev = node_idx;
            if i < chars.len() && node_idx.is_none() {
                i += 1; // make progress on stray characters
            }
        }
    }
}

/// Read characters until the start of a top-level connector (a `-`/`=` run at
/// bracket depth 0). Returns the chunk text and the connector start index.
fn read_node_chunk(chars: &[char], start: usize) -> (String, usize) {
    let mut depth = 0i32;
    let mut i = start;
    let mut s = String::new();
    while i < chars.len() {
        let c = chars[i];
        if matches!(c, '[' | '(' | '{') {
            depth += 1;
        } else if matches!(c, ']' | ')' | '}') {
            depth -= 1;
        } else if depth <= 0 && (c == '-' || c == '=') && is_connector_start(chars, i) {
            break;
        }
        s.push(c);
        i += 1;
    }
    (s, i)
}

/// Whether a connector begins at `i` (`--`, `-.`, or `==`).
fn is_connector_start(chars: &[char], i: usize) -> bool {
    let a = chars.get(i).copied();
    let b = chars.get(i + 1).copied();
    matches!(
        (a, b),
        (Some('-'), Some('-')) | (Some('-'), Some('.')) | (Some('='), Some('='))
    )
}

/// Scan a connector starting at/after `i` (skipping spaces). `None` if none.
fn scan_connector(chars: &[char], mut i: usize) -> Option<Connector> {
    while i < chars.len() && chars[i] == ' ' {
        i += 1;
    }
    if !is_connector_start(chars, i) {
        return None;
    }
    let thick = chars[i] == '=';
    let dotted = chars[i] == '-' && chars.get(i + 1) == Some(&'.');
    let mut j = i;
    let mut arrow = false;
    while j < chars.len() {
        match chars[j] {
            '-' | '=' | '.' | '<' => j += 1,
            '>' => {
                arrow = true;
                j += 1;
            }
            'x' | 'o' => {
                arrow = true;
                j += 1;
                break;
            }
            _ => break,
        }
    }
    let mut label = String::new();
    let mut k = j;
    while k < chars.len() && chars[k] == ' ' {
        k += 1;
    }
    if chars.get(k) == Some(&'|') {
        k += 1;
        while k < chars.len() && chars[k] != '|' {
            label.push(chars[k]);
            k += 1;
        }
        if chars.get(k) == Some(&'|') {
            k += 1;
        }
        j = k;
    }
    let line = if dotted {
        EdgeLine::Dotted
    } else if thick {
        EdgeLine::Thick
    } else {
        EdgeLine::Solid
    };
    Some(Connector {
        end: j,
        line,
        arrow,
        label: label.trim().to_string(),
    })
}

/// Register/update a node from a spec like `A`, `A[Label]`, `A{Decision}`.
fn register_node(fc: &mut Flowchart, spec: &str) -> usize {
    let (id, shape, label) = parse_node_spec(spec);
    let idx = fc.node_index(&id);
    if let Some(label) = label {
        fc.nodes[idx].label = label;
    }
    if let Some(shape) = shape {
        fc.nodes[idx].shape = shape;
    }
    idx
}

/// Split a node spec into `(id, shape, label)`; shape/label are `None` for a
/// bare id reference.
fn parse_node_spec(spec: &str) -> (String, Option<NodeShape>, Option<String>) {
    let id_end = spec
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(spec.len());
    let id = spec[..id_end].to_string();
    let wrap = &spec[id_end..];
    if wrap.is_empty() {
        return (id, None, None);
    }
    let (shape, open, close) = if wrap.starts_with("([") {
        (NodeShape::Stadium, "([", "])")
    } else if wrap.starts_with("[(") {
        (NodeShape::Cylinder, "[(", ")]")
    } else if wrap.starts_with("((") {
        (NodeShape::Circle, "((", "))")
    } else if wrap.starts_with("{{") {
        (NodeShape::Hexagon, "{{", "}}")
    } else if wrap.starts_with('[') {
        (NodeShape::Rect, "[", "]")
    } else if wrap.starts_with('(') {
        (NodeShape::Round, "(", ")")
    } else if wrap.starts_with('{') {
        (NodeShape::Diamond, "{", "}")
    } else if wrap.starts_with('>') {
        (NodeShape::Rect, ">", "]")
    } else {
        return (id, None, None);
    };
    let inner = wrap
        .strip_prefix(open)
        .and_then(|w| w.strip_suffix(close))
        .unwrap_or(wrap);
    let label = inner.trim().trim_matches(['"', '/', '\\', ' ']).to_string();
    (id, Some(shape), Some(label))
}

// ===========================================================================
// State diagram (mapped onto the flowchart graph model)
// ===========================================================================

/// Parse a `stateDiagram`/`stateDiagram-v2` into the shared [`Flowchart`] graph
/// model so it reuses the layered layout. `[*]` becomes a start/end pseudo-node.
/// Composite-state nesting is flattened for now.
pub fn parse_state(src: &str) -> Flowchart {
    let mut fc = Flowchart {
        direction: Direction::Down,
        nodes: Vec::new(),
        edges: Vec::new(),
    };

    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty()
            || line.starts_with("%%")
            || line == "stateDiagram"
            || line == "stateDiagram-v2"
            || line == "}"
            || line.starts_with("note ")
        {
            continue;
        }
        if let Some(dir) = line.strip_prefix("direction ") {
            fc.direction = match dir.trim() {
                "LR" => Direction::Right,
                "RL" => Direction::Left,
                "BT" => Direction::Up,
                _ => Direction::Down,
            };
            continue;
        }
        // `state "Long description" as Id` / `state Id` / `state Id {`
        if let Some(rest) = line.strip_prefix("state ") {
            if let Some((desc, id)) = rest.split_once(" as ") {
                let id = id.trim().trim_end_matches('{').trim();
                let idx = fc.node_index(id);
                fc.nodes[idx].label = desc.trim().trim_matches('"').to_string();
                fc.nodes[idx].shape = NodeShape::Round;
            }
            continue;
        }
        if let Some((l, r)) = line.split_once("-->") {
            let (rhs, label) = match r.split_once(':') {
                Some((a, b)) => (a.trim(), b.trim().to_string()),
                None => (r.trim(), String::new()),
            };
            let from = state_node(&mut fc, l.trim(), true);
            let to = state_node(&mut fc, rhs, false);
            fc.edges.push(FlowEdge {
                from,
                to,
                label,
                line: EdgeLine::Solid,
                arrow: true,
            });
        }
    }
    fc
}

/// Resolve a state token to a node index; `[*]` maps to a start or end marker.
fn state_node(fc: &mut Flowchart, token: &str, is_source: bool) -> usize {
    if token == "[*]" {
        let (id, label) = if is_source {
            ("__start__", "●")
        } else {
            ("__end__", "◉")
        };
        let idx = fc.node_index(id);
        fc.nodes[idx].label = label.to_string();
        fc.nodes[idx].shape = NodeShape::Circle;
        idx
    } else {
        let idx = fc.node_index(token);
        if fc.nodes[idx].shape == NodeShape::Rect {
            fc.nodes[idx].shape = NodeShape::Round;
        }
        idx
    }
}

// ===========================================================================
// Pie chart
// ===========================================================================

/// A parsed pie chart: a title and labelled values.
#[derive(Debug, Clone, Default)]
pub struct Pie {
    pub title: String,
    pub slices: Vec<(String, f64)>,
}

/// Parse a `pie` chart source (`"Label" : value` lines, optional title).
pub fn parse_pie(src: &str) -> Pie {
    let mut pie = Pie::default();
    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("pie") {
            // `pie [showData] [title ...]`
            if let Some(t) = rest.find("title ") {
                pie.title = rest[t + "title ".len()..].trim().to_string();
            }
            continue;
        }
        if let Some(t) = line.strip_prefix("title ") {
            pie.title = t.trim().to_string();
            continue;
        }
        // `"Label" : 42`
        if let Some((label, value)) = line.split_once(':') {
            let label = label.trim().trim_matches('"').to_string();
            if let Ok(v) = value.trim().parse::<f64>() {
                if !label.is_empty() {
                    pie.slices.push((label, v));
                }
            }
        }
    }
    pie
}

// ===========================================================================
// Class diagram
// ===========================================================================

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

// ===========================================================================
// Entity-relationship diagram
// ===========================================================================

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

// ===========================================================================
// Gantt chart
// ===========================================================================

/// Visual status of a Gantt task (from its tags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Plain,
    Active,
    Done,
    Crit,
    Milestone,
}

#[derive(Debug, Clone)]
pub struct GanttTask {
    pub section: String,
    pub name: String,
    /// Start day (relative ordinal) and length in days.
    pub start: i64,
    pub len: i64,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Default)]
pub struct Gantt {
    pub title: String,
    pub tasks: Vec<GanttTask>,
}

fn date_to_days(s: &str) -> Option<i64> {
    let mut it = s.trim().split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) {
        return None;
    }
    Some(y * 365 + CUM[(m - 1) as usize] + d)
}

/// Approximate days-per-month prefix sums (ignores leap years) — adequate for
/// relative bar placement and axis labels.
const CUM: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];

/// Inverse of [`date_to_days`]: format a day ordinal as `YYYY-MM-DD`.
pub fn day_to_date(ord: i64) -> String {
    let ord = ord.max(0);
    let mut year = ord / 365;
    let mut rem = ord % 365;
    if rem == 0 {
        // Day 0 belongs to 31 Dec of the previous year.
        year -= 1;
        rem = 365;
    }
    let month = CUM.iter().rposition(|&c| c < rem).unwrap_or(0);
    let day = rem - CUM[month];
    format!("{year:04}-{:02}-{:02}", month + 1, day)
}

fn dur_to_days(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Some(n) = s.strip_suffix('d') {
        n.trim().parse().ok()
    } else if let Some(n) = s.strip_suffix('w') {
        n.trim().parse::<i64>().ok().map(|w| w * 7)
    } else if let Some(n) = s.strip_suffix('h') {
        n.trim().parse::<i64>().ok().map(|h| (h / 24).max(1))
    } else {
        None
    }
}

/// Parse a `gantt` chart. Resolves `after <id>` dependencies and durations
/// (`Nd`/`Nw`) or explicit end dates; leap years are ignored.
pub fn parse_gantt(src: &str) -> Gantt {
    let mut g = Gantt::default();
    let mut section = String::new();
    // id -> (start, end) for `after` resolution.
    let mut ends: std::collections::HashMap<String, (i64, i64)> = std::collections::HashMap::new();

    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") || line == "gantt" {
            continue;
        }
        if let Some(t) = line.strip_prefix("title ") {
            g.title = t.trim().to_string();
            continue;
        }
        if let Some(s) = line.strip_prefix("section ") {
            section = s.trim().to_string();
            continue;
        }
        // Skip directives without a task body.
        if line.starts_with("dateFormat")
            || line.starts_with("axisFormat")
            || line.starts_with("excludes")
            || line.starts_with("todayMarker")
            || line.starts_with("tickInterval")
        {
            continue;
        }
        let Some((name, meta)) = line.split_once(':') else {
            continue;
        };
        let fields: Vec<&str> = meta.split(',').map(|f| f.trim()).collect();
        let status = if fields.contains(&"milestone") {
            TaskStatus::Milestone
        } else if fields.contains(&"done") {
            TaskStatus::Done
        } else if fields.contains(&"active") {
            TaskStatus::Active
        } else if fields.contains(&"crit") {
            TaskStatus::Crit
        } else {
            TaskStatus::Plain
        };
        // The start field is a date or `after <id>`; duration/end follows.
        let start_idx = fields
            .iter()
            .position(|f| date_to_days(f).is_some() || f.starts_with("after "));
        let Some(si) = start_idx else { continue };
        let start = if let Some(days) = date_to_days(fields[si]) {
            days
        } else if let Some(dep) = fields[si].strip_prefix("after ") {
            ends.get(dep.trim()).map(|(_, e)| *e).unwrap_or(0)
        } else {
            0
        };
        let len = match fields.get(si + 1) {
            Some(f) => dur_to_days(f)
                .or_else(|| date_to_days(f).map(|e| (e - start).max(0)))
                .unwrap_or(1),
            None => 1,
        };
        // An explicit id (non-tag field before the start) enables `after` refs.
        let tags = ["done", "active", "crit", "milestone"];
        if let Some(id) = fields[..si].iter().find(|f| !tags.contains(f)) {
            ends.insert(id.to_string(), (start, start + len));
        }
        g.tasks.push(GanttTask {
            section: section.clone(),
            name: name.trim().to_string(),
            start,
            len,
            status,
        });
    }
    g
}

// ===========================================================================
// User journey
// ===========================================================================

#[derive(Debug, Clone)]
pub struct JourneyTask {
    pub section: String,
    pub name: String,
    pub score: u8,
    pub actors: String,
}

#[derive(Debug, Clone, Default)]
pub struct Journey {
    pub title: String,
    pub tasks: Vec<JourneyTask>,
}

/// Parse a `journey` diagram (`Task: score: actors` lines under sections).
pub fn parse_journey(src: &str) -> Journey {
    let mut j = Journey::default();
    let mut section = String::new();
    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") || line == "journey" {
            continue;
        }
        if let Some(t) = line.strip_prefix("title ") {
            j.title = t.trim().to_string();
            continue;
        }
        if let Some(s) = line.strip_prefix("section ") {
            section = s.trim().to_string();
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, ':').map(|p| p.trim()).collect();
        if parts.len() >= 2 {
            if let Ok(score) = parts[1].parse::<u8>() {
                j.tasks.push(JourneyTask {
                    section: section.clone(),
                    name: parts[0].to_string(),
                    score: score.min(5),
                    actors: parts.get(2).copied().unwrap_or("").to_string(),
                });
            }
        }
    }
    j
}

// ===========================================================================
// Mindmap
// ===========================================================================

/// A mindmap node at a nesting depth (derived from indentation).
#[derive(Debug, Clone)]
pub struct MindNode {
    pub depth: usize,
    pub label: String,
}

#[derive(Debug, Clone, Default)]
pub struct Mindmap {
    pub nodes: Vec<MindNode>,
}

/// Parse a `mindmap` into a flat list of `(depth, label)`, depth from indent.
/// Node shape wrappers (`((x))`, `[x]`, `(x)`) are stripped to the label.
pub fn parse_mindmap(src: &str) -> Mindmap {
    let mut m = Mindmap::default();
    // Map raw indentation widths to contiguous depth levels.
    let mut indents: Vec<usize> = Vec::new();
    for raw in src.lines() {
        if raw.trim().is_empty() || raw.trim().starts_with("%%") || raw.trim() == "mindmap" {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        let depth = match indents.iter().position(|&i| i == indent) {
            Some(d) => d,
            None => {
                // Deeper than any seen → new level; shallower handled by find.
                let d = indents.iter().filter(|&&i| i < indent).count();
                indents.truncate(d);
                indents.push(indent);
                d
            }
        };
        m.nodes.push(MindNode {
            depth,
            label: strip_mind_shape(raw.trim()),
        });
    }
    m
}

fn strip_mind_shape(s: &str) -> String {
    let s = s.trim();
    for (open, close) in [("((", "))"), ("(", ")"), ("[", "]"), ("{{", "}}")] {
        if let Some(inner) = s.strip_prefix(open).and_then(|x| x.strip_suffix(close)) {
            return inner.trim().to_string();
        }
    }
    s.to_string()
}

// ===========================================================================
// Timeline
// ===========================================================================

#[derive(Debug, Clone)]
pub struct TimelineEntry {
    pub section: String,
    pub period: String,
    pub events: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Timeline {
    pub title: String,
    pub entries: Vec<TimelineEntry>,
}

/// Parse a `timeline` (`period : event : event` rows, optional sections).
pub fn parse_timeline(src: &str) -> Timeline {
    let mut t = Timeline::default();
    let mut section = String::new();
    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") || line == "timeline" {
            continue;
        }
        if let Some(s) = line.strip_prefix("title ") {
            t.title = s.trim().to_string();
            continue;
        }
        if let Some(s) = line.strip_prefix("section ") {
            section = s.trim().to_string();
            continue;
        }
        let mut parts = line.split(':').map(|p| p.trim().to_string());
        let Some(period) = parts.next() else { continue };
        let events: Vec<String> = parts.filter(|e| !e.is_empty()).collect();
        t.entries.push(TimelineEntry {
            section: section.clone(),
            period,
            events,
        });
    }
    t
}

// ===========================================================================
// Git graph
// ===========================================================================

#[derive(Debug, Clone)]
pub enum GitOp {
    Commit { label: String },
    Branch(String),
    Checkout(String),
    Merge(String),
}

#[derive(Debug, Clone, Default)]
pub struct GitGraph {
    pub ops: Vec<GitOp>,
}

/// Parse a `gitGraph`: `commit`/`branch`/`checkout`/`switch`/`merge`.
pub fn parse_gitgraph(src: &str) -> GitGraph {
    let mut g = GitGraph::default();
    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") || line.starts_with("gitGraph") {
            continue;
        }
        let kw = line.split_whitespace().next().unwrap_or("");
        match kw {
            "commit" => {
                // Prefer an explicit `id:` then `tag:`, else empty.
                let label = extract_quoted(line, "id:")
                    .or_else(|| extract_quoted(line, "tag:"))
                    .unwrap_or_default();
                g.ops.push(GitOp::Commit { label });
            }
            "branch" => {
                if let Some(name) = line.split_whitespace().nth(1) {
                    g.ops.push(GitOp::Branch(name.to_string()));
                }
            }
            "checkout" | "switch" => {
                if let Some(name) = line.split_whitespace().nth(1) {
                    g.ops.push(GitOp::Checkout(name.to_string()));
                }
            }
            "merge" => {
                if let Some(name) = line.split_whitespace().nth(1) {
                    g.ops.push(GitOp::Merge(name.to_string()));
                }
            }
            _ => {}
        }
    }
    g
}

/// Extract the quoted value following `key` (e.g. `id: "A"` → `A`).
fn extract_quoted(line: &str, key: &str) -> Option<String> {
    let after = &line[line.find(key)? + key.len()..];
    let start = after.find('"')? + 1;
    let end = after[start..].find('"')? + start;
    Some(after[start..end].to_string())
}

// ===========================================================================
// Quadrant chart
// ===========================================================================

#[derive(Debug, Clone)]
pub struct QuadPoint {
    pub name: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Quadrant {
    pub title: String,
    pub x_axis: String,
    pub y_axis: String,
    /// Quadrant labels (1..=4 in Mermaid's numbering: TR, TL, BL, BR).
    pub quads: [String; 4],
    pub points: Vec<QuadPoint>,
}

/// Parse a `quadrantChart` (axes, quadrant labels, and `Name: [x, y]` points).
pub fn parse_quadrant(src: &str) -> Quadrant {
    let mut q = Quadrant::default();
    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") || line == "quadrantChart" {
            continue;
        }
        if let Some(s) = line.strip_prefix("title ") {
            q.title = s.trim().to_string();
        } else if let Some(s) = line.strip_prefix("x-axis ") {
            q.x_axis = s.trim().to_string();
        } else if let Some(s) = line.strip_prefix("y-axis ") {
            q.y_axis = s.trim().to_string();
        } else if let Some(s) = line.strip_prefix("quadrant-") {
            if let Some((n, label)) = s.split_once(' ') {
                if let Ok(i) = n.trim().parse::<usize>() {
                    if (1..=4).contains(&i) {
                        q.quads[i - 1] = label.trim().to_string();
                    }
                }
            }
        } else if let Some((name, coords)) = line.split_once(':') {
            // `Name: [x, y]`
            let coords = coords.trim().trim_start_matches('[').trim_end_matches(']');
            let mut it = coords.split(',').map(|c| c.trim().parse::<f64>());
            if let (Some(Ok(x)), Some(Ok(y))) = (it.next(), it.next()) {
                q.points.push(QuadPoint {
                    name: name.trim().to_string(),
                    x: x.clamp(0.0, 1.0),
                    y: y.clamp(0.0, 1.0),
                });
            }
        }
    }
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_kinds() {
        assert_eq!(
            detect_kind("sequenceDiagram\nA->>B: hi"),
            DiagramKind::Sequence
        );
        assert_eq!(detect_kind("flowchart TD\nA-->B"), DiagramKind::Flowchart);
        assert_eq!(detect_kind("graph LR\nA-->B"), DiagramKind::Flowchart);
        assert_eq!(detect_kind("classDiagram"), DiagramKind::Class);
        assert_eq!(detect_kind("erDiagram"), DiagramKind::Er);
        assert_eq!(detect_kind("gantt"), DiagramKind::Gantt);
        assert_eq!(detect_kind("journey"), DiagramKind::Journey);
        assert_eq!(detect_kind("mindmap"), DiagramKind::Mindmap);
        assert_eq!(detect_kind("gitGraph"), DiagramKind::GitGraph);
        assert_eq!(detect_kind("timeline"), DiagramKind::Timeline);
        assert_eq!(detect_kind("quadrantChart"), DiagramKind::Quadrant);
        assert_eq!(detect_kind("requirementDiagram"), DiagramKind::Other);
        assert_eq!(detect_kind("not a diagram"), DiagramKind::Unknown);
        assert_eq!(
            detect_kind("%% comment\nsequenceDiagram"),
            DiagramKind::Sequence
        );
    }

    #[test]
    fn parses_participants_and_messages() {
        let seq = parse_sequence(
            "sequenceDiagram\nparticipant A as Alice\nA->>B: Hello\nB-->>A: Hi back",
        );
        assert_eq!(seq.participants.len(), 2);
        assert_eq!(seq.participants[0].label, "Alice");
        assert_eq!(seq.participants[1].id, "B");
        assert_eq!(seq.events.len(), 2);
        match &seq.events[0] {
            SeqEvent::Message {
                from,
                to,
                text,
                arrow,
            } => {
                assert_eq!((*from, *to), (0, 1));
                assert_eq!(text, "Hello");
                assert_eq!(arrow.head, ArrowHead::Filled);
                assert!(!arrow.dashed);
            }
            _ => panic!("expected message"),
        }
        match &seq.events[1] {
            SeqEvent::Message { arrow, .. } => assert!(arrow.dashed),
            _ => panic!("expected message"),
        }
    }

    #[test]
    fn auto_registers_participants_in_use_order() {
        let seq = parse_sequence("sequenceDiagram\nBob->>Carol: x\nAlice->>Bob: y");
        let ids: Vec<&str> = seq.participants.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["Bob", "Carol", "Alice"]);
    }

    #[test]
    fn parses_notes() {
        let seq = parse_sequence("sequenceDiagram\nA->>B: hi\nNote over A,B: shared");
        let note = seq.events.iter().find_map(|e| match e {
            SeqEvent::Note {
                placement,
                targets,
                text,
            } => Some((*placement, targets.clone(), text.clone())),
            _ => None,
        });
        let (placement, targets, text) = note.expect("note parsed");
        assert_eq!(placement, NotePlacement::Over);
        assert_eq!(targets.len(), 2);
        assert_eq!(text, "shared");
    }

    #[test]
    fn flowchart_direction_and_chain() {
        let fc = parse_flowchart("flowchart LR\nA --> B --> C");
        assert_eq!(fc.direction, Direction::Right);
        let ids: Vec<&str> = fc.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B", "C"]);
        assert_eq!(fc.edges.len(), 2);
        assert_eq!((fc.edges[0].from, fc.edges[0].to), (0, 1));
        assert_eq!((fc.edges[1].from, fc.edges[1].to), (1, 2));
    }

    #[test]
    fn flowchart_shapes_and_labels() {
        let fc = parse_flowchart("flowchart TD\nA[Start] --> B{ok?}\nB -->|yes| C([Done])");
        assert_eq!(fc.nodes[0].label, "Start");
        assert_eq!(fc.nodes[0].shape, NodeShape::Rect);
        assert_eq!(fc.nodes[1].label, "ok?");
        assert_eq!(fc.nodes[1].shape, NodeShape::Diamond);
        assert_eq!(fc.nodes[2].shape, NodeShape::Stadium);
        let labeled = fc.edges.iter().find(|e| e.label == "yes");
        assert!(labeled.is_some(), "edge label not parsed: {:?}", fc.edges);
    }

    #[test]
    fn flowchart_line_styles() {
        let fc = parse_flowchart("flowchart TD\nA -.-> B\nB ==> C\nC --- D");
        assert_eq!(fc.edges[0].line, EdgeLine::Dotted);
        assert_eq!(fc.edges[1].line, EdgeLine::Thick);
        assert_eq!(fc.edges[2].line, EdgeLine::Solid);
        assert!(!fc.edges[2].arrow, "--- should be an open line");
        assert!(fc.edges[0].arrow);
    }

    #[test]
    fn state_maps_to_graph_with_start_end() {
        let fc = parse_state("stateDiagram-v2\n[*] --> Idle\nIdle --> Run : go\nRun --> [*]");
        // start, Idle, Run, end
        assert!(fc.nodes.iter().any(|n| n.id == "__start__"));
        assert!(fc.nodes.iter().any(|n| n.id == "__end__"));
        assert_eq!(fc.edges.len(), 3);
        let go = fc.edges.iter().find(|e| e.label == "go");
        assert!(go.is_some(), "transition label not parsed: {:?}", fc.edges);
    }

    #[test]
    fn state_alias_label() {
        let fc = parse_state("stateDiagram-v2\nstate \"Doing work\" as Run\n[*] --> Run");
        let run = fc.nodes.iter().find(|n| n.id == "Run").unwrap();
        assert_eq!(run.label, "Doing work");
    }

    #[test]
    fn pie_parses_title_and_slices() {
        let pie = parse_pie("pie title Share\n\"Rust\" : 55\n\"Other\" : 45");
        assert_eq!(pie.title, "Share");
        assert_eq!(pie.slices.len(), 2);
        assert_eq!(pie.slices[0], ("Rust".to_string(), 55.0));
    }
}
