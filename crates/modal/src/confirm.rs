//! Confirmation modal (Yes/No dialog).

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};

use termide_config::constants::MODAL_BUTTON_SPACING;
use termide_i18n as i18n;
use termide_theme::Theme;

use crate::{
    calculate_modal_width, centered_rect_with_size, max_line_width, Modal, ModalResult,
    ModalWidthConfig,
};

/// Confirmation modal window (Yes/No)
#[derive(Debug)]
pub struct ConfirmModal {
    title: String,
    message: String,
    selected: bool, // true = Yes, false = No
    last_buttons_area: Option<Rect>,
}

impl ConfirmModal {
    /// Create a new confirmation modal window
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            selected: true, // Default is Yes
            last_buttons_area: None,
        }
    }

    /// Calculate dynamic modal width based on content
    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        let title_width = self.title.len() as u16 + 2;
        let message_width = max_line_width(&self.message);
        let buttons_width = 17u16; // "[ Yes ]    [ No ]"

        calculate_modal_width(
            [title_width, message_width, buttons_width].into_iter(),
            screen_width,
            ModalWidthConfig::default(),
        )
    }
}

impl Modal for ConfirmModal {
    type Result = bool;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Calculate required height based on content:
        // 1 (top border) + N (message lines) + 1 (buttons) + 1 (bottom border) = N + 3
        let message_lines = self.message.lines().count().max(1);
        let modal_height = (message_lines + 3) as u16;

        // Calculate dynamic width based on content
        let modal_width = self.calculate_modal_width(area.width);

        // Create centered area with calculated dimensions
        let modal_area = centered_rect_with_size(modal_width, modal_height, area);

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

        // Split into: message, buttons
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(message_lines as u16), // Message
                Constraint::Length(1),                    // Buttons
            ])
            .split(inner);

        // Render message
        let message = Paragraph::new(self.message.clone())
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.bg));
        message.render(chunks[0], buf);

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
        buttons_paragraph.render(chunks[1], buf);

        // Save buttons area for mouse handling
        self.last_buttons_area = Some(chunks[1]);
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

        // Check if we have stored buttons area
        let Some(buttons_area) = self.last_buttons_area else {
            return Ok(None);
        };

        // Check if click is within buttons area
        if mouse.row < buttons_area.y
            || mouse.row >= buttons_area.y + buttons_area.height
            || mouse.column < buttons_area.x
            || mouse.column >= buttons_area.x + buttons_area.width
        {
            return Ok(None);
        }

        // Calculate button positions
        // Buttons are centered: "[ Yes ]    [ No ]"
        let t = i18n::t();
        let yes_text = format!("[ {} ]", t.ui_yes());
        let no_text = format!("[ {} ]", t.ui_no());
        let total_text_width = yes_text.len() + MODAL_BUTTON_SPACING as usize + no_text.len();

        let start_col =
            buttons_area.x + (buttons_area.width.saturating_sub(total_text_width as u16)) / 2;
        let yes_end = start_col + yes_text.len() as u16;
        let no_start = yes_end + MODAL_BUTTON_SPACING;
        let no_end = no_start + no_text.len() as u16;

        // Determine which button was clicked
        if mouse.column >= start_col && mouse.column < yes_end {
            // Yes button clicked
            self.selected = true;
            Ok(Some(ModalResult::Confirmed(true)))
        } else if mouse.column >= no_start && mouse.column < no_end {
            // No button clicked
            self.selected = false;
            Ok(Some(ModalResult::Confirmed(false)))
        } else {
            Ok(None)
        }
    }
}
