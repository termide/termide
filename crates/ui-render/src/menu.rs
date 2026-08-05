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
use termide_system_monitor::{format_net_speed, BatteryInfo, RamUnit};
use termide_theme::Theme;
use termide_ui::str_display_width;

/// Parameters for rendering the menu bar.
pub struct MenuRenderParams<'a> {
    pub theme: &'a Theme,
    pub selected_menu_item: Option<usize>,
    pub menu_open: bool,
    pub cpu_usage: u8,
    pub ram_percent: u8,
    pub ram_value: String,
    pub ram_unit: RamUnit,
    /// Network download rate in bytes per second
    pub net_down_rate: u64,
    /// Network upload rate in bytes per second
    pub net_up_rate: u64,
    /// Battery info, if available on this system
    pub battery: Option<BatteryInfo>,
}

/// Menu labels cached per UI language. Recomputed (and leaked) when the
/// language changes so a runtime switch updates the top-level menu. Language
/// changes are rare, so leaking a small `Vec` per switch is acceptable and
/// keeps the `&'static` contract the hot-path callers rely on. The fast path is
/// a single atomic load via [`i18n::language_generation`] — no allocation.
static CACHED_MENU_ITEMS: std::sync::RwLock<Option<(u64, &'static Vec<String>)>> =
    std::sync::RwLock::new(None);

fn compute_menu_items() -> Vec<String> {
    let t = i18n::t();
    vec![
        t.menu_bookmarks().to_string(),
        t.menu_commands().to_string(),
        t.menu_sessions().to_string(),
        t.menu_windows().to_string(),
        t.menu_options().to_string(),
    ]
}

/// Get menu items with translations for the current language.
pub fn get_menu_items() -> &'static Vec<String> {
    let generation = i18n::language_generation();
    // `(u64, &'static …)` is `Copy`, so matching on `*guard` copies the static
    // reference out — no borrow of the guard escapes.
    if let Ok(guard) = CACHED_MENU_ITEMS.read() {
        if let Some((cached_gen, items)) = *guard {
            if cached_gen == generation {
                return items;
            }
        }
    }
    let mut guard = CACHED_MENU_ITEMS
        .write()
        .expect("menu items cache poisoned");
    // Re-check: another thread may have rebuilt it between the locks.
    if let Some((cached_gen, items)) = *guard {
        if cached_gen == generation {
            return items;
        }
    }
    let leaked: &'static Vec<String> = Box::leak(Box::new(compute_menu_items()));
    *guard = Some((generation, leaked));
    leaked
}

/// Number of menu items
pub const MENU_ITEM_COUNT: usize = 5;

/// Number of indicators (net, cpu, ram, clock + disk in status bar)
pub const MENU_INDICATOR_COUNT: usize = 5;

/// Total navigation positions: menu items + indicators
pub const MENU_TOTAL_COUNT: usize = MENU_ITEM_COUNT + MENU_INDICATOR_COUNT;

/// Virtual navigation index for the network (↓/↑) indicator
pub const INDICATOR_NET_INDEX: usize = MENU_ITEM_COUNT;
/// Virtual navigation index for the CPU indicator
pub const INDICATOR_CPU_INDEX: usize = MENU_ITEM_COUNT + 1;
/// Virtual navigation index for the RAM indicator
pub const INDICATOR_RAM_INDEX: usize = MENU_ITEM_COUNT + 2;
/// Virtual navigation index for the clock indicator
pub const INDICATOR_CLOCK_INDEX: usize = MENU_ITEM_COUNT + 3;
/// Virtual navigation index for the disk indicator (status bar)
pub const INDICATOR_DISK_INDEX: usize = MENU_ITEM_COUNT + 4;

/// Index of Bookmarks menu item
pub const BOOKMARKS_MENU_INDEX: usize = 0;

/// Index of Commands menu item
pub const COMMANDS_MENU_INDEX: usize = 1;

/// Index of Sessions menu item
pub const SESSIONS_MENU_INDEX: usize = 2;

/// Index of Windows menu item
pub const WINDOWS_MENU_INDEX: usize = 3;

/// Index of Options menu item
pub const OPTIONS_MENU_INDEX: usize = 4;

/// Pre-computed x positions and widths for all menu items.
/// Avoids repeated `get_menu_items()` allocations in hot paths.
pub struct MenuLayout {
    /// X position of each menu item
    pub x_positions: [u16; MENU_ITEM_COUNT],
    /// Width of each menu item
    pub widths: [u16; MENU_ITEM_COUNT],
    /// Total width used by all menu items (including separators)
    pub total_width: usize,
}

