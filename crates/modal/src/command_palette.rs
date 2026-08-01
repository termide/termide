//! Command Palette modal — fuzzy-searchable list of all app commands.

use anyhow::Result;
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::base::render_modal_block;
use termide_theme::Theme;

use crate::{calculate_modal_width, centered_rect_with_size, Modal, ModalResult, ModalWidthConfig};

/// A single entry in the command palette.
#[derive(Debug, Clone)]
pub struct CommandEntry {
    /// Short command label shown in the list (e.g. "Open Git Status")
    pub label: String,
    /// Category shown dimmed to the right of the label (e.g. "Git")
    pub category: &'static str,
    /// Configured keybinding string (e.g. "Alt+G"), empty if unbound
    pub keybinding: String,
}

/// Command palette modal with fuzzy filter.
///
/// Returns `ModalResult<usize>` — the index in the **original** (unfiltered)
/// `entries` list of the command the user confirmed.
#[derive(Debug)]
pub struct CommandPaletteModal {
    entries: Vec<CommandEntry>,
    filter: String,
    filtered_indices: Vec<usize>,
    cursor: usize,
    scroll_offset: usize,
    last_list_area: Option<Rect>,
}

/// Maximum number of items visible at once (single-line items).
const MAX_VISIBLE_ITEMS: usize = 12;

/// Height of empty row + filter row + separator above the list.
const FILTER_ROWS: u16 = 3;

impl CommandPaletteModal {
    /// Create a new command palette modal with the given entries.
    pub fn new(entries: Vec<CommandEntry>) -> Self {
        let filtered_indices = (0..entries.len()).collect();
        Self {
            entries,
            filter: String::new(),
            filtered_indices,
            cursor: 0,
            scroll_offset: 0,
            last_list_area: None,
        }
    }

    /// Recompute `filtered_indices` from the current filter value.
    fn apply_filter(&mut self) {
        let f = self.filter.to_lowercase();
        if f.is_empty() {
            self.filtered_indices = (0..self.entries.len()).collect();
        } else {
            self.filtered_indices = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    e.label.to_lowercase().contains(&f)
                        || e.category.to_lowercase().contains(&f)
                        || e.keybinding.to_lowercase().contains(&f)
                })
                .map(|(i, _)| i)
                .collect();
        }
        self.cursor = self
            .cursor
            .min(self.filtered_indices.len().saturating_sub(1));
        self.scroll_offset = 0;
        self.adjust_scroll();
    }

    fn cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.adjust_scroll();
        }
    }

    fn cursor_down(&mut self) {
        if self.cursor < self.filtered_indices.len().saturating_sub(1) {
            self.cursor += 1;
            self.adjust_scroll();
        }
    }

    fn cursor_home(&mut self) {
        self.cursor = 0;
        self.adjust_scroll();
    }

    fn cursor_end(&mut self) {
        self.cursor = self.filtered_indices.len().saturating_sub(1);
        self.adjust_scroll();
    }

    fn cursor_page_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(MAX_VISIBLE_ITEMS);
        self.adjust_scroll();
    }

    fn cursor_page_down(&mut self) {
        let last = self.filtered_indices.len().saturating_sub(1);
        self.cursor = (self.cursor + MAX_VISIBLE_ITEMS).min(last);
        self.adjust_scroll();
    }

    fn adjust_scroll(&mut self) {
        self.scroll_offset =
            termide_ui::ensure_offset_visible(self.scroll_offset, self.cursor, MAX_VISIBLE_ITEMS);
    }

    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        // Title
        let title_width = "Command Palette".len() as u16 + 4;

        // Max entry width: "▶ " + label + "  " + category + "  " + keybinding
        let max_entry_width = self
            .entries
            .iter()
            .map(|e| 2 + e.label.width() + 2 + e.category.width() + 2 + e.keybinding.width())
            .max()
            .unwrap_or(50) as u16;

        let filter_prefix = "  Filter: ".width() as u16;

        calculate_modal_width(
            [title_width, max_entry_width + 2, filter_prefix].into_iter(),
            screen_width,
            ModalWidthConfig::wide(),
        )
    }
}

impl Modal for CommandPaletteModal {
    type Result = usize;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let modal_width = self.calculate_modal_width(area.width);

        let visible_items = self.filtered_indices.len().min(MAX_VISIBLE_ITEMS);
        let list_height = visible_items as u16;

        // 2 borders + FILTER_ROWS + list rows
        let modal_height = 2 + FILTER_ROWS + list_height.max(1);

