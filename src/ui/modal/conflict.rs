use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Widget},
};
use std::path::Path;

use super::{Modal, ModalResult};
use crate::theme::Theme;

/// Conflict resolution options for copy/move operations
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConflictResolution {
    /// Overwrite existing file
    Overwrite,
    /// Skip this file
    Skip,
    /// Rename this file
    Rename,
    /// Overwrite and apply to all subsequent files
    OverwriteAll,
    /// Skip all subsequent files
    SkipAll,
    /// Rename all subsequent files
    RenameAll,
}

/// File conflict resolution modal window
#[derive(Debug)]
pub struct ConflictModal {
    title: String,
    source_name: String,
    dest_name: String,
    is_directory: bool,
    cursor: usize,
}

impl ConflictModal {
    /// Create a conflict modal window
    pub fn new(source: &Path, destination: &Path) -> Self {
        let source_name = source.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        let dest_name = destination.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        let is_directory = destination.is_dir();

        let title = if is_directory {
            "Directory Conflict".to_string()
        } else {
            "File Conflict".to_string()
        };

        Self {
            title,
            source_name,
            dest_name,
            is_directory,
            cursor: 0,
        }
    }

    fn get_options(&self) -> Vec<(String, String)> {
        let item_type = if self.is_directory { "directory" } else { "file" };

        vec![
            ("Overwrite".to_string(), format!("Replace existing {}", item_type)),
            ("Skip".to_string(), format!("Do not copy this {}", item_type)),
            ("Rename".to_string(), format!("Set new name for this {}", item_type)),
            ("Overwrite All".to_string(), "Replace all subsequent conflicts".to_string()),
            ("Skip All".to_string(), "Skip all subsequent conflicts".to_string()),
            ("Rename All".to_string(), "Set pattern for all conflicts".to_string()),
        ]
    }

    fn get_resolution(&self) -> ConflictResolution {
        match self.cursor {
            0 => ConflictResolution::Overwrite,
            1 => ConflictResolution::Skip,
            2 => ConflictResolution::Rename,
            3 => ConflictResolution::OverwriteAll,
            4 => ConflictResolution::SkipAll,
            _ => ConflictResolution::RenameAll,
        }
    }

    /// Calculate dynamic modal width
    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        // 1. Title width (with spaces on the edges)
        let title_width = self.title.len() as u16 + 2;

        // 2. Message width
        let item_type = if self.is_directory { "Directory" } else { "File" };
        let message = format!("{} '{}' already exists.\nWhat to do?", item_type, self.dest_name);
        let message_max_line_width = message.lines()
            .map(|line| line.len())
            .max()
            .unwrap_or(0) as u16;

        // 3. Maximum option width
        let options = self.get_options();
        let max_option_width = options.iter()
            .map(|(label, desc)| {
                // "▶ " + label + " - " + desc = prefix 2 + label + 3 + desc
                2 + label.len() + 3 + desc.len()
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

impl Modal for ConflictModal {
    type Result = ConflictResolution;

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Calculate dynamic width
        let modal_width = self.calculate_modal_width(area.width);

        // Calculate height:
        // 1 (top border) + 3 (message) + 8 (list of 6 options with border) + 1 (hint) + 1 (bottom border) = 14
        let modal_height = 14;

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
                Constraint::Length(3),  // Message
                Constraint::Min(7),     // Option list (6 options + borders)
                Constraint::Length(1),  // Hint
            ])
            .split(inner);

        // Conflict message
        let item_type = if self.is_directory { "Directory" } else { "File" };
        let message = format!(
            "{} '{}' already exists.\nWhat to do?",
            item_type,
            self.dest_name
        );
        let prompt = Paragraph::new(message)
            .alignment(Alignment::Left)
            .style(Style::default().fg(theme.background));
        prompt.render(chunks[0], buf);

        // Option list
        let options = self.get_options();
        let items: Vec<ListItem> = options
            .iter()
            .enumerate()
            .map(|(idx, (label, desc))| {
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
                    Span::styled(format!("{} - {}", label, desc), style),
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
        match key.code {
            KeyCode::Up => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                Ok(None)
            }
            KeyCode::Down => {
                if self.cursor < 5 {
                    self.cursor += 1;
                }
                Ok(None)
            }
            KeyCode::Home => {
                self.cursor = 0;
                Ok(None)
            }
            KeyCode::End => {
                self.cursor = 5;
                Ok(None)
            }
            KeyCode::Enter => {
                Ok(Some(ModalResult::Confirmed(self.get_resolution())))
            }
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
            _ => Ok(None),
        }
    }
}
