#![allow(dead_code)]
use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::Widget,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use std::any::Any;
use unicode_width::UnicodeWidthStr;

use termide_core::{Panel, PanelEvent, RenderContext};
use termide_logger::LogLevel;

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

    let continuation_width = max_width.saturating_sub(indent);
    let parts: Vec<&str> = text.split_inclusive(' ').collect();

    for part in parts {
        let part_width = part.width();
        let effective_max = if is_first_line {
            max_width
        } else {
            continuation_width
        };

        if part_width > effective_max {
            if !current_line.is_empty() {
                lines.push(current_line.clone());
                current_line.clear();
                current_width = 0;
                is_first_line = false;
            }

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
            if !current_line.is_empty() {
                lines.push(current_line.clone());
            }
            current_line = part.to_string();
            current_width = part_width;
            is_first_line = false;
        } else {
            current_line.push_str(part);
            current_width += part_width;
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Debug log panel
pub struct DebugPanel {
    scroll_offset: usize,
}

impl DebugPanel {
    pub fn new() -> Self {
        Self { scroll_offset: 0 }
    }

    fn calculate_autoscroll_offset(&self, width: usize, height: usize) -> usize {
        let entries = termide_logger::get_entries();
        if entries.is_empty() || height == 0 {
            return 0;
        }

        const PREFIX_WIDTH: usize = 17;
        let message_width = if width > PREFIX_WIDTH {
            width.saturating_sub(PREFIX_WIDTH)
        } else {
            width
        };

        let mut visual_lines_per_entry: Vec<usize> = Vec::with_capacity(entries.len());
        for entry in entries.iter() {
            let wrapped = wrap_message_with_indent(&entry.message, message_width, 0);
            visual_lines_per_entry.push(wrapped.len());
        }

        let mut accumulated = 0;
        let mut start_idx = entries.len();

        for (idx, &lines) in visual_lines_per_entry.iter().enumerate().rev() {
            if accumulated + lines > height {
                break;
            }
            accumulated += lines;
            start_idx = idx;
        }

        start_idx
    }

    fn get_log_lines(
        &self,
        ctx: &RenderContext,
        height: usize,
        width: usize,
    ) -> Vec<Line<'static>> {
        let entries = termide_logger::get_entries();
        let start = self.scroll_offset;
        let mut lines = Vec::new();

        const PREFIX_WIDTH: usize = 17;
        let message_width = if width > PREFIX_WIDTH {
            width.saturating_sub(PREFIX_WIDTH)
        } else {
            width
        };

        for entry in entries.iter().skip(start) {
            if lines.len() >= height {
                break;
            }

            let level_style = match entry.level {
                LogLevel::Debug => Style::default().fg(Color::DarkGray),
                LogLevel::Info => Style::default().fg(ctx.theme.fg),
                LogLevel::Warn => Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                LogLevel::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            };

            let level_text = match entry.level {
                LogLevel::Debug => "DEBUG",
                LogLevel::Info => "INFO ",
                LogLevel::Warn => "WARN ",
                LogLevel::Error => "ERROR",
            };

            let time_style = Style::default().fg(Color::DarkGray);
            let wrapped_lines = wrap_message_with_indent(&entry.message, message_width, 0);

            for (idx, wrapped_text) in wrapped_lines.iter().enumerate() {
                if lines.len() >= height {
                    break;
                }

                if idx == 0 {
                    lines.push(Line::from(vec![
                        Span::styled(format!("[{}] ", entry.timestamp), time_style),
                        Span::styled(level_text, level_style),
                        Span::raw(" "),
                        Span::raw(wrapped_text.clone()),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(PREFIX_WIDTH)),
                        Span::raw(wrapped_text.clone()),
                    ]));
                }
            }
        }

        if lines.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "Log is empty. Perform any operations to display logs.",
                Style::default().fg(Color::DarkGray),
            )]));
        }

        lines
    }
}

impl Panel for DebugPanel {
    fn name(&self) -> &'static str {
        "debug"
    }

    fn title(&self) -> String {
        "Log".to_string()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        let content_height = area.height as usize;
        let content_width = area.width as usize;

        if self.scroll_offset == 0 {
            self.scroll_offset = self.calculate_autoscroll_offset(content_width, content_height);
        }

        let log_lines = self.get_log_lines(ctx, content_height, content_width);
        let paragraph = Paragraph::new(log_lines);
        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Vec<PanelEvent> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
            }
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
            }
            KeyCode::PageDown => {
                self.scroll_offset = self.scroll_offset.saturating_add(10);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.scroll_offset = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.scroll_offset = 0;
            }
            _ => {}
        }
        vec![]
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, _area: Rect) -> Vec<PanelEvent> {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(3);
            }
            MouseEventKind::ScrollDown => {
                self.scroll_offset = self.scroll_offset.saturating_add(3);
            }
            _ => {}
        }
        vec![]
    }

    fn to_session(&self, _session_dir: &std::path::Path) -> Option<termide_core::SessionPanel> {
        Some(termide_core::SessionPanel::Debug)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl Default for DebugPanel {
    fn default() -> Self {
        Self::new()
    }
}
