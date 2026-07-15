//! `flowchart`/`graph` and `stateDiagram` parsing onto the shared graph model.

use super::{Direction, EdgeLine, FlowEdge, Flowchart, NodeShape};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
