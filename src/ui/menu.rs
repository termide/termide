use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use chrono::Local;

use crate::state::AppState;
use crate::i18n;

/// Get menu items with translations
pub fn get_menu_items() -> Vec<String> {
    let t = i18n::t();
    vec![
        t.menu_files().to_string(),
        t.menu_terminal().to_string(),
        t.menu_editor().to_string(),
        t.menu_debug().to_string(),
        t.menu_preferences().to_string(),
        t.menu_help().to_string(),
        t.menu_quit().to_string(),
    ]
}

/// Number of menu items
pub const MENU_ITEM_COUNT: usize = 7;

/// Render top menu in Midnight Commander style
pub fn render_menu(frame: &mut Frame, area: Rect, state: &AppState) {
    let mut spans = vec![Span::raw(" ")];
    let menu_items = get_menu_items();
    let t = i18n::t();

    for (i, item) in menu_items.iter().enumerate() {
        // Determine menu item style
        let is_selected = state.ui.selected_menu_item == Some(i);
        let style = if is_selected && state.ui.menu_open {
            Style::default()
                .fg(state.theme.selection_fg)
                .bg(state.theme.selection_bg)
                .add_modifier(Modifier::BOLD)
        } else if state.ui.menu_open {
            Style::default().fg(state.theme.text_primary)
        } else {
            Style::default().fg(state.theme.text_primary)
        };

        // Add menu item
        spans.push(Span::styled(item.as_str(), style));
        spans.push(Span::raw("  "));
    }

    // Add hint and clock on the right
    let hint = if state.ui.menu_open {
        t.menu_navigate_hint()
    } else {
        t.menu_open_hint()
    };

    // Get current time
    let current_time = Local::now().format("%H:%M").to_string();
    let clock_text = format!(" {} ", current_time);

    // Calculate how much space is left for hint and clock
    let used_width: usize = spans.iter().map(|s| s.width()).sum();
    let remaining = (area.width as usize)
        .saturating_sub(used_width + hint.len() + 2 + clock_text.len());

    if remaining > 0 {
        spans.push(Span::raw(" ".repeat(remaining)));
    }

    // Add hint
    spans.push(Span::styled(
        format!(" {} ", hint),
        Style::default().fg(Color::DarkGray),
    ));

    // Add clock
    spans.push(Span::styled(
        clock_text,
        Style::default()
            .fg(state.theme.text_primary)
            .add_modifier(Modifier::BOLD),
    ));

    let menu = Paragraph::new(Line::from(spans))
        .style(Style::default().bg(state.theme.background));

    frame.render_widget(menu, area);
}

/// Get X coordinate of menu item for dropdown positioning
pub fn get_menu_item_x(index: usize) -> u16 {
    let menu_items = get_menu_items();
    let mut x = 1_u16;
    for (i, item) in menu_items.iter().enumerate() {
        if i == index {
            return x;
        }
        x += item.len() as u16 + 2; // +2 for spaces
    }
    x
}
