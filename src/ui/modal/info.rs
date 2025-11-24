use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use super::{Modal, ModalResult};
use crate::constants::{SPINNER_FRAMES, SPINNER_FRAMES_COUNT};
use crate::i18n;
use crate::theme::Theme;

/// Information modal window (closes on any key)
#[derive(Debug)]
pub struct InfoModal {
    title: String,
    lines: Vec<(String, String)>,   // (key, value) pairs for table
    spinner_frame: usize,           // Frame counter for spinner animation
    last_button_area: Option<Rect>, // For mouse handling
}

impl InfoModal {
    /// Create a new information modal window with tabular data
    pub fn new(title: impl Into<String>, lines: Vec<(String, String)>) -> Self {
        Self {
            title: title.into(),
            lines,
            spinner_frame: 0,
            last_button_area: None,
        }
    }

    /// Update a specific field value by key
    pub fn update_value(&mut self, key: &str, new_value: String) {
        if let Some(line) = self.lines.iter_mut().find(|(k, _)| k == key) {
            line.1 = new_value;
        }
    }

    /// Advance the spinner frame counter (for animation)
    pub fn advance_spinner(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES_COUNT;
    }

    /// Get the current spinner character
    fn get_spinner_char(&self) -> &str {
        SPINNER_FRAMES[self.spinner_frame]
    }

    /// Calculate dynamic modal width based on content
    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        // 1. Title width (with padding on edges)
        let title_width = self.title.width() as u16 + 2;

        // 2. Maximum line width (key + ": " + value)
        let max_line_width = self
            .lines
            .iter()
            .map(|(key, value)| key.width() + 2 + value.width()) // +2 for ": "
            .max()
            .unwrap_or(0) as u16;

        // Take maximum of all components
        let content_width = title_width.max(max_line_width);

        // Add margins and borders:
        // - 2 for border (1 on each side)
        // - 4 for padding (2 on each side for readability)
        let total_width = content_width + 6;

        // Apply constraints:
        // - Minimum 30 characters
        // - Maximum 80% of screen width
        let max_width = (screen_width as f32 * 0.8) as u16;
        total_width.max(30).min(max_width).min(screen_width)
    }

    /// Create a centered rectangle with fixed size
    fn centered_rect_with_size(width: u16, height: u16, r: Rect) -> Rect {
        use ratatui::layout::{Constraint, Direction, Layout};

        // Calculate margins
        let horizontal_margin = r.width.saturating_sub(width) / 2;
        let vertical_margin = r.height.saturating_sub(height) / 2;

        let vertical_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(vertical_margin),
                Constraint::Length(height),
                Constraint::Length(vertical_margin),
            ])
            .split(r);

        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(horizontal_margin),
                Constraint::Length(width),
                Constraint::Length(horizontal_margin),
            ])
            .split(vertical_layout[1])[1]
    }
}

impl Modal for InfoModal {
    type Result = ();

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Calculate required height based on content:
        // 1 (top border) + 1 (empty line) + N (data lines) +
        // 1 (empty line) + 1 (button) + 1 (bottom border) = N + 5
        let modal_height = (self.lines.len() + 5) as u16;

        // Calculate dynamic width based on content
        let modal_width = self.calculate_modal_width(area.width);

        // Create centered area with calculated dimensions
        let modal_area = Self::centered_rect_with_size(modal_width, modal_height, area);

        // Clear the area
        Clear.render(modal_area, buf);

        // Create block with inverted colors
        let block = Block::default()
            .title(Span::styled(
                format!(" {} ", self.title),
                Style::default().fg(theme.bg).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.bg))
            .style(Style::default().bg(theme.fg));

        let inner = block.inner(modal_area);
        block.render(modal_area, buf);

        // Split into: empty line, data, empty line, button
        use ratatui::layout::{Constraint, Direction, Layout};
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),                       // Empty line at top
                Constraint::Length(self.lines.len() as u16), // Data
                Constraint::Length(1),                       // Empty line
                Constraint::Length(1),                       // Button
            ])
            .split(inner);

        // Find maximum key length for alignment
        let max_key_len = self
            .lines
            .iter()
            .map(|(key, _)| key.width())
            .max()
            .unwrap_or(0);

        // Render tabular data with left alignment
        let t = i18n::t();
        let mut text_lines = Vec::new();
        for (key, value) in &self.lines {
            let padding = " ".repeat(max_key_len - key.width());

            // If value contains calculating text, show spinner
            let display_value = if value.contains(t.file_info_calculating()) {
                format!("{} {}", self.get_spinner_char(), value)
            } else {
                value.clone()
            };

            text_lines.push(Line::from(vec![
                Span::styled(
                    format!("  {}{}", key, padding),
                    Style::default()
                        .fg(theme.accented_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(": "),
                Span::styled(display_value, Style::default().fg(theme.bg)),
            ]));
        }

        let data = Paragraph::new(text_lines).alignment(Alignment::Left);
        data.render(chunks[1], buf);

        // Render Close button (always highlighted)
        let close_button = Line::from(vec![Span::styled(
            format!("[ {} ]", t.ui_close()),
            Style::default()
                .fg(theme.fg)
                .bg(theme.accented_fg)
                .add_modifier(Modifier::BOLD),
        )]);

        let button_paragraph = Paragraph::new(close_button).alignment(Alignment::Center);
        button_paragraph.render(chunks[3], buf);

        // Save button area for mouse handling
        self.last_button_area = Some(chunks[3]);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>> {
        // Close on any key
        match key.code {
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
            _ => Ok(Some(ModalResult::Confirmed(()))),
        }
    }

    fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        use crossterm::event::MouseEventKind;

        // Only handle left button press
        if mouse.kind != MouseEventKind::Down(crossterm::event::MouseButton::Left) {
            return Ok(None);
        }

        // Check if we have stored button area
        let Some(button_area) = self.last_button_area else {
            return Ok(None);
        };

        // Check if click is within button area
        if mouse.row < button_area.y
            || mouse.row >= button_area.y + button_area.height
            || mouse.column < button_area.x
            || mouse.column >= button_area.x + button_area.width
        {
            return Ok(None);
        }

        // Calculate button position
        // Button is centered: "[ Close ]"
        let t = i18n::t();
        let button_text = format!("[ {} ]", t.ui_close());
        let button_width = button_text.len() as u16;

        let start_col = button_area.x + (button_area.width.saturating_sub(button_width)) / 2;
        let end_col = start_col + button_width;

        // Check if click is within button bounds
        if mouse.column >= start_col && mouse.column < end_col {
            // Close button clicked
            Ok(Some(ModalResult::Confirmed(())))
        } else {
            Ok(None)
        }
    }
}
