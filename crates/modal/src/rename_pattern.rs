//! Rename pattern input modal dialog.

use std::time::SystemTime;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};

use termide_i18n as i18n;
use termide_state::RenamePattern;
use termide_theme::Theme;

use crate::{centered_rect_with_size, Modal, ModalResult, TextInputHandler};

/// Focus area in the modal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Input,
    Buttons,
}

/// Rename pattern input modal window
#[derive(Debug)]
pub struct RenamePatternModal {
    title: String,
    original_name: String,
    input_handler: TextInputHandler,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
    focus: FocusArea,
    selected_button: usize, // 0 = Continue, 1 = Cancel
    last_buttons_area: Option<Rect>,
}

impl RenamePatternModal {
    /// Create a new rename pattern input modal window
    pub fn new(
        title: &str,
        original_name: &str,
        default: &str,
        created: Option<SystemTime>,
        modified: Option<SystemTime>,
    ) -> Self {
        Self {
            title: title.to_string(),
            original_name: original_name.to_string(),
            input_handler: TextInputHandler::with_default(default),
            created,
            modified,
            focus: FocusArea::Input,
            selected_button: 0, // Continue button selected by default
            last_buttons_area: None,
        }
    }

    /// Get result preview
    fn get_preview(&self) -> String {
        if self.input_handler.is_empty() {
            return String::new();
        }

        let pattern = RenamePattern::new(self.input_handler.text().to_string());
        pattern.apply(&self.original_name, 1, self.created, self.modified)
    }

    /// Check result validity
    fn is_valid(&self) -> bool {
        if self.input_handler.is_empty() {
            return false;
        }

        let pattern = RenamePattern::new(self.input_handler.text().to_string());
        let result = pattern.preview(&self.original_name);
        pattern.is_valid_result(&result)
    }

    fn get_help_lines(&self, theme: &Theme) -> Vec<Line<'static>> {
        vec![
            Line::from(Span::styled(
                "Variables:",
                Style::default()
                    .fg(theme.accented_fg)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "  $0-full name  $1-9-parts  $-1-9-from end",
                Style::default().fg(theme.bg),
            )),
            Line::from(Span::styled(
                "  $I-counter  $C-created  $M-modified",
                Style::default().fg(theme.bg),
            )),
        ]
    }
}

impl Modal for RenamePatternModal {
    type Result = String;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Dynamic height (including outer block borders):
        // 1 (original name) + 3 (input field) + 1 (preview)
        // + 1 (empty) + 3 (help) + 1 (empty) + 1 (buttons) + 1 (empty)
        // = 12 lines inside + 2 borders = 14 lines
        let modal_height = 14;
        let modal_width = 70;

