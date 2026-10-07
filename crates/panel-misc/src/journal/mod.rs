//! Journal panel based on Editor with read-only mode.
//!
//! Provides a full-featured journal/log viewer with:
//! - Cursor navigation and text selection
//! - Copy to clipboard
//! - Auto-scroll to new entries
//! - Log level highlighting (DEBUG, INFO, WARN, ERROR)
//! - Clickable per-level toggles in a one-row header strip

pub mod highlighting;

use crossterm::event::{KeyCode, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use std::any::Any;

use termide_core::{Panel, PanelEvent, RenderContext, WidthPreference};
use termide_highlight::LineHighlighter;
use termide_logger::LogLevel;
use termide_panel_editor::{config::EditorConfig, Editor};
use termide_theme::Theme;

use highlighting::LogHighlightCache;

/// Levels in the order they appear in the header strip. Index into
/// [`JournalPanel::level_enabled`] / [`JournalPanel::pill_areas`].
const LEVELS: [LogLevel; 5] = [
    LogLevel::Trace,
    LogLevel::Debug,
    LogLevel::Info,
    LogLevel::Warn,
    LogLevel::Error,
];

fn level_label(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Trace => "TRACE",
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
    }
}

fn level_index(level: LogLevel) -> usize {
    match level {
        LogLevel::Trace => 0,
        LogLevel::Debug => 1,
        LogLevel::Info => 2,
        LogLevel::Warn => 3,
        LogLevel::Error => 4,
    }
}

/// Log viewer panel with Editor-based text display.
pub struct JournalPanel {
    /// Internal editor in read-only mode
    editor: Editor,
    /// Custom highlighter for log levels
    highlight_cache: LogHighlightCache,
    /// Auto-scroll enabled (scroll to new entries)
    auto_scroll: bool,
    /// Number of log entries already inspected for sync. Counts every
    /// entry the logger has produced, not just lines in the buffer —
    /// filtered-out entries also advance this counter so we never re-
    /// process them on the next tick.
    last_synced_count: usize,
    /// Per-level toggle. Levels with `false` are hidden from the
    /// buffer; toggling rebuilds it from scratch.
    level_enabled: [bool; LEVELS.len()],
    /// Cached screen rects of each pill, updated on every render so
    /// `handle_mouse` can hit-test clicks without recomputing layout.
    pill_areas: [Rect; LEVELS.len()],
    /// Cached theme for rendering
    cached_theme: Theme,
    /// Cached config for rendering
    cached_config: termide_config::Config,
}

impl JournalPanel {
    /// Create a new log viewer panel.
    pub fn new(theme: &termide_theme::Theme) -> Self {
        // Create editor with view_only config
        let mut config = EditorConfig::view_only();
        config.syntax_highlighting = true; // Enable to use our custom highlighter

        let editor = Editor::with_config(config);
        let highlight_cache = LogHighlightCache::new(*theme);

        Self {
            editor,
            highlight_cache,
            auto_scroll: true,
            last_synced_count: 0,
            level_enabled: [true; LEVELS.len()],
            pill_areas: [Rect::default(); LEVELS.len()],
            cached_theme: *theme,
            cached_config: termide_config::Config::default(),
        }
    }

    /// Rebuild the in-buffer view from scratch using current
    /// `level_enabled` flags. Cheap on small logs; called when the
    /// user toggles a pill so the filter takes effect immediately.
    fn rebuild_buffer(&mut self) {
        let mut config = EditorConfig::view_only();
        config.syntax_highlighting = true;
        self.editor = Editor::with_config(config);
        self.last_synced_count = 0;
        self.highlight_cache.invalidate_all();
        self.auto_scroll = true;
    }

    fn toggle_level(&mut self, idx: usize) {
        if idx >= LEVELS.len() {
            return;
        }
        self.level_enabled[idx] = !self.level_enabled[idx];
        self.rebuild_buffer();
    }

