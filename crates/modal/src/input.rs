//! Text input modal dialog.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
    ModalWidthConfig, TextInputHandler,
};

/// Focus area in the modal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Input,
    Buttons,
}

/// Text input modal window
#[derive(Debug)]
pub struct InputModal {
    title: String,
    prompt: String,
    input_handler: TextInputHandler,
    focus: FocusArea,
    selected_button: usize, // 0 = OK, 1 = Cancel
    last_buttons_area: Option<Rect>,
}

impl InputModal {
    /// Create a new input modal window
    pub fn new(title: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            prompt: prompt.into(),
            input_handler: TextInputHandler::new(),
            focus: FocusArea::Input,
            selected_button: 0, // OK button selected by default
            last_buttons_area: None,
        }
    }

    /// Create with default value
    pub fn with_default(
        title: impl Into<String>,
        prompt: impl Into<String>,
        default: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            prompt: prompt.into(),
            input_handler: TextInputHandler::with_default(default),
            focus: FocusArea::Input,
            selected_button: 0, // OK button selected by default
            last_buttons_area: None,
        }
    }

    /// Calculate dynamic modal width and height
    fn calculate_modal_size(&self, screen_width: u16, screen_height: u16) -> (u16, u16) {
        let title_width = self.title.len() as u16 + 2;
        let prompt_width = max_line_width(&self.prompt);
        let buttons_width = 21u16; // "[ OK ]    [ Cancel ]"
        let input_width = self.input_handler.text().chars().count() as u16 + 20;

        let width = calculate_modal_width(
            [title_width, prompt_width, buttons_width, input_width].into_iter(),
            screen_width,
            ModalWidthConfig {
                wide: false,
                double_border: true,
            },
        );

        // Calculate height: border + prompt + input(3) + buttons + border
        let prompt_lines = if self.prompt.is_empty() {
            0
        } else {
            self.prompt.lines().count().max(1) as u16
        };
        let height = (1 + prompt_lines + 3 + 1 + 1).min(screen_height);

        (width, height)
    }
}

impl Modal for InputModal {
    type Result = String;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Calculate dynamic dimensions
        let (modal_width, modal_height) = self.calculate_modal_size(area.width, area.height);

        // Create centered area
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

        // Split into prompt (if not empty), input, and buttons
        let prompt_lines = if self.prompt.is_empty() {
            0
        } else {
            self.prompt.lines().count().max(1) as u16
        };