        let modal_area = centered_rect_with_size(modal_width, modal_height, area);
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

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Original name
                Constraint::Length(3), // Input field
                Constraint::Length(1), // Preview
                Constraint::Length(1), // Empty line
                Constraint::Length(3), // Help
                Constraint::Length(1), // Empty line
                Constraint::Length(1), // Buttons
                Constraint::Length(1), // Empty line at bottom
            ])
            .split(inner);

        // Original name
        let original = Paragraph::new(format!("Original: {}", self.original_name))
            .style(Style::default().fg(theme.disabled));
        original.render(chunks[0], buf);

        // Input field
        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accented_fg))
            .title(" Pattern ");

        let input_area = input_block.inner(chunks[1]);
        input_block.render(chunks[1], buf);

        let input_text =
            Paragraph::new(self.input_handler.text()).style(Style::default().fg(theme.bg));
        input_text.render(input_area, buf);

        // Cursor
        let cursor_pos = self.input_handler.cursor_pos();
        let char_count = self.input_handler.text().chars().count();
        if cursor_pos <= char_count {
            let cursor_x = input_area.x + cursor_pos as u16;
            if cursor_x < input_area.right() {
                buf[(cursor_x, input_area.y)].set_style(Style::default().bg(theme.fg).fg(theme.bg));
            }
        }

        // Preview
        let preview = self.get_preview();
        let is_valid = self.is_valid();
        let preview_color = if is_valid { theme.success } else { theme.error };
        let preview_text = if preview.is_empty() {
            "".to_string()
        } else if is_valid {
            format!("→ {}", preview)
        } else {
            format!("✗ {}", preview)
        };

        let preview_para = Paragraph::new(preview_text).style(Style::default().fg(preview_color));
        preview_para.render(chunks[2], buf);

        // Help
        let help_text =
            Paragraph::new(self.get_help_lines(theme)).style(Style::default().fg(theme.bg));
        help_text.render(chunks[4], buf);

        // Buttons
        let t = i18n::t();

        let continue_style = if self.focus == FocusArea::Buttons && self.selected_button == 0 {
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
            Span::styled(format!("[ {} ]", t.ui_continue()), continue_style),
            Span::raw("    "),
            Span::styled(format!("[ {} ]", t.ui_cancel()), cancel_style),
        ]);

        let buttons_paragraph = Paragraph::new(buttons).alignment(Alignment::Center);
        buttons_paragraph.render(chunks[6], buf);

        // Save buttons area for mouse handling
        self.last_buttons_area = Some(chunks[6]);
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
                    KeyCode::Enter | KeyCode::Tab => {
                        if self.is_valid() {
                            Ok(Some(ModalResult::Confirmed(
                                self.input_handler.text().to_string(),
                            )))
                        } else {
                            Ok(None)
                        }
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
                    KeyCode::Backspace => {
                        self.input_handler.backspace();
                        Ok(None)
                    }
                    KeyCode::Delete => {
                        self.input_handler.delete();
                        Ok(None)
                    }
                    KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.input_handler.move_home();
                        Ok(None)
                    }
                    KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.input_handler.move_end();
                        Ok(None)
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.input_handler.clear();
                        Ok(None)
                    }
                    KeyCode::Char(c) => {
                        if !key.modifiers.contains(KeyModifiers::CONTROL) {
                            self.input_handler.insert_char(c);
                        }
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
                            // Continue button
                            if self.is_valid() {
                                Ok(Some(ModalResult::Confirmed(
                                    self.input_handler.text().to_string(),
                                )))
                            } else {
                                Ok(None)
                            }
                        } else {
                            // Cancel button
                            Ok(Some(ModalResult::Cancelled))
                        }
                    }
                    KeyCode::Char(c) => {
                        if !key.modifiers.contains(KeyModifiers::CONTROL) {
                            // Switch back to input and insert character
                            self.focus = FocusArea::Input;
                            self.input_handler.insert_char(c);
                        }
                        Ok(None)
                    }
                    KeyCode::Backspace => {
                        // Switch back to input and delete character
                        self.focus = FocusArea::Input;
                        self.input_handler.backspace();
                        Ok(None)
                    }
                    KeyCode::Delete => {
                        // Switch back to input and delete character forward
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
        let t = i18n::t();
        let continue_text = format!("[ {} ]", t.ui_continue());
        let cancel_text = format!("[ {} ]", t.ui_cancel());
        let total_text_width = continue_text.len() + 4 + cancel_text.len();

        let start_col =
            buttons_area.x + (buttons_area.width.saturating_sub(total_text_width as u16)) / 2;
        let continue_end = start_col + continue_text.len() as u16;
        let cancel_start = continue_end + 4;
        let cancel_end = cancel_start + cancel_text.len() as u16;

        // Determine which button was clicked
        if mouse.column >= start_col && mouse.column < continue_end {
            // Continue button clicked
            self.focus = FocusArea::Buttons;
            self.selected_button = 0;
            // Execute Continue action
            if self.is_valid() {
                Ok(Some(ModalResult::Confirmed(
                    self.input_handler.text().to_string(),
                )))
            } else {
                Ok(None)
            }
        } else if mouse.column >= cancel_start && mouse.column < cancel_end {
            // Cancel button clicked
            self.focus = FocusArea::Buttons;
            self.selected_button = 1;
            // Execute Cancel action
            Ok(Some(ModalResult::Cancelled))
        } else {
            Ok(None)
        }
    }
}
