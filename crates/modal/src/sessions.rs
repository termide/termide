//! Sessions selection modal dialog.

use anyhow::Result;
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Widget},
};

use crate::base::render_modal_block;
use std::path::PathBuf;
use unicode_width::UnicodeWidthStr;

use termide_theme::Theme;

use crate::{calculate_modal_width, centered_rect_with_size, Modal, ModalResult, ModalWidthConfig};

/// Action returned by the sessions modal
#[derive(Debug, Clone)]
pub enum SessionAction {
    /// Switch to the selected session
    Switch(PathBuf),
    /// Request deletion of the selected session
    Delete(PathBuf),
}

/// Item representing a session in the list
#[derive(Debug, Clone)]
pub struct SessionItem {
    /// Original project path
    pub project_path: PathBuf,
    /// Display path (potentially shortened)
    pub display_path: String,
    /// Relative time since last modification (e.g., "2 hours ago")
    pub relative_time: String,
    /// Whether this is the current session
    pub is_current: bool,
}

/// Sessions selection modal window
#[derive(Debug)]
pub struct SessionsModal {
    title: String,
    items: Vec<SessionItem>,
    cursor: usize,
    scroll_offset: usize,
    last_list_area: Option<Rect>,
    filter: String,
    filtered_indices: Vec<usize>,
}

/// Maximum number of items visible at once (each item takes 2 lines)
const MAX_VISIBLE_ITEMS: usize = 6;

/// Height of the empty line + filter row + separator above the list
const FILTER_ROWS: u16 = 3;

impl SessionsModal {
    /// Create a new sessions modal
    pub fn new(title: impl Into<String>, items: Vec<SessionItem>) -> Self {
        let filtered_indices = (0..items.len()).collect();
        Self {
            title: title.into(),
            items,
            cursor: 0,
            scroll_offset: 0,
            last_list_area: None,
            filter: String::new(),
            filtered_indices,
        }
    }

    /// Set initial cursor position (for selecting current session)
    pub fn with_cursor(mut self, index: usize) -> Self {
        // filtered_indices starts as 0..items.len(), so cursor == item index
        self.cursor = index.min(self.filtered_indices.len().saturating_sub(1));
        self.adjust_scroll();
        self
    }

    /// Calculate dynamic modal width
    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        let title_width = self.title.len() as u16 + 4;

        // Find max path width across all items (not just filtered, to keep stable width)
        let max_path_width = self
            .items
            .iter()
            .map(|item| item.display_path.len() as u16 + 4) // "▶ " + path + " (current)"
            .max()
            .unwrap_or(40);

        // Filter row needs space for "  Filter: " prefix + input text
        let filter_prefix_width = 12u16; // "  Filter: ".width()

        calculate_modal_width(
            [title_width, max_path_width, filter_prefix_width].into_iter(),
            screen_width,
            ModalWidthConfig::wide(),
        )
    }

    /// Recompute filtered_indices from current filter value
    fn apply_filter(&mut self) {
        let f = self.filter.to_lowercase();
        if f.is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
        } else {
            self.filtered_indices = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.display_path.to_lowercase().contains(&f))
                .map(|(i, _)| i)
                .collect();
        }
        self.cursor = self
            .cursor
            .min(self.filtered_indices.len().saturating_sub(1));
        self.scroll_offset = 0;
        self.adjust_scroll();
    }

    /// Move cursor up
    fn cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.adjust_scroll();
        }
    }

    /// Move cursor down
    fn cursor_down(&mut self) {
        if self.cursor < self.filtered_indices.len().saturating_sub(1) {
            self.cursor += 1;
            self.adjust_scroll();
        }
    }

    /// Go to first item
    fn cursor_home(&mut self) {
        self.cursor = 0;
        self.adjust_scroll();
    }

    /// Go to last item
    fn cursor_end(&mut self) {
        self.cursor = self.filtered_indices.len().saturating_sub(1);
        self.adjust_scroll();
    }

    /// Adjust scroll to keep cursor visible
    fn adjust_scroll(&mut self) {
        self.scroll_offset =
            termide_ui::ensure_offset_visible(self.scroll_offset, self.cursor, MAX_VISIBLE_ITEMS);
    }

    /// Get the selected session from filtered list
    fn get_selected(&self) -> Option<&SessionItem> {
        self.filtered_indices
            .get(self.cursor)
            .and_then(|&i| self.items.get(i))
    }
}

impl Modal for SessionsModal {
    type Result = SessionAction;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let modal_width = self.calculate_modal_width(area.width);

        // Each item takes 2 lines; list is preceded by filter row + separator
        let visible_items = self.filtered_indices.len().min(MAX_VISIBLE_ITEMS);
        let list_height = (visible_items * 2) as u16;

        // Height: 1 (top border) + FILTER_ROWS + list_height + 1 (bottom border)
        let modal_height = 2 + FILTER_ROWS + list_height;

        let modal_area = centered_rect_with_size(modal_width, modal_height, area);

        let inner = render_modal_block(modal_area, buf, &self.title, theme);

        // --- Filter input row ---
        let filter_label = "  Filter: ";
        let filter_text = format!("{}{}", filter_label, self.filter);
        // Pad to full inner width, reserving 1 cell for the block cursor
        let padding_len = (inner.width as usize).saturating_sub(filter_text.width() + 1);
        let padding = " ".repeat(padding_len);

        let filter_style = Style::default().fg(theme.fg);
        let cursor_style = Style::default()
            .fg(theme.bg)
            .bg(theme.fg)
            .add_modifier(Modifier::BOLD);

