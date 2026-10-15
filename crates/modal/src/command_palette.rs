//! Command Palette modal — fuzzy-searchable list of all app commands.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::base::{render_modal_block, render_text_input};
use crate::input_keys::{handle_input_key, InputKeyResult};
use termide_theme::Theme;
use termide_ui::fuzzy::{highlight, rank, Query};

use crate::{
    calculate_modal_width, centered_rect_with_size, Modal, ModalResult, ModalWidthConfig,
    TextInputHandler,
};

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
    /// The filter field; edits like any other text field.
    filter: TextInputHandler,
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
            filter: TextInputHandler::new(),
            filtered_indices,
            cursor: 0,
            scroll_offset: 0,
            last_list_area: None,
        }
    }

    /// Recompute `filtered_indices` from the current filter value: every
    /// entry whose label, category or keybinding matches, best match first.
    fn apply_filter(&mut self) {
        let mut query = Query::fuzzy(self.filter.text());
        self.filtered_indices = rank(
            self.entries
                .iter()
                .map(|e| query.score_any(&[&e.label, e.category, &e.keybinding])),
        );
        self.cursor = 0;
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
        let label_width = (filter_label.width() as u16).min(inner.width);
        let label_area = Rect {
            x: inner.x,
            y: inner.y + 1,
            width: label_width,
            height: 1,
        };
        ratatui::widgets::Paragraph::new(filter_label)
            .style(Style::default().fg(theme.fg))
            .render(label_area, buf);
        let field_area = Rect {
            x: inner.x + label_width,
            y: inner.y + 1,
            width: inner.width.saturating_sub(label_width + 1),
            height: 1,
        };
        render_text_input(buf, field_area, &mut self.filter, true, theme);

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
        let mut query = Query::fuzzy(self.filter.text());

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

            // A command found by its category or keybinding has no hits in
            // the label.
            let positions = query.positions(&entry.label).unwrap_or_default();
            let hit_style = label_style
                .fg(theme.accented_fg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            let mut spans = vec![Span::styled(prefix, label_style)];
            spans.extend(highlight(&entry.label, &positions, label_style, hit_style));
            spans.push(Span::styled(" ".repeat(gap), label_style));
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
            KeyCode::Esc => return Ok(Some(ModalResult::Cancelled)),
            KeyCode::Enter => {
                return Ok(self
                    .filtered_indices
                    .get(self.cursor)
                    .map(|&idx| ModalResult::Confirmed(idx)));
            }
            KeyCode::Up => self.cursor_up(),
            KeyCode::Down => self.cursor_down(),
            KeyCode::PageUp => self.cursor_page_up(),
            KeyCode::PageDown => self.cursor_page_down(),
            // `Home`/`End` belong to the filter field; with `Ctrl` they jump
            // to the first and the last command.
            KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => self.cursor_home(),
            KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => self.cursor_end(),
            _ => {
                if handle_input_key(&mut self.filter, key) == InputKeyResult::TextModified {
                    self.apply_filter();
                }
            }
        }
        Ok(None)
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        self.filter.paste(text);
        self.apply_filter();
        true
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn entry(label: &str, category: &'static str, keybinding: &str) -> CommandEntry {
        CommandEntry {
            label: label.into(),
            category,
            keybinding: keybinding.into(),
        }
    }

    fn palette() -> CommandPaletteModal {
        CommandPaletteModal::new(vec![
            entry("Open Git Status", "Git", "Alt+G"),
            entry("Toggle Stack", "Layout", "Alt+Backspace"),
            entry("Open Git Log", "Git", "Alt+C"),
            entry("Quit", "App", "Alt+Q"),
        ])
    }

    fn type_text(modal: &mut CommandPaletteModal, text: &str) {
        for ch in text.chars() {
            let key = KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE);
            modal
                .handle_key(termide_core::KeyChord::identity(key))
                .unwrap();
        }
    }

    fn labels(modal: &CommandPaletteModal) -> Vec<&str> {
        modal
            .filtered_indices
            .iter()
            .map(|&i| modal.entries[i].label.as_str())
            .collect()
    }

    #[test]
    fn filters_by_subsequence_best_match_first() {
        let mut modal = palette();
        type_text(&mut modal, "gitlog");
        assert_eq!(labels(&modal), ["Open Git Log"]);
    }

    #[test]
    fn matches_category_and_keybinding_too() {
        let mut modal = palette();
        type_text(&mut modal, "layout");
        assert_eq!(labels(&modal), ["Toggle Stack"]);

        let mut modal = palette();
        type_text(&mut modal, "alt+q");
        assert_eq!(labels(&modal)[0], "Quit");
    }

    #[test]
    fn a_new_filter_puts_the_cursor_on_the_best_match() {
        let mut modal = palette();
        modal.cursor_end();
        type_text(&mut modal, "git");
        assert_eq!(modal.cursor, 0);
        assert!(labels(&modal).iter().all(|l| l.contains("Git")));
    }

    #[test]
    fn renders_the_matched_label_graphemes_highlighted() {
        let mut modal = palette();
        type_text(&mut modal, "quit");
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        modal.render(area, &mut buf, &theme);
        let cell = (0..area.height)
            .flat_map(|y| (0..area.width).map(move |x| (x, y)))
            .find(|&(x, y)| buf[(x, y)].symbol() == "Q")
            .map(|(x, y)| buf[(x, y)].clone())
            .expect("the Quit row is drawn");
        assert!(cell.modifier.contains(Modifier::UNDERLINED));
    }

    fn press(modal: &mut CommandPaletteModal, code: KeyCode, modifiers: KeyModifiers) {
        let key = KeyEvent::new(code, modifiers);
        modal
            .handle_key(termide_core::KeyChord::identity(key))
            .unwrap();
    }

    #[test]
    fn the_filter_edits_at_its_cursor_and_refilters() {
        let mut modal = palette();
        type_text(&mut modal, "qit");
        press(&mut modal, KeyCode::Left, KeyModifiers::NONE);
        press(&mut modal, KeyCode::Left, KeyModifiers::NONE);
        type_text(&mut modal, "u");
        assert_eq!(modal.filter.text(), "quit");
        assert_eq!(labels(&modal), ["Quit"]);
    }

    #[test]
    fn a_selection_in_the_filter_is_deleted_as_one() {
        let mut modal = palette();
        type_text(&mut modal, "gitlog");
        press(&mut modal, KeyCode::Left, KeyModifiers::SHIFT);
        press(&mut modal, KeyCode::Left, KeyModifiers::SHIFT);
        press(&mut modal, KeyCode::Left, KeyModifiers::SHIFT);
        press(&mut modal, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(modal.filter.text(), "git");
        assert!(labels(&modal).contains(&"Open Git Status"));
    }

    #[test]
    fn ctrl_v_does_not_type_a_v() {
        let mut modal = palette();
        type_text(&mut modal, "git");
        // A stand-in clipboard, so the user's is never read or written.
        termide_ui::clipboard::use_fake_clipboard("PASTE");
        let clip = "PASTE";
        press(&mut modal, KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert_eq!(modal.filter.text(), format!("git{clip}"));
    }

    #[test]
    fn up_and_down_still_move_the_list_and_ctrl_home_end_jump() {
        let mut modal = palette();
        press(&mut modal, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(modal.cursor, 1);
        press(&mut modal, KeyCode::End, KeyModifiers::CONTROL);
        assert_eq!(modal.cursor, 3);
        press(&mut modal, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(modal.cursor, 2);
        press(&mut modal, KeyCode::Home, KeyModifiers::CONTROL);
        assert_eq!(modal.cursor, 0);
    }

    #[test]
    fn a_bracketed_paste_lands_in_the_filter() {
        let mut modal = palette();
        assert!(modal.handle_paste("quit"));
        assert_eq!(labels(&modal), ["Quit"]);
    }
}
