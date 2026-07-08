//! A small scrollable dropdown list, shared by the database and table
//! selectors so both behave and render identically.

use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use termide_core::ThemeColors;

/// Outcome of a key press while the dropdown is open.
pub(crate) enum DropdownKey {
    /// Cursor moved / nothing else.
    Nav,
    /// An item was chosen; carries its index.
    Pick(usize),
    /// The dropdown was dismissed.
    Closed,
    /// Key not handled by the dropdown.
    Unhandled,
}

/// Open/scroll state for one selector's list.
#[derive(Default)]
pub(crate) struct Dropdown {
    pub open: bool,
    pub cursor: usize,
    pub scroll: usize,
    /// Visible row count (set during render; used for PageUp/Down + scrolling).
    pub page_size: usize,
}

impl Dropdown {
    pub fn open_at(&mut self, index: usize) {
        self.open = true;
        self.cursor = index;
    }

    /// Handle a key while the list is open.
    pub fn handle_key(&mut self, code: KeyCode, len: usize) -> DropdownKey {
        match code {
            KeyCode::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                DropdownKey::Nav
            }
            KeyCode::Down => {
                if self.cursor + 1 < len {
                    self.cursor += 1;
                }
                DropdownKey::Nav
            }
            KeyCode::PageUp => {
                self.cursor = self.cursor.saturating_sub(self.page_size.max(1));
                DropdownKey::Nav
            }
            KeyCode::PageDown => {
                self.cursor = (self.cursor + self.page_size.max(1)).min(len.saturating_sub(1));
                DropdownKey::Nav
            }
            KeyCode::Home => {
                self.cursor = 0;
                DropdownKey::Nav
            }
            KeyCode::End => {
                self.cursor = len.saturating_sub(1);
                DropdownKey::Nav
            }
            KeyCode::Enter => {
                self.open = false;
                DropdownKey::Pick(self.cursor)
            }
            KeyCode::Esc => {
                self.open = false;
                DropdownKey::Closed
            }
            _ => DropdownKey::Unhandled,
        }
    }

    /// Map a clicked screen row to a list index (None if outside the list rows).
    /// `list_top` is the first item row (inside the top border).
    pub fn index_at_row(&self, clicked: u16, list_top: u16) -> Option<usize> {
        if clicked < list_top || clicked >= list_top + self.page_size as u16 {
            return None;
        }
        Some(self.scroll + (clicked - list_top) as usize)
    }

    /// Render the list as a bordered, scrollable overlay anchored under `area`'s
    /// top row. Width adapts to the longest item; the window follows the cursor.
    /// The border matches the shared selector dropdowns (e.g. the git branch
    /// picker) via `theme.border_focused`.
    pub fn render(&mut self, buf: &mut Buffer, area: Rect, items: &[String], theme: &ThemeColors) {
        if items.is_empty() {
            return;
        }
        let base = Style::default().fg(theme.fg).bg(theme.bg);
        let border = Style::default().fg(theme.border_focused).bg(theme.bg);

        // Rows below the selector row available for the box (top + items + bottom).
        let avail = area.height.saturating_sub(1);
        let visible = (avail.saturating_sub(2) as usize).max(1).min(items.len());
        self.page_size = visible;

        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + visible {
            self.scroll = self.cursor + 1 - visible;
        }
        let start = self.scroll.min(items.len().saturating_sub(1));
        let end = (start + visible).min(items.len());

        let longest = items
            .iter()
            .map(|n| UnicodeWidthStr::width(n.as_str()))
            .max()
            .unwrap_or(0);
        // +1 leading space of padding, +2 for the side borders.
        let width = ((longest + 3) as u16).clamp(4, area.width);
        let box_h = ((visible as u16) + 2).min(avail).max(2);
        let y_top = area.y + 1;

        // Draw the border box (top/bottom rules, side rails, blank interior).
        for dy in 0..box_h {
            let y = y_top + dy;
            for dx in 0..width {
                let cell = &mut buf[(area.x + dx, y)];
                if dy == 0 || dy == box_h - 1 {
                    let sym = if dx == 0 {
                        if dy == 0 {
                            "┌"
                        } else {
                            "└"
                        }
                    } else if dx == width - 1 {
                        if dy == 0 {
                            "┐"
                        } else {
                            "┘"
                        }
                    } else {
                        "─"
                    };
                    cell.set_symbol(sym).set_style(border);
                } else if dx == 0 || dx == width - 1 {
                    cell.set_symbol("│").set_style(border);
                } else {
                    cell.set_symbol(" ").set_style(base);
                }
            }
        }

        // Draw items inside the box (first item row is just below the top border).
        let inner = width.saturating_sub(2) as usize;
        for (row, i) in (start..end).enumerate() {
            let y = y_top + 1 + row as u16;
            if y >= y_top + box_h - 1 {
                break;
            }
            let style = if i == self.cursor {
                Style::default()
                    .fg(theme.selection_fg)
                    .bg(theme.selection_bg)
            } else {
                base
            };
            let blanks = " ".repeat(inner);
            buf.set_stringn(area.x + 1, y, &blanks, inner, style);
            buf.set_stringn(area.x + 1, y, &items[i], inner, style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn dropdown_is_bordered() {
        // Regression: the table/database dropdown must draw a border like the
        // shared selector dropdowns (e.g. the git branch picker).
        let theme = ThemeColors::default();
        let items: Vec<String> = vec!["users".into(), "orders".into(), "products".into()];
        let mut dd = Dropdown::default();
        dd.open_at(0);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        dd.render(&mut buf, area, &items, &theme);

        // Box starts one row below the selector row.
        assert_eq!(buf[(0, 1)].symbol(), "┌", "top-left corner");
        assert_eq!(buf[(0, 2)].symbol(), "│", "left rail on the first item row");
        // The first item text sits inside the border.
        let row: String = (1..8).map(|x| buf[(x, 2)].symbol().to_string()).collect();
        assert!(row.contains("users"), "item text inside border: {row:?}");
    }

    #[test]
    fn index_at_row_maps_rows_below_the_top_border() {
        let dd = Dropdown {
            page_size: 3,
            ..Default::default()
        };
        // `list_top` is the first item row (just inside the top border).
        assert_eq!(dd.index_at_row(2, 2), Some(0));
        assert_eq!(dd.index_at_row(4, 2), Some(2));
        // The top border row and rows past the visible window are not items.
        assert_eq!(dd.index_at_row(1, 2), None);
        assert_eq!(dd.index_at_row(5, 2), None);
    }
}
