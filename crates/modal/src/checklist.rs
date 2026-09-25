//! Checklist modal: checkboxes under group headings, applied together.

use anyhow::Result;
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use termide_core::ChecklistItem;
use termide_theme::Theme;

use crate::{
    base::render_modal_block, calculate_modal_width, centered_rect_with_size, max_line_width,
    Modal, ModalResult, ModalWidthConfig,
};

/// Rows the list shows at most before it scrolls.
const MAX_ROWS: usize = 20;

/// One row of the list: a group heading, or the item at an index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Heading(usize),
    Item(usize),
}

/// Checkboxes listed under their group headings. `Space` (or a click)
/// toggles the item under the cursor, `Enter` applies them all, `Esc` leaves
/// everything as it was. A locked item is shown greyed and keeps its state.
#[derive(Debug)]
pub struct ChecklistModal {
    title: String,
    prompt: String,
    items: Vec<ChecklistItem>,
    rows: Vec<Row>,
    /// Index into `items` of the item under the cursor.
    cursor: usize,
    /// First row shown.
    scroll: usize,
    /// Screen rect of the list from the last render, for clicks.
    list_area: Option<Rect>,
}

impl ChecklistModal {
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        prompt: impl Into<String>,
        items: Vec<ChecklistItem>,
    ) -> Self {
        let mut rows = Vec::new();
        let mut group: Option<&str> = None;
        for (index, item) in items.iter().enumerate() {
            if group != Some(item.group.as_str()) {
                if !item.group.is_empty() {
                    rows.push(Row::Heading(index));
                }
                group = Some(item.group.as_str());
            }
            rows.push(Row::Item(index));
        }
        Self {
            title: title.into(),
            prompt: prompt.into(),
            items,
            rows,
            cursor: 0,
            scroll: 0,
            list_area: None,
        }
    }

    /// The keys of the items checked now, in list order.
    #[must_use]
    pub fn checked(&self) -> Vec<String> {
        self.items
            .iter()
            .filter(|item| item.checked)
            .map(|item| item.key.clone())
            .collect()
    }

    fn toggle(&mut self, index: usize) {
        if let Some(item) = self.items.get_mut(index) {
            if item.enabled {
                item.checked = !item.checked;
            }
        }
    }

    fn move_by(&mut self, delta: isize) {
        let last = self.items.len().saturating_sub(1) as isize;
        self.cursor = (self.cursor as isize + delta).clamp(0, last.max(0)) as usize;
    }

    /// The row an item sits on.
    fn row_of(&self, index: usize) -> usize {
        self.rows
            .iter()
            .position(|row| *row == Row::Item(index))
            .unwrap_or(0)
    }

    /// The text of an item's row after the checkbox.
    fn item_text(item: &ChecklistItem) -> String {
        if item.note.is_empty() {
            item.label.clone()
        } else {
            format!("{} — {}", item.label, item.note)
        }
    }

    fn modal_width(&self, screen_width: u16) -> u16 {
        let title = UnicodeWidthStr::width(self.title.as_str()) as u16 + 2;
        let prompt = max_line_width(&self.prompt);
        // " [x] " before each item's text, a space after it.
        let items = self
            .items
            .iter()
            .map(|item| UnicodeWidthStr::width(Self::item_text(item).as_str()) as u16 + 6)
            .chain(
                self.items
                    .iter()
                    .map(|item| UnicodeWidthStr::width(item.group.as_str()) as u16 + 2),
            )
            .max()
            .unwrap_or(0);
        calculate_modal_width(
            [title, prompt, items].into_iter(),
            screen_width,
            ModalWidthConfig::default(),
        )
    }
}

impl Modal for ChecklistModal {
    /// The keys left checked.
    type Result = Vec<String>;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let width = self.modal_width(area.width);
        let prompt_lines = self.prompt.lines().count() as u16;
        let screen_rows = area.height.saturating_sub(4 + prompt_lines) as usize;
        let visible = self.rows.len().min(MAX_ROWS).min(screen_rows.max(1));
        // Keep the cursor's row, and its group heading when it has one, in view.
        let row = self.row_of(self.cursor);
        let first = if row > 0 && matches!(self.rows[row - 1], Row::Heading(_)) {
            row - 1
        } else {
            row
        };
        if first < self.scroll {
            self.scroll = first;
        } else if row >= self.scroll + visible {
            self.scroll = row + 1 - visible;
        }
        let height = 2 + prompt_lines + u16::from(prompt_lines > 0) + visible as u16;
        let modal = centered_rect_with_size(width, height, area);
        let inner = render_modal_block(modal, buf, &self.title, theme);

