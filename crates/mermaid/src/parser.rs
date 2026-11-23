//! Minimal Mermaid parser.
//!
//! Parses `sequenceDiagram`, `flowchart`/`graph`, `stateDiagram`, `pie`,
//! `classDiagram`, and `erDiagram`. Other kinds are detected (so the viewer can
//! show an informative placeholder) but not yet parsed.
//!
//! Diagram grammars live in per-type submodules; this root keeps the shared
//! [`DiagramKind`] detection and the graph model (`Direction`, `Flowchart`, …)
//! reused by the flowchart and state parsers, re-exporting each submodule's
//! public parse entry points and types.

mod chart;
mod gitgraph;
mod graph;
mod relational;
mod sequence;

pub use chart::{
    day_to_date, parse_gantt, parse_journey, parse_mindmap, parse_pie, parse_quadrant,
    parse_timeline, Gantt, GanttTask, Journey, JourneyTask, MindNode, Mindmap, Pie, QuadPoint,
    Quadrant, TaskStatus, Timeline, TimelineEntry,
};
pub use gitgraph::{parse_gitgraph, GitGraph, GitOp};
pub use graph::{parse_flowchart, parse_state};
pub use relational::{
    parse_class, parse_er, ClassDiagram, ClassEntry, ErDiagram, ErEntry, Relation,
};
pub use sequence::{
    parse_sequence, Arrow, ArrowHead, NotePlacement, Participant, SeqEvent, Sequence,
};

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

/// The source from its header line on: a leading YAML front-matter block
/// (`---` … `---`), comments and `%%{init: …}%%` directives (which may span
/// several lines) come before the header and are skipped.
#[must_use]
pub fn diagram_body(src: &str) -> &str {
    let mut rest = src;
    let mut front_matter = false;
    let mut directive = false;
    let mut first = true;
    while !rest.is_empty() {
        let (raw, next) = match rest.find('\n') {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, ""),
        };
        let line = raw.trim();
        if front_matter {
            front_matter = line != "---";
        } else if directive {
            directive = !line.contains("}%%");
        } else if first && line == "---" {
            front_matter = true;
        } else if line.starts_with("%%{") {
            directive = !line.contains("}%%");
        } else if !line.is_empty() && !line.starts_with("%%") {
            return rest;
        }
        if !line.is_empty() {
            first = false;
        }
        rest = next;
    }
    rest
}

/// Detect the diagram kind from its header line.
#[must_use]
pub fn detect_kind(src: &str) -> DiagramKind {
    let line = diagram_body(src).lines().next().unwrap_or("").trim();
    let head = line.split_whitespace().next().unwrap_or("");
    match head {
        "sequenceDiagram" => DiagramKind::Sequence,
        "flowchart" | "flowchart-elk" | "graph" => DiagramKind::Flowchart,
        "stateDiagram" | "stateDiagram-v2" => DiagramKind::State,
        "pie" => DiagramKind::Pie,
        "classDiagram" | "classDiagram-v2" => DiagramKind::Class,
        "erDiagram" => DiagramKind::Er,
        "gantt" => DiagramKind::Gantt,
        "journey" => DiagramKind::Journey,
        "mindmap" => DiagramKind::Mindmap,
        "timeline" => DiagramKind::Timeline,
        "gitGraph" => DiagramKind::GitGraph,
        "quadrantChart" => DiagramKind::Quadrant,
        "requirementDiagram" | "C4Context" | "C4Container" | "C4Component" | "C4Dynamic"
        | "C4Deployment" | "zenuml" | "sankey" | "sankey-beta" | "xychart" | "xychart-beta"
        | "block" | "block-beta" | "packet" | "packet-beta" | "kanban" | "architecture"
        | "architecture-beta" | "radar-beta" | "treemap" | "treemap-beta" => DiagramKind::Other,
        _ => DiagramKind::Unknown,
    }
}

// ===========================================================================
// Shared graph model (reused by the flowchart and state parsers, and by the
// flowchart/relational renderers)
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
    fn front_matter_and_directives_come_before_the_header() {
        let src = "---\ntitle: Flow\nconfig:\n  theme: dark\n---\nflowchart LR\nA-->B";
        assert_eq!(detect_kind(src), DiagramKind::Flowchart);
        assert_eq!(diagram_body(src), "flowchart LR\nA-->B");
        let src = "%%{init: {'theme': 'dark'}}%%\nsequenceDiagram\nA->>B: hi";
        assert_eq!(detect_kind(src), DiagramKind::Sequence);
        let src = "%%{\n  init: {\n    'theme': 'dark'\n  }\n}%%\ngantt\n";
        assert_eq!(detect_kind(src), DiagramKind::Gantt);
    }

    #[test]
    fn header_variants_and_unrendered_kinds() {
        assert_eq!(detect_kind("classDiagram-v2\nA <|-- B"), DiagramKind::Class);
        assert_eq!(
            detect_kind("flowchart-elk TD\nA-->B"),
            DiagramKind::Flowchart
        );
        for head in [
            "C4Container",
            "C4Component",
            "C4Dynamic",
            "C4Deployment",
            "sankey-beta",
            "xychart-beta",
            "block-beta",
            "kanban",
            "architecture-beta",
            "packet-beta",
            "radar-beta",
        ] {
            assert_eq!(detect_kind(head), DiagramKind::Other, "{head}");
        }
    }
}