        let filter_line = Line::from(vec![
            Span::styled(filter_text, filter_style),
            Span::styled("█", cursor_style),
            Span::styled(padding, filter_style),
        ]);

        // Render filter row on inner.y + 1 (row 0 is blank padding)
        let filter_area = Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: 1,
        };
        ratatui::widgets::Paragraph::new(filter_line).render(filter_area, buf);

        // --- Separator row ---
        let sep_y = inner.y + 2;
        let sep_char = "─";
        for x in inner.x..inner.x + inner.width {
            buf[(x, sep_y)]
                .set_symbol(sep_char)
                .set_style(Style::default().fg(theme.accented_bg));
        }

        // --- Session list (below filter + separator) ---
        let list_area = Rect {
            x: inner.x,
            y: inner.y + FILTER_ROWS,
            width: inner.width,
            height: inner.height.saturating_sub(FILTER_ROWS),
        };

        let mut list_items: Vec<ListItem> = Vec::new();
        let t = termide_i18n::t();

        for (pos, &item_idx) in self
            .filtered_indices
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(MAX_VISIBLE_ITEMS)
        {
            let item = &self.items[item_idx];
            let is_selected = pos == self.cursor;
            let is_current = item.is_current;

            // Line 1: Path with selection indicator
            let prefix = if is_selected { "▶ " } else { "  " };

            let path_suffix = if is_current {
                format!(" {}", t.sessions_current())
            } else {
                String::new()
            };

            let path_style = if is_selected {
                Style::default()
                    .fg(theme.fg)
                    .bg(theme.bg)
                    .add_modifier(Modifier::BOLD)
            } else if is_current {
                Style::default().fg(theme.accented_fg)
            } else {
                Style::default().fg(theme.fg)
            };

            // Pad line1 to full width
            let line1_width = prefix.width() + item.display_path.width() + path_suffix.width();
            let padding1 = " ".repeat((list_area.width as usize).saturating_sub(line1_width));

            let line1 = Line::from(vec![
                Span::styled(prefix, path_style),
                Span::styled(&item.display_path, path_style),
                Span::styled(path_suffix, path_style),
                Span::styled(padding1, path_style),
            ]);

            // Line 2: Relative time (indented, dimmed)
            let time_style = Style::default()
                .fg(theme.accented_bg)
                .add_modifier(Modifier::DIM);

            let line2_prefix = "  ";
            let line2_width = line2_prefix.width() + item.relative_time.width();
            let padding2 = " ".repeat((list_area.width as usize).saturating_sub(line2_width));

            let line2 = Line::from(vec![
                Span::styled(line2_prefix, time_style),
                Span::styled(&item.relative_time, time_style),
                Span::styled(padding2, time_style),
            ]);

            list_items.push(ListItem::new(vec![line1, line2]));
        }

        let list = List::new(list_items).style(Style::default().bg(theme.bg));
        list.render(list_area, buf);

        self.last_list_area = Some(list_area);
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let key = chord.raw;
        match key.code {
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),

            // Navigation
            KeyCode::Up => {
                self.cursor_up();
                Ok(None)
            }
            KeyCode::Down => {
                self.cursor_down();
                Ok(None)
            }
            KeyCode::Home => {
                self.cursor_home();
                Ok(None)
            }
            KeyCode::End => {
                self.cursor_end();
                Ok(None)
            }

            // Confirm selection
            KeyCode::Enter => {
                if let Some(item) = self.get_selected() {
                    if item.is_current {
                        Ok(Some(ModalResult::Cancelled))
                    } else {
                        Ok(Some(ModalResult::Confirmed(SessionAction::Switch(
                            item.project_path.clone(),
                        ))))
                    }
                } else {
                    Ok(None)
                }
            }

            // Delete session (Delete or F8)
            KeyCode::Delete | KeyCode::F(8) => {
                if let Some(item) = self.get_selected() {
                    if !item.is_current {
                        Ok(Some(ModalResult::Confirmed(SessionAction::Delete(
                            item.project_path.clone(),
                        ))))
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }

            // Filter text input
            KeyCode::Backspace => {
                self.filter.pop();
                self.apply_filter();
                Ok(None)
            }
            KeyCode::Char(ch) => {
                self.filter.push(ch);
                self.apply_filter();
                Ok(None)
            }

            _ => Ok(None),
        }
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        use crate::{check_mouse_click_with_item_height, MouseClickResult};

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                for _ in 0..3 {
                    self.cursor_up();
                }
                return Ok(None);
            }
            MouseEventKind::ScrollDown => {
                for _ in 0..3 {
                    self.cursor_down();
                }
                return Ok(None);
            }
            _ => {}
        }

        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return Ok(None);
        }

        const LINES_PER_ITEM: usize = 2;

        match check_mouse_click_with_item_height(
            mouse.column,
            mouse.row,
            None,
            self.last_list_area,
            self.scroll_offset,
            LINES_PER_ITEM,
        ) {
            MouseClickResult::OutsideModal | MouseClickResult::OutsideList => Ok(None),
            MouseClickResult::OnListItem(clicked_index) => {
                if clicked_index < self.filtered_indices.len() {
                    let item_idx = self.filtered_indices[clicked_index];
                    let item = &self.items[item_idx];
                    self.cursor = clicked_index;

                    if item.is_current {
                        return Ok(Some(ModalResult::Cancelled));
                    } else {
                        return Ok(Some(ModalResult::Confirmed(SessionAction::Switch(
                            item.project_path.clone(),
                        ))));
                    }
                }
                Ok(None)
            }
        }
    }
}
