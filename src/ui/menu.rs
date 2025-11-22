use chrono::Local;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::i18n;
use crate::state::AppState;

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

/// Выбрать цвет индикатора по уровню нагрузки
/// < 50% - зеленый (success)
/// 50-75% - желтый (warning)
/// > 75% - красный (error)
fn resource_color(usage: u8, theme: &crate::theme::Theme) -> Color {
    if usage > 75 {
        theme.error
    } else if usage >= 50 {
        theme.warning
    } else {
        theme.success
    }
}

/// Render top menu in Midnight Commander style
pub fn render_menu(frame: &mut Frame, area: Rect, state: &AppState) {
    let mut spans = vec![Span::raw(" ")];
    let menu_items = get_menu_items();
    let t = i18n::t();

    for (i, item) in menu_items.iter().enumerate() {
        // Determine menu item style
        let is_selected = state.ui.selected_menu_item == Some(i);
        let (base_style, accent_style) = if is_selected && state.ui.menu_open {
            let base = Style::default()
                .fg(state.theme.selected_fg)
                .bg(state.theme.selected_bg)
                .add_modifier(Modifier::BOLD);
            (base, base)
        } else {
            let base = Style::default().fg(state.theme.fg);
            let accent = Style::default()
                .fg(state.theme.accented_fg)
                .add_modifier(Modifier::BOLD);
            (base, accent)
        };

        // Split menu item into first letter and rest
        // Highlight first letter (keyboard accelerator) with accent color
        if let Some(first_char) = item.chars().next() {
            let first = first_char.to_string();
            let rest = &item[first.len()..];

            spans.push(Span::styled(first, accent_style));
            if !rest.is_empty() {
                spans.push(Span::styled(rest, base_style));
            }
        }

        spans.push(Span::raw("  "));
    }

    // Add hint, resource indicators, and clock on the right
    let hint = if state.ui.menu_open {
        t.menu_navigate_hint()
    } else {
        t.menu_open_hint()
    };

    // Get system resource info
    let cpu_usage = state.system_monitor.cpu_usage();
    let ram_percent = state.system_monitor.ram_usage_percent();
    let (ram_value, ram_unit) = state.system_monitor.format_ram();
    let ram_unit_str = match ram_unit {
        crate::system_monitor::RamUnit::Gigabytes => t.size_gigabytes(),
        crate::system_monitor::RamUnit::Megabytes => t.size_megabytes(),
    };

    // CPU индикатор с фиксированной шириной, выравнивание влево
    // Формат: "CPU 9%   " (9 символов всегда)
    let cpu_text = format!("{:<9}", format!("CPU {}%", cpu_usage));
    let cpu_color = resource_color(cpu_usage, state.theme);

    // RAM индикатор
    let ram_text = format!("RAM {}{} ", ram_value, ram_unit_str);
    let ram_color = resource_color(ram_percent, state.theme);

    // Get current time
    let current_time = Local::now().format("%H:%M").to_string();
    let clock_text = format!(" {} ", current_time);

    // Calculate how much space is left for hint, resources, and clock
    let used_width: usize = spans.iter().map(|s| s.width()).sum();
    let remaining = (area.width as usize).saturating_sub(
        used_width + hint.len() + 2 + cpu_text.len() + ram_text.len() + clock_text.len(),
    );

    if remaining > 0 {
        spans.push(Span::raw(" ".repeat(remaining)));
    }

    // Add hint
    spans.push(Span::styled(
        format!(" {} ", hint),
        Style::default().fg(Color::DarkGray),
    ));

    // Add CPU indicator
    spans.push(Span::styled(cpu_text, Style::default().fg(cpu_color)));

    // Add RAM indicator
    spans.push(Span::styled(ram_text, Style::default().fg(ram_color)));

    // Add clock
    spans.push(Span::styled(
        clock_text,
        Style::default()
            .fg(state.theme.fg)
            .add_modifier(Modifier::BOLD),
    ));

    let menu =
        Paragraph::new(Line::from(spans)).style(Style::default().bg(state.theme.accented_bg));

    frame.render_widget(menu, area);
}

/// Get X coordinate of menu item for dropdown positioning
#[allow(dead_code)]
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
