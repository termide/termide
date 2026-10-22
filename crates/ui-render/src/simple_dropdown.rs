//! Simple dropdown list overlay for selector widgets.

use ratatui::{
    buffer::Buffer,
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use termide_core::ThemeColors;
use termide_git::RepoRow;
use termide_ui::ScrollBar;

/// The mark of an open directory, and of a closed one: the arrows of the file tree.
const DIR_EXPANDED: &str = "▼";
const DIR_COLLAPSED: &str = if cfg!(windows) { "►" } else { "▶" };

/// Render a bordered dropdown list overlay directly into the buffer.
///
/// The dropdown draws a box at `(x, y)` and lists `rows` from `scroll` on, the row
/// at `cursor` in selection colors and the repository `selected` (an index into
/// the repositories, not the rows) in cursor color. A directory row is drawn dim,
/// with the arrow that says whether it is open, and the tree lines before every
/// row are dim too: the shape of the tree, not things to choose.
///
/// Width auto-fits to the widest row as drawn, clamped to `max_width`. Height is
/// clamped to `max_height` rows. Returns the scroll actually used — `scroll`, moved
/// only as far as it takes to keep the cursor in a window the buffer may have made
/// smaller — for the caller to keep for the next frame and its mouse hit-tests.
#[allow(clippy::too_many_arguments)]
pub fn render_simple_dropdown(
    rows: &[RepoRow],
    selected: usize,
    cursor: usize,
    scroll: usize,
    x: u16,
    y: u16,
    max_width: u16,
    max_height: u16,
    buf: &mut Buffer,
    theme: &ThemeColors,
) -> usize {
    if rows.is_empty() {
        return 0;
    }

    let lines = dropdown_lines(rows);
    // Auto-fit width to the widest row as it will be drawn, plus padding + borders.
    let item_max_width = lines_width(&lines);
    let width = ((item_max_width + 4).min(max_width as usize) as u16)
        .min(buf.area.width)
        .max(2);

    // Clamp the box to the buffer: this function indexes `buf` directly, so a
    // box taller/wider than the terminal would write out of bounds and panic
    // (issue #25). The visible window shrinks to the rows that fit.
    let dropdown_height = ((rows.len().min(max_height as usize) as u16) + 2)
        .min(buf.area.height)
        .max(1);
    let x = x.min(buf.area.width.saturating_sub(width));
    let y = y.min(buf.area.height.saturating_sub(dropdown_height));

    let visible_count = dropdown_height.saturating_sub(2) as usize;
    let scroll_offset = follow_scroll(scroll, cursor, visible_count, rows.len());

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

    for (i, (row, (prefix, text))) in rows
        .iter()
        .zip(&lines)
        .enumerate()
        .skip(scroll_offset)
        .take(visible_count)
        .map(|(i, pair)| (i - scroll_offset, pair))
    {
        let (style, line_style) = dropdown_row_styles(
            row,
            scroll_offset + i == cursor,
            row.repo == Some(selected),
            theme,
        );
        draw_dropdown_row(
            buf,
            x + 1,
            y + 1 + i as u16,
            width - 2,
            (prefix, text),
            style,
            line_style,
        );
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
    scroll_offset
}

/// The scroll that keeps the row at `cursor` in a window of `visible` rows over
/// `total`, moved from `scroll` no further than that takes: the list stays where
/// it is while the cursor walks inside it, and steps one row when the cursor
/// steps past an edge — never jumps back so the cursor sits pinned to the bottom.
pub fn follow_scroll(scroll: usize, cursor: usize, visible: usize, total: usize) -> usize {
    if visible == 0 {
        return 0;
    }
    let mut scroll = scroll.min(total.saturating_sub(visible));
    if cursor < scroll {
        scroll = cursor;
    } else if cursor >= scroll + visible {
        scroll = cursor + 1 - visible;
    }
    scroll
}

/// Each row as drawn: the tree lines before it, then its text — a directory with
/// the arrow that says whether it is open. A caller that draws these rows
/// elsewhere — behind a filter field, say — uses the same function, so the two
/// never drift apart.
pub fn dropdown_lines(rows: &[RepoRow]) -> Vec<(String, String)> {
    termide_git::tree_prefixes(rows)
        .into_iter()
        .zip(rows)
        .map(|(prefix, row)| {
            let text = match row.repo {
                Some(_) => row.label.clone(),
                None => {
                    let mark = if row.expanded {
                        DIR_EXPANDED
                    } else {
                        DIR_COLLAPSED
                    };
                    format!("{mark} {}", row.label)
                }
            };
            (prefix, text)
        })
        .collect()
}

/// The width the widest of [`dropdown_lines`] takes on the screen. A caller that
/// sizes a box around these rows measures them the same way, so the two never
/// disagree.
pub fn dropdown_rows_width(rows: &[RepoRow]) -> usize {
    lines_width(&dropdown_lines(rows))
}

/// Where a dropdown box goes and how wide it is: as wide as its content of
/// `content_width` cells plus borders and padding, never wider than the span
/// `left..right`. It opens at `anchor_x` — under its selector — and moves left
/// only as far as it must to stay inside the span.
pub fn dropdown_span(anchor_x: u16, content_width: usize, left: u16, right: u16) -> (u16, u16) {
    let span = right.saturating_sub(left);
    // +4: two borders and a cell of padding on each side.
    let width = (content_width.saturating_add(4).min(span as usize) as u16).max(2.min(span));
    let x = anchor_x.max(left).min(right.saturating_sub(width));
    (x, width)
}

fn lines_width(lines: &[(String, String)]) -> usize {
    lines
        .iter()
        .map(|(prefix, text)| prefix.width() + text.width())
        .max()
        .unwrap_or(10)
}

/// The style of a row's text and of the tree lines before it. The cursor row is
/// in selection colors whole; otherwise a repository is in the foreground (the
/// chosen one in cursor color), a directory dim, and the lines dim.
pub fn dropdown_row_styles(
    row: &RepoRow,
    is_cursor: bool,
    is_selected: bool,
    theme: &ThemeColors,
) -> (Style, Style) {
    let plain = Style::default().remove_modifier(Modifier::all());
    if is_cursor {
        let style = plain.fg(theme.selection_fg).bg(theme.selection_bg);
        return (style, style);
    }
    let dim = plain.fg(theme.disabled);
    let style = if row.is_selectable() {
        plain.fg(if is_selected { theme.cursor } else { theme.fg })
    } else {
        dim
    };
    (style, dim)
}

/// Draw one row `width` cells wide at `(x, y)`: the line cleared in `style`, the
/// tree lines in `line_style`, the text in `style`, cut to fit by display width.
pub fn draw_dropdown_row(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    (prefix, text): (&str, &str),
    style: Style,
    line_style: Style,
) {
    for dx in 0..width {
        buf[(x + dx, y)].set_symbol(" ").set_style(style);
    }
    let (after_prefix, _) = buf.set_stringn(x, y, prefix, width as usize, line_style);
    let used = after_prefix.saturating_sub(x);
    buf.set_stringn(
        after_prefix,
        y,
        text,
        width.saturating_sub(used) as usize,
        style,
    );
}

#[cfg(test)]
mod overflow_tests {
    use super::{dropdown_rows_width, dropdown_span, follow_scroll, render_simple_dropdown};
    use ratatui::{buffer::Buffer, layout::Rect};
    use termide_core::ThemeColors;
    use termide_git::RepoRow;
    use termide_theme::Theme;

    // The box fits its widest row, not the panel: narrow rows give a narrow box,
    // wide ones grow it to the right and then move it left, never past the span.
    #[test]
    fn the_box_fits_its_widest_row_within_the_span() {
        // Fits where it opens.
        assert_eq!(dropdown_span(10, 6, 10, 50), (10, 10));
        // Too wide to open at the anchor: moved left just enough.
        assert_eq!(dropdown_span(30, 20, 10, 50), (26, 24));
        // Wider than the span: the whole span.
        assert_eq!(dropdown_span(30, 80, 10, 50), (10, 40));
    }

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
        render_simple_dropdown(&rows, 29, 29, 0, 50, 13, 80, 25, &mut buf, &colors);
    }

    // A directory is drawn dim with its arrow, under the lines of the tree, and it
    // takes the cursor without ever being the chosen repository.
    #[test]
    fn a_directory_is_dim_and_the_tree_has_its_lines() {
        let colors = ThemeColors::from(Theme::get_by_name("default"));
        let rows = vec![
            RepoRow::heading("sigma", 0).at("sigma"),
            RepoRow::repo("crm", 1, 0).at("sigma/crm"),
            RepoRow::repo("dcb", 1, 1).at("sigma/dcb"),
        ];
        let mut buf = Buffer::empty(Rect::new(0, 0, 24, 8));
        render_simple_dropdown(&rows, 1, 2, 0, 0, 0, 24, 6, &mut buf, &colors);

        let width = (dropdown_rows_width(&rows) + 4) as u16;
        let row = |y: u16| -> String {
            (1..width - 1)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        };
        assert_eq!(row(1).trim_end(), "▼ sigma");
        assert_eq!(row(2).trim_end(), "├─ crm");
        assert_eq!(row(3).trim_end(), "└─ dcb");
        // The cursor is on `dcb`: its row carries the selection background, while
        // the directory and the lines of the other rows stay dim.
        assert_eq!(buf[(5, 3)].style().bg, Some(colors.selection_bg));
        assert_eq!(buf[(3, 1)].style().fg, Some(colors.disabled));
        assert_eq!(buf[(1, 2)].style().fg, Some(colors.disabled));
        assert_ne!(buf[(4, 2)].style().fg, Some(colors.disabled));
    }

    // The window holds still while the cursor moves inside it and steps a row at a
    // time past its edges: walking back up must not drag the list along.
    #[test]
    fn the_window_moves_only_when_the_cursor_leaves_it() {
        // Down past the bottom: one row at a time.
        assert_eq!(follow_scroll(0, 4, 5, 20), 0);
        assert_eq!(follow_scroll(0, 5, 5, 20), 1);
        assert_eq!(follow_scroll(1, 6, 5, 20), 2);
        // Back up inside the window: it stays.
        assert_eq!(follow_scroll(2, 5, 5, 20), 2);
        assert_eq!(follow_scroll(2, 2, 5, 20), 2);
        // Up past the top: one row at a time.
        assert_eq!(follow_scroll(2, 1, 5, 20), 1);
        // A list that shrank under the window: the window comes back to fit it.
        assert_eq!(follow_scroll(15, 3, 5, 6), 1);
    }
}
