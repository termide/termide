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
//! the panel in place for the editable source.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect, style::Style};

use termide_core::{
    CommandResult, Config, HotkeyTable, KeyChord, LinkOpen, Panel, PanelCommand, PanelEvent,
    PanelState, RenderContext, SegmentKind, StatusSegment, Theme, ThemeColors, WidthPreference,
};
use termide_modal::FindBar;
use termide_richtext::Rendered;
use termide_ui::ScrollBar;

use termide_ui::text_utils::{char_col_to_display, display_to_char_col, url_fragment};

mod links;
mod navigation;
mod search;

/// A cursor / selection position: `(line index, character column)`.
type Pos = (usize, usize);

/// A search match: `(line index, character column)`.
type Match = (usize, usize);

/// Rendered HTML viewer.
pub struct HtmlPanel {
    /// Scrollbar drawn by the last render, for mouse thumb dragging.
    scrollbars: termide_core::ScrollBars,
    /// Path to the HTML file.
    file_path: PathBuf,
    /// Display title (filename).
    title: String,
    /// Raw HTML source.
    source: String,
    /// Error message if the file could not be read.
    error: Option<String>,

    /// Rendered output for `layout_width`; rebuilt when width or content changes.
    doc: Rendered,
    /// Width the current `doc` was laid out for.
    layout_width: u16,
    /// First visible line index (scroll offset).
    top: usize,
    /// Content area from the last render (for click mapping + paging).
    last_area: Rect,
    /// Cursor position (character column within the line).
    cursor: Pos,
    /// Selection anchor; `Some` while a selection is active.
    anchor: Option<Pos>,
    /// Origin of an in-progress mouse drag-selection.
    drag_from: Option<Pos>,

    /// Inline find bar, when open.
    find_bar: Option<FindBar>,
    /// Search matches (start of each occurrence).
    matches: Vec<Match>,
    /// Character length of the current search needle.
    match_len: usize,
    /// Index of the current match within `matches`.
    match_idx: usize,

    /// Cached theme colors.
    colors: ThemeColors,
    /// Full theme, cached for rendering the find bar.
    theme_full: Option<Theme>,
    /// Whether the active theme is light (for code highlighting).
    is_light: bool,
    /// Configurable hotkeys (toggle preview/source).
    hotkeys: HotkeyTable,
    /// Pointer of the last `Arc<Config>` used to build hotkeys.
    last_config_ptr: usize,
    /// Origin URL when the content was fetched (not read from a file). `None`
    /// for file-backed viewers. Used for the title, base-URL link resolution,
    /// and navigation; URL-backed viewers are not persisted across runs.
    source_url: Option<String>,
    /// Browsing history (URLs) for an in-panel navigated viewer.
    history: Vec<String>,
    /// Current position within `history`.
    hist_idx: usize,
    /// Where a followed page/link opens by default (from config).
    open_links: LinkOpen,
    /// Fragment to scroll to once content is (re)laid out — set when content
    /// loads from a URL carrying a `#fragment`.
    pending_anchor: Option<String>,
    /// The fetch this viewer waits for, `(request id, URL)`: its title shows a
    /// spinner and the URL until the app delivers the page or the failure.
    loading: Option<(u64, String)>,
}

impl HtmlPanel {
    /// A blank viewer with no content (file_path empty); fill via `set_file`
    /// or `from_source`.
    fn empty() -> Self {
        Self {
            file_path: PathBuf::new(),
            title: String::new(),
            source: String::new(),
            error: None,
            doc: Rendered {
                lines: Vec::new(),
                copy: Vec::new(),
                links: Vec::new(),
                anchors: Vec::new(),
            },
            layout_width: 0,
            top: 0,
            scrollbars: termide_core::ScrollBars::default(),
            last_area: Rect::default(),
            cursor: (0, 0),
            anchor: None,
            drag_from: None,
            find_bar: None,
            matches: Vec::new(),
            match_len: 0,
            match_idx: 0,
            colors: ThemeColors::default(),
            theme_full: None,
            is_light: false,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            source_url: None,
            history: Vec::new(),
            hist_idx: 0,
            open_links: LinkOpen::default(),
            pending_anchor: None,
            loading: None,
        }
    }

