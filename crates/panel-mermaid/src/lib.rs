//! Mermaid diagram viewer panel (read-only, experimental).
//!
//! Opened for `.mmd` files via `F3` (view); `Enter`/`F4` open the source in the
//! editor. Sequence and flowchart diagrams render as text pseudographics (via
//! the `termide-mermaid` crate); other kinds show an informative placeholder
//! over the source until their layout lands. `Ctrl+E` (or the `Edit` status
//! chip) swaps the panel in place for the editable source. `y`/`Ctrl+C` (or the
//! `Copy diagram` entry in the panel `[≡]` menu) copies the rendered diagram to
//! the system clipboard. `Ctrl+S` (or the `Save diagram as…` menu entry) exports
//! the diagram source to a chosen `.mmd` file. The canvas scrolls in two
//! dimensions.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect, style::Modifier, style::Style};

use termide_core::{
    Config, HotkeyTable, KeyChord, Panel, PanelEvent, RenderContext, SegmentKind, SessionPanel,
    StatusSegment, Theme, ThemeColors, WidthPreference,
};
use termide_mermaid::parser::{self, DiagramKind};
use termide_ui::ScrollBar;

/// Mermaid diagram viewer.
pub struct MermaidPanel {
    file_path: PathBuf,
    title: String,
    source: String,
    error: Option<String>,

    /// Detected diagram kind.
    kind: DiagramKind,
    /// Whether `canvas` is a real rendered diagram (vs. a source placeholder).
    rendered: bool,
    /// Rendered canvas lines (diagram pseudographics or a placeholder).
    canvas: Vec<String>,
    /// Widest canvas line (for horizontal scroll bounds).
    canvas_width: usize,
    /// Scroll offsets (top row, left column).
    scroll_y: usize,
    scroll_x: usize,
    /// Content area from the last render (for paging / clamping).
    last_area: Rect,

    colors: ThemeColors,
    is_light: bool,
    hotkeys: HotkeyTable,
    last_config_ptr: usize,
}

impl MermaidPanel {
    /// Open a `.mmd` file in the diagram viewer.
    pub fn new(path: PathBuf) -> anyhow::Result<Self> {
        let mut panel = Self {
            file_path: path.clone(),
            title: String::new(),
            source: String::new(),
            error: None,
            kind: DiagramKind::Unknown,
            rendered: false,
            canvas: Vec::new(),
            canvas_width: 0,
            scroll_y: 0,
            scroll_x: 0,
            last_area: Rect::default(),
            colors: ThemeColors::default(),
            is_light: false,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
        };
        panel.set_file(path);
        Ok(panel)
    }

    /// Point the panel at a new file, reloading and re-rendering.
    pub fn set_file(&mut self, path: PathBuf) {
        self.title = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        self.file_path = path;
        self.scroll_x = 0;
        self.scroll_y = 0;
        match std::fs::read_to_string(&self.file_path) {
            Ok(s) => {
                self.source = s;
                self.error = None;
            }
            Err(e) => {
                self.source = String::new();
                self.error = Some(e.to_string());
            }
        }
        self.rebuild();
    }

    /// Parse + lay out the current source into `canvas`. Supported kinds render
    /// a diagram; the rest fall back to a labelled source view.
    fn rebuild(&mut self) {
        self.kind = parser::detect_kind(&self.source);
        self.canvas = match termide_mermaid::render_to_lines(&self.source) {
            Some(lines) => {
                self.rendered = true;
                lines
            }
            None => {
                self.rendered = false;
                self.placeholder()
            }
        };
        self.canvas_width = self
            .canvas
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
    }

    /// Source view with a one-line note for kinds without a layout yet.
    fn placeholder(&self) -> Vec<String> {
        let note = match self.kind {
            DiagramKind::Flowchart => "Flowchart layout is not implemented yet — showing source:",
            DiagramKind::Other => "This diagram kind is not rendered yet — showing source:",
            _ => "Not a recognized Mermaid diagram — showing source:",
        };
        let mut out = vec![note.to_string(), String::new()];
        out.extend(self.source.lines().map(|l| l.to_string()));
        out
    }

