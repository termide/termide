//! Rename pattern input modal dialog.

use std::time::SystemTime;

use anyhow::Result;
use crossterm::event::KeyCode;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::base::{
    button_style, drag_field, field_box, press_field_box, render_modal_block, render_text_input,
};
use crate::input_keys::{handle_input_key, InputKeyResult};

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
    /// The pattern field as last drawn, for the mouse.
    last_input_area: Option<Rect>,
    /// A press landed in the pattern field and the button is still down.
    dragging: bool,
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
            last_input_area: None,
            dragging: false,
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
                Style::default().fg(theme.fg),
            )),
            Line::from(Span::styled(
                "  $I-counter  $C-created  $M-modified",
                Style::default().fg(theme.fg),
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

        let inner = render_modal_block(modal_area, buf, &self.title, theme);

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

        // Empty line (was: Original name)
        // chunks[0] left empty

        // Input field
        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accented_fg))
            .title(" Pattern ");

        let input_area = input_block.inner(chunks[1]);
        input_block.render(chunks[1], buf);

        // Render input content with cursor and selection
        render_text_input(
            buf,
            input_area,
            &mut self.input_handler,
            self.focus == FocusArea::Input,
            theme,
        );
        self.last_input_area = Some(input_area);

        // Preview - show "New name: ..." in accented_fg color
        let preview = self.get_preview();
        let is_valid = self.is_valid();
        if !preview.is_empty() {
            let preview_line = if is_valid {
                Line::from(vec![
                    Span::styled("New name: ", Style::default().fg(theme.accented_fg)),
                    Span::styled(preview, Style::default().fg(theme.accented_fg)),
                ])
            } else {
                Line::from(vec![
                    Span::styled("New name: ", Style::default().fg(theme.error)),
                    Span::styled(preview, Style::default().fg(theme.error)),
                ])
            };
            let preview_para = Paragraph::new(preview_line);
            preview_para.render(chunks[2], buf);
        }

        // Help
        let help_text =
            Paragraph::new(self.get_help_lines(theme)).style(Style::default().fg(theme.fg));
        help_text.render(chunks[4], buf);

        // Buttons
        let t = i18n::t();

        let continue_style = button_style(
            self.focus == FocusArea::Buttons && self.selected_button == 0,
            theme,
        );
        let cancel_style = button_style(
            self.focus == FocusArea::Buttons && self.selected_button == 1,
            theme,
        );

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

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let key = chord.raw;
        // Escape always cancels
        if key.code == KeyCode::Esc {
            return Ok(Some(ModalResult::Cancelled));
        }

        match self.focus {
            FocusArea::Input => {
                // Try common input handling first
                match handle_input_key(&mut self.input_handler, key) {
                    InputKeyResult::Handled | InputKeyResult::TextModified => {
                        return Ok(None);
                    }
                    InputKeyResult::NotHandled => {}
                }

                // Modal-specific handling
                match key.code {
                    KeyCode::Down | KeyCode::Tab => {
                        // Move focus to buttons
                        self.focus = FocusArea::Buttons;
                        Ok(None)
                    }
                    KeyCode::Enter => {
                        if self.is_valid() {
                            Ok(Some(ModalResult::Confirmed(
                                self.input_handler.text().to_string(),
                            )))
                        } else {
                            Ok(None)
                        }
                    }
                    _ => Ok(None),
                }
            }
            FocusArea::Buttons => {
                // Handle text input keys even when on buttons
                match handle_input_key(&mut self.input_handler, key) {
                    InputKeyResult::Handled | InputKeyResult::TextModified => {
                        self.focus = FocusArea::Input;
                        return Ok(None);
                    }
                    InputKeyResult::NotHandled => {}
                }

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
                    KeyCode::Up | KeyCode::BackTab => {
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
        use crossterm::event::{MouseButton, MouseEventKind};

        match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                drag_field(&mut self.input_handler, self.last_input_area, &mouse);
                return Ok(None);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.dragging = false;
                return Ok(None);
            }
            MouseEventKind::Down(MouseButton::Left) => {}
            _ => return Ok(None),
        }

        // A press on the pattern field's box, its border included, places
        // the cursor.
        let frame = field_box(self.last_input_area, true);
        if press_field_box(&mut self.input_handler, self.last_input_area, frame, &mouse) {
            self.focus = FocusArea::Input;
            self.dragging = true;
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

    /// A paste goes into the pattern, as typing does: focus returns to the
    /// field and the preview follows on the next draw.
    fn handle_paste(&mut self, text: &str) -> bool {
        self.input_handler.paste(text);
        self.focus = FocusArea::Input;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bracketed_paste_lands_in_the_pattern() {
        let mut modal = RenamePatternModal::new("Rename", "a.txt", "x", None, None);
        modal.focus = FocusArea::Buttons;
        assert!(modal.handle_paste("-$0\n"));
        assert_eq!(modal.input_handler.text(), "x-$0");
        assert_eq!(modal.focus, FocusArea::Input);
    }

    #[test]
    fn a_click_places_the_cursor_and_a_drag_selects_in_the_pattern() {
        use crate::base::test_mouse::{drag, press, release};
        let mut modal = RenamePatternModal::new("Rename", "a.txt", "prefix-$0", None, None);
        let area = Rect::new(0, 0, 100, 30);
        modal.render(area, &mut Buffer::empty(area), &Theme::default());
        modal.focus = FocusArea::Buttons;
        let field = modal.last_input_area.unwrap();
        modal
            .handle_mouse(press(field.x + 2, field.y), area)
            .unwrap();
        assert_eq!(modal.focus, FocusArea::Input);
        assert_eq!(modal.input_handler.cursor_pos(), 2);
        modal
            .handle_mouse(drag(field.x + 6, field.y + 3), area)
            .unwrap();
        assert_eq!(modal.input_handler.selected_text(), Some("efix"));
        // Past the right side the selection reaches the end of the text.
        modal
            .handle_mouse(drag(field.right() + 5, field.y), area)
            .unwrap();
        assert_eq!(modal.input_handler.selected_text(), Some("efix-$0"));
        modal
            .handle_mouse(release(field.right() + 5, field.y), area)
            .unwrap();
        modal.handle_mouse(drag(field.x, field.y), area).unwrap();
        assert_eq!(
            modal.input_handler.selected_text(),
            Some("efix-$0"),
            "a drag after the release no longer selects"
        );
    }

    /// A double click on the pattern takes the word under it, a triple click
    /// the whole pattern.
    #[test]
    fn a_double_click_selects_a_word_and_a_triple_click_the_pattern() {
        use crate::base::test_mouse::{press, release};
        let mut modal = RenamePatternModal::new("Rename", "a.txt", "new_name-$0 копия", None, None);
        let area = Rect::new(0, 0, 100, 30);
        modal.render(area, &mut Buffer::empty(area), &Theme::default());
        let field = modal.last_input_area.unwrap();
        let click = |modal: &mut RenamePatternModal, column| {
            modal.handle_mouse(press(column, field.y), area).unwrap();
            modal.handle_mouse(release(column, field.y), area).unwrap();
        };
        click(&mut modal, field.x + 2);
        click(&mut modal, field.x + 2);
        assert_eq!(modal.input_handler.selected_text(), Some("new_name"));
        click(&mut modal, field.x + 2);
        assert_eq!(
            modal.input_handler.selected_text(),
            Some("new_name-$0 копия")
        );
        click(&mut modal, field.x + 14);
        click(&mut modal, field.x + 14);
        assert_eq!(modal.input_handler.selected_text(), Some("копия"));
    }
}
