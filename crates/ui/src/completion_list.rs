//! A list of completions for whatever is being typed — `/commands` in the
//! agent panel, paths in the open prompt — drawn just above or just below
//! the input it completes. The list owns the selection and the keys that move it; the
//! caller decides what typing does and what accepting means, so the same
//! widget serves any text source.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use termide_core::ThemeColors;

use crate::fuzzy::highlight;

/// One completion: the `value` the caller inserts, and what the row shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub value: String,
    /// Shown first on the row; the value itself when empty.
    pub label: String,
    /// Shown after the label, dimmer: what comes next (`<path>`).
    pub hint: String,
    /// Shown after a gap: what the completion is.
    pub description: String,
    /// Grapheme indices of the label to highlight: what the query matched.
    pub matched: Vec<usize>,
}

impl CompletionItem {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: String::new(),
            hint: String::new(),
            description: String::new(),
            matched: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Highlight these grapheme indices of the label (from
    /// [`crate::fuzzy::Query::positions`]).
    #[must_use]
    pub fn with_matched(mut self, matched: Vec<usize>) -> Self {
        self.matched = matched;
        self
    }
}

/// What a key did to the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionAction {
    /// The selection moved.
    Handled,
    /// `Tab` or `Enter`: take the selected item.
    Accept,
    /// `Esc`: close the list, leave the input alone.
    Dismiss,
    /// Not a list key; the caller handles it (typing, most of all).
    NotHandled,
}

pub struct CompletionList {
    items: Vec<CompletionItem>,
    selected: usize,
    max_rows: usize,
    /// Rows hang from the top of the area (below an input) instead of
    /// standing on its bottom (above one).
    top: bool,
    /// Where the last render put the rows, and which item the first row was.
    drawn: Option<(Rect, usize)>,
}

impl CompletionList {
    pub const DEFAULT_MAX_ROWS: usize = 6;

    #[must_use]
    pub fn new(items: Vec<CompletionItem>) -> Self {
        Self {
            items,
            selected: 0,
            max_rows: Self::DEFAULT_MAX_ROWS,
            top: false,
            drawn: None,
        }
    }

    #[must_use]
    pub fn with_max_rows(mut self, max_rows: usize) -> Self {
        self.max_rows = max_rows.max(1);
        self
    }