    fn type_label(&self) -> &'static str {
        match self.kind {
            DiagramKind::Sequence => "sequence",
            DiagramKind::Flowchart => "flowchart",
            DiagramKind::State => "state",
            DiagramKind::Pie => "pie",
            DiagramKind::Class => "class",
            DiagramKind::Er => "er",
            DiagramKind::Gantt => "gantt",
            DiagramKind::Journey => "journey",
            DiagramKind::Mindmap => "mindmap",
            DiagramKind::Timeline => "timeline",
            DiagramKind::GitGraph => "gitgraph",
            DiagramKind::Quadrant => "quadrant",
            DiagramKind::Other => "other",
            DiagramKind::Unknown => "text",
        }
    }

    fn viewport_h(&self) -> usize {
        self.last_area.height.max(1) as usize
    }

    fn viewport_w(&self) -> usize {
        // One column reserved for the scrollbar gutter.
        self.last_area.width.saturating_sub(1).max(1) as usize
    }

    fn max_scroll_y(&self) -> usize {
        self.canvas.len().saturating_sub(self.viewport_h())
    }

    fn max_scroll_x(&self) -> usize {
        self.canvas_width.saturating_sub(self.viewport_w())
    }

    fn scroll_v(&mut self, delta: i32) {
        let max = self.max_scroll_y() as i64;
        self.scroll_y = (self.scroll_y as i64 + delta as i64).clamp(0, max) as usize;
    }

    fn scroll_h(&mut self, delta: i32) {
        let max = self.max_scroll_x() as i64;
        self.scroll_x = (self.scroll_x as i64 + delta as i64).clamp(0, max) as usize;
    }

    /// The viewed diagram as plain text: the rendered pseudographics canvas
    /// (trailing blanks trimmed per line). Falls back to the raw source when no
    /// diagram is rendered, so the action always yields something useful.
    fn copy_text(&self) -> String {
        if self.rendered {
            self.canvas
                .iter()
                .map(|l| l.trim_end())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            self.source.clone()
        }
    }

    /// Copy the viewed diagram to the system clipboard with a status note.
    fn copy_to_clipboard(&self) -> Vec<PanelEvent> {
        let text = self.copy_text();
        let lines = text.lines().count();
        vec![
            PanelEvent::CopyToClipboard(text),
            PanelEvent::SetStatusMessage {
                message: termide_i18n::t().status_diagram_copied(lines),
                is_error: false,
            },
        ]
    }

    /// Request a "Save As" dialog to export the diagram source (`.mmd`). The
    /// default name follows the current file, or `diagram.mmd` for a generated
    /// temp file.
    fn save_as_event(&self) -> Vec<PanelEvent> {
        let stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("diagram");
        let default_name = if stem.starts_with("termide-diagram") {
            "diagram.mmd".to_string()
        } else {
            format!("{stem}.mmd")
        };
        vec![PanelEvent::SaveContentAs {
            content: self.source.clone(),
            default_name,
        }]
    }
}

