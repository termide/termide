//! Rendered HTML preview panel (read-only).
//!
//! Opened for `.html`/`.htm` files via `F3` (view), this panel renders the
//! document as text pseudographics through the shared HTML engine (see
//! [`termide_html`]). `Enter`/`F4` open the source in the editor instead.
//!
//! The preview is interactive: a movable cursor, keyboard/mouse text selection
//! with copy-to-clipboard, clickable links and image placeholders (open in the
//! browser), incremental search (`Ctrl+F`), and vertical scrolling with line
//! wrapping. The configured toggle-view hotkey (or the `Edit` status chip) swaps
//! the panel in place for the editable source. All of that is the shared
//! [`Viewer`]; this panel adds the loading placeholder of a page being fetched
//! and the export of a page as Markdown.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyModifiers, MouseEvent};
use ratatui::{buffer::Buffer, layout::Rect};

use termide_core::{
    CommandResult, Config, KeyChord, Panel, PanelCommand, PanelEvent, PanelState, RenderContext,
    SegmentKind, StatusSegment, Theme, WidthPreference,
};
use termide_html::render_html;
use termide_viewer::{Viewer, ViewerPanel};

/// Rendered HTML viewer.
pub struct HtmlPanel {
    /// The document, its cursor, search and links.
    view: Viewer,
}

impl HtmlPanel {
    /// Open an HTML file in the preview panel.
    pub fn new(path: PathBuf) -> anyhow::Result<Self> {
        let mut view = Viewer::new(render_html);
        view.set_file(path);
        Ok(Self { view })
    }

    /// Build a viewer over in-memory `source` (e.g. content fetched over HTTP),
    /// with `source_url` as its origin. Not read from, nor written to, disk.
    pub fn from_source(title: String, source: String, source_url: Option<String>) -> Self {
        Self {
            view: Viewer::from_source(render_html, title, source, source_url),
        }
    }

    /// A viewer opened before its page arrives: it shows a spinner and `url`
    /// in the title and a loading line until the app hands it the fetch `id`
    /// result ([`Viewer::apply_fetched`] or [`fail_loading`](Self::fail_loading)).
    pub fn loading(id: u64, url: String) -> Self {
        let mut panel = Self::from_source(String::new(), String::new(), Some(url.clone()));
        panel.view.loading = Some((id, url));
        panel
    }

    /// Whether this is a [`loading`](Self::loading) viewer still without a page.
    pub fn is_placeholder(&self) -> bool {
        self.view.loading.is_some() && self.view.source.is_empty()
    }

    /// The fetch failed: a viewer still without a page shows `message` in
    /// place of it, one that has a page keeps it.
    pub fn fail_loading(&mut self, message: String) {
        if self.is_placeholder() {
            self.view.error = Some(message);
        }
        self.view.loading = None;
    }

    /// Request a "Save As" dialog to export the page as Markdown. Links resolve
    /// against the page URL, or the file for a local page. The default name
    /// follows the file, else the page `<title>`, else the URL host.
    fn save_markdown_event(&self) -> Vec<PanelEvent> {
        let view = &self.view;
        if view.error.is_some() || self.is_placeholder() {
            return vec![];
        }
        let base = match &view.source_url {
            Some(url) => url.clone(),
            None => url::Url::from_file_path(&view.file_path)
                .map(String::from)
                .unwrap_or_default(),
        };
        let converted = termide_html_markdown::html_to_markdown(&view.source, &base);
        let file_stem = match view.source_url {
            None => view.file_path.file_stem().and_then(|s| s.to_str()),
            Some(_) => None,
        };
        let host = view
            .source_url
            .as_deref()
            .and_then(|u| url::Url::parse(u).ok())
            .and_then(|u| u.host_str().map(str::to_string));
        let stem = [file_stem, Some(converted.title.as_str()), host.as_deref()]
            .into_iter()
            .flatten()
            .map(file_name_part)
            .find(|s| !s.is_empty())
            .unwrap_or_else(|| "page".to_string());
        let mut content = converted.markdown;
        content.push('\n');
        vec![PanelEvent::SaveContentAs {
            content,
            default_name: format!("{stem}.md"),
        }]
    }
}

/// `text` made safe as a file name: path separators, reserved and control
/// characters become spaces, runs of whitespace collapse, at most 80 chars.
fn file_name_part(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let joined = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    joined
        .trim_matches('.')
        .trim()
        .chars()
        .take(80)
        .collect::<String>()
        .trim_end()
        .to_string()
}

impl ViewerPanel for HtmlPanel {
    fn view(&self) -> &Viewer {
        &self.view
    }

    fn view_mut(&mut self) -> &mut Viewer {
        &mut self.view
    }
}

