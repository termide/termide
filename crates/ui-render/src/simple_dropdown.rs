//! Simple dropdown list overlay for selector widgets.

use ratatui::{
    buffer::Buffer,
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use termide_core::ThemeColors;
use termide_git::RepoRow;
use termide_ui::ScrollBar;

/// The two characters a row is indented per level of the tree.
const INDENT: &str = "  ";

/// Render a bordered dropdown list overlay directly into the buffer.
///
/// The dropdown draws a box at `(x, y)` and lists `rows`, highlighting the row of
/// the `cursor` repository with selection colors and the row of the `selected` one
/// with cursor color. Both are indices into the repositories, not into the rows: a
/// row that names a directory (`repo: None`) is drawn dim and indented, takes no
/// selection, and the cursor never rests on it — so a caller that moves the cursor
/// over its repositories keeps doing exactly that.
///
/// Width auto-fits to the widest row as drawn (indent and heading mark included),
/// clamped to `max_width`. Height is clamped to `max_height` rows; the list
/// scrolls to keep the cursor's row visible.
#[allow(clippy::too_many_arguments)]
pub fn render_simple_dropdown(
    rows: &[RepoRow],
    selected: usize,
    cursor: usize,
    x: u16,
    y: u16,
    max_width: u16,
    max_height: u16,
    buf: &mut Buffer,
    theme: &ThemeColors,
) {
    if rows.is_empty() {
        return;
    }

    // Auto-fit width to the widest row as it will be drawn, plus padding + borders.
    let item_max_width = rows.iter().map(dropdown_row_width).max().unwrap_or(10);
    let width = ((item_max_width + 4).min(max_width as usize) as u16)
        .min(buf.area.width)
        .max(1);

    // Clamp the box to the buffer: this function indexes `buf` directly, so a
    // box taller/wider than the terminal would write out of bounds and panic
    // (issue #25). The visible window shrinks to the rows that fit.
    let dropdown_height = ((rows.len().min(max_height as usize) as u16) + 2)
        .min(buf.area.height)
        .max(1);
    let x = x.min(buf.area.width.saturating_sub(width));
    let y = y.min(buf.area.height.saturating_sub(dropdown_height));

    let visible_count = dropdown_height.saturating_sub(2) as usize;
    // The cursor is a repository, and the rows hold headings between them: scroll
    // by the row the cursor rests on, not by its index in the repository list.
    let cursor_row = rows
        .iter()
        .position(|row| row.repo == Some(cursor))
        .unwrap_or(0);
    let scroll_offset = if visible_count > 0 && cursor_row >= visible_count {
        cursor_row - visible_count + 1
    } else {
        0
    };

    let border_style = Style::default().fg(theme.border_focused);
    let bg_style = Style::default()
        .bg(theme.bg)
        .remove_modifier(Modifier::all());

    // Clear area and draw border
    for dy in 0..dropdown_height {
        for dx in 0..width {
            let cell = &mut buf[(x + dx, y + dy)];
            cell.set_style(bg_style);
            if dy == 0 || dy == dropdown_height - 1 {
                if dx == 0 {
                    cell.set_symbol(if dy == 0 { "┌" } else { "└" });
                } else if dx == width - 1 {
                    cell.set_symbol(if dy == 0 { "┐" } else { "┘" });
                } else {
                    cell.set_symbol("─");
                }
                cell.set_style(border_style);
            } else if dx == 0 || dx == width - 1 {
                cell.set_symbol("│").set_style(border_style);
            } else {
                cell.set_symbol(" ");
            }
        }
    }

    // Draw rows
    for (i, row) in rows
        .iter()
        .skip(scroll_offset)
        .take(visible_count)
        .enumerate()
    {
        let item_y = y + 1 + i as u16;
        let is_cursor = row.repo == Some(cursor);
        let is_selected = row.repo == Some(selected);

        let style = if is_cursor {
            Style::default()
                .fg(theme.selection_fg)
                .bg(theme.selection_bg)
                .remove_modifier(Modifier::all())
        } else if row.is_selectable() {
            Style::default()
                .fg(if is_selected { theme.cursor } else { theme.fg })
                .remove_modifier(Modifier::all())
        } else {
            // A directory: the shape of the tree, not a thing to choose.
            Style::default()
                .fg(theme.disabled)
                .remove_modifier(Modifier::all())
        };

        let text = dropdown_row_label(row);
        // Truncate to fit inside borders, by display width.
        let max_item_width = (width - 2) as usize;
        let display: std::borrow::Cow<str> = if text.width() > max_item_width {
            let mut end = 0;
            let mut w = 0;
            for ch in text.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if w + cw > max_item_width {
                    break;
                }
                w += cw;
                end += ch.len_utf8();
            }
            std::borrow::Cow::Borrowed(&text[..end])
        } else {
            std::borrow::Cow::Borrowed(&text)
        };

        // Clear line and draw the row
        for dx in 1..width - 1 {
            buf[(x + dx, item_y)].set_symbol(" ").set_style(style);
        }
        buf.set_string(x + 1, item_y, display, style);
    }

    // Scrollbar on the right border when the list overflows the visible window.
    ScrollBar::render(
        buf,
        x + width - 1,
        y + 1,
        visible_count as u16,
        scroll_offset,
        visible_count,
        rows.len(),
        theme,
        true,
    );
}