        let dim = Style::default().fg(theme.disabled);
        for (i, line) in self.prompt.lines().enumerate() {
            buf.set_stringn(
                inner.x + 1,
                inner.y + i as u16,
                line,
                inner.width.saturating_sub(2) as usize,
                dim,
            );
        }
        let top = inner.y + prompt_lines + u16::from(prompt_lines > 0);
        let list = Rect {
            x: inner.x,
            y: top,
            width: inner.width,
            height: visible as u16,
        };
        self.list_area = Some(list);
        for (line, row) in self.rows.iter().skip(self.scroll).take(visible).enumerate() {
            let y = top + line as u16;
            match *row {
                Row::Heading(index) => {
                    let style = Style::default()
                        .fg(theme.accented_fg)
                        .add_modifier(Modifier::BOLD);
                    buf.set_stringn(
                        inner.x + 1,
                        y,
                        &self.items[index].group,
                        inner.width as usize,
                        style,
                    );
                }
                Row::Item(index) => {
                    let item = &self.items[index];
                    let mark = if item.checked { "[x]" } else { "[ ]" };
                    let text = format!(" {mark} {}", Self::item_text(item));
                    let style = if index == self.cursor {
                        Style::default().fg(theme.bg).bg(theme.fg)
                    } else if item.enabled {
                        Style::default().fg(theme.fg)
                    } else {
                        dim
                    };
                    let padded = format!("{text:<width$}", width = inner.width as usize);
                    buf.set_stringn(inner.x, y, padded, inner.width as usize, style);
                }
            }
        }
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        match chord.canonical.code {
            KeyCode::Esc => return Ok(Some(ModalResult::Cancelled)),
            KeyCode::Enter => return Ok(Some(ModalResult::Confirmed(self.checked()))),
            KeyCode::Char(' ') => self.toggle(self.cursor),
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::PageUp => self.move_by(-(MAX_ROWS as isize)),
            KeyCode::PageDown => self.move_by(MAX_ROWS as isize),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.items.len().saturating_sub(1),
            _ => {}
        }
        Ok(None)
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_by(-3),
            MouseEventKind::ScrollDown => self.move_by(3),
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(list) = self.list_area else {
                    return Ok(None);
                };
                let inside = mouse.column >= list.x
                    && mouse.column < list.x + list.width
                    && mouse.row >= list.y
                    && mouse.row < list.y + list.height;
                if inside {
                    let row = self.scroll + (mouse.row - list.y) as usize;
                    // A click on an item moves the cursor there and toggles it.
                    if let Some(Row::Item(index)) = self.rows.get(row).copied() {
                        self.cursor = index;
                        self.toggle(index);
                    }
                }
            }
            _ => {}
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn item(key: &str, group: &str, checked: bool, enabled: bool) -> ChecklistItem {
        ChecklistItem {
            key: key.into(),
            label: key.into(),
            group: group.into(),
            checked,
            enabled,
            note: if enabled {
                String::new()
            } else {
                "locked".into()
            },
        }
    }

    fn key(code: KeyCode) -> termide_core::KeyChord {
        termide_core::KeyChord::identity(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn rows(modal: &mut ChecklistModal) -> Vec<String> {
        let area = Rect::new(0, 0, 50, 16);
        let mut buf = Buffer::empty(area);
        modal.render(area, &mut buf, &Theme::default());
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn items_toggle_under_their_headings_and_apply_together() {
        let mut modal = ChecklistModal::new(
            "Tools",
            "Space toggles",
            vec![
                item("read", "Built-in", true, true),
                item("bash", "Built-in", true, true),
                item("review", "Skills", false, false),
            ],
        );
        let shown = rows(&mut modal);
        assert!(shown.iter().any(|r| r.contains("Built-in")), "{shown:?}");
        assert!(shown.iter().any(|r| r.contains("[x] read")), "{shown:?}");
        assert!(
            shown.iter().any(|r| r.contains("[ ] review — locked")),
            "{shown:?}"
        );
        // Down to `bash`, off; down to the locked skill, which stays off.
        modal.handle_key(key(KeyCode::Down)).unwrap();
        modal.handle_key(key(KeyCode::Char(' '))).unwrap();
        modal.handle_key(key(KeyCode::Down)).unwrap();
        modal.handle_key(key(KeyCode::Char(' '))).unwrap();
        let Some(ModalResult::Confirmed(checked)) = modal.handle_key(key(KeyCode::Enter)).unwrap()
        else {
            panic!("Enter applies");
        };
        assert_eq!(checked, vec!["read".to_string()]);
    }

    #[test]
    fn a_click_toggles_the_item_under_it_and_esc_changes_nothing() {
        let mut modal =
            ChecklistModal::new("Tools", "", vec![item("read", "Built-in", true, true)]);
        let shown = rows(&mut modal);
        let y = shown.iter().position(|r| r.contains("[x] read")).unwrap();
        let row = &shown[y];
        let x = row[..row.find("[x]").unwrap()].chars().count() as u16;
        let y = y as u16;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        modal.handle_mouse(click, Rect::default()).unwrap();
        assert!(modal.checked().is_empty());
        assert!(matches!(
            modal.handle_key(key(KeyCode::Esc)).unwrap(),
            Some(ModalResult::Cancelled)
        ));
    }
}
