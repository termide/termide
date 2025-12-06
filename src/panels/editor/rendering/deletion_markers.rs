//! Git deletion marker rendering.
//!
//! This module provides functions for rendering virtual lines that indicate deleted
//! content in git diff mode.

use ratatui::{buffer::Buffer, layout::Rect, style::Style};

/// Render a deletion marker virtual line.
///
/// Displays a visual indicator showing where lines were deleted according to git diff.
/// The marker includes:
/// - Gutter: 4 spaces + red ▶ marker + space
/// - Content: horizontal line (─) with centered deletion count text
pub fn render_deletion_marker(
    buf: &mut Buffer,
    area: Rect,
    row: usize,
    deletion_count: usize,
    theme: &crate::theme::Theme,
    content_width: usize,
    line_number_width: u16,
) {
    // Render gutter (line number area)
    render_deletion_gutter(buf, area, row, theme);

    // Render content area (horizontal line with text)
    render_deletion_content(
        buf,
        area,
        row,
        deletion_count,
        theme,
        content_width,
        line_number_width,
    );
}

/// Render the gutter portion of deletion marker.
///
/// Format: "    ▶ " (4 spaces + red marker + space)
fn render_deletion_gutter(buf: &mut Buffer, area: Rect, row: usize, theme: &crate::theme::Theme) {
    let y = area.y + row as u16;

    // Empty space for line number (4 spaces)
    for i in 0..4 {
        let x = area.x + i as u16;
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char(' ');
            cell.set_style(Style::default().fg(theme.disabled));
        }
    }

    // Red marker ▶ (shows deletion occurred here)
    let marker_style = Style::default().fg(theme.error);
    let x = area.x + 4; // Position after spaces
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char('▶');
        cell.set_style(marker_style);
    }

    // Empty space after marker
    let x = area.x + 5;
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(' ');
        cell.set_style(Style::default().fg(theme.disabled));
    }
}

/// Render the content portion of deletion marker.
///
/// Displays a horizontal line (─) with centered text indicating deletion count.
fn render_deletion_content(
    buf: &mut Buffer,
    area: Rect,
    row: usize,
    deletion_count: usize,
    theme: &crate::theme::Theme,
    content_width: usize,
    line_number_width: u16,
) {
    let y = area.y + row as u16;
    let line_style = Style::default().fg(theme.disabled);

    // Format deletion text
    let deletion_text = format!(
        " {} ",
        crate::i18n::t().editor_deletion_marker(deletion_count)
    );

    // Performance optimization: Convert to Vec<char> once for O(1) indexing
    let deletion_chars: Vec<char> = deletion_text.chars().collect();
    let text_len = deletion_chars.len();

    // Calculate position to center text
    let text_start_col = if content_width > text_len {
        (content_width - text_len) / 2
    } else {
        0
    };

    // Render content area
    for col in 0..content_width {
        let x = area.x + line_number_width + col as u16;
        if x < area.x + area.width && y < area.y + area.height {
            if let Some(cell) = buf.cell_mut((x, y)) {
                // Check if this position is in the text area
                if col >= text_start_col && col < text_start_col + text_len {
                    // Render character from text (O(1) indexing)
                    let text_idx = col - text_start_col;
                    let ch = deletion_chars.get(text_idx).copied().unwrap_or('─');
                    cell.set_char(ch);
                } else {
                    // Render line
                    cell.set_char('─');
                }
                cell.set_style(line_style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn create_test_theme() -> crate::theme::Theme {
        crate::theme::Theme {
            name: "test",
            bg: Color::Black,
            fg: Color::White,
            selected_bg: Color::Blue,
            selected_fg: Color::White,
            accented_bg: Color::DarkGray,
            accented_fg: Color::Cyan,
            disabled: Color::Gray,
            error: Color::Red,
            success: Color::Green,
            warning: Color::Yellow,
        }
    }

    #[test]
    fn test_render_deletion_gutter() {
        let theme = create_test_theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        let area = Rect::new(0, 0, 20, 10);

        render_deletion_gutter(&mut buf, area, 2, &theme);

        // Check spaces (positions 0-3)
        for i in 0..4 {
            if let Some(cell) = buf.cell((i, 2)) {
                assert_eq!(cell.symbol(), " ");
                assert_eq!(cell.fg, Color::Gray); // disabled color
            }
        }

        // Check marker (position 4)
        if let Some(cell) = buf.cell((4, 2)) {
            assert_eq!(cell.symbol(), "▶");
            assert_eq!(cell.fg, Color::Red); // error color
        }

        // Check space after marker (position 5)
        if let Some(cell) = buf.cell((5, 2)) {
            assert_eq!(cell.symbol(), " ");
        }
    }

    #[test]
    fn test_render_deletion_content() {
        // Initialize i18n for translation functions
        crate::i18n::init_with_language("en");

        let theme = create_test_theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let area = Rect::new(0, 0, 30, 10);
        let content_width = 20;
        let line_number_width = 6;

        render_deletion_content(
            &mut buf,
            area,
            3,
            5,
            &theme,
            content_width,
            line_number_width,
        );

        // Verify line characters are rendered (exact positions depend on text centering)
        // At least check that some cells have the line character
        let mut has_line_char = false;
        for col in 0..content_width {
            let x = line_number_width + col as u16;
            if let Some(cell) = buf.cell((x, 3)) {
                if cell.symbol() == "─" {
                    has_line_char = true;
                    assert_eq!(cell.fg, Color::Gray); // disabled color
                }
            }
        }
        assert!(has_line_char, "Should have at least one line character");
    }

    #[test]
    fn test_render_deletion_marker_full() {
        // Initialize i18n for translation functions
        crate::i18n::init_with_language("en");

        let theme = create_test_theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let area = Rect::new(0, 0, 30, 10);

        render_deletion_marker(&mut buf, area, 1, 3, &theme, 20, 6);

        // Verify marker is present
        if let Some(cell) = buf.cell((4, 1)) {
            assert_eq!(cell.symbol(), "▶");
            assert_eq!(cell.fg, Color::Red);
        }

        // Verify content area has styling
        if let Some(cell) = buf.cell((10, 1)) {
            assert_eq!(cell.fg, Color::Gray);
        }
    }
}
