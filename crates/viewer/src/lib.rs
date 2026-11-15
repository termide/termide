//! Interactive viewer over a [`Rendered`] document, shared by the Markdown
//! and HTML preview panels.
//!
//! [`Viewer`] holds what both panels keep: the source and its layout, the
//! cursor and selection, the find bar and its matches, link history and the
//! fetch a viewer waits for. It draws the document and handles the keys and
//! mouse events every preview treats alike; [`ViewerPanel`] is the seam a
//! panel embeds it through, overriding what is its own (where a followed
//! link goes, whether there is a source file).

mod links;
mod navigation;
mod search;

use std::path::PathBuf;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use termide_core::{
    CommandResult, Config, HotkeyTable, KeyChord, PanelCommand, PanelEvent, RenderContext,
    ScrollBars, Theme, ThemeColors,
};
use termide_modal::FindBar;
use termide_ui::text_utils::{char_col_to_display, display_to_char_col, url_fragment};
use termide_ui::ScrollBar;

use termide_richtext::Rendered;

/// A cursor / selection position: `(line index, character column)`.
pub type Pos = (usize, usize);

/// A search match: `(line index, character column)`.
pub type Match = (usize, usize);

/// Lays a source document out at a width: `(source, width, colors, is_light)`.
pub type LayoutFn = fn(&str, u16, &ThemeColors, bool) -> Rendered;

/// State and behaviour of a rendered-document viewer.
pub struct Viewer {
    /// How the source is laid out (the Markdown or the HTML front-end).
    layout: LayoutFn,
    /// Scrollbar drawn by the last render, for mouse thumb dragging.
    pub scrollbars: ScrollBars,
    /// Path to the source file; empty for content not read from a file.
    pub file_path: PathBuf,
    /// Display title (filename).
    pub title: String,
    /// Raw document source.
    pub source: String,
    /// Error message if the file could not be read.
    pub error: Option<String>,

    /// Rendered output for `layout_width`; rebuilt when width or content changes.
    pub doc: Rendered,
    /// Width the current `doc` was laid out for; `0` forces a re-layout.
    pub layout_width: u16,
    /// First visible line index (scroll offset).
    pub top: usize,
    /// Content area from the last render (for click mapping + paging).
    pub last_area: Rect,
    /// Cursor position (character column within the line).
    pub cursor: Pos,
    /// Selection anchor; `Some` while a selection is active.
    pub anchor: Option<Pos>,
    /// Origin of an in-progress mouse drag-selection.
    pub drag_from: Option<Pos>,

    /// Inline find bar, when open.
    pub find_bar: Option<FindBar>,
    /// Search matches (start of each occurrence).
    pub matches: Vec<Match>,
    /// Character length of the current search needle.
    pub match_len: usize,
    /// Index of the current match within `matches`.
    pub match_idx: usize,

    /// Cached theme colors.
    pub colors: ThemeColors,
    /// Full theme, cached for rendering the find bar.
    pub theme_full: Option<Theme>,
    /// Whether the active theme is light (for code highlighting).
    pub is_light: bool,
    /// Configurable hotkeys (toggle preview/source, open externally).
    pub hotkeys: HotkeyTable,
    /// Pointer of the last `Arc<Config>` used to build hotkeys.
    pub last_config_ptr: usize,
    /// Origin URL when the content was fetched (not read from a file). `None`
    /// for file-backed viewers. Used for the title, base-URL link resolution,
    /// and navigation; URL-backed viewers are not persisted across runs.
    pub source_url: Option<String>,
    /// Browsing history (URLs) for an in-panel navigated viewer.
    pub history: Vec<String>,
    /// Current position within `history`.
    pub hist_idx: usize,
    /// Fragment to scroll to once content is (re)laid out — set when content
    /// loads from a URL carrying a `#fragment`.
    pub pending_anchor: Option<String>,
    /// The fetch this viewer waits for, `(request id, URL)`: its title shows a
    /// spinner and the URL until the app delivers the page or the failure.
    pub loading: Option<(u64, String)>,
}

