//! Rendered Markdown preview panel (read-only).
//!
//! Opened for `.md` files via `F3` (view), this panel parses the document with
//! `pulldown-cmark` and draws it as text pseudographics (see [`render`]).
//! `Enter`/`F4` open the source in the editor instead.
//!
//! The preview is interactive: a movable cursor, keyboard/mouse text selection
//! with copy-to-clipboard, clickable links and image placeholders (open in the
//! browser), incremental search (`Ctrl+F`), and vertical scrolling with line
//! wrapping. `Ctrl+E` (or the `Edit` status chip) swaps the panel in place for
//! the editable source. All of that is the shared
//! [`Viewer`](termide_viewer::Viewer); this panel adds what plugin
//! panels need.

mod render;
mod stream;

pub use render::{render_markdown, render_markdown_with};
pub use stream::{render_streamed, StreamedHead};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossterm::event::MouseEvent;
use ratatui::{buffer::Buffer, layout::Rect};

use termide_core::{
    CommandResult, Config, KeyChord, Panel, PanelCommand, PanelEvent, PanelState, RenderContext,
    SegmentKind, StatusSegment, Theme, WidthPreference,
};
use termide_viewer::{Viewer, ViewerPanel};

/// What a plugin panel's own `[≡]` entries are routed back under.
const PLUGIN_MENU_PREFIX: &str = "plugin-menu:";

/// Rendered Markdown viewer.
pub struct MarkdownPanel {
    /// The document, its cursor, search and links.
    view: Viewer,
    /// The id the app gave the panel when a plugin opened it; its content
    /// comes from the plugin, a `plugin:` link is an action for it, and it
    /// has no source to edit nor a place in the saved layout.
    plugin_panel: Option<u64>,
    /// A plugin panel's own entries of its `[≡]` menu: label and action.
    plugin_menu: Vec<(String, String)>,
}

impl MarkdownPanel {
    fn with_view(view: Viewer) -> Self {
        Self {
            view,
            plugin_panel: None,
            plugin_menu: Vec::new(),
        }
    }

    /// A panel a plugin opened: `source` is its Markdown, `id` what the app
    /// knows it by.
    pub fn for_plugin(id: u64, title: String, source: String) -> Self {
        let mut panel = Self::with_view(Viewer::new(render_markdown));
        panel.view.title = title;
        panel.view.source = source;
        panel.plugin_panel = Some(id);
        panel
    }

    /// Set a plugin panel's own `[≡]` entries, `(label, action)`: one chosen
    /// is handed to the plugin as the action.
    pub fn set_plugin_menu(&mut self, menu: Vec<(String, String)>) {
        self.plugin_menu = menu;
    }

    /// The id of a panel a plugin opened; `None` for any other viewer.
    pub fn plugin_panel_id(&self) -> Option<u64> {
        self.plugin_panel
    }

    /// Replace a plugin panel's content, keeping the reader where they were
    /// as far as the new content reaches.
    pub fn set_plugin_content(&mut self, title: Option<String>, source: String) {
        let view = &mut self.view;
        if let Some(title) = title {
            view.title = title;
        }
        view.source = source;
        view.matches.clear();
        view.anchor = None;
        // Laid out again on the next render, which clamps the position.
        view.layout_width = 0;
    }

    /// Open a markdown file in the preview panel.
    pub fn new(path: PathBuf) -> anyhow::Result<Self> {
        let mut view = Viewer::new(render_markdown);
        view.set_file(path);
        Ok(Self::with_view(view))
    }

    /// Build a viewer over in-memory `source` (e.g. content fetched over HTTP),
    /// with `source_url` as its origin. Not read from, nor written to, disk.
    pub fn from_source(title: String, source: String, source_url: Option<String>) -> Self {
        Self::with_view(Viewer::from_source(
            render_markdown,
            title,
            source,
            source_url,
        ))
    }
}

impl ViewerPanel for MarkdownPanel {
    fn view(&self) -> &Viewer {
        &self.view
    }

    fn view_mut(&mut self) -> &mut Viewer {
        &mut self.view
    }

    /// A plugin's panel has no source to edit nor a file to re-read.
    fn has_source_file(&self) -> bool {
        self.plugin_panel.is_none()
    }

    fn follow_link(&mut self, href: &str) -> Vec<PanelEvent> {
        // In a plugin's panel, `plugin:<action>` is a button of the plugin's.
        if let (Some(panel), Some(action)) = (self.plugin_panel, href.strip_prefix("plugin:")) {
            return vec![PanelEvent::PluginAction {
                panel,
                action: action.to_string(),
            }];
        }
        self.view.activate_link(href)
    }
}