    /// Open an HTML file in the preview panel.
    pub fn new(path: PathBuf) -> anyhow::Result<Self> {
        let mut panel = Self::empty();
        panel.set_file(path);
        Ok(panel)
    }

    /// Build a viewer over in-memory `source` (e.g. content fetched over HTTP),
    /// with `source_url` as its origin. Not read from, nor written to, disk.
    pub fn from_source(title: String, source: String, source_url: Option<String>) -> Self {
        let mut panel = Self::empty();
        panel.title = title;
        panel.source = source;
        if let Some(url) = &source_url {
            panel.history = vec![url.clone()];
            panel.hist_idx = 0;
            panel.pending_anchor = url_fragment(url);
        }
        panel.source_url = source_url;
        panel
    }

    /// A viewer opened before its page arrives: it shows a spinner and `url`
    /// in the title and a loading line until the app hands it the fetch `id`
    /// result ([`apply_fetched`](Self::apply_fetched) or
    /// [`fail_loading`](Self::fail_loading)).
    pub fn loading(id: u64, url: String) -> Self {
        let mut panel = Self::from_source(String::new(), String::new(), Some(url.clone()));
        panel.loading = Some((id, url));
        panel
    }

    /// Whether this is a [`loading`](Self::loading) viewer still without a page.
    pub fn is_placeholder(&self) -> bool {
        self.loading.is_some() && self.source.is_empty()
    }

    /// The fetch failed: a viewer still without a page shows `message` in
    /// place of it, one that has a page keeps it.
    pub fn fail_loading(&mut self, message: String) {
        if self.is_placeholder() {
            self.error = Some(message);
        }
        self.loading = None;
    }

    /// Mark the viewer as waiting for fetch `id` of `url` (a followed link or
    /// a history step); the page it shows stays until the result arrives.
    pub fn start_loading(&mut self, id: u64, url: String) {
        self.loading = Some((id, url));
    }

    /// The fetch this viewer waits for, if any.
    pub fn loading_id(&self) -> Option<u64> {
        self.loading.as_ref().map(|(id, _)| *id)
    }

    /// Stop waiting: the fetch failed or its result opened elsewhere.
    pub fn stop_loading(&mut self) {
        self.loading = None;
    }

    /// Replace the content in place with a navigated document (link/history
    /// step). History is managed by the caller's navigation, not here.
    pub fn apply_fetched(&mut self, title: String, source: String, final_url: String) {
        self.loading = None;
        self.title = title;
        self.source = source;
        self.pending_anchor = url_fragment(&final_url);
        self.source_url = Some(final_url);
        self.top = 0;
        self.cursor = (0, 0);
        self.anchor = None;
        self.matches.clear();
        self.layout_width = 0;
    }

    /// Point the panel at a new file, reloading its content.
    pub fn set_file(&mut self, path: PathBuf) {
        self.title = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        self.file_path = path;
        self.top = 0;
        self.cursor = (0, 0);
        self.anchor = None;
        self.matches.clear();
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
        self.layout_width = 0; // force re-layout
    }

    /// Request a "Save As" dialog to export the page as Markdown. Links resolve
    /// against the page URL, or the file for a local page. The default name
    /// follows the file, else the page `<title>`, else the URL host.
    fn save_markdown_event(&self) -> Vec<PanelEvent> {
        if self.error.is_some() || self.is_placeholder() {
            return vec![];
        }
        let base = match &self.source_url {
            Some(url) => url.clone(),
            None => url::Url::from_file_path(&self.file_path)
                .map(String::from)
                .unwrap_or_default(),
        };
        let converted = termide_html_markdown::html_to_markdown(&self.source, &base);
        let file_stem = match self.source_url {
            None => self.file_path.file_stem().and_then(|s| s.to_str()),
            Some(_) => None,
        };
        let host = self
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

impl Panel for HtmlPanel {
    fn name(&self) -> &'static str {
        "html"
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            PanelCommand::GetScrollBars => CommandResult::ScrollBars(self.scrollbars),
            PanelCommand::SetScrollOffset { offset, .. } => {
                self.top = offset.min(self.line_count().saturating_sub(1));
                CommandResult::NeedsRedraw(true)
            }
            _ => CommandResult::None,
        }
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferWide
    }

