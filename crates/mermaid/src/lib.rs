//! Mermaid parsing and text-pseudographics layout — a UI-agnostic library.
//!
//! Returns plain `Vec<String>` canvases so it can be reused both by the
//! standalone Mermaid viewer panel and by the Markdown renderer for embedded
//! ```` ```mermaid ```` blocks. No terminal/UI dependencies.

mod canvas;
pub mod chart;
pub mod flowchart;
pub mod gitgraph;
pub mod parser;
pub mod relational;
pub mod render;

pub use parser::{
    detect_kind, parse_class, parse_er, parse_flowchart, parse_gantt, parse_gitgraph,
    parse_journey, parse_mindmap, parse_pie, parse_quadrant, parse_sequence, parse_state,
    parse_timeline, ClassDiagram, DiagramKind, ErDiagram, Flowchart, Gantt, GitGraph, Journey,
    Mindmap, Pie, Quadrant, Sequence, Timeline,
};
pub use render::render_sequence;

pub use chart::{
    render_gantt, render_journey, render_mindmap, render_pie, render_quadrant, render_timeline,
};
pub use flowchart::render_flowchart;
pub use gitgraph::render_gitgraph;
pub use relational::{render_class, render_er};

/// Render any supported Mermaid source to canvas lines. Unsupported kinds
/// return `None` so the caller can fall back (e.g. show the source).
#[must_use]
pub fn render_to_lines(src: &str) -> Option<Vec<String>> {
    let src = parser::diagram_body(src);
    match detect_kind(src) {
        DiagramKind::Sequence => Some(render_sequence(&parse_sequence(src))),
        DiagramKind::Flowchart => Some(render_flowchart(&parse_flowchart(src))),
        DiagramKind::State => Some(render_flowchart(&parse_state(src))),
        DiagramKind::Pie => Some(render_pie(&parse_pie(src))),
        DiagramKind::Class => Some(render_class(&parse_class(src))),
        DiagramKind::Er => Some(render_er(&parse_er(src))),
        DiagramKind::Gantt => Some(render_gantt(&parse_gantt(src))),
        DiagramKind::Journey => Some(render_journey(&parse_journey(src))),
        DiagramKind::Mindmap => Some(render_mindmap(&parse_mindmap(src))),
        DiagramKind::Timeline => Some(render_timeline(&parse_timeline(src))),
        DiagramKind::GitGraph => Some(render_gitgraph(&parse_gitgraph(src))),
        DiagramKind::Quadrant => Some(render_quadrant(&parse_quadrant(src))),
        DiagramKind::Other | DiagramKind::Unknown => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn boxes_around_wide_labels_line_up() {
        let lines = render_to_lines("graph TD\n  A[日本語] --> B[ok]\n").expect("flowchart");
        let label = lines
            .iter()
            .position(|l| l.contains("日本語"))
            .expect("label drawn");
        let top = &lines[label - 1];
        let row = &lines[label];
        let right_corner = top.width() - top.trim_start().width() + top.trim().width();
        let right_wall = row.width() - row.trim_start().width() + row.trim().width();
        assert_eq!(right_corner, right_wall, "{lines:#?}");
        assert!(row.contains("│日本語│"), "{lines:#?}");
    }

    #[test]
    fn front_matter_is_not_drawn() {
        let lines = render_to_lines("---\ntitle: T\n---\nclassDiagram-v2\n  Animal <|-- Dog\n")
            .expect("class diagram");
        let text = lines.join("\n");
        assert!(text.contains("Animal") && text.contains("Dog"), "{text}");
        assert!(!text.contains("title") && !text.contains("---"), "{text}");
    }

    #[test]
    fn a_gantt_row_with_a_wide_task_name_keeps_its_columns() {
        let lines = render_to_lines(
            "gantt\n  dateFormat YYYY-MM-DD\n  section S\n  日本 :a, 2024-01-01, 3d\n  ab :b, 2024-01-01, 3d\n",
        )
        .expect("gantt");
        let wide = lines.iter().find(|l| l.contains("日本")).expect("wide row");
        let narrow = lines.iter().find(|l| l.contains("ab")).expect("narrow row");
        let bar = |l: &str| {
            let at = l.find('▒').expect("bar");
            l[..at].width()
        };
        assert_eq!(bar(wide), bar(narrow), "{lines:#?}");
    }
}
