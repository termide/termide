//! Menu bar rendering.
//!
//! Provides menu item definitions, color utilities, and menu rendering.

use chrono::Local;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use unicode_width::UnicodeWidthStr;

use termide_i18n as i18n;
use termide_system_monitor::RamUnit;
use termide_theme::Theme;

/// Parameters for rendering the menu bar.
pub struct MenuRenderParams<'a> {
    pub theme: &'a Theme,
    pub selected_menu_item: Option<usize>,
    pub menu_open: bool,
    pub cpu_usage: u8,
    pub ram_percent: u8,
    pub ram_value: String,
    pub ram_unit: RamUnit,
}

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

/// Choose color indicator by load level
/// < 50% - green (success)
/// 50-75% - yellow (warning)
/// > 75% - red (error)
pub fn resource_color(usage: u8, theme: &Theme) -> Color {
    if usage > 75 {
        theme.error
    } else if usage >= 50 {
        theme.warning
    } else {
        theme.success
    }
}

/// Render top menu in Midnight Commander style
pub fn render_menu(frame: &mut Frame, area: Rect, params: &MenuRenderParams) {
    let mut spans = vec![Span::raw(" ")];
    let menu_items = get_menu_items();
    let t = i18n::t();

    for (i, item) in menu_items.iter().enumerate() {
        // Determine menu item style
        let is_selected = params.selected_menu_item == Some(i);
        let (base_style, accent_style) = if is_selected && params.menu_open {
            let base = Style::default()
                .fg(params.theme.selected_fg)
                .bg(params.theme.selected_bg)
                .add_modifier(Modifier::BOLD);
            (base, base)
        } else {
            let base = Style::default().fg(params.theme.fg);
            let accent = Style::default()
                .fg(params.theme.accented_fg)
                .add_modifier(Modifier::BOLD);
            (base, accent)
        };

        // Highlight first letter (keyboard accelerator) only for English locale
        // In other locales, menu text doesn't match keyboard shortcuts
        if i18n::current_language() == "en" {
            // English: Split menu item and highlight first letter
            if let Some(first_char) = item.chars().next() {
                let first = first_char.to_string();
                let rest = &item[first.len()..];

                spans.push(Span::styled(first, accent_style));
                if !rest.is_empty() {
                    spans.push(Span::styled(rest, base_style));
                }
            }
        } else {
            // Non-English: Don't highlight first letter
            spans.push(Span::styled(item.as_str(), base_style));
        }

        spans.push(Span::raw("  "));
    }

    // Add hint, resource indicators, and clock on the right
    let hint = if params.menu_open {
        t.menu_navigate_hint()
    } else {
        t.menu_open_hint()
    };

    // System resource info
    let ram_unit_str = match params.ram_unit {
        RamUnit::Gigabytes => t.size_gigabytes(),
        RamUnit::Megabytes => t.size_megabytes(),
    };

    // CPU indicator with fixed width
    let cpu_text = format!("{:<9}", format!("CPU {}%", params.cpu_usage));
    let cpu_color = resource_color(params.cpu_usage, params.theme);

    // RAM indicator
    let ram_text = format!("RAM {} {} ", params.ram_value, ram_unit_str);
    let ram_color = resource_color(params.ram_percent, params.theme);

    // Current time
    let current_time = Local::now().format("%H:%M").to_string();
    let clock_text = format!(" {} ", current_time);

    // Calculate spacing
    let used_width: usize = spans.iter().map(|s| s.width()).sum();
    let remaining = (area.width as usize).saturating_sub(
        used_width + hint.width() + 2 + cpu_text.width() + ram_text.width() + clock_text.width(),
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
            .fg(params.theme.fg)
            .add_modifier(Modifier::BOLD),
    ));

    let menu =
        Paragraph::new(Line::from(spans)).style(Style::default().bg(params.theme.accented_bg));

    frame.render_widget(menu, area);
}
