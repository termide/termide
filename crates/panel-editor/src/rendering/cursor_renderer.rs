//! Cursor rendering utilities.
//!
//! This module provides functions for rendering the cursor with proper visual feedback.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

use termide_theme::Theme;

/// Render cursor by inverting cell colors at the given position.
///
/// Swaps foreground and background colors with fallback to theme colors.
/// Adds BOLD modifier for better visibility.
pub fn render_cursor_at(buf: &mut Buffer, x: u16, y: u16, area: Rect, theme: &Theme) {
    if x < area.x + area.width && y < area.y + area.height {
        if let Some(cell) = buf.cell_mut((x, y)) {
            invert_cell_colors(cell, theme);
        }
    }
}

/// Invert cell colors for cursor display.
///
/// Swaps foreground and background colors, using theme defaults for Reset colors.
/// Adds BOLD modifier for better cursor visibility.
fn invert_cell_colors(cell: &mut ratatui::buffer::Cell, theme: &Theme) {
    // Get current colors with fallback to theme defaults
    let current_fg = match cell.fg {
        Color::Reset => theme.fg,
        color => color,
    };
    let current_bg = match cell.bg {
        Color::Reset => theme.bg,
        color => color,
    };

    // Swap colors and add BOLD modifier
    cell.set_style(
        Style::default()
            .bg(current_fg)
            .fg(current_bg)
            .add_modifier(Modifier::BOLD),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn create_test_theme() -> Theme {
        Theme {
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
    fn test_invert_cell_colors_with_reset() {
        let theme = create_test_theme();
        let mut cell = ratatui::buffer::Cell::default();
        cell.set_style(Style::default().fg(Color::Reset).bg(Color::Reset));

        invert_cell_colors(&mut cell, &theme);

        assert_eq!(cell.fg, Color::Black); // Inverted from theme.bg
        assert_eq!(cell.bg, Color::White); // Inverted from theme.fg
        assert!(cell.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn test_invert_cell_colors_with_custom_colors() {
        let theme = create_test_theme();
        let mut cell = ratatui::buffer::Cell::default();
        cell.set_style(Style::default().fg(Color::Red).bg(Color::Blue));

        invert_cell_colors(&mut cell, &theme);

        assert_eq!(cell.fg, Color::Blue); // Swapped
        assert_eq!(cell.bg, Color::Red); // Swapped
        assert!(cell.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn test_render_cursor_at_within_bounds() {
        let theme = create_test_theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 10));
        let area = Rect::new(0, 0, 10, 10);

        // Set initial colors
        if let Some(cell) = buf.cell_mut((5, 5)) {
            cell.set_style(Style::default().fg(Color::Red).bg(Color::Blue));
        }

        render_cursor_at(&mut buf, 5, 5, area, &theme);

        // Verify colors were inverted
        if let Some(cell) = buf.cell((5, 5)) {
            assert_eq!(cell.fg, Color::Blue);
            assert_eq!(cell.bg, Color::Red);
            assert!(cell.modifier.contains(Modifier::BOLD));
        }
    }

    #[test]
    fn test_render_cursor_at_out_of_bounds() {
        let theme = create_test_theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 10));
        let area = Rect::new(0, 0, 10, 10);

        // This should not panic even if coordinates are out of area bounds
        render_cursor_at(&mut buf, 20, 20, area, &theme);
    }
}