    /// Sync log entries from logger to buffer.
    ///
    /// Uses incremental synchronization: only fetches new entries since
    /// the last sync, avoiding O(n) clone of all log entries.
    fn sync_logs(&mut self) {
        // First check if there are new entries (O(1) operation)
        let new_count = termide_logger::entry_count();

        if new_count > self.last_synced_count {
            // Only fetch entries we haven't synced yet
            let new_entries = termide_logger::get_entries_from(self.last_synced_count);

            // Get buffer access through editor
            let buffer = self.editor.buffer_mut();

            // Append new entries that pass the active level filter.
            // `last_synced_count` still advances over filtered-out
            // entries so a later toggle (via `rebuild_buffer`) starts
            // from a clean slate and re-includes them.
            for entry in &new_entries {
                if !self.level_enabled[level_index(entry.level)] {
                    continue;
                }
                let level_text = match entry.level {
                    LogLevel::Trace => "TRACE",
                    LogLevel::Debug => "DEBUG",
                    LogLevel::Info => "INFO ",
                    LogLevel::Warn => "WARN ",
                    LogLevel::Error => "ERROR",
                };

                let line = format!("[{}] {} {}\n", entry.timestamp, level_text, entry.message);
                buffer.append(&line);
            }

            // Invalidate highlight cache for new lines
            self.highlight_cache.invalidate_from(self.last_synced_count);

            self.last_synced_count = new_count;
        }
    }

    /// Render the one-row header strip of clickable level pills.
    /// Stores each pill's screen rect in `pill_areas` for mouse
    /// hit-testing.
    fn render_pills(&mut self, area: Rect, buf: &mut Buffer) {
        let active_style = Style::default()
            .fg(self.cached_theme.fg)
            .add_modifier(Modifier::BOLD);
        let inactive_style = Style::default().fg(self.cached_theme.disabled);

        // Layout: " [TRACE] [DEBUG] [INFO] [WARN] [ERROR] "
        let mut spans = Vec::with_capacity(LEVELS.len() * 2 + 1);
        spans.push(Span::raw(" "));
        let mut x = area.x.saturating_add(1);
        for (i, level) in LEVELS.iter().enumerate() {
            let label = format!("[{}]", level_label(*level));
            let width = label.chars().count() as u16;
            self.pill_areas[i] = Rect {
                x,
                y: area.y,
                width,
                height: 1,
            };
            let style = if self.level_enabled[i] {
                active_style
            } else {
                inactive_style
            };
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(" "));
            x = x.saturating_add(width + 1);
        }

        Paragraph::new(Line::from(spans)).render(area, buf);
    }

    /// Access the inner editor (for search, modal requests, etc.).
    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    /// Scroll to the end of the log (word-wrap aware).
    fn scroll_to_end(&mut self) {
        self.editor.scroll_to_document_end();
    }

    /// Check if currently at the end of the log.
    fn is_at_end(&self, content_height: usize) -> bool {
        let line_count = self.editor.buffer().line_count();
        let top_line = self.editor.viewport().top_line;
        // Consider "at end" if we can see the last line
        top_line + content_height >= line_count
    }
}

