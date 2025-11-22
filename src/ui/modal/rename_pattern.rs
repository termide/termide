use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};
use std::time::SystemTime;

use super::{Modal, ModalResult};
use crate::rename_pattern::RenamePattern;
use crate::theme::Theme;

/// Rename pattern input modal window
#[derive(Debug)]
pub struct RenamePatternModal {
    title: String,
    original_name: String,
    input: String,
    cursor_position: usize,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
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
            input: default.to_string(),
            cursor_position: default.chars().count(),
            created,
            modified,
        }
    }

    /// Get result preview
    fn get_preview(&self) -> String {
        if self.input.is_empty() {
            return String::new();
        }

        let pattern = RenamePattern::new(self.input.clone());
        pattern.apply(&self.original_name, 1, self.created, self.modified)
    }

    /// Check result validity
    fn is_valid(&self) -> bool {
        if self.input.is_empty() {
            return false;
        }

        let pattern = RenamePattern::new(self.input.clone());
        let result = pattern.preview(&self.original_name);
        pattern.is_valid_result(&result)
    }

    /// Get byte index for cursor position (character-based)
    fn byte_index(&self) -> usize {
        self.input
            .char_indices()
            .nth(self.cursor_position)
            .map(|(idx, _)| idx)
            .unwrap_or(self.input.len())
    }

    /// Insert character at cursor position
    fn insert_char(&mut self, c: char) {
        let byte_idx = self.byte_index();
        self.input.insert(byte_idx, c);
        self.cursor_position += 1;
    }

    /// Delete character before cursor
    fn delete_char(&mut self) {
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
            let byte_idx = self.byte_index();
            self.input.remove(byte_idx);
        }
    }

    /// Delete character under cursor
    fn delete_char_forward(&mut self) {
        let char_count = self.input.chars().count();
        if self.cursor_position < char_count {
            let byte_idx = self.byte_index();
            self.input.remove(byte_idx);
        }
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

    /// Create a centered rectangle
    fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
        let horizontal_margin = r.width.saturating_sub(width) / 2;
        let vertical_margin = r.height.saturating_sub(height) / 2;

        let popup_layout = Layout::default()
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
            .split(popup_layout[1])[1]
    }
}

impl Modal for RenamePatternModal {
    type Result = String;

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Dynamic height (including outer block borders):
        // 1 (original name) + 3 (input field) + 1 (preview)
        // + 1 (empty) + 3 (help) + 1 (empty) + 1 (buttons) + 1 (empty)
        // = 12 lines inside + 2 borders = 14 lines
        let modal_height = 14;
        let modal_width = 70;

        let modal_area = Self::centered_rect(modal_width, modal_height, area);
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

        let input_text = Paragraph::new(self.input.as_str()).style(Style::default().fg(theme.bg));
        input_text.render(input_area, buf);

        // Cursor (cursor_position is character index, not byte index)
        let char_count = self.input.chars().count();
        if self.cursor_position <= char_count {
            let cursor_x = input_area.x + self.cursor_position as u16;
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
        let buttons = Paragraph::new("Tab/Enter - Continue | Esc - Cancel")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.disabled));
        buttons.render(chunks[6], buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>> {
        match key.code {
            KeyCode::Enter | KeyCode::Tab => {
                if self.is_valid() {
                    Ok(Some(ModalResult::Confirmed(self.input.clone())))
                } else {
                    Ok(None)
                }
            }
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
            KeyCode::Left => {
                if self.cursor_position > 0 {
                    self.cursor_position -= 1;
                }
                Ok(None)
            }
            KeyCode::Right => {
                let char_count = self.input.chars().count();
                if self.cursor_position < char_count {
                    self.cursor_position += 1;
                }
                Ok(None)
            }
            KeyCode::Home => {
                self.cursor_position = 0;
                Ok(None)
            }
            KeyCode::End => {
                self.cursor_position = self.input.chars().count();
                Ok(None)
            }
            KeyCode::Backspace => {
                self.delete_char();
                Ok(None)
            }
            KeyCode::Delete => {
                self.delete_char_forward();
                Ok(None)
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor_position = 0;
                Ok(None)
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor_position = self.input.chars().count();
                Ok(None)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.clear();
                self.cursor_position = 0;
                Ok(None)
            }
            KeyCode::Char(c) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.insert_char(c);
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}