impl Panel for HtmlPanel {
    fn name(&self) -> &'static str {
        "html"
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        self.view.handle_command(cmd)
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferWide
    }

    fn title(&self) -> String {
        self.view.title()
    }

    fn icon(&self) -> Option<&str> {
        // A globe for a fetched web page (matching the bookmark icon).
        self.view.source_url.as_ref().map(|_| "🌐")
    }

    fn tick(&mut self) -> Vec<PanelEvent> {
        self.view.tick()
    }

    fn prepare_render(&mut self, theme: &Theme, config: &Arc<Config>) {
        self.view.prepare_render(theme, config);
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        let placeholder = self
            .is_placeholder()
            .then(|| termide_i18n::t().viewer_loading());
        self.view.render(area, buf, ctx, placeholder);
    }

    fn handle_key(&mut self, chord: KeyChord) -> Vec<PanelEvent> {
        if let Some(events) = self.handle_viewer_key(chord) {
            return events;
        }
        // Ctrl+S: export the page as Markdown to a chosen file (Save As).
        let key = chord.canonical;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            return self.save_markdown_event();
        }
        vec![]
    }

    fn handle_scroll(&mut self, delta: i32, _panel_area: Rect) -> Vec<PanelEvent> {
        self.view.handle_scroll(delta)
    }

    fn handle_mouse(&mut self, event: MouseEvent, _panel_area: Rect) -> Vec<PanelEvent> {
        self.handle_viewer_mouse(event)
    }

    fn captures_escape(&self) -> bool {
        self.view.captures_escape()
    }

    fn status_segments(&self) -> Vec<StatusSegment> {
        let t = termide_i18n::t();
        if self.view.error.is_some() {
            return vec![];
        }
        let sep = || StatusSegment::new(" │ ", SegmentKind::Label);
        // Same field order as the editor: View first, then Edit.
        vec![
            StatusSegment::new(" ", SegmentKind::Label),
            StatusSegment::new(format!("{} ", t.status_view()), SegmentKind::Label),
            StatusSegment::new(t.status_view_rendered(), SegmentKind::Value),
            sep(),
            StatusSegment::clickable(
                format!("{} ", t.status_edit()),
                SegmentKind::Label,
                "edit_source",
            ),
            StatusSegment::clickable(t.ui_no(), SegmentKind::Active, "edit_source"),
            sep(),
            StatusSegment::new(format!("{} ", t.status_line()), SegmentKind::Label),
            StatusSegment::new(self.view.line_position(), SegmentKind::Value),
        ]
    }

    fn handle_status_action(&mut self, action: &str) -> Vec<PanelEvent> {
        match action {
            "edit_source" => vec![PanelEvent::SwapActiveToText(self.view.file_path.clone())],
            "save_markdown" => self.save_markdown_event(),
            _ => vec![],
        }
    }

    fn context_menu_items(&self) -> Vec<(String, String)> {
        if self.view.error.is_some() || self.is_placeholder() {
            return vec![];
        }
        let t = termide_i18n::t();
        vec![(
            t.menu_save_page_as_markdown().to_string(),
            "save_markdown".into(),
        )]
    }

    fn reload(&mut self) -> anyhow::Result<()> {
        self.view.reload();
        Ok(())
    }

    fn to_state(&self, _project_dir: &Path) -> Option<PanelState> {
        // Only file-backed viewers persist; fetched URLs are not restored.
        if self.view.source_url.is_some() {
            return None;
        }
        Some(PanelState::Html {
            path: self.view.file_path.clone(),
        })
    }

    fn get_working_directory(&self) -> Option<PathBuf> {
        self.view.working_directory()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn panel_from(src: &str) -> HtmlPanel {
        let mut view = Viewer::new(render_html);
        view.file_path = PathBuf::from("/x/page.html");
        view.title = "page.html".to_string();
        view.source = src.to_string();
        view.last_area = Rect::new(0, 0, 80, 10);
        view.relayout_if_needed(80);
        HtmlPanel { view }
    }

    fn press(p: &mut HtmlPanel, code: KeyCode, modifiers: KeyModifiers) -> Vec<PanelEvent> {
        let key = KeyEvent::new(code, modifiers);
        p.handle_key(KeyChord {
            raw: key,
            canonical: key,
        })
    }

    #[test]
    fn name_is_html() {
        let p = panel_from("<p>hi</p>");
        assert_eq!(p.name(), "html");
    }

    #[test]
    fn edit_source_action_swaps_to_text() {
        let mut p = panel_from("<p>hi</p>");
        let evs = p.handle_status_action("edit_source");
        assert!(matches!(evs.as_slice(), [PanelEvent::SwapActiveToText(_)]));
    }

    #[test]
    fn to_state_round_trips_path() {
        let p = panel_from("<p>x</p>");
        match p.to_state(Path::new("/tmp")) {
            Some(PanelState::Html { path }) => {
                assert_eq!(path, PathBuf::from("/x/page.html"))
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn link_under_cursor_opens() {
        let mut p = panel_from("<p><a href=\"https://ex.com\">docs</a></p>");
        assert_eq!(p.view.doc.links.len(), 1, "{:?}", p.view.doc.links);
        assert_eq!(p.view.doc.links[0].url, "https://ex.com");
        let evs = press(&mut p, KeyCode::Enter, KeyModifiers::NONE);
        assert!(
            matches!(evs.as_slice(), [PanelEvent::OpenLink(_)]),
            "{evs:?}"
        );
    }

    #[test]
    fn ctrl_a_selects_the_whole_page_for_ctrl_c() {
        let mut p = panel_from("<h1>Top</h1><p>middle</p><p>end</p>");
        let whole: Vec<String> = (0..p.view.line_count())
            .map(|i| p.view.line_text(i))
            .collect();
        press(&mut p, KeyCode::Char('a'), KeyModifiers::CONTROL);
        let evs = press(&mut p, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(
            matches!(evs.as_slice(), [PanelEvent::CopyToClipboard(t)] if *t == whole.join("\n")),
            "{evs:?}"
        );
        assert!(whole.iter().any(|l| l.contains("end")));
    }

    fn saved(evs: &[PanelEvent]) -> (&str, &str) {
        match evs {
            [PanelEvent::SaveContentAs {
                content,
                default_name,
            }] => (content, default_name),
            other => panic!("expected SaveContentAs, got {other:?}"),
        }
    }

    #[test]
    fn a_file_page_saves_as_markdown_next_to_its_name() {
        let mut p = panel_from(r#"<h1>Title</h1><p>See <a href="b.html">b</a>.</p>"#);
        assert_eq!(
            p.context_menu_items()
                .iter()
                .map(|(_, id)| id.as_str())
                .collect::<Vec<_>>(),
            ["save_markdown"]
        );
        let evs = p.handle_status_action("save_markdown");
        let (content, name) = saved(&evs);
        assert_eq!(content, "# Title\n\nSee [b](file:///x/b.html).\n");
        assert_eq!(name, "page.md");
    }

    #[test]
    fn a_fetched_page_is_named_after_its_title_else_its_host() {
        let src = r#"<head><title> A/B: "c" </title></head><p><a href="/d">d</a></p>"#;
        let mut p = HtmlPanel::from_source("t".into(), src.into(), Some("https://ex.com/p".into()));
        let evs = press(&mut p, KeyCode::Char('s'), KeyModifiers::CONTROL);
        let (content, name) = saved(&evs);
        assert_eq!(content, "[d](https://ex.com/d)\n");
        assert_eq!(name, "A B c.md");

        let p = HtmlPanel::from_source(
            "t".into(),
            "<p>x</p>".into(),
            Some("https://ex.com/".into()),
        );
        assert_eq!(saved(&p.save_markdown_event()).1, "ex.com.md");
    }

    #[test]
    fn an_unreadable_file_offers_no_export() {
        let mut p = HtmlPanel::new(PathBuf::from("/nonexistent/x.html")).unwrap();
        assert!(p.context_menu_items().is_empty());
        assert!(p.handle_status_action("save_markdown").is_empty());
    }

    #[test]
    fn a_loading_viewer_spins_in_its_title_until_the_page_arrives() {
        let mut p = HtmlPanel::loading(4, "https://ex.com/a".into());
        let title = Panel::title(&p);
        assert!(title.ends_with(" https://ex.com/a"), "{title}");
        assert!(termide_config::constants::SPINNER_FRAMES
            .iter()
            .any(|f| title.starts_with(f)));
        assert!(matches!(p.tick().as_slice(), [PanelEvent::NeedsRedraw]));
        assert!(p.context_menu_items().is_empty(), "nothing to save yet");

        p.view
            .apply_fetched("a".into(), "<p>hi</p>".into(), "https://ex.com/a".into());
        assert_eq!(Panel::title(&p), "https://ex.com/a");
        assert!(p.tick().is_empty());
        assert!(!p.context_menu_items().is_empty());
    }

    #[test]
    fn a_failed_load_shows_in_a_placeholder_only() {
        let mut p = HtmlPanel::loading(1, "https://ex.com/a".into());
        p.fail_loading("Fetch failed: x".into());
        assert_eq!(p.view.error.as_deref(), Some("Fetch failed: x"));
        assert_eq!(p.view.loading_id(), None);

        let mut p = panel_from("<p>page</p>");
        p.view.start_loading(2, "https://ex.com/b".into());
        p.fail_loading("Fetch failed: x".into());
        assert_eq!(p.view.error, None);
        assert_eq!(p.view.loading_id(), None);
    }
}