    /// Draw from the top of the area down, for a list below its input.
    #[must_use]
    pub fn below_input(mut self) -> Self {
        self.top = true;
        self
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn items(&self) -> &[CompletionItem] {
        &self.items
    }

    /// Replace the items as the typed prefix changes; the selection stays
    /// where it can.
    pub fn set_items(&mut self, items: Vec<CompletionItem>) {
        self.items = items;
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
    }

    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    #[must_use]
    pub fn selected_item(&self) -> Option<&CompletionItem> {
        self.items.get(self.selected)
    }

    pub fn select_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn select_down(&mut self) {
        self.selected = (self.selected + 1).min(self.items.len().saturating_sub(1));
    }

    /// Select `index` (a clicked row); `false` when out of range.
    pub fn select(&mut self, index: usize) -> bool {
        if index < self.items.len() {
            self.selected = index;
            true
        } else {
            false
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> CompletionAction {
        match key.code {
            KeyCode::Up => {
                self.select_up();
                CompletionAction::Handled
            }
            KeyCode::Down => {
                self.select_down();
                CompletionAction::Handled
            }
            KeyCode::Tab | KeyCode::Enter => CompletionAction::Accept,
            KeyCode::Esc => CompletionAction::Dismiss,
            _ => CompletionAction::NotHandled,
        }
    }

    /// Rows the list wants, within `available`.
    #[must_use]
    pub fn rows(&self, available: u16) -> u16 {
        self.items.len().min(self.max_rows).min(available as usize) as u16
    }

    /// Draw the list at the bottom of `area` (the region ending right above
    /// the input), or at its top for a list [below the input](Self::below_input),
    /// the selected row in the selection colours and kept in view. Returns
    /// the rows painted.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, colors: &ThemeColors) -> Rect {
        let rows = self.rows(area.height) as usize;
        if rows == 0 || area.width == 0 {
            self.drawn = None;
            return Rect::new(area.x, area.y + area.height, area.width, 0);
        }
        let first = self
            .selected
            .saturating_sub(rows - 1)
            .min(self.items.len() - rows);
        let top = if self.top {
            area.y
        } else {
            area.y + area.height - rows as u16
        };
        let width = area.width as usize;
        for (row, item) in self.items.iter().skip(first).take(rows).enumerate() {
            let selected = first + row == self.selected;
            let style = if selected {
                Style::default()
                    .fg(colors.selection_fg)
                    .bg(colors.selection_bg)
            } else {
                Style::default().fg(colors.fg).bg(colors.bg)
            };
            let dim = if selected {
                style
            } else {
                Style::default().fg(colors.disabled).bg(colors.bg)
            };
            let y = top + row as u16;
            buf.set_string(area.x, y, " ".repeat(width), style);
            let label = if item.label.is_empty() {
                &item.value
            } else {
                &item.label
            };
            let mut x = area.x + 1;
            let end = area.x + area.width;
            let hit = style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            for span in highlight(label, &item.matched, style, hit) {
                x = put(buf, x, end, y, &span.content, span.style);
            }
            if !item.hint.is_empty() {
                x = put(buf, x, end, y, " ", style);
                x = put(buf, x, end, y, &item.hint, dim);
            }
            if !item.description.is_empty() {
                x = put(buf, x, end, y, "  ", style);
                put(buf, x, end, y, &item.description, dim);
            }
        }
        let drawn = Rect::new(area.x, top, area.width, rows as u16);
        self.drawn = Some((drawn, first));
        drawn
    }

    /// The item under a click at `(x, y)`, from the last render.
    #[must_use]
    pub fn hit(&self, x: u16, y: u16) -> Option<usize> {
        let (rect, first) = self.drawn?;
        let inside =
            x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height;
        if !inside {
            return None;
        }
        let index = first + (y - rect.y) as usize;
        (index < self.items.len()).then_some(index)
    }
}

/// Write `text` at `x`, clipped at `end`; returns where the next piece goes.
fn put(buf: &mut Buffer, x: u16, end: u16, y: u16, text: &str, style: Style) -> u16 {
    if x >= end {
        return end;
    }
    let width = (end - x) as usize;
    buf.set_stringn(x, y, text, width, style);
    let used = unicode_width::UnicodeWidthStr::width(text).min(width) as u16;
    x + used
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<CompletionItem> {
        ["review", "tests", "release"]
            .iter()
            .map(|name| {
                CompletionItem::new(*name)
                    .with_label(format!("/{name}"))
                    .with_hint("<path>")
                    .with_description(format!("{name} things"))
            })
            .collect()
    }

    #[test]
    fn keys_move_accept_and_dismiss() {
        let mut list = CompletionList::new(items());
        assert_eq!(
            list.handle_key(KeyEvent::from(KeyCode::Down)),
            CompletionAction::Handled
        );
        assert_eq!(list.selected_item().unwrap().value, "tests");
        list.select_down();
        list.select_down();
        assert_eq!(list.selected(), 2, "clamped at the end");
        assert_eq!(
            list.handle_key(KeyEvent::from(KeyCode::Tab)),
            CompletionAction::Accept
        );
        assert_eq!(
            list.handle_key(KeyEvent::from(KeyCode::Enter)),
            CompletionAction::Accept
        );
        assert_eq!(
            list.handle_key(KeyEvent::from(KeyCode::Esc)),
            CompletionAction::Dismiss
        );
        assert_eq!(
            list.handle_key(KeyEvent::from(KeyCode::Char('x'))),
            CompletionAction::NotHandled
        );
        list.set_items(items()[..1].to_vec());
        assert_eq!(list.selected(), 0, "selection clamped to the new items");
        assert!(!list.select(5));
    }

    #[test]
    fn renders_at_the_bottom_and_maps_clicks_back() {
        let mut list = CompletionList::new(items()).with_max_rows(2);
        list.select(2);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let drawn = list.render(area, &mut buf, &ThemeColors::default());
        assert_eq!(
            (drawn.y, drawn.height),
            (8, 2),
            "two rows, anchored at the bottom"
        );
        let row =
            |y: u16| -> String { (0..40).map(|x| buf[(x, y)].symbol().to_string()).collect() };
        assert!(row(8).contains("/tests <path>  tests things"), "{}", row(8));
        assert!(row(9).contains("/release"), "selected item kept in view");
        assert_eq!(list.hit(3, 9), Some(2));
        assert_eq!(list.hit(3, 8), Some(1));
        assert_eq!(list.hit(3, 7), None);
        assert_eq!(list.rows(1), 1);
        assert_eq!(list.rows(0), 0);
    }

    #[test]
    fn below_input_hangs_from_the_top_and_highlights_matches() {
        let items = vec![CompletionItem::new("src/main.rs").with_matched(vec![4, 5])];
        let mut list = CompletionList::new(items).below_input();
        let area = Rect::new(0, 3, 30, 5);
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let drawn = list.render(area, &mut buf, &ThemeColors::default());
        assert_eq!((drawn.y, drawn.height), (3, 1));
        let styled = |x: u16| buf[(x, 3)].modifier.contains(Modifier::UNDERLINED);
        assert_eq!(buf[(5, 3)].symbol(), "m");
        assert!(styled(5) && styled(6), "the matched `ma` is highlighted");
        assert!(!styled(4) && !styled(7));
        assert_eq!(list.hit(2, 3), Some(0));
    }
}
