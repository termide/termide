//! Save As modal dialog with executable checkbox.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::base::{button_style, render_input_field, render_modal_block};
use crate::input_keys::{handle_input_key, InputKeyResult};

use termide_config::constants::MODAL_BUTTON_SPACING;
use termide_i18n as i18n;
use termide_theme::Theme;

use crate::{
    calculate_modal_width, centered_rect_with_size, max_line_width, Modal, ModalResult,
    ModalWidthConfig, TextInputHandler,
};

/// Result of Save As modal
#[derive(Debug, Clone)]
pub struct SaveAsResult {
    /// File path to save to
    pub path: String,
    /// Whether to set executable flag
    pub executable: bool,
}

/// Focus area in the modal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Input,
    Checkbox,
    Buttons,
}

/// Save As modal window with executable checkbox
#[derive(Debug)]
pub struct SaveAsModal {
    title: String,
    input_handler: TextInputHandler,
    executable: bool,
    focus: FocusArea,
    selected_button: usize, // 0 = OK, 1 = Cancel
    last_buttons_area: Option<Rect>,
    last_checkbox_area: Option<Rect>,
}

impl SaveAsModal {
    /// Create a new Save As modal window with default value
    pub fn new(title: impl Into<String>, default: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            input_handler: TextInputHandler::with_default(default),
            executable: false,
            focus: FocusArea::Input,
            selected_button: 0, // OK button selected by default
            last_buttons_area: None,
            last_checkbox_area: None,
        }
    }

    /// Calculate dynamic modal width and height
    fn calculate_modal_size(&self, screen_width: u16, screen_height: u16) -> (u16, u16) {
        let t = i18n::t();
        let title_width = self.title.len() as u16 + 2;
        let buttons_width = 21u16; // "[ OK ]    [ Cancel ]"
        let input_width = self.input_handler.text().chars().count() as u16 + 20;
        let checkbox_width = max_line_width(&format!("[✓] {}", t.checkbox_executable()));

        let width = calculate_modal_width(
            [title_width, buttons_width, input_width, checkbox_width].into_iter(),
            screen_width,
            ModalWidthConfig {
                wide: false,
                double_border: true,
            },
        );

        // Calculate height: border + input(3) + checkbox(1) + buttons(1) + border
        let height = (1 + 3 + 1 + 1 + 1).min(screen_height);

        (width, height)
    }

    /// Move focus to next element
    fn focus_next(&mut self) {
        self.focus = match self.focus {
            FocusArea::Input => FocusArea::Checkbox,
            FocusArea::Checkbox => FocusArea::Buttons,
            FocusArea::Buttons => FocusArea::Input,
        };
    }

    /// Move focus to previous element
    fn focus_prev(&mut self) {
        self.focus = match self.focus {
            FocusArea::Input => FocusArea::Buttons,
            FocusArea::Checkbox => FocusArea::Input,
            FocusArea::Buttons => FocusArea::Checkbox,
        };
    }

    /// Confirm and return result
    fn confirm(&self) -> Option<ModalResult<SaveAsResult>> {
        if self.input_handler.is_empty() {
            Some(ModalResult::Cancelled)
        } else {
            Some(ModalResult::Confirmed(SaveAsResult {
                path: self.input_handler.text().to_string(),
                executable: self.executable,
            }))
        }
    }
}

impl Modal for SaveAsModal {
    type Result = SaveAsResult;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let t = i18n::t();

        // Calculate dynamic dimensions
        let (modal_width, modal_height) = self.calculate_modal_size(area.width, area.height);

        // Create centered area
        let modal_area = centered_rect_with_size(modal_width, modal_height, area);

        let inner = render_modal_block(modal_area, buf, &self.title, theme);

