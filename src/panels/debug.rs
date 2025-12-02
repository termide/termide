use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::Widget,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::Panel;
use crate::state::{AppState, LogLevel};

/// Debug log panel
pub struct Debug {
    /// Scroll offset (for viewing old messages)
    scroll_offset: usize,
}

impl Debug {
    /// Create new log panel
    pub fn new() -> Self {
        Self { scroll_offset: 0 }
    }

    /// Get log lines for display
    fn get_log_lines(&self, state: &AppState, height: usize) -> Vec<Line<'static>> {
        let entries = state.get_log_entries();

        // Determine range for display
        let start = self.scroll_offset;

        let mut lines = Vec::new();

        // Display log entries (from old to new)
        for entry in entries.iter().skip(start).take(height) {
            let level_style = match entry.level {
                LogLevel::Info => Style::default().fg(state.theme.fg),
                LogLevel::Error => Style::default()
                    .fg(state.theme.error)
                    .add_modifier(Modifier::BOLD),
                LogLevel::Success => Style::default().fg(state.theme.success),
            };

            let level_text = match entry.level {
                LogLevel::Info => "INFO ",
                LogLevel::Error => "ERROR",
                LogLevel::Success => "OK   ",
            };

            let time_style = Style::default().fg(Color::DarkGray);

            lines.push(Line::from(vec![
                Span::styled(format!("[{:>8}ms] ", entry.timestamp), time_style),
                Span::styled(level_text, level_style),
                Span::raw(" "),
                Span::raw(entry.message.clone()),
            ]));
        }

        // If log is empty, show hint
        if lines.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "Log is empty. Perform any operations to display logs.",
                Style::default().fg(Color::DarkGray),
            )]));
        }

        lines
    }
}

impl Panel for Debug {
    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        _is_focused: bool,
        _panel_index: usize,
        state: &AppState,
    ) {
        // area is already the inner content area (accordion drew outer border)
        let content_height = area.height as usize;
        let log_lines = self.get_log_lines(state, content_height);

        // Auto-scroll to last messages if not manually scrolled
        let total_entries = state.get_log_entries().len();
        if total_entries > content_height && self.scroll_offset == 0 {
            self.scroll_offset = total_entries.saturating_sub(content_height);
        }

        // Render log content directly (accordion already drew border with title/buttons)
        let paragraph = Paragraph::new(log_lines);

        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                // Scroll up
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                // Scroll down
                self.scroll_offset = self.scroll_offset.saturating_add(1);
            }
            KeyCode::PageUp => {
                // Scroll page up
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
            }
            KeyCode::PageDown => {
                // Scroll page down
                self.scroll_offset = self.scroll_offset.saturating_add(10);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                // To beginning
                self.scroll_offset = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                // To end (auto-scroll)
                self.scroll_offset = 0;
            }
            _ => {}
        }
        Ok(())
    }

    fn title(&self) -> String {
        "Log".to_string()
    }

    fn to_session_panel(&self) -> Option<crate::session::SessionPanel> {
        // Save debug panel (no state to persist)
        Some(crate::session::SessionPanel::Debug)
    }
}

impl Default for Debug {
    fn default() -> Self {
        Self::new()
    }
}