        let constraints = if prompt_lines > 0 {
            vec![
                Constraint::Length(prompt_lines), // Prompt
                Constraint::Length(3),            // Input
                Constraint::Length(1),            // Buttons
            ]
        } else {
            vec![
                Constraint::Length(3), // Input
                Constraint::Length(1), // Buttons
            ]
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        let mut chunk_idx = 0;

        // Render prompt if not empty
        if prompt_lines > 0 {
            let prompt = Paragraph::new(self.prompt.clone())
                .alignment(Alignment::Left)
                .style(Style::default().fg(theme.bg));
            prompt.render(chunks[chunk_idx], buf);
            chunk_idx += 1;
        }

        // Render input field
        let input_line = Line::from(vec![
            Span::styled(
                self.input_handler.text_before_cursor(),
                Style::default().fg(theme.bg),
            ),
            Span::styled("█", Style::default().fg(theme.success)),
            Span::styled(
                self.input_handler.text_after_cursor(),
                Style::default().fg(theme.bg),
            ),
        ]);

        let input_paragraph = Paragraph::new(input_line)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.success)),
            )
            .style(Style::default().bg(theme.fg));
        input_paragraph.render(chunks[chunk_idx], buf);
        chunk_idx += 1;

        // Render buttons
        let t = i18n::t();

        let ok_style = if self.focus == FocusArea::Buttons && self.selected_button == 0 {
            Style::default()
                .fg(theme.fg)
                .bg(theme.accented_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.accented_fg)
        };

        let cancel_style = if self.focus == FocusArea::Buttons && self.selected_button == 1 {
            Style::default()
                .fg(theme.fg)
                .bg(theme.accented_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.accented_fg)
        };

        let buttons = Line::from(vec![
            Span::styled(format!("[ {} ]", t.ui_ok()), ok_style),
            Span::raw("    "),
            Span::styled(format!("[ {} ]", t.ui_cancel()), cancel_style),
        ]);

        let buttons_paragraph = Paragraph::new(buttons).alignment(Alignment::Center);
        buttons_paragraph.render(chunks[chunk_idx], buf);

        // Save buttons area for mouse handling
        self.last_buttons_area = Some(chunks[chunk_idx]);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>> {
        // Escape always cancels
        if key.code == KeyCode::Esc {
            return Ok(Some(ModalResult::Cancelled));
        }

        match self.focus {
            FocusArea::Input => {
                match key.code {
                    KeyCode::Down => {
                        // Move focus to buttons
                        self.focus = FocusArea::Buttons;
                        Ok(None)
                    }
                    KeyCode::Enter => {
                        // Confirm input (or cancel if empty)
                        if self.input_handler.is_empty() {
                            Ok(Some(ModalResult::Cancelled))
                        } else {
                            Ok(Some(ModalResult::Confirmed(
                                self.input_handler.text().to_string(),
                            )))
                        }
                    }
                    KeyCode::Char(c) => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            return Ok(None);
                        }
                        self.input_handler.insert_char(c);
                        Ok(None)
                    }
                    KeyCode::Backspace => {
                        self.input_handler.backspace();
                        Ok(None)
                    }
                    KeyCode::Delete => {
                        self.input_handler.delete();
                        Ok(None)
                    }
                    KeyCode::Left => {
                        self.input_handler.move_left();
                        Ok(None)
                    }
                    KeyCode::Right => {
                        self.input_handler.move_right();
                        Ok(None)
                    }
                    KeyCode::Home => {
                        self.input_handler.move_home();
                        Ok(None)
                    }
                    KeyCode::End => {
                        self.input_handler.move_end();
                        Ok(None)
                    }
                    _ => Ok(None),
                }
            }
            FocusArea::Buttons => {
                match key.code {
                    KeyCode::Left => {
                        // Move to previous button (wrap around)
                        self.selected_button = if self.selected_button == 0 { 1 } else { 0 };
                        Ok(None)
                    }
                    KeyCode::Right => {
                        // Move to next button (wrap around)
                        self.selected_button = if self.selected_button == 1 { 0 } else { 1 };
                        Ok(None)
                    }
                    KeyCode::Up => {
                        // Move focus back to input
                        self.focus = FocusArea::Input;
                        Ok(None)
                    }
                    KeyCode::Enter => {
                        // Execute selected button action
                        if self.selected_button == 0 {
                            // OK button
                            if self.input_handler.is_empty() {
                                Ok(Some(ModalResult::Cancelled))
                            } else {
                                Ok(Some(ModalResult::Confirmed(
                                    self.input_handler.text().to_string(),
                                )))
                            }
                        } else {
                            // Cancel button
                            Ok(Some(ModalResult::Cancelled))
                        }
                    }
                    KeyCode::Char(c) => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            return Ok(None);
                        }
                        // Switch back to input and insert character
                        self.focus = FocusArea::Input;
                        self.input_handler.insert_char(c);
                        Ok(None)
                    }
                    KeyCode::Backspace => {
                        // Switch back to input and delete character
                        self.focus = FocusArea::Input;
                        self.input_handler.backspace();
                        Ok(None)
                    }
                    KeyCode::Delete => {
                        // Switch back to input and delete character
                        self.focus = FocusArea::Input;
                        self.input_handler.delete();
                        Ok(None)
                    }
                    _ => Ok(None),
                }
            }
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
        // Buttons are centered: "[ OK ]    [ Cancel ]"
        let t = i18n::t();
        let ok_text = format!("[ {} ]", t.ui_ok());
        let cancel_text = format!("[ {} ]", t.ui_cancel());
        let total_text_width = ok_text.len() + MODAL_BUTTON_SPACING as usize + cancel_text.len();

        let start_col =
            buttons_area.x + (buttons_area.width.saturating_sub(total_text_width as u16)) / 2;
        let ok_end = start_col + ok_text.len() as u16;
        let cancel_start = ok_end + MODAL_BUTTON_SPACING;
        let cancel_end = cancel_start + cancel_text.len() as u16;

        // Determine which button was clicked
        if mouse.column >= start_col && mouse.column < ok_end {
            // OK button clicked
            self.focus = FocusArea::Buttons;
            self.selected_button = 0;
            // Execute OK action immediately
            if self.input_handler.is_empty() {
                Ok(Some(ModalResult::Cancelled))
            } else {
                Ok(Some(ModalResult::Confirmed(
                    self.input_handler.text().to_string(),
                )))
            }
        } else if mouse.column >= cancel_start && mouse.column < cancel_end {
            // Cancel button clicked
            self.focus = FocusArea::Buttons;
            self.selected_button = 1;
            // Execute Cancel action immediately
            Ok(Some(ModalResult::Cancelled))
        } else {
            Ok(None)
        }
    }
}
