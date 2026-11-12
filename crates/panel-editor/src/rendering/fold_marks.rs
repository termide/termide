//! The marks of a fold on its header line: `▸` in the gutter's last cell, a
//! click on which unfolds it, and after the text how many lines it hides.

use ratatui::{buffer::Buffer, layout::Rect, style::Style};

use termide_theme::Theme;

/// Mark the header of a fold hiding `hidden` lines, drawn on screen `row`;
/// its text ends `text_end` columns into the content area.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_fold_marks(
    buf: &mut Buffer,
    area: Rect,
    row: usize,
    line_number_width: u16,
    content_width: usize,
    text_end: usize,
    hidden: usize,
    theme: &Theme,
) {
    let y = area.y + row as u16;
    if y >= area.y + area.height || line_number_width == 0 {
        return;
    }
    let style = Style::default().fg(theme.disabled);
    if let Some(cell) = buf.cell_mut((area.x + line_number_width - 1, y)) {
        cell.set_char('▸');
        cell.set_style(style);
    }
    // One column apart from the text, and only what fits.
    let start = text_end + 1;
    if start >= content_width {
        return;
    }
    let label = termide_i18n::t().editor_folded_lines(hidden);
    buf.set_stringn(
        area.x + line_number_width + start as u16,
        y,
        label,
        content_width - start,
        style,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drawn(text_end: usize, content_width: usize) -> String {
        let area = Rect::new(0, 0, 4 + content_width as u16, 1);
        let mut buf = Buffer::empty(area);
        render_fold_marks(
            &mut buf,
            area,
            0,
            4,
            content_width,
            text_end,
            12,
            &Theme::default(),
        );
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn the_header_shows_the_arrow_and_the_count() {
        assert_eq!(drawn(3, 20), "   ▸    ⋯ 12 lines      ");
    }

    #[test]
    fn the_count_is_cut_to_the_width() {
        assert_eq!(drawn(3, 8), "   ▸    ⋯ 12");
        assert_eq!(drawn(9, 8), "   ▸        ");
    }
}