/// Menu layout cached per UI language. Widths depend on the translated labels,
/// so this is rebuilt (and leaked) alongside [`get_menu_items`] on a language
/// switch; see that function for the leak/`&'static` rationale.
static CACHED_LAYOUT: std::sync::RwLock<Option<(u64, &'static MenuLayout)>> =
    std::sync::RwLock::new(None);

impl MenuLayout {
    fn build() -> MenuLayout {
        let menu_items = get_menu_items();
        let mut x_positions = [0u16; MENU_ITEM_COUNT];
        let mut widths = [0u16; MENU_ITEM_COUNT];
        let mut x = 1u16; // initial " " padding

        for (i, item) in menu_items.iter().enumerate() {
            x_positions[i] = x;
            widths[i] = str_display_width(item) as u16;
            x += widths[i] + 2; // item + "  " separator
        }

        let total_width = x as usize - 1; // subtract trailing separator overshoot
        MenuLayout {
            x_positions,
            widths,
            total_width,
        }
    }

    pub fn compute() -> &'static Self {
        let generation = i18n::language_generation();
        if let Ok(guard) = CACHED_LAYOUT.read() {
            if let Some((cached_gen, layout)) = *guard {
                if cached_gen == generation {
                    return layout;
                }
            }
        }
        let mut guard = CACHED_LAYOUT.write().expect("menu layout cache poisoned");
        if let Some((cached_gen, layout)) = *guard {
            if cached_gen == generation {
                return layout;
            }
        }
        let leaked: &'static MenuLayout = Box::leak(Box::new(Self::build()));
        *guard = Some((generation, leaked));
        leaked
    }
}

/// Calculate x position of a menu item by index.
/// Used for positioning submenus next to their parent menu item.
pub fn get_menu_item_x_position(menu_index: usize) -> u16 {
    MenuLayout::compute().x_positions[menu_index.min(MENU_ITEM_COUNT - 1)]
}

/// Pick the battery indicator icon based on AC state.
fn battery_icon(info: BatteryInfo) -> &'static str {
    if info.charging {
        "⚡"
    } else {
        "🔋"
    }
}

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

/// Compute x-ranges of the network, CPU, RAM and clock indicators in the menu bar.
///
/// Returns `(net_range, cpu_range, ram_range, clock_range)` as `Range<u16>` values.
/// These ranges correspond to the positions computed in `render_menu()`.
pub fn get_resource_indicator_ranges(
    area_width: u16,
    params: &MenuRenderParams,
) -> (
    std::ops::Range<u16>,
    std::ops::Range<u16>,
    std::ops::Range<u16>,
    std::ops::Range<u16>,
) {
    let t = i18n::t();
    let layout = MenuLayout::compute();

    // Replicate the layout math from render_menu
    let used_width = 1 + layout.total_width;

    let ram_unit_str = match params.ram_unit {
        RamUnit::Gigabytes => t.size_gigabytes(),
        RamUnit::Megabytes => t.size_megabytes(),
    };

    let net_down_text = format!("↓{} ", format_net_speed(params.net_down_rate));
    let net_up_text = format!("↑{} ", format_net_speed(params.net_up_rate));
    let cpu_text = format!("CPU {}% ", params.cpu_usage);
    let ram_text = format!("RAM {}{} ", params.ram_value, ram_unit_str);
    let battery_text = params
        .battery
        .map(|b| format!("{}{}% ", battery_icon(b), b.percent));
    let battery_width = battery_text.as_deref().map(|s| s.width()).unwrap_or(0);
    let current_time = chrono::Local::now().format("%H:%M").to_string();
    let clock_text = format!(" {} ", current_time);

    // Calculate positions from the right side
    // Layout order: ... [padding] [net_down] [net_up] [cpu] [ram] [battery?] [clock]
    let remaining = (area_width as usize).saturating_sub(
        used_width
            + net_down_text.width()
            + net_up_text.width()
            + cpu_text.width()
            + ram_text.width()
            + battery_width
            + clock_text.width(),
    );

    let net_start = (used_width + remaining) as u16;
    let net_end = net_start + (net_down_text.width() + net_up_text.width()) as u16;

    let cpu_start = net_end;
    let cpu_end = cpu_start + cpu_text.width() as u16;

    let ram_start = cpu_end;
    let ram_end = ram_start + ram_text.width() as u16;

    let clock_start = ram_end + battery_width as u16;
    let clock_end = clock_start + clock_text.width() as u16;

    (
        net_start..net_end,
        cpu_start..cpu_end,
        ram_start..ram_end,
        clock_start..clock_end,
    )
}

