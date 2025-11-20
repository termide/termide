use ratatui::{
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, Borders},
};

use crate::state::{AppState, LayoutMode};

/// Create a block for panel with title and [X] button
/// [X] button is added to the title, position is calculated from the right edge
pub fn create_panel_block<'a>(
    title: &'a str,
    is_focused: bool,
    panel_index: usize,
    state: &AppState,
) -> Block<'a> {
    let title_style = if is_focused {
        Style::default()
            .fg(state.theme.accent_primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(state.theme.accent_secondary)
    };

    // Determine if this panel can be closed
    let can_close = can_close_panel(panel_index, state);

    // [X] at the beginning of title for easy clicking
    let title_text = if can_close {
        format!("[X] {} ", title)
    } else {
        format!(" {} ", title)
    };

    Block::default()
        .borders(Borders::ALL)
        .border_style(title_style)
        .title(Span::styled(title_text, title_style))
}

/// Check if panel can be closed
pub fn can_close_panel(panel_index: usize, state: &AppState) -> bool {
    // Cannot close FM (panel 0) in MultiPanel mode
    if panel_index == 0 && state.layout_mode == LayoutMode::MultiPanel {
        return false;
    }
    true
}

/// Check if mouse click hit the [X] button
/// [X] button is located at the beginning of panel title
pub fn is_click_on_close_button(
    click_x: u16,
    click_y: u16,
    panel_x: u16,
    panel_y: u16,
    _title: &str,
    can_close: bool,
) -> bool {
    // Y must be equal to panel_y (title line)
    if click_y != panel_y {
        return false;
    }

    // Only if panel can be closed
    if !can_close {
        return false;
    }

    // Block title format: "┌─[X] Title ─...──┐"
    // title_text has format: "[X] Title "
    // It starts after "┌─" (2 characters)

    let title_start = panel_x + 2; // After "┌─"

    // [X] is located at the very beginning of title
    // "[X]" = 3 characters starting from title_start
    let x_button_start = title_start;
    let x_button_end = title_start + 2; // last character ']' at position start+2

    click_x >= x_button_start && click_x <= x_button_end
}