impl Viewer {
    /// A blank viewer with no content (file_path empty); fill via
    /// [`set_file`](Self::set_file) or [`from_source`](Self::from_source).
    pub fn new(layout: LayoutFn) -> Self {
        Self {
            layout,
            scrollbars: ScrollBars::default(),
            file_path: PathBuf::new(),
            title: String::new(),
            source: String::new(),
            error: None,
            doc: Rendered::default(),
            layout_width: 0,
            top: 0,
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
            pending_anchor: None,
            loading: None,
        }
    }

    /// A viewer over in-memory `source` (e.g. content fetched over HTTP),
    /// with `source_url` as its origin. Not read from, nor written to, disk.
    pub fn from_source(
        layout: LayoutFn,
        title: String,
        source: String,
        source_url: Option<String>,
    ) -> Self {
        let mut view = Self::new(layout);
        view.title = title;
        view.source = source;
        if let Some(url) = &source_url {
            view.history = vec![url.clone()];
            view.hist_idx = 0;
            view.pending_anchor = url_fragment(url);
        }
        view.source_url = source_url;
        view
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

    /// Point the viewer at a new file, reloading its content.
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

    /// Re-read the file; URL-backed content has no file and stays as it is.
    pub fn reload(&mut self) {
        if self.source_url.is_some() {
            return;
        }
        let path = self.file_path.clone();
        self.set_file(path);
    }

    /// The directory of the source file.
    pub fn working_directory(&self) -> Option<PathBuf> {
        self.file_path.parent().map(|p| p.to_path_buf())
    }

    /// The panel title: a spinner and the URL while a fetch is in flight, else
    /// the URL of a fetched page or the filename of a file-backed view.
    pub fn title(&self) -> String {
        if let Some((_, url)) = &self.loading {
            return format!("{} {url}", termide_config::constants::spinner_frame());
        }
        self.source_url
            .clone()
            .unwrap_or_else(|| self.title.clone())
    }

    /// Animate the title spinner while a fetch is in flight.
    pub fn tick(&mut self) -> Vec<PanelEvent> {
        if self.loading.is_some() {
            vec![PanelEvent::NeedsRedraw]
        } else {
            vec![]
        }
    }

    /// Whether `Esc` is the viewer's: it closes the find bar or the selection.
    pub fn captures_escape(&self) -> bool {
        self.find_bar.is_some() || self.anchor.is_some()
    }

    /// The cursor line and the line count, as the status bar shows them.
    pub fn line_position(&self) -> String {
        let total = self.line_count().max(1);
        let pos = (self.cursor.0 + 1).min(total);
        format!("{pos}/{total}")
    }

    pub fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            PanelCommand::GetScrollBars => CommandResult::ScrollBars(self.scrollbars),
            PanelCommand::SetScrollOffset { offset, .. } => {
                self.top = offset.min(self.line_count().saturating_sub(1));
                CommandResult::NeedsRedraw(true)
            }
            // The global copy key, whatever it is bound to: the selection.
            // While the find bar is open it owns the keyboard (Ctrl+C/X/V
            // reach it as keys); a terminal paste goes into its query too.
            PanelCommand::Copy if self.find_bar.is_none() => {
                let text = self.selected_text();
                if !text.is_empty() {
                    let _ = termide_ui::clipboard::copy(&text);
                }
                CommandResult::Handled(true)
            }
            PanelCommand::PasteText { text } => match self.find_bar.as_mut() {
                Some(bar) => {
                    if let Some(action) = bar.paste(&text) {
                        self.handle_find_action(action);
                    }
                    CommandResult::Handled(true)
                }
                None => CommandResult::None,
            },
            _ => CommandResult::None,
        }
    }

    pub fn prepare_render(&mut self, theme: &Theme, config: &Arc<Config>) {
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
            t.insert("open_external", &config.viewer.keybindings.open_external);
            self.hotkeys = t;
        }
    }

    /// Draw the viewer. `placeholder` is a line shown instead of the document
    /// (a page still loading).
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: &RenderContext,
        placeholder: Option<&str>,
    ) {
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
        if let Some(text) = placeholder {
            let msg = ratatui::text::Line::styled(
                format!(" {text}"),
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

    pub fn handle_scroll(&mut self, delta: i32) -> Vec<PanelEvent> {
        self.scroll_by(delta);
        vec![PanelEvent::NeedsRedraw]
    }
}

/// A panel built around a [`Viewer`]. The provided methods handle the keys and
/// mouse events every viewer treats alike; a panel overrides what is its own.
pub trait ViewerPanel {
    fn view(&self) -> &Viewer;
    fn view_mut(&mut self) -> &mut Viewer;

    /// Whether the content has a source file to edit and re-read.
    fn has_source_file(&self) -> bool {
        true
    }

    /// Follow a link inside termide (`Enter`, a click); see
    /// [`Viewer::activate_link`].
    fn follow_link(&mut self, href: &str) -> Vec<PanelEvent> {
        self.view_mut().activate_link(href)
    }

    /// The keys every viewer handles; `None` for a key that is none of them,
    /// which the panel may take or drop.
    fn handle_viewer_key(&mut self, chord: KeyChord) -> Option<Vec<PanelEvent>> {
        let key = chord.raw;

        // While the find bar is open it owns input (Esc / Ctrl+F close it).
        if self.view().find_bar.is_some() {
            let view = self.view_mut();
            let shortcut = chord.canonical;
            let ctrl_f =
                shortcut.code == KeyCode::Char('f') && shortcut.modifiers == KeyModifiers::CONTROL;
            if ctrl_f {
                view.close_find();
                return Some(vec![PanelEvent::NeedsRedraw]);
            }
            let action = view.find_bar.as_mut().unwrap().handle_key(key);
            return Some(match action {
                Some(a) => view.handle_find_action(a),
                None => vec![PanelEvent::NeedsRedraw],
            });
        }

        // Below the find bar there is no text input, so match shortcuts against
        // the canonical (layout-normalized) key — `[`/`]`, `o`, `g`, … then work
        // regardless of the active keyboard layout (e.g. Cyrillic `х`/`ъ`).
        let key = chord.canonical;

        let from_file = self.has_source_file();
        if from_file && self.view().hotkeys.matches("toggle_view", &key) {
            return Some(vec![PanelEvent::SwapActiveToText(
                self.view().file_path.clone(),
            )]);
        }
        // Open the link under the cursor outside termide (`O`/`Alt+Enter` by
        // default), even where `Enter` would follow it in place.
        if self.view().hotkeys.matches_canonical("open_external", &key) {
            let view = self.view_mut();
            return Some(match view.link_under_cursor().map(|l| l.url.clone()) {
                Some(url) => view.activate_link_external(&url),
                None => vec![],
            });
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = (self.view().viewport_height() as i32 - 1).max(1);

        // Ctrl+R: re-read from disk (pick up external edits), keeping position.
        if from_file && ctrl && key.code == KeyCode::Char('r') {
            let view = self.view_mut();
            let (top, cursor) = (view.top, view.cursor);
            view.reload();
            view.cursor = cursor;
            view.clamp_cursor();
            view.top = top.min(view.max_top());
            return Some(vec![PanelEvent::NeedsRedraw]);
        }

        if ctrl && key.code == KeyCode::Char('f') {
            self.view_mut().open_find();
            return Some(vec![PanelEvent::NeedsRedraw]);
        }
        if ctrl && key.code == KeyCode::Char('a') {
            self.view_mut().select_all();
            return Some(vec![PanelEvent::NeedsRedraw]);
        }
        if ctrl && key.code == KeyCode::Char('c') {
            let text = self.view().selected_text();
            if text.is_empty() {
                return Some(vec![]);
            }
            return Some(vec![PanelEvent::CopyToClipboard(text)]);
        }

        let view = self.view_mut();
        match key.code {
            // History back/forward in a navigated view.
            KeyCode::Char('[') | KeyCode::Backspace => return Some(view.go_back()),
            KeyCode::Char(']') => return Some(view.go_forward()),
            KeyCode::Up => view.move_vertical(-1, shift),
            KeyCode::Down | KeyCode::Char('j') => view.move_vertical(1, shift),
            KeyCode::Char('k') => view.move_vertical(-1, shift),
            KeyCode::Left | KeyCode::Char('h') => view.move_horizontal(false, shift),
            KeyCode::Right | KeyCode::Char('l') => view.move_horizontal(true, shift),
            KeyCode::PageUp => view.move_vertical(-page, shift),
            KeyCode::PageDown | KeyCode::Char(' ') => view.move_vertical(page, shift),
            KeyCode::Home => view.move_cursor((view.cursor.0, 0), shift),
            KeyCode::End => {
                let end = view.line_len(view.cursor.0);
                view.move_cursor((view.cursor.0, end), shift);
            }
            KeyCode::Char('g') => view.move_cursor((0, 0), shift),
            KeyCode::Char('G') => {
                let last = view.line_count().saturating_sub(1);
                view.move_cursor((last, 0), shift);
            }
            KeyCode::Enter => {
                return Some(match view.link_under_cursor().map(|l| l.url.clone()) {
                    Some(url) => self.follow_link(&url),
                    None => vec![],
                });
            }
            KeyCode::Esc if view.anchor.is_some() => {
                view.anchor = None;
            }
            _ => return None,
        }
        Some(vec![PanelEvent::NeedsRedraw])
    }

    /// Mouse input: the find bar, wheel scrolling, cursor placement, drag
    /// selection and link clicks.
    fn handle_viewer_mouse(&mut self, event: MouseEvent) -> Vec<PanelEvent> {
        let view = self.view_mut();
        // Route what the find bar owns to it: a press on it, and a drag
        // started there, which keeps selecting past its edges.
        if let Some(bar) = view.find_bar.as_mut() {
            if bar.mouse_hits(event) {
                if let Some(action) = bar.handle_mouse(event) {
                    return view.handle_find_action(action);
                }
                return vec![PanelEvent::NeedsRedraw];
            }
        }

        // Map against the content area actually drawn in render (not the raw
        // panel area, which may include a header), so clicks land precisely.
        let area = view.last_area;
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
        let line_idx = view.top + rel_row as usize;

        match event.kind {
            MouseEventKind::ScrollDown => {
                view.scroll_by(3);
            }
            MouseEventKind::ScrollUp => {
                view.scroll_by(-3);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if line_idx >= view.line_count() {
                    return vec![];
                }
                // Move the cursor to the click first, then open a link if the
                // click landed on one.
                let col = display_to_char_col(&view.line_text(line_idx), rel_col);
                view.cursor = (line_idx, col);
                view.anchor = None;
                view.drag_from = Some(view.cursor);
                if let Some(url) = view.link_at(line_idx, rel_col).map(|l| l.url.clone()) {
                    // A click follows the link inside; Alt+Click opens it
                    // outside, as `O` does.
                    let mut evs = vec![PanelEvent::NeedsRedraw];
                    if event.modifiers.contains(KeyModifiers::ALT) {
                        evs.extend(view.activate_link_external(&url));
                    } else {
                        evs.extend(self.follow_link(&url));
                    }
                    return evs;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(origin) = view.drag_from {
                    if line_idx < view.line_count() {
                        let col = display_to_char_col(&view.line_text(line_idx), rel_col);
                        view.anchor = Some(origin);
                        view.cursor = (line_idx, col);
                        view.clamp_cursor();
                        view.ensure_cursor_visible();
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                view.drag_from = None;
                if view.anchor == Some(view.cursor) {
                    view.anchor = None; // a click, not a drag
                }
            }
            _ => return vec![],
        }
        vec![PanelEvent::NeedsRedraw]
    }
}

#[cfg(test)]
mod tests;