/// Render top menu in Midnight Commander style
pub fn render_menu(frame: &mut Frame, area: Rect, params: &MenuRenderParams) {
    let mut spans = vec![Span::raw(" ")];
    let menu_items = get_menu_items();
    let t = i18n::t();

    for (i, item) in menu_items.iter().enumerate() {
        // Determine menu item style
        let is_selected = params.selected_menu_item == Some(i);
        let style = if is_selected && params.menu_open {
            Style::default()
                .fg(params.theme.selected_fg)
                .bg(params.theme.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(params.theme.accented_fg)
        };

        spans.push(Span::styled(item.as_str(), style));
        spans.push(Span::raw("  "));
    }

    // System resource info
    let ram_unit_str = match params.ram_unit {
        RamUnit::Gigabytes => t.size_gigabytes(),
        RamUnit::Megabytes => t.size_megabytes(),
    };

    // Network indicators
    let net_down_text = format!("↓{} ", format_net_speed(params.net_down_rate));
    let net_up_text = format!("↑{} ", format_net_speed(params.net_up_rate));

    // CPU indicator
    let cpu_text = format!("CPU {}% ", params.cpu_usage);
    let cpu_color = resource_color(params.cpu_usage, params.theme);

    // RAM indicator
    let ram_text = format!("RAM {}{} ", params.ram_value, ram_unit_str);
    let ram_color = resource_color(params.ram_percent, params.theme);

    // Battery indicator (only if a battery is present)
    let battery_text = params
        .battery
        .map(|b| format!("{}{}% ", battery_icon(b), b.percent));
    let battery_color = params.battery.map(|b| {
        if b.charging {
            params.theme.success
        } else {
            resource_color(100u8.saturating_sub(b.percent), params.theme)
        }
    });
    let battery_width = battery_text.as_deref().map(|s| s.width()).unwrap_or(0);

    // Current time
    let current_time = Local::now().format("%H:%M").to_string();
    let clock_text = format!(" {} ", current_time);

    // Calculate spacing
    let used_width: usize = spans.iter().map(|s| s.width()).sum();
    let remaining = (area.width as usize).saturating_sub(
        used_width
            + net_down_text.width()
            + net_up_text.width()
            + cpu_text.width()
            + ram_text.width()
            + battery_width
            + clock_text.width(),
    );

    if remaining > 0 {
        spans.push(Span::raw(" ".repeat(remaining)));
    }

    // Keyboard-selected indicator style (same as selected menu item)
    let indicator_selected_style = Style::default()
        .fg(params.theme.selected_fg)
        .bg(params.theme.selected_bg)
        .add_modifier(Modifier::BOLD);

    let net_kbd = params.menu_open && params.selected_menu_item == Some(INDICATOR_NET_INDEX);
    let cpu_kbd = params.menu_open && params.selected_menu_item == Some(INDICATOR_CPU_INDEX);
    let ram_kbd = params.menu_open && params.selected_menu_item == Some(INDICATOR_RAM_INDEX);
    let clock_kbd = params.menu_open && params.selected_menu_item == Some(INDICATOR_CLOCK_INDEX);

    // Pre-compute styles to avoid repeated Style::default() calls
    let cpu_style = if cpu_kbd {
        indicator_selected_style
    } else {
        Style::default().fg(cpu_color)
    };
    let ram_style = if ram_kbd {
        indicator_selected_style
    } else {
        Style::default().fg(ram_color)
    };
    let clock_style = if clock_kbd {
        indicator_selected_style
    } else {
        Style::default()
            .fg(params.theme.fg)
            .add_modifier(Modifier::BOLD)
    };

    // Add network indicators
    let net_down_style = if net_kbd {
        indicator_selected_style
    } else {
        Style::default().fg(params.theme.success)
    };
    let net_up_style = if net_kbd {
        indicator_selected_style
    } else {
        Style::default().fg(params.theme.warning)
    };
    spans.push(Span::styled(net_down_text, net_down_style));
    spans.push(Span::styled(net_up_text, net_up_style));

    // Add CPU indicator
    spans.push(Span::styled(cpu_text, cpu_style));

    // Add RAM indicator
    spans.push(Span::styled(ram_text, ram_style));

    // Add battery indicator (between RAM and clock) when available
    if let (Some(text), Some(color)) = (battery_text, battery_color) {
        spans.push(Span::styled(text, Style::default().fg(color)));
    }

    // Add clock
    spans.push(Span::styled(clock_text, clock_style));

    let menu =
        Paragraph::new(Line::from(spans)).style(Style::default().bg(params.theme.accented_bg));

    frame.render_widget(menu, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: the top-level menu was cached at first access and never
    /// rebuilt, so a runtime language switch left it in the old language.
    #[test]
    fn menu_labels_follow_runtime_language_switch() {
        // This test mutates the process-global translation singleton. Restore
        // it to English on the way out — via a drop guard so a panicking
        // assertion below can't leave the global set to "ru" and pollute other
        // tests in this crate that read `i18n::t()`.
        struct RestoreLang;
        impl Drop for RestoreLang {
            fn drop(&mut self) {
                let _ = i18n::set_language("en");
            }
        }
        let _restore = RestoreLang;

        i18n::set_language("en").unwrap();
        let en = get_menu_items().clone();

        i18n::set_language("ru").unwrap();
        let ru = get_menu_items().clone();
        assert_ne!(en, ru, "menu labels must change with the language");

        // Layout rebuilds too: each width matches the *current* label, not the
        // first-seen one.
        let layout = MenuLayout::compute();
        for (i, label) in ru.iter().enumerate() {
            assert_eq!(
                layout.widths[i],
                str_display_width(label) as u16,
                "layout width must track the switched language"
            );
        }

        // Switching back restores the original labels.
        i18n::set_language("en").unwrap();
        assert_eq!(*get_menu_items(), en);
    }
}