    fn title(&self) -> String {
        if let Some((_, url)) = &self.loading {
            return format!("{} {url}", termide_config::constants::spinner_frame());
        }
        // A fetched page shows its URL; a file-backed view shows the filename.
        self.source_url
            .clone()
            .unwrap_or_else(|| self.title.clone())
    }

    fn icon(&self) -> Option<&'static str> {
        // A globe for a fetched web page (matching the bookmark icon).
        self.source_url.as_ref().map(|_| "🌐")
    }

    fn tick(&mut self) -> Vec<PanelEvent> {
        // Animate the title spinner while a fetch is in flight.
        if self.loading.is_some() {
            vec![PanelEvent::NeedsRedraw]
        } else {
            vec![]
        }
    }

    fn prepare_render(&mut self, theme: &Theme, config: &Arc<Config>) {
        let new_light = theme.is_light_theme();
        if new_light != self.is_light {
            self.layout_width = 0;
        }
        self.colors = ThemeColors::from(theme);
        self.theme_full = Some(*theme);
        self.is_light = new_light;

        let ptr = Arc::as_ptr(config) as usize;
        if self.last_config_ptr != ptr {
            self.last_config_ptr = ptr;
            let mut t = HotkeyTable::new();
            t.insert("toggle_view", &config.viewer.keybindings.toggle_view);
            self.hotkeys = t;
            self.open_links = config.viewer.open_links;
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        buf.set_style(area, Style::default().fg(self.colors.fg).bg(self.colors.bg));

        // Find bar docked at the BOTTOM with a separator above (where every
        // input in termide lives).
        let mut content = area;
        if let (Some(bar), Some(theme)) = (self.find_bar.as_mut(), self.theme_full.as_ref()) {
            let bar_h = bar.height().min(area.height);
            let bar_area = Rect {
                x: area.x,
                y: area.y + area.height - bar_h,
                width: area.width,
                height: bar_h,
            };
            bar.render(bar_area, buf, theme, true);
            // The bar draws its own titled top border, which is the divider.
            content = Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: area.height.saturating_sub(bar_h),
            };
        }
        self.last_area = content;

        if let Some(err) = &self.error {
            let msg = ratatui::text::Line::styled(
                format!(" Cannot open: {err}"),
                Style::default().fg(self.colors.error),
            );
            buf.set_line(content.x, content.y, &msg, content.width);
            return;
        }
        if self.is_placeholder() {
            let msg = ratatui::text::Line::styled(
                format!(" {}", termide_i18n::t().viewer_loading()),
                Style::default().fg(self.colors.disabled),
            );
            buf.set_line(content.x, content.y, &msg, content.width);
            return;
        }

        // Reserve the rightmost column as a scrollbar gutter so wrapped text
        // never sits under the bar (keeps the layout stable frame-to-frame).
        let text_width = content.width.saturating_sub(1).max(1);
        self.relayout_if_needed(text_width);
        self.top = self.top.min(self.max_top());

        let sel = self.selection();
        for i in 0..(content.height as usize) {
            let line_idx = self.top + i;
            let Some(line) = self.doc.lines.get(line_idx) else {
                break;
            };
            let y = content.y + i as u16;
            buf.set_line(content.x, y, line, text_width);

            let text = self.line_text(line_idx);

            // Search matches.
            let match_style = Style::default().fg(self.colors.bg).bg(self.colors.warning);
            for &(ml, mc) in &self.matches {
                if ml != line_idx {
                    continue;
                }
                let x0 = char_col_to_display(&text, mc) as u16;
                let x1 = char_col_to_display(&text, mc + self.match_len) as u16;
                for dx in x0..x1.max(x0) {
                    if dx < text_width {
                        buf[(content.x + dx, y)].set_style(match_style);
                    }
                }
            }

            // Selection highlight (per display column).
            if let Some((s, e)) = sel {
                if line_idx >= s.0 && line_idx <= e.0 {
                    let c0 = if line_idx == s.0 { s.1 } else { 0 };
                    let c1 = if line_idx == e.0 {
                        e.1
                    } else {
                        text.chars().count()
                    };
                    let style = Style::default()
                        .fg(self.colors.selection_fg)
                        .bg(self.colors.selection_bg);
                    let x0 = char_col_to_display(&text, c0) as u16;
                    let x1 = char_col_to_display(&text, c1) as u16;
                    for dx in x0..x1.max(x0) {
                        if dx < text_width {
                            buf[(content.x + dx, y)].set_style(style);
                        }
                    }
                }
            }
        }

        // Cursor cell (only when focused, on a visible line, and not searching);
        // an unfocused preview shows no cursor.
        if ctx.is_focused
            && self.find_bar.is_none()
            && self.cursor.0 >= self.top
            && self.cursor.0 < self.top + content.height as usize
        {
            let y = content.y + (self.cursor.0 - self.top) as u16;
            let dx = char_col_to_display(&self.line_text(self.cursor.0), self.cursor.1) as u16;
            if dx < text_width {
                let style = Style::default().fg(self.colors.bg).bg(self.colors.cursor);
                buf[(content.x + dx, y)].set_style(style);
            }
        }

        // Vertical scrollbar on the panel's right border (replacing it), not one
        // column inside it — otherwise it reads as detached from the edge.
        self.scrollbars.vertical = ScrollBar::render_tracked(
            buf,
            ctx.border_right_x.unwrap_or(content.x + content.width - 1),
            content.y,
            content.height,
            self.top,
            content.height as usize,
            self.line_count(),
            &self.colors,
            ctx.is_focused,
        );
    }

    fn handle_key(&mut self, chord: KeyChord) -> Vec<PanelEvent> {
        let key = chord.raw;

        // While the find bar is open it owns input (Esc / Ctrl+F close it).
        if self.find_bar.is_some() {
            let shortcut = chord.canonical;
            let ctrl_f =
                shortcut.code == KeyCode::Char('f') && shortcut.modifiers == KeyModifiers::CONTROL;
            if ctrl_f {
                self.close_find();
                return vec![PanelEvent::NeedsRedraw];
            }
            let action = self.find_bar.as_mut().unwrap().handle_key(key);
            return match action {
                Some(a) => self.handle_find_action(a),
                None => vec![PanelEvent::NeedsRedraw],
            };
        }

        // Below the find bar there is no text input, so match shortcuts against
        // the canonical (layout-normalized) key — `[`/`]`, `o`, `g`, … then work
        // regardless of the active keyboard layout (e.g. Cyrillic `х`/`ъ`).
        let key = chord.canonical;

        if self.hotkeys.matches("toggle_view", &key) {
            return vec![PanelEvent::SwapActiveToText(self.file_path.clone())];
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = (self.viewport_height() as i32 - 1).max(1);

        // Ctrl+R: re-read from disk (pick up external edits), keeping position.
        if ctrl && key.code == KeyCode::Char('r') {
            let (top, cursor) = (self.top, self.cursor);
            let _ = self.reload();
            self.cursor = cursor;
            self.clamp_cursor();
            self.top = top.min(self.max_top());
            return vec![PanelEvent::NeedsRedraw];
        }

        if ctrl && key.code == KeyCode::Char('f') {
            self.open_find();
            return vec![PanelEvent::NeedsRedraw];
        }
        if ctrl && key.code == KeyCode::Char('a') {
            self.select_all();
            return vec![PanelEvent::NeedsRedraw];
        }
        if ctrl && key.code == KeyCode::Char('c') {
            let text = self.selected_text();
            if text.is_empty() {
                return vec![];
            }
            return vec![PanelEvent::CopyToClipboard(text)];
        }
        // Ctrl+S: export the page as Markdown to a chosen file (Save As).
        if ctrl && key.code == KeyCode::Char('s') {
            return self.save_markdown_event();
        }

        match key.code {
            // History back/forward in a navigated view.
            KeyCode::Char('[') | KeyCode::Backspace => return self.go_back(),
            KeyCode::Char(']') => return self.go_forward(),
            KeyCode::Up => self.move_vertical(-1, shift),
            KeyCode::Down | KeyCode::Char('j') => self.move_vertical(1, shift),
            KeyCode::Char('k') => self.move_vertical(-1, shift),
            KeyCode::Left | KeyCode::Char('h') => self.move_horizontal(false, shift),
            KeyCode::Right | KeyCode::Char('l') => self.move_horizontal(true, shift),
            KeyCode::PageUp => self.move_vertical(-page, shift),
            KeyCode::PageDown | KeyCode::Char(' ') => self.move_vertical(page, shift),
            KeyCode::Home => self.move_cursor((self.cursor.0, 0), shift),
            KeyCode::End => {
                let end = self.line_len(self.cursor.0);
                self.move_cursor((self.cursor.0, end), shift);
            }
            KeyCode::Char('g') => self.move_cursor((0, 0), shift),
            KeyCode::Char('G') => {
                let last = self.line_count().saturating_sub(1);
                self.move_cursor((last, 0), shift);
            }
            KeyCode::Enter => {
                if let Some(url) = self.link_under_cursor().map(|l| l.url.clone()) {
                    return self.activate_link(&url);
                }
                return vec![];
            }
            // Open the link under the cursor in the external browser, even when
            // the viewer would otherwise navigate it in place.
            KeyCode::Char('o') | KeyCode::Char('O') => {
                if let Some(url) = self.link_under_cursor().map(|l| l.url.clone()) {
                    return vec![PanelEvent::OpenExternal(PathBuf::from(self.resolve(&url)))];
                }
                return vec![];
            }
            KeyCode::Esc if self.anchor.is_some() => {
                self.anchor = None;
            }
            _ => return vec![],
        }
        vec![PanelEvent::NeedsRedraw]
    }

    fn handle_scroll(&mut self, delta: i32, _panel_area: Rect) -> Vec<PanelEvent> {
        self.scroll_by(delta);
        vec![PanelEvent::NeedsRedraw]
    }

    fn handle_mouse(&mut self, event: MouseEvent, _panel_area: Rect) -> Vec<PanelEvent> {
        // Route what the find bar owns to it: a press on it, and a drag
        // started there, which keeps selecting past its edges.
        if let Some(bar) = self.find_bar.as_mut() {
            if bar.mouse_hits(event) {
                if let Some(action) = bar.handle_mouse(event) {
                    return self.handle_find_action(action);
                }
                return vec![PanelEvent::NeedsRedraw];
            }
        }

        // Map against the content area actually drawn in render (not the raw
        // panel area, which may include a header), so clicks land precisely.
        let area = self.last_area;
        if event.column < area.x || event.row < area.y {
            // Outside the content (e.g. the find bar zone) — ignore non-wheel.
            if !matches!(
                event.kind,
                MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
            ) {
                return vec![];
            }
        }
        let rel_col = event.column.saturating_sub(area.x);
        let rel_row = event.row.saturating_sub(area.y);
        let line_idx = self.top + rel_row as usize;

        match event.kind {
            MouseEventKind::ScrollDown => {
                self.scroll_by(3);
            }
            MouseEventKind::ScrollUp => {
                self.scroll_by(-3);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if line_idx >= self.line_count() {
                    return vec![];
                }
                // Move the cursor to the click first, then open a link if the
                // click landed on one.
                let col = display_to_char_col(&self.line_text(line_idx), rel_col);
                self.cursor = (line_idx, col);
                self.anchor = None;
                self.drag_from = Some(self.cursor);
                if let Some(url) = self.link_at(line_idx, rel_col).map(|l| l.url.clone()) {
                    let mut evs = vec![PanelEvent::NeedsRedraw];
                    evs.extend(self.activate_link(&url));
                    return evs;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(origin) = self.drag_from {
                    if line_idx < self.line_count() {
                        let col = display_to_char_col(&self.line_text(line_idx), rel_col);
                        self.anchor = Some(origin);
                        self.cursor = (line_idx, col);
                        self.clamp_cursor();
                        self.ensure_cursor_visible();
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag_from = None;
                if self.anchor == Some(self.cursor) {
                    self.anchor = None; // a click, not a drag
                }
            }
            _ => return vec![],
        }
        vec![PanelEvent::NeedsRedraw]
    }

    fn captures_escape(&self) -> bool {
        self.find_bar.is_some() || self.anchor.is_some()
    }

    fn status_segments(&self) -> Vec<StatusSegment> {
        if self.error.is_some() {
            return vec![];
        }
        let sep = || StatusSegment::new(" │ ", SegmentKind::Label);
        let total = self.line_count().max(1);
        let pos = (self.cursor.0 + 1).min(total);
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
            StatusSegment::new(format!("{pos}/{total}"), SegmentKind::Value),
        ]
    }

    fn handle_status_action(&mut self, action: &str) -> Vec<PanelEvent> {
        match action {
            "edit_source" => vec![PanelEvent::SwapActiveToText(self.file_path.clone())],
            "save_markdown" => self.save_markdown_event(),
            _ => vec![],
        }
    }

    fn context_menu_items(&self) -> Vec<(String, &'static str)> {
        if self.error.is_some() || self.is_placeholder() {
            return vec![];
        }
        let t = termide_i18n::t();
        vec![(t.menu_save_page_as_markdown().to_string(), "save_markdown")]
    }

    fn reload(&mut self) -> anyhow::Result<()> {
        // URL-backed content has no file to re-read; leave it as-is.
        if self.source_url.is_some() {
            return Ok(());
        }
        let path = self.file_path.clone();
        self.set_file(path);
        Ok(())
    }

    fn to_state(&self, _project_dir: &Path) -> Option<PanelState> {
        // Only file-backed viewers persist; fetched URLs are not restored.
        if self.source_url.is_some() {
            return None;
        }
        Some(PanelState::Html {
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
    use termide_core::LinkTarget;
    use termide_html::render_html;
    use termide_modal::FindField;

    fn panel_from(src: &str) -> HtmlPanel {
        let mut p = HtmlPanel {
            file_path: PathBuf::from("/x/page.html"),
            title: "page.html".to_string(),
            source: src.to_string(),
            error: None,
            doc: Rendered {
                lines: Vec::new(),
                copy: Vec::new(),
                links: Vec::new(),
                anchors: Vec::new(),
            },
            layout_width: 0,
            top: 0,
            scrollbars: termide_core::ScrollBars::default(),
            last_area: Rect::new(0, 0, 80, 10),
            cursor: (0, 0),
            anchor: None,
            drag_from: None,
            find_bar: None,
            matches: Vec::new(),
            match_len: 0,
            match_idx: 0,
            colors: ThemeColors::default(),
            theme_full: None,
            is_light: false,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            source_url: None,
            history: Vec::new(),
            hist_idx: 0,
            open_links: LinkOpen::Panel,
            pending_anchor: None,
            loading: None,
        };
        p.doc = render_html(src, 80, &p.colors, false);
        p.layout_width = 80;
        p
    }

    #[test]
    fn name_is_html() {
        let p = panel_from("<p>hi</p>");
        assert_eq!(p.name(), "html");
    }

    #[test]
    fn selection_copies_across_lines() {
        let mut p = panel_from("<p>alpha</p><p>beta</p>");
        p.cursor = (0, 0);
        p.anchor = Some((0, 0));
        let last = p.line_count() - 1;
        p.cursor = (last, p.line_len(last));
        let text = p.selected_text();
        assert!(text.contains("alpha"), "{text:?}");
        assert!(text.contains("beta"), "{text:?}");
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
    fn search_finds_matches_and_moves_cursor() {
        let mut p = panel_from("<p>one two</p><p>two three two</p>");
        p.open_find();
        p.find_bar
            .as_mut()
            .unwrap()
            .set_text(FindField::Find, "two".to_string());
        p.run_search();
        assert_eq!(p.matches.len(), 3, "{:?}", p.matches);
        assert_eq!(p.cursor, p.matches[p.match_idx]);
        let first = p.cursor;
        p.step_match(true);
        assert_ne!(p.cursor, first, "next match should move the cursor");
    }

    #[test]
    fn relative_link_resolves_and_navigates_in_place() {
        let mut p = HtmlPanel::from_source(
            "page".into(),
            "x".into(),
            Some("https://ex.com/dir/".into()),
        );
        let evs = p.activate_link("sub/x.html");
        match evs.as_slice() {
            [PanelEvent::NavigateUrl(u)] => assert_eq!(u, "https://ex.com/dir/sub/x.html"),
            _ => panic!("{evs:?}"),
        }
        assert_eq!(p.history.last().unwrap(), "https://ex.com/dir/sub/x.html");
    }

    #[test]
    fn file_backed_links_go_to_the_app() {
        // A web link from a file-backed view, and a local file, are the app's
        // to open (by the open_links / open_images settings), not in place.
        let mut p = panel_from("<p>x</p>");
        let evs = p.activate_link("https://ex.com");
        assert!(
            matches!(evs.as_slice(), [PanelEvent::OpenLink(LinkTarget::Url(u))] if u == "https://ex.com"),
            "{evs:?}"
        );
        let evs = p.activate_link("/pics/logo.png");
        assert!(
            matches!(evs.as_slice(), [PanelEvent::OpenLink(LinkTarget::Path(p))] if p == std::path::Path::new("/pics/logo.png")),
            "{evs:?}"
        );
        // A scheme the app does not open goes to the system opener.
        let evs = p.activate_link("mailto:a@b.c");
        assert!(
            matches!(evs.as_slice(), [PanelEvent::OpenExternal(_)]),
            "{evs:?}"
        );
    }

    #[test]
    fn empty_link_is_noop() {
        let mut p = panel_from("<p>x</p>");
        assert!(p.activate_link("").is_empty(), "empty href");
    }

    #[test]
    fn same_page_anchor_scrolls() {
        let mut html = String::from("<p>top</p>");
        for i in 0..40 {
            html.push_str(&format!("<p>line {i}</p>"));
        }
        html.push_str("<h2 id=\"target\">Target</h2><p>after</p>");
        let mut p = panel_from(&html);
        assert!(
            p.doc.anchors.iter().any(|(id, _)| id == "target"),
            "anchor not registered: {:?}",
            p.doc.anchors
        );
        let before = p.top;
        let evs = p.activate_link("#target");
        assert!(
            matches!(evs.as_slice(), [PanelEvent::NeedsRedraw]),
            "{evs:?}"
        );
        assert!(
            p.top > before,
            "should scroll down to the anchor (top={}, before={before})",
            p.top
        );
    }

    #[test]
    fn external_setting_leaves_a_fetched_page_in_place() {
        // With links set to open externally, a link in a fetched page goes to
        // the app (the browser) instead of replacing the page.
        let mut p = HtmlPanel::from_source("p".into(), "x".into(), Some("https://ex.com/a".into()));
        p.open_links = LinkOpen::External;
        let evs = p.activate_link("https://ex.com/b");
        assert!(
            matches!(evs.as_slice(), [PanelEvent::OpenLink(LinkTarget::Url(_))]),
            "{evs:?}"
        );
        assert_eq!(p.history.len(), 1);
    }

    #[test]
    fn history_back_and_forward() {
        let mut p = HtmlPanel::from_source("p".into(), "x".into(), Some("https://ex.com/a".into()));
        p.activate_link("https://ex.com/b");
        p.activate_link("https://ex.com/c");
        let back = p.go_back();
        assert!(
            matches!(back.as_slice(), [PanelEvent::NavigateUrl(u)] if u == "https://ex.com/b"),
            "{back:?}"
        );
        assert!(
            matches!(p.go_back().as_slice(), [PanelEvent::NavigateUrl(u)] if u == "https://ex.com/a")
        );
        assert!(p.go_back().is_empty(), "no history before the first page");
        assert!(
            matches!(p.go_forward().as_slice(), [PanelEvent::NavigateUrl(u)] if u == "https://ex.com/b")
        );
    }

    #[test]
    fn bracket_history_works_on_cyrillic_layout() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut p = HtmlPanel::from_source("p".into(), "x".into(), Some("https://ex.com/a".into()));
        p.activate_link("https://ex.com/b"); // history [a, b], idx = 1
                                             // On a Russian layout the `[` key emits 'х'; the normalizer puts '[' in
                                             // `canonical`, which the viewer matches.
        let chord = KeyChord {
            raw: KeyEvent::new(KeyCode::Char('х'), KeyModifiers::NONE),
            canonical: KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE),
        };
        let evs = p.handle_key(chord);
        assert!(
            matches!(evs.as_slice(), [PanelEvent::NavigateUrl(u)] if u == "https://ex.com/a"),
            "{evs:?}"
        );
    }

    #[test]
    fn link_under_cursor_opens() {
        let p = panel_from("<p><a href=\"https://ex.com\">docs</a></p>");
        assert_eq!(p.doc.links.len(), 1, "{:?}", p.doc.links);
        assert_eq!(p.doc.links[0].url, "https://ex.com");
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
                .map(|(_, id)| *id)
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
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let src = r#"<head><title> A/B: "c" </title></head><p><a href="/d">d</a></p>"#;
        let mut p = HtmlPanel::from_source("t".into(), src.into(), Some("https://ex.com/p".into()));
        let ctrl_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        let evs = p.handle_key(KeyChord {
            raw: ctrl_s,
            canonical: ctrl_s,
        });
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
    fn ctrl_a_selects_the_whole_page_for_ctrl_c() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut p = panel_from("<h1>Top</h1><p>middle</p><p>end</p>");
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
        let whole: Vec<String> = (0..p.line_count()).map(|i| p.line_text(i)).collect();
        press(&mut p, 'a');
        assert_eq!(p.top, 0, "the view stays where it was");
        let evs = press(&mut p, 'c');
        assert!(
            matches!(evs.as_slice(), [PanelEvent::CopyToClipboard(t)] if *t == whole.join("\n")),
            "{evs:?}"
        );
        assert!(whole.iter().any(|l| l.contains("Top")));
        assert!(whole.iter().any(|l| l.contains("end")));
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

        p.apply_fetched("a".into(), "<p>hi</p>".into(), "https://ex.com/a".into());
        assert_eq!(Panel::title(&p), "https://ex.com/a");
        assert!(p.tick().is_empty());
        assert!(!p.context_menu_items().is_empty());
    }

    #[test]
    fn a_failed_load_shows_in_a_placeholder_only() {
        let mut p = HtmlPanel::loading(1, "https://ex.com/a".into());
        p.fail_loading("Fetch failed: x".into());
        assert_eq!(p.error.as_deref(), Some("Fetch failed: x"));
        assert_eq!(p.loading_id(), None);

        let mut p = panel_from("<p>page</p>");
        p.start_loading(2, "https://ex.com/b".into());
        p.fail_loading("Fetch failed: x".into());
        assert_eq!(p.error, None);
        assert_eq!(p.loading_id(), None);
    }
}
