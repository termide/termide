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
use crate::constants::{
    MODAL_MAX_WIDTH_PERCENTAGE_DEFAULT, MODAL_MIN_WIDTH_DEFAULT, MODAL_PADDING_WITH_BORDER,
};
use crate::theme::Theme;
use crate::ui::centered_rect_with_size;

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
    #[allow(dead_code)]
    source_name: String,
    dest_name: String,
    is_directory: bool,
    remaining_items: usize, // Number of items remaining in queue (excluding current)
    cursor: usize,
    last_list_area: Option<Rect>,
}

impl ConflictModal {
    /// Create a conflict modal window
    pub fn new(source: &Path, destination: &Path, remaining_items: usize) -> Self {
        let source_name = source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        let dest_name = destination
            .file_name()
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
            remaining_items,
            cursor: 0,
            last_list_area: None,
        }
    }

    fn get_options(&self) -> Vec<(String, String)> {
        let item_type = if self.is_directory {
            "directory"
        } else {
            "file"
        };

        let mut options = vec![
            (
                "Overwrite".to_string(),
                format!("Replace existing {}", item_type),
            ),
            (
                "Skip".to_string(),
                format!("Do not copy this {}", item_type),
            ),
            (
                "Rename".to_string(),
                format!("Set new name for this {}", item_type),
            ),
        ];

        // Only add "All" variants if there are more items remaining
        if self.remaining_items > 0 {
            options.extend(vec![
                (
                    "Overwrite All".to_string(),
                    "Replace all subsequent conflicts".to_string(),
                ),
                (
                    "Skip All".to_string(),
                    "Skip all subsequent conflicts".to_string(),
                ),
                (
                    "Rename All".to_string(),
                    "Set pattern for all conflicts".to_string(),
                ),
            ]);
        }

        options
    }

    fn get_resolution(&self) -> ConflictResolution {
        // If there are no remaining items, only 3 options available
        if self.remaining_items == 0 {
            match self.cursor {
                0 => ConflictResolution::Overwrite,
                1 => ConflictResolution::Skip,
                _ => ConflictResolution::Rename,
            }
        } else {
            // All 6 options available
            match self.cursor {
                0 => ConflictResolution::Overwrite,
                1 => ConflictResolution::Skip,
                2 => ConflictResolution::Rename,
                3 => ConflictResolution::OverwriteAll,
                4 => ConflictResolution::SkipAll,
                _ => ConflictResolution::RenameAll,
            }
        }
    }

    /// Calculate dynamic modal width
    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        // 1. Title width (with spaces on the edges)
        let title_width = self.title.len() as u16 + 2;

        // 2. Message width
        let item_type = if self.is_directory {
            "Directory"
        } else {
            "File"
        };
        let message = format!(
            "{} '{}' already exists.\nWhat to do?",
            item_type, self.dest_name
        );
        let message_max_line_width =
            message.lines().map(|line| line.len()).max().unwrap_or(0) as u16;

        // 3. Maximum option width
        let options = self.get_options();
        let max_option_width = options
            .iter()
            .map(|(label, desc)| {
                // "▶ " + label + " - " + desc = prefix 2 + label + 3 + desc
                2 + label.len() + 3 + desc.len()
            })
            .max()
            .unwrap_or(0) as u16;

        // Take the maximum of all components
        let content_width = title_width
            .max(message_max_line_width)
            .max(max_option_width);

        // Add padding and borders
        let total_width = content_width + MODAL_PADDING_WITH_BORDER;

        // Apply constraints
        let max_width = (screen_width as f32 * MODAL_MAX_WIDTH_PERCENTAGE_DEFAULT) as u16;
        total_width
            .max(MODAL_MIN_WIDTH_DEFAULT)
            .min(max_width)
            .min(screen_width)
    }
}

impl Modal for ConflictModal {
    type Result = ConflictResolution;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Calculate dynamic width
        let modal_width = self.calculate_modal_width(area.width);

        // Calculate height dynamically based on number of options:
        // - 3 options: 1 (top border) + 2 (message) + 3 (list) + 1 (bottom border) = 7
        // - 6 options: 1 (top border) + 2 (message) + 6 (list) + 1 (bottom border) = 10
        let modal_height = if self.remaining_items == 0 {
            7 // Only 3 options
        } else {
            10 // All 6 options
        };

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

        use ratatui::layout::{Constraint, Direction, Layout};

        // Calculate list constraint based on number of options
        let list_constraint = if self.remaining_items == 0 {
            Constraint::Length(3) // 3 options
        } else {
            Constraint::Length(6) // 6 options
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // Message
                list_constraint,       // Option list
            ])
            .split(inner);

        // Conflict message
        let item_type = if self.is_directory {
            "Directory"
        } else {
            "File"
        };
        let message = format!(
            "{} '{}' already exists.\nWhat to do?",
            item_type, self.dest_name
        );
        let prompt = Paragraph::new(message)
            .alignment(Alignment::Left)
            .style(Style::default().fg(theme.bg));
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
                        .fg(theme.fg)
                        .bg(theme.accented_fg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.bg)
                };

                ListItem::new(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(format!("{} - {}", label, desc), style),
                ]))
            })
            .collect();

        let list = List::new(items).style(Style::default().bg(theme.fg));

        list.render(chunks[1], buf);

        // Save list area for mouse handling
        self.last_list_area = Some(chunks[1]);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>> {
        // Calculate max cursor based on number of options
        let max_cursor = if self.remaining_items == 0 {
            2 // Only 3 options: Overwrite, Skip, Rename
        } else {
            5 // All 6 options
        };

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
            KeyCode::Enter => Ok(Some(ModalResult::Confirmed(self.get_resolution()))),
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

        // Calculate max cursor based on number of options
        let max_items = if self.remaining_items == 0 { 3 } else { 6 };

        if clicked_item < max_items {
            // Item clicked - select and confirm immediately
            self.cursor = clicked_item;
            Ok(Some(ModalResult::Confirmed(self.get_resolution())))
        } else {
            Ok(None)
        }
    }
}
