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

/// Text input modal window
#[derive(Debug)]
pub struct InputModal {
    title: String,
    prompt: String,
    input: String,
    cursor_pos: usize,
}

impl InputModal {
    /// Create a new input modal window
    pub fn new(title: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            prompt: prompt.into(),
            input: String::new(),
            cursor_pos: 0,
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
        }
    }

    /// Calculate dynamic modal width and height
    fn calculate_modal_size(&self, screen_width: u16, screen_height: u16) -> (u16, u16) {
        // 1. Title width (with spaces on the edges)
        let title_width = self.title.len() as u16 + 2;

        // 2. Maximum prompt line width
        let prompt_max_line_width = self
            .prompt
            .lines()
            .map(|line| line.len())
            .max()
            .unwrap_or(0) as u16;

        // 3. Hint width: "Enter - confirm | Esc - cancel" = 38 characters
        let hint_width = 38;

        // 4. Input field width with room for at least 20 characters
        // Current input length + minimum 20 characters for input
        let current_input_len = self.input.chars().count() as u16;
        let min_input_width = current_input_len + 20;

        // Take the maximum of all components
        let content_width = title_width
            .max(prompt_max_line_width)
            .max(hint_width)
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
        // 1 (top border) + 2 (prompt) + 3 (input with border) + 1 (hint) + 1 (bottom border) = 8
        let prompt_lines = self.prompt.lines().count().max(1) as u16;
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

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
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

        // Split into prompt, input, and hint
        use ratatui::layout::{Constraint, Direction, Layout};
        let prompt_lines = self.prompt.lines().count().max(1) as u16;
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(prompt_lines), // Prompt (dynamic)
                Constraint::Length(3),            // Input
                Constraint::Length(1),            // Hint
            ])
            .split(inner);

        // Render prompt
        let prompt = Paragraph::new(self.prompt.clone())
            .alignment(Alignment::Left)
            .style(Style::default().fg(theme.bg));
        prompt.render(chunks[0], buf);

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
        input_paragraph.render(chunks[1], buf);

        // Render hint
        let t = i18n::t();
        let hint_text = format!(
            "{}{}{}",
            t.ui_enter_confirm(),
            t.ui_hint_separator(),
            t.ui_esc_cancel()
        );
        let hint = Paragraph::new(hint_text)
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.disabled));
        hint.render(chunks[2], buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>> {
        match key.code {
            KeyCode::Enter => {
                if self.input.is_empty() {
                    Ok(Some(ModalResult::Cancelled))
                } else {
                    Ok(Some(ModalResult::Confirmed(self.input.clone())))
                }
            }
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
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
}