/// The text of a row: a directory with its mark, a repository indented under it.
/// A caller that draws these rows elsewhere — behind a filter field, say — uses the
/// same function, so the two never drift apart.
pub fn dropdown_row_label(row: &RepoRow) -> String {
    let indent = INDENT.repeat(row.depth);
    match row.repo {
        Some(_) => format!("{indent}{}", row.label),
        None => format!("{indent}▾ {}", row.label),
    }
}

/// The width [`dropdown_row_label`] takes on the screen. A caller that sizes a box
/// around these rows measures them the same way, so the two never disagree.
pub fn dropdown_row_width(row: &RepoRow) -> usize {
    dropdown_row_label(row).width()
}

#[cfg(test)]
mod overflow_tests {
    use super::{dropdown_row_width, render_simple_dropdown};
    use ratatui::{buffer::Buffer, layout::Rect};
    use termide_core::ThemeColors;
    use termide_git::RepoRow;
    use termide_theme::Theme;

    // Regression for #25: a simple dropdown taller/wider than the terminal must
    // not write out of bounds (this function indexes the buffer directly).
    #[test]
    fn render_does_not_overflow_short_terminal() {
        let colors = ThemeColors::from(Theme::get_by_name("default"));
        let rows: Vec<RepoRow> = (0..30)
            .map(|i| RepoRow::repo(format!("item-{i:02}"), 0, i))
            .collect();
        let mut buf = Buffer::empty(Rect::new(0, 0, 57, 15));
        // max_height larger than the buffer, anchored near the bottom-right.
        render_simple_dropdown(&rows, 29, 29, 50, 13, 80, 25, &mut buf, &colors);
    }

    // Headings take no selection and the cursor rests on repositories alone: a
    // row between them must never be drawn as the chosen one.
    #[test]
    fn a_heading_takes_neither_the_cursor_nor_the_selection() {
        let colors = ThemeColors::from(Theme::get_by_name("default"));
        let rows = vec![
            RepoRow::heading("sigma", 0),
            RepoRow::repo("crm", 1, 0),
            RepoRow::repo("dcb", 1, 1),
        ];
        let mut buf = Buffer::empty(Rect::new(0, 0, 24, 8));
        render_simple_dropdown(&rows, 1, 1, 0, 0, 24, 6, &mut buf, &colors);

        // The box fits the widest row as drawn, plus borders and padding.
        let width = (rows.iter().map(dropdown_row_width).max().unwrap_or(0) + 4) as u16;
        // One row per entry below the top border: the heading, then the two
        // repositories indented under it.
        let row = |y: u16| -> String {
            (1..width - 1)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        };
        assert_eq!(row(1).trim_end(), "▾ sigma");
        assert_eq!(row(2).trim_end(), "  crm");
        assert_eq!(row(3).trim_end(), "  dcb");
        // The cursor is on `dcb`: its row carries the selection background, while
        // the heading stays dim and unselected.
        assert_eq!(buf[(4, 3)].style().bg, Some(colors.selection_bg));
        assert_eq!(buf[(2, 1)].style().fg, Some(colors.disabled));
        assert_ne!(buf[(2, 1)].style().bg, Some(colors.selection_bg));
    }
}
