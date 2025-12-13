//! Log viewer panel based on Editor with read-only mode.
//!
//! Provides a full-featured log viewer with:
//! - Cursor navigation and text selection
//! - Copy to clipboard
//! - Auto-scroll to new entries
//! - Log level highlighting (DEBUG, INFO, WARN, ERROR)

pub mod highlighting;

use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect};
use std::any::Any;

use termide_core::{Panel, PanelEvent, RenderContext};
use termide_highlight::LineHighlighter;
use termide_logger::LogLevel;
use termide_panel_editor::{config::EditorConfig, Editor};
use termide_theme::Theme;

use highlighting::LogHighlightCache;

/// Log viewer panel with Editor-based text display.
pub struct LogViewerPanel {
    /// Internal editor in read-only mode
    editor: Editor,
    /// Custom highlighter for log levels
    highlight_cache: LogHighlightCache,
    /// Auto-scroll enabled (scroll to new entries)
    auto_scroll: bool,
    /// Number of log entries already synced to buffer
    last_synced_count: usize,
    /// Cached theme for rendering
    cached_theme: Theme,
    /// Cached config for rendering
    cached_config: termide_config::Config,
}

impl LogViewerPanel {
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
            cached_theme: *theme,
            cached_config: termide_config::Config::default(),
        }
    }

    /// Sync log entries from logger to buffer.
    fn sync_logs(&mut self) {
        let entries = termide_logger::get_entries();
        let new_count = entries.len();

        if new_count > self.last_synced_count {
            // Get buffer access through editor
            let buffer = self.editor.buffer_mut();

            // Append new entries
            for entry in entries.iter().skip(self.last_synced_count) {
                let level_text = match entry.level {
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

    /// Scroll to the end of the log.
    fn scroll_to_end(&mut self, content_height: usize) {
        let line_count = self.editor.buffer().line_count();
        if line_count > content_height {
            self.editor.viewport_mut().top_line = line_count.saturating_sub(content_height);
        }
        // Move cursor to last line
        let last_line = line_count.saturating_sub(1);
        self.editor.set_cursor_line(last_line);
    }

    /// Check if currently at the end of the log.
    fn is_at_end(&self, content_height: usize) -> bool {
        let line_count = self.editor.buffer().line_count();
        let top_line = self.editor.viewport().top_line;
        // Consider "at end" if we can see the last line
        top_line + content_height >= line_count
    }
}

impl Panel for LogViewerPanel {
    fn name(&self) -> &'static str {
        "log_viewer"
    }

    fn title(&self) -> String {
        "Log".to_string()
    }

    fn prepare_render(&mut self, theme: &Theme, config: &termide_config::Config) {
        self.cached_theme = *theme;
        self.cached_config = config.clone();
        self.highlight_cache.set_theme(*theme);
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _ctx: &RenderContext) {
        // Sync new log entries
        self.sync_logs();

        let content_height = area.height as usize;

        // Auto-scroll if enabled
        if self.auto_scroll && content_height > 0 {
            self.scroll_to_end(content_height);
        }

        // Render using editor's rendering with our custom highlighter
        self.editor.render_with_highlighter(
            area,
            buf,
            &self.cached_theme,
            &self.cached_config,
            &mut self.highlight_cache,
        );
    }

    fn handle_key(&mut self, key: KeyEvent) -> Vec<PanelEvent> {
        // Check for auto-scroll toggle keys
        match key.code {
            // Disable auto-scroll on scroll up
            KeyCode::Up
            | KeyCode::Char('k')
            | KeyCode::PageUp
            | KeyCode::Home
            | KeyCode::Char('g') => {
                self.auto_scroll = false;
            }
            // Enable auto-scroll on scroll to end
            KeyCode::End | KeyCode::Char('G') => {
                self.auto_scroll = true;
            }
            _ => {}
        }

        // Delegate to editor for actual handling
        let _ = self.editor.handle_key(key);
        vec![]
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, area: Rect) -> Vec<PanelEvent> {
        // Check for scroll events that affect auto-scroll
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.auto_scroll = false;
            }
            MouseEventKind::ScrollDown => {
                // Check if scrolling to end
                let content_height = area.height as usize;
                if self.is_at_end(content_height) {
                    self.auto_scroll = true;
                }
            }
            _ => {}
        }

        // Delegate to editor
        let _ = self.editor.handle_mouse(mouse, area);
        vec![]
    }

    fn to_session(&self, _session_dir: &std::path::Path) -> Option<termide_core::SessionPanel> {
        // Save as Debug panel type (same session type)
        Some(termide_core::SessionPanel::Debug)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl Default for LogViewerPanel {
    fn default() -> Self {
        Self::new(&termide_theme::Theme::default())
    }
}