impl Panel for MarkdownPanel {
    fn name(&self) -> &'static str {
        "markdown"
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
        if self.plugin_panel.is_some() {
            return Some("🧩");
        }
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
        self.view.render(area, buf, ctx, None);
    }

    fn handle_key(&mut self, chord: KeyChord) -> Vec<PanelEvent> {
        self.handle_viewer_key(chord).unwrap_or_default()
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
        if self.view.error.is_some() {
            return vec![];
        }
        let sep = || StatusSegment::new(" │ ", SegmentKind::Label);
        let line = StatusSegment::new(self.view.line_position(), SegmentKind::Value);
        if self.plugin_panel.is_some() {
            return vec![
                StatusSegment::new(" ", SegmentKind::Label),
                StatusSegment::new("Line: ", SegmentKind::Label),
                line,
            ];
        }
        // Same field order as the editor: View first, then Edit.
        vec![
            StatusSegment::new(" ", SegmentKind::Label),
            StatusSegment::new("View: ", SegmentKind::Label),
            StatusSegment::new("Rendered", SegmentKind::Value),
            sep(),
            StatusSegment::clickable("Edit: ", SegmentKind::Label, "edit_source"),
            StatusSegment::clickable("No", SegmentKind::Active, "edit_source"),
            sep(),
            StatusSegment::new("Line: ", SegmentKind::Label),
            line,
        ]
    }

    fn context_menu_items(&self) -> Vec<(String, String)> {
        self.plugin_menu
            .iter()
            .map(|(label, action)| (label.clone(), format!("{PLUGIN_MENU_PREFIX}{action}")))
            .collect()
    }

    fn handle_status_action(&mut self, action: &str) -> Vec<PanelEvent> {
        if let (Some(panel), Some(action)) =
            (self.plugin_panel, action.strip_prefix(PLUGIN_MENU_PREFIX))
        {
            return vec![PanelEvent::PluginAction {
                panel,
                action: action.to_string(),
            }];
        }
        match action {
            "edit_source" if self.plugin_panel.is_none() => {
                vec![PanelEvent::SwapActiveToText(self.view.file_path.clone())]
            }
            _ => vec![],
        }
    }

    fn reload(&mut self) -> anyhow::Result<()> {
        self.view.reload();
        Ok(())
    }

    fn to_state(&self, _project_dir: &Path) -> Option<PanelState> {
        // Only file-backed viewers persist; fetched URLs and plugin panels
        // are not restored.
        if self.view.source_url.is_some() || self.plugin_panel.is_some() {
            return None;
        }
        Some(PanelState::Markdown {
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

    fn panel_from(src: &str) -> MarkdownPanel {
        let mut view = Viewer::new(render_markdown);
        view.file_path = PathBuf::from("/x/doc.md");
        view.title = "doc.md".to_string();
        view.source = src.to_string();
        view.last_area = Rect::new(0, 0, 80, 10);
        view.relayout_if_needed(80);
        MarkdownPanel::with_view(view)
    }

    #[test]
    fn a_plugin_panel_hands_its_links_to_the_plugin() {
        let mut p =
            MarkdownPanel::for_plugin(7, "Status".into(), "[Refresh](plugin:refresh)".into());
        p.view.relayout_if_needed(80);
        let url = p
            .view
            .doc
            .links
            .first()
            .expect("the link is laid out")
            .url
            .clone();
        assert!(matches!(
            p.follow_link(&url).as_slice(),
            [PanelEvent::PluginAction { panel: 7, action }] if action == "refresh"
        ));
        assert!(
            p.to_state(Path::new("/")).is_none(),
            "not saved in the layout"
        );
        assert_eq!(p.icon(), Some("🧩"));
        assert!(p.handle_status_action("edit_source").is_empty());
        p.set_plugin_menu(vec![("Refresh".into(), "refresh".into())]);
        let (label, id) = p.context_menu_items().remove(0);
        assert_eq!(label, "Refresh");
        assert!(matches!(
            p.handle_status_action(&id).as_slice(),
            [PanelEvent::PluginAction { panel: 7, action }] if action == "refresh"
        ));

        p.view.top = 0;
        p.set_plugin_content(Some("Status 2".into()), "# New\n\nmore".into());
        p.view.relayout_if_needed(80);
        assert_eq!(p.view.title, "Status 2");
        assert!(p.view.selected_text().is_empty());
        assert!((0..p.view.line_count()).any(|i| p.view.line_text(i).contains("New")));
        // A `plugin:` link in an ordinary viewer is not an action.
        let mut plain = panel_from("[x](plugin:refresh)");
        assert!(!matches!(
            plain.follow_link("plugin:refresh").as_slice(),
            [PanelEvent::PluginAction { .. }]
        ));
    }

    #[test]
    fn name_is_markdown() {
        let p = panel_from("# hi");
        assert_eq!(p.name(), "markdown");
    }

    #[test]
    fn edit_source_action_swaps_to_text() {
        let mut p = panel_from("# hi");
        let evs = p.handle_status_action("edit_source");
        assert!(matches!(evs.as_slice(), [PanelEvent::SwapActiveToText(_)]));
    }

    #[test]
    fn to_state_round_trips_path() {
        let p = panel_from("x");
        match p.to_state(Path::new("/tmp")) {
            Some(PanelState::Markdown { path }) => {
                assert_eq!(path, PathBuf::from("/x/doc.md"))
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn ctrl_a_selects_the_whole_page_for_ctrl_c() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut p = panel_from("# Top\n\nmiddle\n\nend");
        let press = |p: &mut _, c| {
            let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
            Panel::handle_key(
                p,
                KeyChord {
                    raw: key,
                    canonical: key,
                },
            )
        };
        let whole: Vec<String> = (0..p.view.line_count())
            .map(|i| p.view.line_text(i))
            .collect();
        press(&mut p, 'a');
        let evs = press(&mut p, 'c');
        assert!(
            matches!(evs.as_slice(), [PanelEvent::CopyToClipboard(t)] if *t == whole.join("\n")),
            "{evs:?}"
        );
        assert!(whole.iter().any(|l| l.contains("Top")));
    }

    #[test]
    fn a_plugin_panel_has_no_source_to_edit_or_reload() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut p = MarkdownPanel::for_plugin(1, "S".into(), "body".into());
        let ctrl_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        Panel::handle_key(
            &mut p,
            KeyChord {
                raw: ctrl_r,
                canonical: ctrl_r,
            },
        );
        assert_eq!(p.view.source, "body", "Ctrl+R did not read a file");
        assert_eq!(p.view.error, None);
    }
}
