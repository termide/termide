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
use crate::ui::centered_rect_with_size;

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
    #[allow(dead_code)]
    source_name: String,
    dest_name: String,
    cursor: usize,
    last_list_area: Option<Rect>,
}

impl OverwriteModal {
    /// Create a new overwrite modal
    #[allow(dead_code)]
    pub fn new(source_name: String, dest_name: String) -> Self {
        Self {
            source_name,
            dest_name,
            cursor: 0,
            last_list_area: None,
        }
    }

    /// Calculate dynamic modal width
    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        // 1. Title width
        let title_width = " File already exists ".len() as u16;

        // 2. Message width
        let message = format!(
            "File '{}' already exists in the target directory.\nSelect action:",
            self.dest_name
        );
        let message_max_line_width =
            message.lines().map(|line| line.len()).max().unwrap_or(0) as u16;

        // 3. Maximum option width
        let options = OverwriteChoice::all();
        let max_option_width = options
            .iter()
            .map(|choice| {
                // "▶ " + label = prefix 2 + label
                2 + choice.label().len()
            })
            .max()
            .unwrap_or(0) as u16;

        // Take the maximum of all components
        let content_width = title_width
            .max(message_max_line_width)
            .max(max_option_width);

        // Add padding and borders:
        // - 2 for outer block border
        // - 4 for padding
        let total_width = content_width + 6;

        // Apply constraints
        let max_width = (screen_width as f32 * 0.75) as u16;
        total_width.max(20).min(max_width).min(screen_width)
    }
}

impl Modal for OverwriteModal {
    type Result = OverwriteChoice;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Calculate dynamic width
        let modal_width = self.calculate_modal_width(area.width);

        // Calculate height:
        // 1 (top border) + 2 (message) + 4 (list) + 1 (bottom border) = 8
        let modal_height = 8;

        // Create centered area
        let modal_area = centered_rect_with_size(modal_width, modal_height, area);
        Clear.render(modal_area, buf);

        // Create block with inverted colors
        let block = Block::default()
            .title(Span::styled(
                " File already exists ",
                Style::default().fg(theme.bg).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.bg))
            .style(Style::default().bg(theme.fg));

        let inner = block.inner(modal_area);
        block.render(modal_area, buf);

        use ratatui::layout::{Constraint, Direction, Layout};
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // Message
                Constraint::Min(5),    // Option list
            ])
            .split(inner);

        // Conflict message
        let message = format!(
            "File '{}' already exists in the target directory.\nSelect action:",
            self.dest_name
        );
        let prompt = Paragraph::new(message)
            .alignment(Alignment::Left)
            .style(Style::default().fg(theme.bg));
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
                        .fg(theme.fg)
                        .bg(theme.accented_fg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.bg)
                };

                ListItem::new(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(choice.label(), style),
                ]))
            })
            .collect();

        let list = List::new(items).style(Style::default().bg(theme.fg));

        list.render(chunks[1], buf);

        // Save list area for mouse handling
        self.last_list_area = Some(chunks[1]);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>> {
        let max_cursor = OverwriteChoice::all().len().saturating_sub(1);

        match key.code {
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
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
        let all_choices = OverwriteChoice::all();

        if clicked_item < all_choices.len() {
            // Item clicked - select and confirm immediately
            self.cursor = clicked_item;
            let choice = all_choices[self.cursor];
            Ok(Some(ModalResult::Confirmed(choice)))
        } else {
            Ok(None)
        }
    }
}
