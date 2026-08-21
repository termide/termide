//! Confirmation modal (Yes/No dialog).

use anyhow::Result;
use crossterm::event::KeyCode;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use termide_config::constants::MODAL_BUTTON_SPACING;
use termide_i18n as i18n;
use termide_theme::Theme;

use crate::{
    base::{button_style, render_modal_block},
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

    /// Start with "No" focused instead of "Yes".
    ///
    /// For a destructive, unrecoverable action — deleting files — the safe
    /// answer should be the one a stray Enter picks.
    pub fn defaulting_to_no(mut self) -> Self {
        self.selected = false;
        self
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
        // 1 (top border) + N (message lines) + 1 (empty) + 1 (buttons) + 1 (bottom border) = N + 4
        let message_lines = self.message.lines().count().max(1);
        let modal_height = (message_lines + 4) as u16;

        // Calculate dynamic width based on content
        let modal_width = self.calculate_modal_width(area.width);

        // Create centered area with calculated dimensions
        let modal_area = centered_rect_with_size(modal_width, modal_height, area);

        let inner = render_modal_block(modal_area, buf, &self.title, theme);

        // Split into: message, empty line, buttons
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(message_lines as u16), // Message
                Constraint::Length(1),                    // Empty line
                Constraint::Length(1),                    // Buttons
            ])
            .split(inner);

        // Render message
        let message = Paragraph::new(self.message.clone())
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.fg));
        message.render(chunks[0], buf);

        // Render buttons
        let t = i18n::t();

        let yes_style = button_style(self.selected, theme);
        let no_style = button_style(!self.selected, theme);

        let buttons = Line::from(vec![
            Span::styled(format!("[ {} ]", t.ui_yes()), yes_style),
            Span::raw("    "),
            Span::styled(format!("[ {} ]", t.ui_no()), no_style),
        ]);

        let buttons_paragraph = Paragraph::new(buttons).alignment(Alignment::Center);
        buttons_paragraph.render(chunks[2], buf);

        // Save buttons area for mouse handling
        self.last_buttons_area = Some(chunks[2]);
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let key = chord.raw;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use termide_core::KeyChord;

    fn press(modal: &mut ConfirmModal, code: KeyCode) -> Option<ModalResult<bool>> {
        modal
            .handle_key(KeyChord::identity(KeyEvent::new(code, KeyModifiers::NONE)))
            .unwrap()
    }

    #[test]
    fn confirmation_defaults_to_yes() {
        let mut modal = ConfirmModal::new("Confirm", "Proceed?");
        assert!(matches!(
            press(&mut modal, KeyCode::Enter),
            Some(ModalResult::Confirmed(true))
        ));
    }

    /// Delete prompts opt into "No" so a stray Enter cannot wipe files.
    #[test]
    fn defaulting_to_no_answers_no_on_enter() {
        let mut modal = ConfirmModal::new("Confirm", "Delete?").defaulting_to_no();
        assert!(matches!(
            press(&mut modal, KeyCode::Enter),
            Some(ModalResult::Confirmed(false))
        ));
    }

    /// The default only moves the starting focus; navigation still works.
    #[test]
    fn defaulting_to_no_still_allows_choosing_yes() {
        let mut modal = ConfirmModal::new("Confirm", "Delete?").defaulting_to_no();
        assert!(press(&mut modal, KeyCode::Left).is_none());
        assert!(matches!(
            press(&mut modal, KeyCode::Enter),
            Some(ModalResult::Confirmed(true))
        ));
    }
}
