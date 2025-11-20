use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Widget},
};

use super::{Modal, ModalResult};
use crate::theme::Theme;

/// Selection modal window (single selection only)
#[derive(Debug)]
pub struct SelectModal {
    title: String,
    prompt: String,
    items: Vec<String>,
    cursor: usize,
}

impl SelectModal {
    /// Create a single selection window from strings
    pub fn single(
        title: impl Into<String>,
        prompt: impl Into<String>,
        labels: Vec<String>,
    ) -> Self {
        Self {
            title: title.into(),
            prompt: prompt.into(),
            items: labels,
            cursor: 0,
        }
    }

    /// Calculate dynamic modal width
    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        // 1. Title width (with spaces on the edges)
        let title_width = self.title.len() as u16 + 2;

        // 2. Maximum prompt line width
        let prompt_max_line_width = self.prompt.lines()
            .map(|line| line.len())
            .max()
            .unwrap_or(0) as u16;

        // 3. Maximum list item width
        let max_item_width = self.items.iter()
            .map(|item| {
                // "▶ " + item = prefix 2 + item
                2 + item.len()
            })
            .max()
            .unwrap_or(0) as u16;

        // 4. Hint width
        let hint_width = "↑↓ - select | Enter - confirm | Esc - cancel".len() as u16;

        // Take the maximum of all components
        let content_width = title_width
            .max(prompt_max_line_width)
            .max(max_item_width)
            .max(hint_width);

        // Add padding and borders:
        // - 2 for outer block border
        // - 2 for list border
        // - 4 for padding
        let total_width = content_width + 8;

        // Apply constraints
        let max_width = (screen_width as f32 * 0.75) as u16;
        total_width.max(20).min(max_width).min(screen_width)
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

impl Modal for SelectModal {
    type Result = Vec<usize>;

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Calculate dynamic width
        let modal_width = self.calculate_modal_width(area.width);

        // Calculate height:
        // 1 (top border) + 2 (prompt) + N (list with border) + 2 (hint) + 1 (bottom border)
        let list_height = self.items.len().min(10) as u16; // Limit to 10 items
        let modal_height = 1 + 2 + (list_height + 2) + 2 + 1;

        // Create centered area
        let modal_area = Self::centered_rect_with_size(modal_width, modal_height, area);
        Clear.render(modal_area, buf);

        // Create block with inverted colors
        let block = Block::default()
            .title(Span::styled(
                format!(" {} ", self.title),
                Style::default()
                    .fg(theme.background)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.background))
            .style(Style::default().bg(theme.text_primary));

        let inner = block.inner(modal_area);
        block.render(modal_area, buf);

        use ratatui::layout::{Constraint, Direction, Layout};
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),  // Prompt
                Constraint::Min(5),     // List
                Constraint::Length(2),  // Hint
            ])
            .split(inner);

        let prompt = Paragraph::new(self.prompt.clone())
            .alignment(Alignment::Left)
            .style(Style::default().fg(theme.background));
        prompt.render(chunks[0], buf);

        let items: Vec<ListItem> = self
            .items
            .iter()
            .enumerate()
            .map(|(idx, label)| {
                let prefix = if idx == self.cursor { "▶ " } else { "  " };

                let style = if idx == self.cursor {
                    Style::default()
                        .fg(theme.text_primary)
                        .bg(theme.accent_primary)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.background)
                };

                ListItem::new(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(label, style),
                ]))
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.text_secondary)),
            )
            .style(Style::default().bg(theme.text_primary));

        list.render(chunks[1], buf);

        let hint = Paragraph::new("↑↓ - select | Enter - confirm | Esc - cancel")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.text_secondary));
        hint.render(chunks[2], buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>> {
        match key.code {
            KeyCode::Up => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                Ok(None)
            }
            KeyCode::Down => {
                if self.cursor < self.items.len().saturating_sub(1) {
                    self.cursor += 1;
                }
                Ok(None)
            }
            KeyCode::Home => {
                self.cursor = 0;
                Ok(None)
            }
            KeyCode::End => {
                self.cursor = self.items.len().saturating_sub(1);
                Ok(None)
            }
            KeyCode::Enter => {
                Ok(Some(ModalResult::Confirmed(vec![self.cursor])))
            }
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
            _ => Ok(None),
        }
    }
}
