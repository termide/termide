use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};

use super::{Modal, ModalResult};
use crate::i18n;
use crate::theme::Theme;

/// Confirmation modal window (Yes/No)
#[derive(Debug)]
pub struct ConfirmModal {
    title: String,
    message: String,
    selected: bool, // true = Yes, false = No
}

impl ConfirmModal {
    /// Create a new confirmation modal window
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            selected: true, // Default is Yes
        }
    }

    /// Calculate dynamic modal width based on content
    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        // 1. Title width (with spaces on the edges)
        let title_width = self.title.len() as u16 + 2;

        // 2. Maximum message line width
        let message_max_line_width = self
            .message
            .lines()
            .map(|line| line.len())
            .max()
            .unwrap_or(0) as u16;

        // 3. Buttons width: "[ Yes ]    [ No ]" = 17 characters
        let buttons_width = 17;

        // Take the maximum of all components
        let content_width = title_width.max(message_max_line_width).max(buttons_width);

        // Add padding and borders:
        // - 2 for border (1 on each side)
        // - 4 for padding (2 on each side for readability)
        let total_width = content_width + 6;

        // Apply constraints:
        // - Minimum 20 characters
        // - Maximum 75% of screen width
        let max_width = (screen_width as f32 * 0.75) as u16;
        total_width.max(20).min(max_width).min(screen_width)
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

impl Modal for ConfirmModal {
    type Result = bool;

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Calculate required height based on content:
        // 1 (top border) + 1 (empty line) + N (message lines) +
        // 1 (empty line) + 1 (buttons) + 1 (empty line) + 1 (bottom border) = N + 6
        let message_lines = self.message.lines().count().max(1);
        let modal_height = (message_lines + 6) as u16;

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

        // Split into: empty line, message, empty line, buttons, empty line
        use ratatui::layout::{Constraint, Direction, Layout};
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),                    // Empty line at top
                Constraint::Length(message_lines as u16), // Message
                Constraint::Length(1),                    // Empty line between message and buttons
                Constraint::Length(1),                    // Buttons
                Constraint::Length(1),                    // Empty line at bottom
            ])
            .split(inner);

        // Render message
        let message = Paragraph::new(self.message.clone())
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.bg));
        message.render(chunks[1], buf);

        // Render buttons
        let t = i18n::t();

        let yes_style = if self.selected {
            Style::default()
                .fg(theme.fg)
                .bg(theme.accented_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.accented_fg)
        };

        let no_style = if !self.selected {
            Style::default()
                .fg(theme.fg)
                .bg(theme.accented_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.accented_fg)
        };

        let buttons = Line::from(vec![
            Span::styled(format!("[ {} ]", t.ui_yes()), yes_style),
            Span::raw("    "),
            Span::styled(format!("[ {} ]", t.ui_no()), no_style),
        ]);

        let buttons_paragraph = Paragraph::new(buttons).alignment(Alignment::Center);
        buttons_paragraph.render(chunks[3], buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>> {
        match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                self.selected = !self.selected;
                Ok(None)
            }
            KeyCode::Enter => Ok(Some(ModalResult::Confirmed(self.selected))),
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
            KeyCode::Char('y') | KeyCode::Char('Y') => Ok(Some(ModalResult::Confirmed(true))),
            KeyCode::Char('n') | KeyCode::Char('N') => Ok(Some(ModalResult::Confirmed(false))),
            _ => Ok(None),
        }
    }
}
