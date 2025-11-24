use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
    input: String,
    cursor_pos: usize,
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
            input: String::new(),
            cursor_pos: 0,
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
        let default = default.into();
        let cursor_pos = default.chars().count(); // Use character count, not bytes
        Self {
            title: title.into(),
            prompt: prompt.into(),
            input: default,
            cursor_pos,
            focus: FocusArea::Input,
            selected_button: 0, // OK button selected by default
            last_buttons_area: None,
        }
    }

    /// Calculate dynamic modal width and height
    fn calculate_modal_size(&self, screen_width: u16, screen_height: u16) -> (u16, u16) {
        // 1. Title width (with spaces on the edges)
        let title_width = self.title.len() as u16 + 2;

        // 2. Maximum prompt line width
        let prompt_max_line_width = if self.prompt.is_empty() {
            0
        } else {
            self.prompt
                .lines()
                .map(|line| line.len())
                .max()
                .unwrap_or(0) as u16
        };

        // 3. Buttons width: "[ OK ]    [ Cancel ]" = ~21 characters
        let buttons_width = 21;

        // 4. Input field width with room for at least 20 characters
        // Current input length + minimum 20 characters for input
        let current_input_len = self.input.chars().count() as u16;
        let min_input_width = current_input_len + 20;

        // Take the maximum of all components
        let content_width = title_width
            .max(prompt_max_line_width)
            .max(buttons_width)
            .max(min_input_width);

        // Add padding and borders:
        // - 2 for outer block border
        // - 2 for inner input field border
        // - 4 for padding
        let total_width = content_width + 8;

        // Apply width constraints
        let max_width = (screen_width as f32 * 0.75) as u16;
        let width = total_width.max(20).min(max_width).min(screen_width);

        // Calculate height:
        // 1 (top border) + prompt_lines + 3 (input with border) + 1 (buttons) + 1 (bottom border)
        let prompt_lines = if self.prompt.is_empty() {
            0
        } else {
            self.prompt.lines().count().max(1) as u16
        };
        let height = (1 + prompt_lines + 3 + 1 + 1).min(screen_height);

        (width, height)
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

impl Modal for InputModal {
    type Result = String;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Calculate dynamic dimensions
        let (modal_width, modal_height) = self.calculate_modal_size(area.width, area.height);

        // Create centered area
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

        // Split into prompt (if not empty), input, and buttons
        use ratatui::layout::{Constraint, Direction, Layout};

        let prompt_lines = if self.prompt.is_empty() {
            0
        } else {
            self.prompt.lines().count().max(1) as u16
        };

        let mut constraints = Vec::new();
        if prompt_lines > 0 {
            constraints.push(Constraint::Length(prompt_lines)); // Prompt (if exists)
        }
        constraints.push(Constraint::Length(3)); // Input
        constraints.push(Constraint::Length(1)); // Buttons

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
        // Convert cursor position (in characters) to byte index
        let byte_pos = self
            .input
            .chars()
            .take(self.cursor_pos)
            .map(|c| c.len_utf8())
            .sum::<usize>();

        let input_line = Line::from(vec![
            Span::styled(&self.input[..byte_pos], Style::default().fg(theme.bg)),
            Span::styled("█", Style::default().fg(theme.success)),
            Span::styled(&self.input[byte_pos..], Style::default().fg(theme.bg)),
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
                        if self.input.is_empty() {
                            Ok(Some(ModalResult::Cancelled))
                        } else {
                            Ok(Some(ModalResult::Confirmed(self.input.clone())))
                        }
                    }
                    KeyCode::Char(c) => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            return Ok(None);
                        }
                        // Convert character position to byte index
                        let byte_pos = self
                            .input
                            .chars()
                            .take(self.cursor_pos)
                            .map(|c| c.len_utf8())
                            .sum::<usize>();
                        self.input.insert(byte_pos, c);
                        self.cursor_pos += 1;
                        Ok(None)
                    }
                    KeyCode::Backspace => {
                        if self.cursor_pos > 0 {
                            // Convert character position to byte index
                            let byte_pos = self
                                .input
                                .chars()
                                .take(self.cursor_pos - 1)
                                .map(|c| c.len_utf8())
                                .sum::<usize>();
                            self.input.remove(byte_pos);
                            self.cursor_pos -= 1;
                        }
                        Ok(None)
                    }
                    KeyCode::Delete => {
                        let char_count = self.input.chars().count();
                        if self.cursor_pos < char_count {
                            // Convert character position to byte index
                            let byte_pos = self
                                .input
                                .chars()
                                .take(self.cursor_pos)
                                .map(|c| c.len_utf8())
                                .sum::<usize>();
                            self.input.remove(byte_pos);
                        }
                        Ok(None)
                    }
                    KeyCode::Left => {
                        if self.cursor_pos > 0 {
                            self.cursor_pos -= 1;
                        }
                        Ok(None)
                    }
                    KeyCode::Right => {
                        let char_count = self.input.chars().count();
                        if self.cursor_pos < char_count {
                            self.cursor_pos += 1;
                        }
                        Ok(None)
                    }
                    KeyCode::Home => {
                        self.cursor_pos = 0;
                        Ok(None)
                    }
                    KeyCode::End => {
                        self.cursor_pos = self.input.chars().count();
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
                            if self.input.is_empty() {
                                Ok(Some(ModalResult::Cancelled))
                            } else {
                                Ok(Some(ModalResult::Confirmed(self.input.clone())))
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
                        let byte_pos = self
                            .input
                            .chars()
                            .take(self.cursor_pos)
                            .map(|c| c.len_utf8())
                            .sum::<usize>();
                        self.input.insert(byte_pos, c);
                        self.cursor_pos += 1;
                        Ok(None)
                    }
                    KeyCode::Backspace => {
                        // Switch back to input and delete character
                        self.focus = FocusArea::Input;
                        if self.cursor_pos > 0 {
                            let byte_pos = self
                                .input
                                .chars()
                                .take(self.cursor_pos - 1)
                                .map(|c| c.len_utf8())
                                .sum::<usize>();
                            self.input.remove(byte_pos);
                            self.cursor_pos -= 1;
                        }
                        Ok(None)
                    }
                    KeyCode::Delete => {
                        // Switch back to input and delete character
                        self.focus = FocusArea::Input;
                        let char_count = self.input.chars().count();
                        if self.cursor_pos < char_count {
                            let byte_pos = self
                                .input
                                .chars()
                                .take(self.cursor_pos)
                                .map(|c| c.len_utf8())
                                .sum::<usize>();
                            self.input.remove(byte_pos);
                        }
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
        let total_text_width = ok_text.len() + 4 + cancel_text.len(); // +4 for spacing

        let start_col =
            buttons_area.x + (buttons_area.width.saturating_sub(total_text_width as u16)) / 2;
        let ok_end = start_col + ok_text.len() as u16;
        let cancel_start = ok_end + 4; // 4 spaces between buttons
        let cancel_end = cancel_start + cancel_text.len() as u16;

        // Determine which button was clicked
        if mouse.column >= start_col && mouse.column < ok_end {
            // OK button clicked
            self.focus = FocusArea::Buttons;
            self.selected_button = 0;
            // Execute OK action immediately
            if self.input.is_empty() {
                Ok(Some(ModalResult::Cancelled))
            } else {
                Ok(Some(ModalResult::Confirmed(self.input.clone())))
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
