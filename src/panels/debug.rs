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
use unicode_width::UnicodeWidthStr;

use super::Panel;
use crate::logger::LogLevel;
use crate::state::AppState;

/// Wrap text with hanging indent for continuation lines
fn wrap_message_with_indent(text: &str, max_width: usize, indent: usize) -> Vec<String> {
    if max_width == 0 || max_width <= indent {
        return vec![text.to_string()];
    }

    let text_width = text.width();
    if text_width <= max_width {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0;
    let mut is_first_line = true;

    // Effective width for continuation lines (accounting for indent)
    let continuation_width = max_width.saturating_sub(indent);

    // Split by words for better readability
    let parts: Vec<&str> = text.split_inclusive(' ').collect();

    for part in parts {
        let part_width = part.width();
        let effective_max = if is_first_line {
            max_width
        } else {
            continuation_width
        };

        // If part alone is too long, do hard break
        if part_width > effective_max {
            // Finish current line if any
            if !current_line.is_empty() {
                lines.push(current_line.clone());
                current_line.clear();
                current_width = 0;
                is_first_line = false;
            }

            // Break the long part character by character
            for ch in part.chars() {
                let ch_width = ch.to_string().width();
                let effective_max = if is_first_line {
                    max_width
                } else {
                    continuation_width
                };

                if current_width + ch_width > effective_max {
                    lines.push(current_line.clone());
                    current_line.clear();
                    current_width = 0;
                    is_first_line = false;
                }
                current_line.push(ch);
                current_width += ch_width;
            }
        } else if current_width + part_width > effective_max {
            // Part would overflow, start new line
            if !current_line.is_empty() {
                lines.push(current_line.clone());
            }
            current_line = part.to_string();
            current_width = part_width;
            is_first_line = false;
        } else {
            // Part fits in current line
            current_line.push_str(part);
            current_width += part_width;
        }
    }

    // Add remaining line
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

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

    /// Get log lines for display with word wrapping
    fn get_log_lines(&self, state: &AppState, height: usize, width: usize) -> Vec<Line<'static>> {
        let entries = crate::logger::get_entries();

        // Determine range for display
        let start = self.scroll_offset;

        let mut lines = Vec::new();

        // Prefix width: "[HH:MM:SS] LEVEL " = 11 + 5 + 1 = 17 characters
        const PREFIX_WIDTH: usize = 17;

        // Calculate available width for message text
        let message_width = if width > PREFIX_WIDTH {
            width.saturating_sub(PREFIX_WIDTH)
        } else {
            width
        };

        // Display log entries (from old to new)
        for entry in entries.iter().skip(start) {
            if lines.len() >= height {
                break;
            }

            let level_style = match entry.level {
                LogLevel::Debug => Style::default().fg(Color::DarkGray),
                LogLevel::Info => Style::default().fg(state.theme.fg),
                LogLevel::Warn => Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                LogLevel::Error => Style::default()
                    .fg(state.theme.error)
                    .add_modifier(Modifier::BOLD),
            };

            let level_text = match entry.level {
                LogLevel::Debug => "DEBUG",
                LogLevel::Info => "INFO ",
                LogLevel::Warn => "WARN ",
                LogLevel::Error => "ERROR",
            };

            let time_style = Style::default().fg(Color::DarkGray);

            // Wrap the message if needed
            let wrapped_lines = wrap_message_with_indent(&entry.message, message_width, 0);

            for (idx, wrapped_text) in wrapped_lines.iter().enumerate() {
                if lines.len() >= height {
                    break;
                }

                if idx == 0 {
                    // First line: timestamp + level + message
                    lines.push(Line::from(vec![
                        Span::styled(format!("[{}] ", entry.timestamp), time_style),
                        Span::styled(level_text, level_style),
                        Span::raw(" "),
                        Span::raw(wrapped_text.clone()),
                    ]));
                } else {
                    // Continuation lines: indent + message
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(PREFIX_WIDTH)),
                        Span::raw(wrapped_text.clone()),
                    ]));
                }
            }
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
        let content_width = area.width as usize;
        let log_lines = self.get_log_lines(state, content_height, content_width);

        // Auto-scroll to last messages if not manually scrolled
        let total_entries = crate::logger::get_entries().len();
        if total_entries > content_height && self.scroll_offset == 0 {
            self.scroll_offset = total_entries.saturating_sub(content_height);
        }

        // Render log content directly (accordion already drew border with title/buttons)
        // No need for .wrap() since we manually wrap in get_log_lines()
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

    fn to_session_panel(
        &mut self,
        session_dir: &std::path::Path,
    ) -> Option<crate::session::SessionPanel> {
        let _ = session_dir; // Unused for Debug panels
                             // Save debug panel (no state to persist)
        Some(crate::session::SessionPanel::Debug)
    }
}

impl Default for Debug {
    fn default() -> Self {
        Self::new()
    }
}
