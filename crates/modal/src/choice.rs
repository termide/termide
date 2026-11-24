//! Choice modal with horizontal buttons.

use anyhow::Result;
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use termide_theme::Theme;

use crate::{
    base::{button_style, render_modal_block},
    calculate_modal_width, centered_rect_with_size, Modal, ModalResult, ModalWidthConfig,
};

/// Choice modal with horizontal buttons
#[derive(Debug)]
pub struct ChoiceModal {
    title: String,
    message: Option<String>,
    buttons: Vec<String>,
    selected: usize,
    last_buttons_area: Option<Rect>,
    last_button_positions: Vec<(u16, u16)>, // (start_col, end_col) for each button
}

impl ChoiceModal {
    /// Create a new choice modal with buttons
    pub fn new(title: impl Into<String>, message: Option<String>, buttons: Vec<String>) -> Self {
        Self {
            title: title.into(),
            message,
            buttons,
            selected: 0,
            last_buttons_area: None,
            last_button_positions: Vec::new(),
        }
    }

    /// Create choice modal without message
    pub fn buttons_only(title: impl Into<String>, buttons: Vec<String>) -> Self {
        Self::new(title, None, buttons)
    }

    /// Calculate modal width based on content
    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        let title_width = self.title.width() as u16 + 4;

        let message_width = self.message.as_ref().map(|m| m.width() as u16).unwrap_or(0);

        // Buttons: "[ btn1 ]  [ btn2 ]  [ btn3 ]"
        let buttons_width: u16 = self
            .buttons
            .iter()
            .map(|b| b.width() as u16 + 4) // "[ label ]"
            .sum::<u16>()
            + (self.buttons.len().saturating_sub(1) as u16) * 2; // spacing

        calculate_modal_width(
            [title_width, message_width, buttons_width].into_iter(),
            screen_width,
            ModalWidthConfig::default(),
        )
    }
}

impl Modal for ChoiceModal {
    type Result = usize; // Returns selected button index

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let has_message = self.message.is_some();
        let message_lines = self
            .message
            .as_ref()
            .map(|m| m.lines().count().max(1))
            .unwrap_or(0);

        // Height: border(1) + message(N) + empty(1) + buttons(1) + empty(1) + border(1)
        let modal_height = if has_message {
            (message_lines + 4) as u16
        } else {
            4u16 // Just buttons with padding
        };

        let modal_width = self.calculate_modal_width(area.width);
        let modal_area = centered_rect_with_size(modal_width, modal_height, area);

        let inner = render_modal_block(modal_area, buf, &self.title, theme);

        if has_message {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(message_lines as u16), // Message
                    Constraint::Length(1),                    // Empty
                    Constraint::Length(1),                    // Buttons
                    Constraint::Min(0),                       // Remaining
                ])
                .split(inner);

            // Render message
            if let Some(ref msg) = self.message {
                let message = Paragraph::new(msg.as_str())
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(theme.fg));
                message.render(chunks[0], buf);
            }

            self.render_buttons(chunks[2], buf, theme);
        } else {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // Empty/padding
                    Constraint::Length(1), // Buttons
                    Constraint::Min(0),    // Remaining
                ])
                .split(inner);

            self.render_buttons(chunks[1], buf, theme);
        }
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let key = chord.raw;
        match key.code {
            KeyCode::Left => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                Ok(None)
            }
            KeyCode::Right | KeyCode::Tab => {
                if self.selected < self.buttons.len().saturating_sub(1) {
                    self.selected += 1;
                }
                Ok(None)
            }
            KeyCode::Enter => Ok(Some(ModalResult::Confirmed(self.selected))),
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
            // Number keys for quick selection (1, 2, 3...)
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let num = c.to_digit(10).unwrap_or(0) as usize;
                if num >= 1 && num <= self.buttons.len() {
                    Ok(Some(ModalResult::Confirmed(num - 1)))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return Ok(None);
        }

        let Some(buttons_area) = self.last_buttons_area else {
            return Ok(None);
        };

        // Check if click is in buttons row
        if mouse.row != buttons_area.y {
            return Ok(None);
        }

        // Find which button was clicked
        for (i, (start, end)) in self.last_button_positions.iter().enumerate() {
            if mouse.column >= *start && mouse.column < *end {
                self.selected = i;
                return Ok(Some(ModalResult::Confirmed(i)));
            }
        }

        Ok(None)
    }
}

impl ChoiceModal {
    fn render_buttons(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        self.last_buttons_area = Some(area);
        self.last_button_positions.clear();

        // Build button spans
        let mut spans = Vec::new();
        for (i, label) in self.buttons.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            let style = button_style(i == self.selected, theme);
            spans.push(Span::styled(format!("[ {} ]", label), style));
        }

        // Calculate total width and starting position for centering
        let total_width: usize = self.buttons.iter().map(|b| b.width() + 4).sum::<usize>()
            + (self.buttons.len().saturating_sub(1)) * 2;

        let start_col = crate::base::centered_start(area, total_width);

        // Track button positions for mouse handling
        let mut col = start_col;
        for label in &self.buttons {
            let btn_width = label.width() as u16 + 4;
            self.last_button_positions.push((col, col + btn_width));
            col += btn_width + 2; // button + spacing
        }

        let buttons_line = Line::from(spans);
        crate::base::render_centered_line(&buttons_line, area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Labels in a script of several bytes a character are still clicked
    /// where they are drawn: the hit areas count columns, not bytes.
    #[test]
    fn a_click_lands_on_the_button_drawn_under_it() {
        let mut modal =
            ChoiceModal::new("x", None, vec!["Удалить".to_string(), "Отмена".to_string()]);
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        modal.render_buttons(area, &mut buf, &Theme::default());
        // The second button, `[ Отмена ]`, from its bracket to its bracket.
        let label = (0..area.width)
            .find(|&x| buf[(x, 0)].symbol() == "О")
            .expect("the second label is drawn");
        for column in label - 2..label + "Отмена".width() as u16 + 2 {
            let result = modal.handle_mouse(click(column, 0), area).unwrap();
            assert!(
                matches!(result, Some(ModalResult::Confirmed(1))),
                "column {column}: {result:?}"
            );
        }
    }
}
