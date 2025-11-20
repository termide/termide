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

/// File overwrite action options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverwriteChoice {
    /// Replace the file
    Replace,
    /// Replace if newer
    ReplaceIfNewer,
    /// Replace if larger
    ReplaceIfLarger,
    /// Skip
    Skip,
}

impl OverwriteChoice {
    fn label(&self) -> &'static str {
        match self {
            Self::Replace => "Replace",
            Self::ReplaceIfNewer => "Replace if newer",
            Self::ReplaceIfLarger => "Replace if larger",
            Self::Skip => "Skip",
        }
    }

    fn all() -> &'static [OverwriteChoice] {
        &[
            Self::Replace,
            Self::ReplaceIfNewer,
            Self::ReplaceIfLarger,
            Self::Skip,
        ]
    }
}

/// File overwrite confirmation modal window
#[derive(Debug)]
pub struct OverwriteModal {
    source_name: String,
    dest_name: String,
    cursor: usize,
}

impl OverwriteModal {

    /// Calculate dynamic modal width
    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        // 1. Title width
        let title_width = " File already exists ".len() as u16;

        // 2. Message width
        let message = format!(
            "File '{}' already exists in the target directory.\nSelect action:",
            self.dest_name
        );
        let message_max_line_width = message.lines()
            .map(|line| line.len())
            .max()
            .unwrap_or(0) as u16;

        // 3. Maximum option width
        let options = OverwriteChoice::all();
        let max_option_width = options.iter()
            .map(|choice| {
                // "▶ " + label = prefix 2 + label
                2 + choice.label().len()
            })
            .max()
            .unwrap_or(0) as u16;

        // 4. Hint width
        let hint_width = "↑↓ - select | Enter - confirm | Esc - cancel".len() as u16;

        // Take the maximum of all components
        let content_width = title_width
            .max(message_max_line_width)
            .max(max_option_width)
            .max(hint_width);

        // Add padding and borders:
        // - 2 for outer block border
        // - 2 for option list border
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

impl Modal for OverwriteModal {
    type Result = OverwriteChoice;

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Calculate dynamic width
        let modal_width = self.calculate_modal_width(area.width);

        // Calculate height:
        // 1 (top border) + 3 (message) + 6 (list with border) + 2 (hint) + 1 (bottom border) = 13
        let modal_height = 13;

        // Create centered area
        let modal_area = Self::centered_rect_with_size(modal_width, modal_height, area);
        Clear.render(modal_area, buf);

        // Create block with inverted colors
        let block = Block::default()
            .title(Span::styled(
                " File already exists ",
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
                Constraint::Length(3),  // Message
                Constraint::Min(5),     // Option list
                Constraint::Length(2),  // Hint
            ])
            .split(inner);

        // Conflict message
        let message = format!(
            "File '{}' already exists in the target directory.\nSelect action:",
            self.dest_name
        );
        let prompt = Paragraph::new(message)
            .alignment(Alignment::Left)
            .style(Style::default().fg(theme.background));
        prompt.render(chunks[0], buf);

        // Option list
        let options = OverwriteChoice::all();
        let items: Vec<ListItem> = options
            .iter()
            .enumerate()
            .map(|(idx, choice)| {
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
                    Span::styled(choice.label(), style),
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

        // Hint
        let hint = Paragraph::new("↑↓ - select | Enter - confirm | Esc - cancel")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.text_secondary));
        hint.render(chunks[2], buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>> {
        let max_cursor = OverwriteChoice::all().len().saturating_sub(1);

        match key.code {
            KeyCode::Up => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                Ok(None)
            }
            KeyCode::Down => {
                if self.cursor < max_cursor {
                    self.cursor += 1;
                }
                Ok(None)
            }
            KeyCode::Home => {
                self.cursor = 0;
                Ok(None)
            }
            KeyCode::End => {
                self.cursor = max_cursor;
                Ok(None)
            }
            KeyCode::Enter => {
                let choice = OverwriteChoice::all()[self.cursor];
                Ok(Some(ModalResult::Confirmed(choice)))
            }
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
            _ => Ok(None),
        }
    }
}
