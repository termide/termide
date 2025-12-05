/// UI utility functions for layout and rendering
use ratatui::layout::Rect;

/// Create a centered rectangle with specified width and height within a container
///
/// This utility function is used by modal dialogs to center themselves on screen.
/// It calculates horizontal and vertical margins and uses ratatui's Layout system
/// to create a properly centered rectangle.
///
/// # Arguments
///
/// * `width` - Desired width of the centered rectangle
/// * `height` - Desired height of the centered rectangle
/// * `r` - Container rectangle (usually the full terminal area)
///
/// # Returns
///
/// A `Rect` centered within the container with the specified dimensions
pub fn centered_rect_with_size(width: u16, height: u16, r: Rect) -> Rect {
    use ratatui::layout::{Constraint, Direction, Layout};

    // Calculate margins
    let horizontal_margin = r.width.saturating_sub(width) / 2;
    let vertical_margin = r.height.saturating_sub(height) / 2;

    let vertical_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(vertical_margin),
            Constraint::Length(height),
            Constraint::Length(vertical_margin),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(horizontal_margin),
            Constraint::Length(width),
            Constraint::Length(horizontal_margin),
        ])
        .split(vertical_layout[1])[1]
}
