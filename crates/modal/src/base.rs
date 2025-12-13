//! Common modal rendering utilities.
//!
//! Provides shared functionality for modal windows:
//! - Frame rendering with [X] close button
//! - Input field rendering with cursor
//! - Common positioning utilities

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Clear, Widget},
};
use termide_theme::Theme;

/// Calculate modal position at top-center of screen.
pub fn top_center_rect(width: u16, height: u16, r: Rect) -> Rect {
    let x = r.x + (r.width.saturating_sub(width)) / 2;
    let y = r.y + 1; // Small offset from top
    Rect::new(x, y, width.min(r.width), height.min(r.height))
}

/// Render modal frame with [X] close button.
///
/// Returns (inner_area, close_button_area).
pub fn render_modal_frame(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    title: &str,
) -> (Rect, Rect) {
    // Clear area
    Clear.render(area, buf);

    // Create block with [X] close button on the left
    let title_with_close = format!(" [X] {} ", title);
    let block = Block::default()
        .title(Span::styled(
            title_with_close,
            Style::default().fg(theme.bg).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.bg))
        .style(Style::default().bg(theme.fg));

    // Calculate close button area (the [X] at the beginning of title)
    let close_x = area.x + 1; // Position after space: " [X]"
    let close_button_area = Rect {
        x: close_x,
        y: area.y,
        width: 3,
        height: 1,
    };

    let inner = block.inner(area);
    block.render(area, buf);

    (inner, close_button_area)
}

/// Render a text input field with cursor.
pub fn render_input_field(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    text: &str,
    is_focused: bool,
    theme: &Theme,
) {
    let input_style = if is_focused {
        Style::default().fg(theme.fg).bg(theme.bg)
    } else {
        Style::default().fg(theme.bg)
    };

    // Calculate visible text (scroll if text is longer than width)
    let visible_text = if text.len() as u16 > width {
        let start = text.len().saturating_sub(width as usize);
        &text[start..]
    } else {
        text
    };

    buf.set_string(x, y, visible_text, input_style);

    // Draw cursor if focused
    if is_focused {
        let cursor_screen_pos = x + (visible_text.len() as u16).min(width.saturating_sub(1));
        if cursor_screen_pos < x + width {
            buf[(cursor_screen_pos, y)].set_style(
                Style::default()
                    .bg(theme.bg)
                    .fg(theme.fg)
                    .add_modifier(Modifier::REVERSED),
            );
        }
    }
}

/// Render a labeled input field.
pub fn render_labeled_input(
    buf: &mut Buffer,
    area: Rect,
    label: &str,
    text: &str,
    is_focused: bool,
    theme: &Theme,
) {
    let label_width = label.len() as u16;

    // Render label
    buf.set_string(area.x, area.y, label, Style::default().fg(theme.bg));

    // Render input field
    let input_x = area.x + label_width;
    let input_width = area.width.saturating_sub(label_width);

    render_input_field(buf, input_x, area.y, input_width, text, is_focused, theme);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_top_center_rect_centers_horizontally() {
        let container = Rect::new(0, 0, 100, 50);
        let result = top_center_rect(40, 10, container);

        // Should be centered: (100 - 40) / 2 = 30
        assert_eq!(result.x, 30);
        assert_eq!(result.width, 40);
    }

    #[test]
    fn test_top_center_rect_positions_near_top() {
        let container = Rect::new(0, 0, 100, 50);
        let result = top_center_rect(40, 10, container);

        // Should be 1 line from top
        assert_eq!(result.y, 1);
        assert_eq!(result.height, 10);
    }

    #[test]
    fn test_top_center_rect_clamps_to_container() {
        let container = Rect::new(0, 0, 30, 20);
        let result = top_center_rect(50, 25, container);

        // Should clamp to container dimensions
        assert!(result.width <= container.width);
        assert!(result.height <= container.height);
    }

    #[test]
    fn test_top_center_rect_with_offset_container() {
        let container = Rect::new(10, 5, 100, 50);
        let result = top_center_rect(40, 10, container);

        // x should account for container offset
        assert_eq!(result.x, 10 + 30); // container.x + margin
        assert_eq!(result.y, 5 + 1); // container.y + 1
    }
}
