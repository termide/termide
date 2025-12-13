//! Selection modal dialog (single selection).

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Widget},
};

use termide_theme::Theme;

use crate::{
    calculate_modal_width, centered_rect_with_size, max_item_width, max_line_width, Modal,
    ModalResult, ModalWidthConfig,
};

/// Selection modal window (single selection only)
#[derive(Debug)]
pub struct SelectModal {
    title: String,
    prompt: String,
    items: Vec<String>,
    cursor: usize,
    last_list_area: Option<Rect>,
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
            last_list_area: None,
        }
    }

    /// Calculate dynamic modal width
    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        let title_width = self.title.len() as u16 + 2;
        let prompt_width = max_line_width(&self.prompt);
        let items_width = max_item_width(&self.items, 2); // "▶ " prefix

        calculate_modal_width(
            [title_width, prompt_width, items_width].into_iter(),
            screen_width,
            ModalWidthConfig::default(),
        )
    }
}

impl Modal for SelectModal {
    type Result = Vec<usize>;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Calculate dynamic width
        let modal_width = self.calculate_modal_width(area.width);

        // Calculate prompt lines dynamically
        let prompt_lines = self.prompt.lines().count().max(1) as u16;

        // Calculate height:
        // 1 (top border) + N (prompt) + M (list) + 1 (bottom border)
        let list_height = self.items.len().min(10) as u16; // Limit to 10 items
        let modal_height = 1 + prompt_lines + list_height + 1;

        // Create centered area
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
                Constraint::Length(prompt_lines), // Prompt
                Constraint::Length(list_height),  // List
            ])
            .split(inner);

        let prompt = Paragraph::new(self.prompt.clone())
            .alignment(Alignment::Left)
            .style(Style::default().fg(theme.bg));
        prompt.render(chunks[0], buf);

        let items: Vec<ListItem> = self
            .items
            .iter()
            .enumerate()
            .map(|(idx, label)| {
                let prefix = if idx == self.cursor { "▶ " } else { "  " };

                let style = if idx == self.cursor {
                    Style::default()
                        .fg(theme.fg)
                        .bg(theme.accented_fg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.bg)
                };

                ListItem::new(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(label, style),
                ]))
            })
            .collect();

        let list = List::new(items).style(Style::default().bg(theme.fg));

        list.render(chunks[1], buf);

        // Save list area for mouse handling
        self.last_list_area = Some(chunks[1]);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>> {
        match key.code {
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
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
            KeyCode::Enter => Ok(Some(ModalResult::Confirmed(vec![self.cursor]))),
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

        // Check if we have stored list area
        let Some(list_area) = self.last_list_area else {
            return Ok(None);
        };

        // Check if click is within list area
        if mouse.row < list_area.y
            || mouse.row >= list_area.y + list_area.height
            || mouse.column < list_area.x
            || mouse.column >= list_area.x + list_area.width
        {
            return Ok(None);
        }

        // Calculate which item was clicked
        let clicked_item = (mouse.row - list_area.y) as usize;

        if clicked_item < self.items.len() {
            // Item clicked - select and confirm immediately
            self.cursor = clicked_item;
            Ok(Some(ModalResult::Confirmed(vec![self.cursor])))
        } else {
            Ok(None)
        }
    }
}