        // Split into: input(3), checkbox(1), buttons(1)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Input
                Constraint::Length(1), // Checkbox
                Constraint::Length(1), // Buttons
            ])
            .split(inner);

        // Render input field with border
        let input_border_color = if self.focus == FocusArea::Input {
            theme.accented_fg
        } else {
            theme.disabled
        };

        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(input_border_color));
        let input_inner = input_block.inner(chunks[0]);
        input_block.render(chunks[0], buf);

        // Render input content with cursor and selection
        render_input_field(
            buf,
            input_inner.x,
            input_inner.y,
            input_inner.width,
            self.input_handler.text(),
            self.input_handler.cursor_pos(),
            self.input_handler.selection_range(),
            self.focus == FocusArea::Input,
            theme,
        );

        // Render checkbox
        let checkbox_char = termide_ui::checkbox_mark(self.executable);
        let checkbox_style = if self.focus == FocusArea::Checkbox {
            Style::default().fg(theme.accented_fg).bg(theme.bg)
        } else {
            Style::default().fg(theme.fg).bg(theme.bg)
        };

        let checkbox_text = format!("[{}] {}", checkbox_char, t.checkbox_executable());
        let checkbox = Paragraph::new(checkbox_text)
            .style(checkbox_style)
            .alignment(Alignment::Left);
        checkbox.render(chunks[1], buf);
        self.last_checkbox_area = Some(chunks[1]);

        // Render buttons
        let ok_style = button_style(
            self.focus == FocusArea::Buttons && self.selected_button == 0,
            theme,
        );
        let cancel_style = button_style(
            self.focus == FocusArea::Buttons && self.selected_button == 1,
            theme,
        );

        let buttons = Line::from(vec![
            Span::styled(format!("[ {} ]", t.ui_ok()), ok_style),
            Span::raw("    "),
            Span::styled(format!("[ {} ]", t.ui_cancel()), cancel_style),
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
        // Escape always cancels
        if key.code == KeyCode::Esc {
            return Ok(Some(ModalResult::Cancelled));
        }

        // Tab navigation (works from any focus)
        if key.code == KeyCode::Tab {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                self.focus_prev();
            } else {
                self.focus_next();
            }
            return Ok(None);
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
                    KeyCode::Down => {
                        self.focus = FocusArea::Checkbox;
                        Ok(None)
                    }
                    KeyCode::Enter => Ok(self.confirm()),
                    _ => Ok(None),
                }
            }
            FocusArea::Checkbox => {
                // Space toggles checkbox — must come before handle_input_key which would eat it
                if key.code == KeyCode::Char(' ') {
                    self.executable = !self.executable;
                    return Ok(None);
                }

                // Handle text input keys for quick typing
                match handle_input_key(&mut self.input_handler, key) {
                    InputKeyResult::Handled | InputKeyResult::TextModified => {
                        self.focus = FocusArea::Input;
                        return Ok(None);
                    }
                    InputKeyResult::NotHandled => {}
                }

                match key.code {
                    KeyCode::Up | KeyCode::BackTab => {
                        self.focus = FocusArea::Input;
                        Ok(None)
                    }
                    KeyCode::Down => {
                        self.focus = FocusArea::Buttons;
                        Ok(None)
                    }
                    KeyCode::Enter => Ok(self.confirm()),
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
                        self.selected_button = if self.selected_button == 0 { 1 } else { 0 };
                        Ok(None)
                    }
                    KeyCode::Right => {
                        self.selected_button = if self.selected_button == 1 { 0 } else { 1 };
                        Ok(None)
                    }
                    KeyCode::Up | KeyCode::BackTab => {
                        self.focus = FocusArea::Checkbox;
                        Ok(None)
                    }
                    KeyCode::Enter => {
                        if self.selected_button == 0 {
                            Ok(self.confirm())
                        } else {
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
        use crossterm::event::MouseEventKind;

        // Only handle left button press
        if mouse.kind != MouseEventKind::Down(crossterm::event::MouseButton::Left) {
            return Ok(None);
        }

        let t = i18n::t();

        // Check checkbox click
        if let Some(checkbox_area) = self.last_checkbox_area {
            if mouse.row >= checkbox_area.y
                && mouse.row < checkbox_area.y + checkbox_area.height
                && mouse.column >= checkbox_area.x
                && mouse.column < checkbox_area.x + checkbox_area.width
            {
                self.focus = FocusArea::Checkbox;
                self.executable = !self.executable;
                return Ok(None);
            }
        }

        // Check buttons click
        let Some(buttons_area) = self.last_buttons_area else {
            return Ok(None);
        };

        if mouse.row < buttons_area.y
            || mouse.row >= buttons_area.y + buttons_area.height
            || mouse.column < buttons_area.x
            || mouse.column >= buttons_area.x + buttons_area.width
        {
            return Ok(None);
        }

        // Calculate button positions
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
            Ok(self.confirm())
        } else if mouse.column >= cancel_start && mouse.column < cancel_end {
            // Cancel button clicked
            self.focus = FocusArea::Buttons;
            self.selected_button = 1;
            Ok(Some(ModalResult::Cancelled))
        } else {
            Ok(None)
        }
    }
}