        let modal_area = centered_rect_with_size(modal_width, modal_height, area);
        let inner = render_modal_block(modal_area, buf, "Command Palette", theme);

        // --- Filter input ---
        let filter_label = "  Filter: ";
        let filter_text = format!("{}{}", filter_label, self.filter);
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

        let filter_area = Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: 1,
        };
        ratatui::widgets::Paragraph::new(filter_line).render(filter_area, buf);

        // --- Separator ---
        let sep_y = inner.y + 2;
        for x in inner.x..inner.x + inner.width {
            buf[(x, sep_y)]
                .set_symbol("─")
                .set_style(Style::default().fg(theme.accented_bg));
        }

        // --- Command list ---
        let list_area = Rect {
            x: inner.x,
            y: inner.y + FILTER_ROWS,
            width: inner.width,
            height: inner.height.saturating_sub(FILTER_ROWS),
        };

        let mut list_items: Vec<ListItem> = Vec::new();

        // Compute fixed column widths across all filtered entries for right-alignment
        let max_cat_w = self
            .filtered_indices
            .iter()
            .map(|&i| self.entries[i].category.width())
            .max()
            .unwrap_or(0);
        let max_kb_w = self
            .filtered_indices
            .iter()
            .map(|&i| self.entries[i].keybinding.width())
            .max()
            .unwrap_or(0);
        let right_cols_w = if max_cat_w > 0 { max_cat_w + 2 } else { 0 }
            + if max_kb_w > 0 { max_kb_w + 2 } else { 0 };

        for (pos, &entry_idx) in self
            .filtered_indices
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(MAX_VISIBLE_ITEMS)
        {
            let entry = &self.entries[entry_idx];
            let is_selected = pos == self.cursor;

            let prefix = if is_selected { "▶ " } else { "  " };

            // Styles
            let label_style = if is_selected {
                Style::default()
                    .fg(theme.fg)
                    .bg(theme.bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            let category_style = if is_selected {
                Style::default()
                    .fg(theme.accented_fg)
                    .bg(theme.bg)
                    .add_modifier(Modifier::DIM)
            } else {
                Style::default()
                    .fg(theme.accented_fg)
                    .add_modifier(Modifier::DIM)
            };
            let kb_style = if is_selected {
                Style::default()
                    .fg(theme.accented_bg)
                    .bg(theme.bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.accented_bg)
            };

            // Build spans with fixed right-aligned columns
            let prefix_w = prefix.width();
            let label_w = entry.label.width();
            let total_width = list_area.width as usize;
            let gap = total_width
                .saturating_sub(prefix_w + label_w + right_cols_w)
                .max(1);

            let mut spans = vec![
                Span::styled(prefix, label_style),
                Span::styled(entry.label.clone(), label_style),
                Span::styled(" ".repeat(gap), label_style),
            ];
            if max_cat_w > 0 {
                let cat_text = format!("{:>width$}  ", entry.category, width = max_cat_w);
                spans.push(Span::styled(cat_text, category_style));
            }
            if max_kb_w > 0 {
                let kb_text = format!("{:>width$}  ", entry.keybinding, width = max_kb_w);
                spans.push(Span::styled(kb_text, kb_style));
            }

            list_items.push(ListItem::new(Line::from(spans)));
        }

        if list_items.is_empty() {
            // Show "No commands found" hint
            let hint = Line::from(Span::styled(
                "  No commands found",
                Style::default()
                    .fg(theme.accented_bg)
                    .add_modifier(Modifier::DIM),
            ));
            list_items.push(ListItem::new(hint));
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

            KeyCode::Enter => {
                if let Some(&idx) = self.filtered_indices.get(self.cursor) {
                    Ok(Some(ModalResult::Confirmed(idx)))
                } else {
                    Ok(None)
                }
            }

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
            KeyCode::PageUp => {
                self.cursor_page_up();
                Ok(None)
            }
            KeyCode::PageDown => {
                self.cursor_page_down();
                Ok(None)
            }

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

        match check_mouse_click_with_item_height(
            mouse.column,
            mouse.row,
            None,
            self.last_list_area,
            self.scroll_offset,
            1, // single-line items
        ) {
            MouseClickResult::OutsideModal | MouseClickResult::OutsideList => Ok(None),
            MouseClickResult::OnListItem(clicked_pos) => {
                if clicked_pos < self.filtered_indices.len() {
                    let idx = self.filtered_indices[clicked_pos];
                    self.cursor = clicked_pos;
                    Ok(Some(ModalResult::Confirmed(idx)))
                } else {
                    Ok(None)
                }
            }
        }
    }
}