impl Panel for MermaidPanel {
    fn name(&self) -> &'static str {
        "mermaid"
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferWide
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn prepare_render(&mut self, theme: &Theme, config: &Arc<Config>) {
        self.colors = ThemeColors::from(theme);
        self.is_light = theme.is_light_theme();

        let ptr = Arc::as_ptr(config) as usize;
        if self.last_config_ptr != ptr {
            self.last_config_ptr = ptr;
            let mut t = HotkeyTable::new();
            t.insert("toggle_view", &config.viewer.keybindings.toggle_view);
            self.hotkeys = t;
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        self.last_area = area;
        buf.set_style(area, Style::default().fg(self.colors.fg).bg(self.colors.bg));

        if let Some(err) = &self.error {
            let msg = ratatui::text::Line::styled(
                format!(" Cannot open: {err}"),
                Style::default().fg(self.colors.error),
            );
            buf.set_line(area.x, area.y, &msg, area.width);
            return;
        }

        self.scroll_y = self.scroll_y.min(self.max_scroll_y());
        self.scroll_x = self.scroll_x.min(self.max_scroll_x());

        let text_w = self.viewport_w();
        let note_style = Style::default().fg(self.colors.fg);
        let dim = Style::default().fg(self.colors.disabled);
        for i in 0..(area.height as usize) {
            let Some(line) = self.canvas.get(self.scroll_y + i) else {
                break;
            };
            // The placeholder's leading note line is dimmed; section headers
            // (gantt/journey `▌ …` rows) are bold.
            let style = if !self.rendered && self.scroll_y + i == 0 {
                dim
            } else if line.trim_start().starts_with('▌') {
                note_style.add_modifier(Modifier::BOLD)
            } else {
                note_style
            };
            let visible: String = line.chars().skip(self.scroll_x).take(text_w).collect();
            let l = ratatui::text::Line::styled(visible, style);
            buf.set_line(area.x, area.y + i as u16, &l, text_w as u16);
        }

        // Draw the scrollbar on the panel's right border (replacing it), not one
        // column inside it — otherwise it reads as detached from the edge.
        ScrollBar::render(
            buf,
            ctx.border_right_x.unwrap_or(area.x + area.width - 1),
            area.y,
            area.height,
            self.scroll_y,
            self.viewport_h(),
            self.canvas.len(),
            &self.colors,
            ctx.is_focused,
        );
    }

    fn handle_key(&mut self, chord: KeyChord) -> Vec<PanelEvent> {
        let key = chord.raw;
        if self.hotkeys.matches("toggle_view", &key) {
            return vec![PanelEvent::SwapActiveToText(self.file_path.clone())];
        }
        // Copy the whole diagram: `y` (yank) or Ctrl+C.
        if matches!(key.code, KeyCode::Char('y'))
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            return self.copy_to_clipboard();
        }
        // Ctrl+S: export the diagram source to a chosen file (Save As).
        if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.save_as_event();
        }
        // Ctrl+R: re-read the source from disk (pick up external edits), keeping
        // the scroll position.
        if key.code == KeyCode::Char('r') && key.modifiers.contains(KeyModifiers::CONTROL) {
            let (sx, sy) = (self.scroll_x, self.scroll_y);
            self.set_file(self.file_path.clone());
            self.scroll_x = sx.min(self.max_scroll_x());
            self.scroll_y = sy.min(self.max_scroll_y());
            return vec![PanelEvent::NeedsRedraw];
        }
        let page = (self.viewport_h() as i32 - 1).max(1);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.scroll_v(-1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_v(1),
            KeyCode::Left | KeyCode::Char('h') => self.scroll_h(-2),
            KeyCode::Right | KeyCode::Char('l') => self.scroll_h(2),
            KeyCode::PageUp => self.scroll_v(-page),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_v(page),
            KeyCode::Home | KeyCode::Char('g') => {
                self.scroll_y = 0;
                self.scroll_x = 0;
            }
            KeyCode::End | KeyCode::Char('G') => self.scroll_y = self.max_scroll_y(),
            _ => return vec![],
        }
        vec![PanelEvent::NeedsRedraw]
    }

    fn handle_scroll(&mut self, delta: i32, _panel_area: Rect) -> Vec<PanelEvent> {
        self.scroll_v(delta);
        vec![PanelEvent::NeedsRedraw]
    }

    fn handle_mouse(&mut self, event: MouseEvent, _panel_area: Rect) -> Vec<PanelEvent> {
        match event.kind {
            MouseEventKind::ScrollDown => self.scroll_v(3),
            MouseEventKind::ScrollUp => self.scroll_v(-3),
            // Shift+wheel (reported as horizontal) pans sideways on some terminals.
            MouseEventKind::ScrollRight => self.scroll_h(3),
            MouseEventKind::ScrollLeft => self.scroll_h(-3),
            MouseEventKind::Down(MouseButton::Left) => return vec![],
            _ => return vec![],
        }
        vec![PanelEvent::NeedsRedraw]
    }

    fn status_segments(&self) -> Vec<StatusSegment> {
        if self.error.is_some() {
            return vec![];
        }
        let sep = || StatusSegment::new(" │ ", SegmentKind::Label);
        vec![
            StatusSegment::new(" ", SegmentKind::Label),
            StatusSegment::new("View: ", SegmentKind::Label),
            StatusSegment::new("Rendered", SegmentKind::Value),
            sep(),
            StatusSegment::clickable("Edit: ", SegmentKind::Label, "edit_source"),
            StatusSegment::clickable("No", SegmentKind::Active, "edit_source"),
            sep(),
            StatusSegment::new("Type: ", SegmentKind::Label),
            StatusSegment::new(self.type_label(), SegmentKind::Value),
        ]
    }

    fn handle_status_action(&mut self, action: &str) -> Vec<PanelEvent> {
        match action {
            "edit_source" => vec![PanelEvent::SwapActiveToText(self.file_path.clone())],
            "copy_diagram" => self.copy_to_clipboard(),
            "save_diagram" => self.save_as_event(),
            _ => vec![],
        }
    }

    fn context_menu_items(&self) -> Vec<(String, &'static str)> {
        if self.error.is_some() {
            return vec![];
        }
        let t = termide_i18n::t();
        vec![
            (t.menu_copy_diagram().to_string(), "copy_diagram"),
            (t.menu_save_diagram_as().to_string(), "save_diagram"),
        ]
    }

    fn reload(&mut self) -> anyhow::Result<()> {
        let path = self.file_path.clone();
        self.set_file(path);
        Ok(())
    }

    fn to_session(&self, _session_dir: &Path) -> Option<SessionPanel> {
        Some(SessionPanel::Mermaid {
            path: self.file_path.clone(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn get_working_directory(&self) -> Option<PathBuf> {
        self.file_path.parent().map(|p| p.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel_from(src: &str) -> MermaidPanel {
        let mut p = MermaidPanel {
            file_path: PathBuf::from("/x/d.mmd"),
            title: "d.mmd".to_string(),
            source: src.to_string(),
            error: None,
            kind: DiagramKind::Unknown,
            rendered: false,
            canvas: Vec::new(),
            canvas_width: 0,
            scroll_y: 0,
            scroll_x: 0,
            last_area: Rect::new(0, 0, 40, 10),
            colors: ThemeColors::default(),
            is_light: false,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
        };
        p.rebuild();
        p
    }

    #[test]
    fn name_is_mermaid() {
        assert_eq!(panel_from("sequenceDiagram").name(), "mermaid");
    }

    #[test]
    fn sequence_builds_canvas() {
        let p = panel_from("sequenceDiagram\nA->>B: hi");
        assert!(p.kind == DiagramKind::Sequence);
        assert!(p.canvas.iter().any(|l| l.contains("hi")));
        assert!(p.canvas_width > 0);
    }

    #[test]
    fn flowchart_renders_diagram() {
        let p = panel_from("flowchart TD\nA[Start]-->B[End]");
        assert_eq!(p.kind, DiagramKind::Flowchart);
        assert!(p.rendered, "flowchart should render, not fall back");
        assert!(p.canvas.iter().any(|l| l.contains('┌')), "{:?}", p.canvas);
    }

    #[test]
    fn unsupported_kind_shows_placeholder_and_source() {
        let p = panel_from("requirementDiagram\nrequirement R {\n}");
        assert_eq!(p.kind, DiagramKind::Other);
        assert!(!p.rendered);
        assert!(p.canvas[0].contains("not rendered yet"), "{:?}", p.canvas);
        assert!(p.canvas.iter().any(|l| l.contains("requirement R")));
    }

    #[test]
    fn edit_source_swaps_to_text() {
        let mut p = panel_from("sequenceDiagram");
        assert!(matches!(
            p.handle_status_action("edit_source").as_slice(),
            [PanelEvent::SwapActiveToText(_)]
        ));
    }

    #[test]
    fn copy_emits_clipboard_and_status() {
        let mut p = panel_from("sequenceDiagram\nA->>B: hi");
        let evs = p.handle_key(KeyChord::identity(crossterm::event::KeyEvent::from(
            KeyCode::Char('y'),
        )));
        match evs.as_slice() {
            [PanelEvent::CopyToClipboard(text), PanelEvent::SetStatusMessage { is_error, .. }] => {
                assert!(text.contains("hi"), "diagram text not copied: {text:?}");
                assert!(!is_error);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn to_session_round_trips() {
        let p = panel_from("sequenceDiagram");
        match p.to_session(Path::new("/tmp")) {
            Some(SessionPanel::Mermaid { path }) => assert_eq!(path, PathBuf::from("/x/d.mmd")),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