impl Panel for JournalPanel {
    fn name(&self) -> &'static str {
        "journal"
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferWide
    }

    fn title(&self) -> String {
        termide_i18n::t().panel_journal().to_string()
    }

    fn prepare_render(&mut self, theme: &Theme, config: &std::sync::Arc<termide_config::Config>) {
        self.cached_theme = *theme;
        self.cached_config = (**config).clone();
        self.highlight_cache.set_theme(*theme);
        // Propagate config to inner editor so its hotkey table is built
        // (needed for Ctrl+C copy, Ctrl+F search, etc.)
        self.editor.prepare_render(theme, config);
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        // Sync new log entries
        self.sync_logs();

        // Carve off a one-row header strip for the level pills when
        // the panel is at least 2 rows tall; below that, the strip
        // would steal the only content row, so we render without it.
        let (pills_area, editor_area) = if area.height >= 2 {
            (
                Rect {
                    x: area.x,
                    y: area.y,
                    width: area.width,
                    height: 1,
                },
                Rect {
                    x: area.x,
                    y: area.y + 1,
                    width: area.width,
                    height: area.height - 1,
                },
            )
        } else {
            (Rect::default(), area)
        };

        if pills_area.height > 0 {
            self.render_pills(pills_area, buf);
        }

        // Auto-scroll if enabled, but not when search is active (viewport must follow search cursor)
        if self.auto_scroll
            && editor_area.height > 0
            && self.editor.get_search_match_info().is_none()
        {
            self.scroll_to_end();
        }

        // Render using editor's rendering with our custom highlighter
        self.editor.render_with_highlighter(
            editor_area,
            buf,
            &self.cached_theme,
            &self.cached_config,
            ctx.is_focused,
            &mut self.highlight_cache,
        );
    }

    fn handle_key(&mut self, chord: termide_core::KeyChord) -> Vec<PanelEvent> {
        // Plain 1..5 toggles the corresponding level pill (the pills are
        // also clickable). The buffer is read-only, so digits insert
        // nothing; only the find bar takes text, and it keeps them.
        // Not Alt+digit: Alt chords belong to app-wide actions, which
        // are dispatched before the panel sees the key.
        if chord.raw.modifiers == KeyModifiers::NONE && !self.editor.find_bar_has_focus() {
            if let KeyCode::Char(c) = chord.raw.code {
                if let Some(digit) = c.to_digit(10) {
                    let idx = digit as usize;
                    if (1..=LEVELS.len()).contains(&idx) {
                        self.toggle_level(idx - 1);
                        return vec![PanelEvent::NeedsRedraw];
                    }
                }
            }
        }

        let events = self.editor.handle_key(chord);

        // Auto-scroll when cursor is on the last content line (skip trailing empty line)
        let last_line = self.editor.buffer().line_count().saturating_sub(2);
        self.auto_scroll = self.editor.cursor_line() >= last_line;

        events
    }

    fn tick(&mut self) -> Vec<PanelEvent> {
        if self.auto_scroll && termide_logger::entry_count() > self.last_synced_count {
            vec![PanelEvent::NeedsRedraw]
        } else {
            vec![]
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, area: Rect) -> Vec<PanelEvent> {
        // Pill row gets first dibs on left clicks so a click on a
        // pill doesn't also move the editor cursor underneath.
        if let MouseEventKind::Down(crossterm::event::MouseButton::Left) = mouse.kind {
            for (i, rect) in self.pill_areas.iter().enumerate() {
                if rect.width > 0
                    && mouse.column >= rect.x
                    && mouse.column < rect.x + rect.width
                    && mouse.row == rect.y
                {
                    self.toggle_level(i);
                    return vec![PanelEvent::NeedsRedraw];
                }
            }
        }

        // Delegate to editor first so viewport updates before we check position
        let _ = self.editor.handle_mouse(mouse, area);

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.auto_scroll = false;
            }
            MouseEventKind::ScrollDown => {
                let content_height = area.height as usize;
                if self.is_at_end(content_height) {
                    self.auto_scroll = true;
                }
            }
            MouseEventKind::Down(_) => {
                let last_line = self.editor.buffer().line_count().saturating_sub(2);
                self.auto_scroll = self.editor.cursor_line() >= last_line;
            }
            _ => {}
        }

        vec![]
    }

    fn handle_scroll(&mut self, delta: i32, area: Rect) -> Vec<PanelEvent> {
        // Delegate to editor first so viewport updates before we check position
        let events = self.editor.handle_scroll(delta, area);

        if delta < 0 {
            self.auto_scroll = false;
        } else {
            let content_height = area.height as usize;
            if self.is_at_end(content_height) {
                self.auto_scroll = true;
            }
        }

        events
    }

    fn to_state(&self, _project_dir: &std::path::Path) -> Option<termide_core::PanelState> {
        Some(termide_core::PanelState::Journal)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl Default for JournalPanel {
    fn default() -> Self {
        Self::new(&termide_theme::Theme::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use termide_core::KeyChord;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyChord {
        KeyChord::identity(KeyEvent::new(code, modifiers))
    }

    #[test]
    fn plain_digit_toggles_its_level_pill() {
        let mut journal = JournalPanel::default();
        journal.handle_key(press(KeyCode::Char('3'), KeyModifiers::NONE));
        assert_eq!(journal.level_enabled, [true, true, false, true, true]);
        journal.handle_key(press(KeyCode::Char('3'), KeyModifiers::NONE));
        assert_eq!(journal.level_enabled, [true; LEVELS.len()]);
    }

    #[test]
    fn digits_outside_the_pill_range_and_alt_digits_do_nothing() {
        let mut journal = JournalPanel::default();
        journal.handle_key(press(KeyCode::Char('6'), KeyModifiers::NONE));
        journal.handle_key(press(KeyCode::Char('1'), KeyModifiers::ALT));
        assert_eq!(journal.level_enabled, [true; LEVELS.len()]);
    }

    #[test]
    fn digits_typed_into_the_find_bar_stay_there() {
        let mut journal = JournalPanel::default();
        // The editor builds its hotkey table (Ctrl+F included) from the config.
        let mut config = termide_config::Config::default();
        config.editor.keybindings.with_defaults();
        let config = std::sync::Arc::new(config);
        journal.prepare_render(&Theme::default(), &config);
        journal.handle_key(press(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert!(journal.editor.find_bar_has_focus());
        journal.handle_key(press(KeyCode::Char('1'), KeyModifiers::NONE));
        assert_eq!(journal.level_enabled, [true; LEVELS.len()]);
    }
}
