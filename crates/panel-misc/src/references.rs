//! References panel — shows LSP find-references results.
//!
//! Opened via Shift+F12 in the editor. Displays a list of file locations,
//! one per line: `path/to/file.rs:42  preview text`. Enter navigates to
//! the selected location in the editor.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};
use std::any::Any;

use termide_core::{PanelEvent, ReferenceLocation, RenderContext, ThemeColors, WidthPreference};
use termide_theme::Theme;

/// A resolved reference with preview text (line contents).
#[derive(Debug, Clone)]
struct ResolvedRef {
    path: PathBuf,
    line: usize,
    column: usize,
    /// Short relative path for display
    display_path: String,
    /// Text of the matched line (trimmed)
    preview: String,
}

/// Panel showing LSP find-references results.
pub struct ReferencesPanel {
    items: Vec<ResolvedRef>,
    selected: usize,
    scroll: usize,
    last_height: usize,
    cached_theme: Theme,
    /// Symbol name shown in panel title
    symbol_name: Option<String>,
}

impl ReferencesPanel {
    pub fn new(
        locations: Vec<ReferenceLocation>,
        symbol_name: Option<String>,
        theme: &Theme,
    ) -> Self {
        let items = resolve_locations(locations);
        Self {
            items,
            selected: 0,
            scroll: 0,
            last_height: 0,
            cached_theme: *theme,
            symbol_name,
        }
    }

    /// Replace contents with new results (panel reuse on repeated Shift+F12).
    pub fn update(&mut self, locations: Vec<ReferenceLocation>, symbol_name: Option<String>) {
        self.items = resolve_locations(locations);
        self.selected = 0;
        self.scroll = 0;
        self.symbol_name = symbol_name;
    }

    fn select_next(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1).min(self.items.len() - 1);
        }
        self.ensure_visible();
    }

    fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
        self.ensure_visible();
    }

    fn ensure_visible(&mut self) {
        let height = self.last_height.saturating_sub(2);
        self.scroll = termide_ui::ensure_offset_visible(self.scroll, self.selected, height);
    }

    fn open_selected(&self) -> Vec<PanelEvent> {
        if let Some(item) = self.items.get(self.selected) {
            vec![PanelEvent::OpenFileAt {
                path: item.path.clone(),
                line: item.line,
                column: item.column,
            }]
        } else {
            vec![]
        }
    }
}

impl termide_core::Panel for ReferencesPanel {
    fn name(&self) -> &'static str {
        "references"
    }

    fn title(&self) -> String {
        match &self.symbol_name {
            Some(name) => format!("References: {} ({})", name, self.items.len()),
            None => format!("References ({})", self.items.len()),
        }
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferNarrow
    }

    fn prepare_render(&mut self, theme: &Theme, _config: &Arc<termide_config::Config>) {
        self.cached_theme = *theme;
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        self.last_height = area.height as usize;

        let theme: &ThemeColors = ctx.theme;
        let focused = ctx.is_focused;

        let border_style = if focused {
            Style::default().fg(theme.border_focused)
        } else {
            Style::default().fg(theme.border)
        };

        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(border_style)
            .title(Span::styled(
                format!(" {} ", self.title()),
                Style::default()
                    .fg(theme.border_focused)
                    .add_modifier(Modifier::BOLD),
            ));
        block.render(area, buf);

        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };

        if self.items.is_empty() {
            let msg = Line::from(Span::styled(
                "No references found",
                Style::default().fg(theme.disabled),
            ));
            buf.set_line(inner.x, inner.y, &msg, inner.width);
            return;
        }

        let visible_height = inner.height as usize;
        let items_to_show = self
            .items
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(visible_height);

        for (row, (idx, item)) in items_to_show.enumerate() {
            let y = inner.y + row as u16;
            let is_selected = idx == self.selected;

            let (path_style, preview_style, indicator) = if is_selected {
                (
                    Style::default()
                        .fg(theme.selection_fg)
                        .bg(theme.selection_bg)
                        .add_modifier(Modifier::BOLD),
                    Style::default()
                        .fg(theme.selection_fg)
                        .bg(theme.selection_bg),
                    "►",
                )
            } else {
                (
                    Style::default().fg(theme.info),
                    Style::default().fg(theme.fg),
                    " ",
                )
            };

            let line = Line::from(vec![
                Span::raw(indicator),
                Span::styled(
                    format!("{}:{}", item.display_path, item.line + 1),
                    path_style,
                ),
                Span::styled("  ", preview_style),
                Span::styled(item.preview.clone(), preview_style),
            ]);
            buf.set_line(inner.x, y, &line, inner.width);
        }
    }

    fn handle_key(&mut self, chord: termide_core::KeyChord) -> Vec<PanelEvent> {
        let key = chord.raw;
        match (key.code, key.modifiers) {
            (KeyCode::Down, KeyModifiers::NONE) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                self.select_next();
            }
            (KeyCode::Up, KeyModifiers::NONE) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                self.select_prev();
            }
            (KeyCode::PageDown, KeyModifiers::NONE) => {
                let page = self.last_height.saturating_sub(3);
                for _ in 0..page {
                    self.select_next();
                }
            }
            (KeyCode::PageUp, KeyModifiers::NONE) => {
                let page = self.last_height.saturating_sub(3);
                for _ in 0..page {
                    self.select_prev();
                }
            }
            (KeyCode::Home, KeyModifiers::NONE) | (KeyCode::Char('g'), KeyModifiers::NONE) => {
                self.selected = 0;
                self.scroll = 0;
            }
            (KeyCode::End, KeyModifiers::NONE) => {
                self.selected = self.items.len().saturating_sub(1);
                self.ensure_visible();
            }
            (KeyCode::Char('G'), KeyModifiers::SHIFT) => {
                self.selected = self.items.len().saturating_sub(1);
                self.ensure_visible();
            }
            (KeyCode::Enter, KeyModifiers::NONE) => {
                return self.open_selected();
            }
            _ => {}
        }
        vec![]
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect) -> Vec<PanelEvent> {
        let inner_y = area.y + 1;
        let inner_height = area.height.saturating_sub(2) as usize;

        match event.kind {
            MouseEventKind::ScrollDown => {
                self.select_next();
            }
            MouseEventKind::ScrollUp => {
                self.select_prev();
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let row = event.row.saturating_sub(inner_y) as usize;
                if row < inner_height {
                    let idx = self.scroll + row;
                    if idx < self.items.len() {
                        if self.selected == idx {
                            return self.open_selected();
                        }
                        self.selected = idx;
                    }
                }
            }
            _ => {}
        }
        vec![]
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ──────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────

fn resolve_locations(locations: Vec<ReferenceLocation>) -> Vec<ResolvedRef> {
    locations
        .into_iter()
        .map(|loc| {
            let preview = read_line(&loc.path, loc.line);
            let display_path = short_path(&loc.path);
            ResolvedRef {
                display_path,
                preview,
                path: loc.path,
                line: loc.line,
                column: loc.column,
            }
        })
        .collect()
}

/// Read a single line from a file (0-based). Returns empty string on failure.
fn read_line(path: &PathBuf, line: usize) -> String {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return String::new(),
    };
    let reader = std::io::BufReader::new(file);
    reader
        .lines()
        .nth(line)
        .and_then(|l| l.ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Shorten a path for display: strip cwd prefix if possible, otherwise last 2 components.
fn short_path(path: &Path) -> String {
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(rel) = path.strip_prefix(&cwd) {
            return rel.to_string_lossy().to_string();
        }
    }
    let components: Vec<_> = path.components().collect();
    if components.len() >= 2 {
        let tail: PathBuf = components[components.len() - 2..].iter().collect();
        tail.to_string_lossy().to_string()
    } else {
        path.to_string_lossy().to_string()
    }
}
