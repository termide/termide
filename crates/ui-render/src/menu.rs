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

use crate::project_strip::{fit_project_strip, ProjectButton, ProjectStrip};

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
    /// The open projects, in the order the menus list them. A waiting one
    /// that has no button on screen marks the Projects title instead.
    pub projects: &'a [ProjectButton],
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
        t.menu_projects().to_string(),
        t.menu_ai().to_string(),
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
pub const MENU_ITEM_COUNT: usize = 6;

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

/// Navigation index of the first project button; project button `i` (in
/// the order of [`MenuRenderParams::projects`]) is `PROJECT_BUTTON_BASE + i`.
/// Placed after the fixed positions so their indices never move; the order
/// they are walked in is [`MenuBarLayout::nav_order`].
pub const PROJECT_BUTTON_BASE: usize = MENU_TOTAL_COUNT;

/// The project button a navigation index stands for.
pub fn project_button_of(menu_index: usize) -> Option<usize> {
    menu_index.checked_sub(PROJECT_BUTTON_BASE)
}

/// Index of Bookmarks menu item
pub const BOOKMARKS_MENU_INDEX: usize = 0;

/// Index of Commands menu item
pub const COMMANDS_MENU_INDEX: usize = 1;

/// Index of Projects menu item
pub const PROJECTS_MENU_INDEX: usize = 2;

/// Index of the AI menu item
pub const AI_MENU_INDEX: usize = 3;

/// Index of Windows menu item
pub const WINDOWS_MENU_INDEX: usize = 4;

/// Index of Options menu item
pub const OPTIONS_MENU_INDEX: usize = 5;

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

/// Menu layouts cached per UI language, without and with the Projects
/// attention mark. Widths depend on the translated labels, so these are
/// rebuilt (and leaked) alongside [`get_menu_items`] on a language switch;
/// see that function for the leak/`&'static` rationale.
#[allow(clippy::type_complexity)]
static CACHED_LAYOUT: std::sync::RwLock<Option<(u64, [&'static MenuLayout; 2])>> =
    std::sync::RwLock::new(None);

/// Whether the menu bar last drawn showed the Projects attention mark. The
/// mark widens the Projects title and shifts the items after it, so every
/// position lookup (clicks, dropdown anchors) follows what is on screen.
static PROJECTS_MARKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What follows the Projects title while a background project waits.
fn projects_mark() -> String {
    format!(" {}", termide_core::attention_mark())
}

impl MenuLayout {
    fn build(projects_marked: bool) -> MenuLayout {
        let menu_items = get_menu_items();
        let mut x_positions = [0u16; MENU_ITEM_COUNT];
        let mut widths = [0u16; MENU_ITEM_COUNT];
        let mut x = 1u16; // initial " " padding

        for (i, item) in menu_items.iter().enumerate() {
            x_positions[i] = x;
            widths[i] = str_display_width(item) as u16;
            if i == PROJECTS_MENU_INDEX && projects_marked {
                widths[i] += str_display_width(&projects_mark()) as u16;
            }
            x += widths[i] + 2; // item + "  " separator
        }

        let total_width = x as usize - 1; // subtract trailing separator overshoot
        MenuLayout {
            x_positions,
            widths,
            total_width,
        }
    }

    /// The layout of the menu bar as last drawn.
    pub fn compute() -> &'static Self {
        Self::with_mark(PROJECTS_MARKED.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// The layout with or without the Projects attention mark.
    fn with_mark(marked: bool) -> &'static Self {
        let marked = usize::from(marked);
        let generation = i18n::language_generation();
        if let Ok(guard) = CACHED_LAYOUT.read() {
            if let Some((cached_gen, layouts)) = *guard {
                if cached_gen == generation {
                    return layouts[marked];
                }
            }
        }
        let mut guard = CACHED_LAYOUT.write().expect("menu layout cache poisoned");
        if let Some((cached_gen, layouts)) = *guard {
            if cached_gen == generation {
                return layouts[marked];
            }
        }
        let layouts: [&'static MenuLayout; 2] = [
            Box::leak(Box::new(Self::build(false))),
            Box::leak(Box::new(Self::build(true))),
        ];
        *guard = Some((generation, layouts));
        layouts[marked]
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

/// The texts of the indicators on the right of the menu bar.
struct IndicatorTexts {
    net_down: String,
    net_up: String,
    cpu: String,
    ram: String,
    battery: Option<String>,
    clock: String,
}

impl IndicatorTexts {
    fn of(params: &MenuRenderParams) -> Self {
        let t = i18n::t();
        let ram_unit = match params.ram_unit {
            RamUnit::Gigabytes => t.size_gigabytes(),
            RamUnit::Megabytes => t.size_megabytes(),
        };
        Self {
            net_down: format!("↓{} ", format_net_speed(params.net_down_rate)),
            net_up: format!("↑{} ", format_net_speed(params.net_up_rate)),
            cpu: format!("CPU {}% ", params.cpu_usage),
            ram: format!("RAM {}{} ", params.ram_value, ram_unit),
            battery: params
                .battery
                .map(|b| format!("{}{}% ", battery_icon(b), b.percent)),
            clock: format!(" {} ", Local::now().format("%H:%M")),
        }
    }

    fn width(&self) -> usize {
        self.net_down.width()
            + self.net_up.width()
            + self.cpu.width()
            + self.ram.width()
            + self.battery.as_deref().map_or(0, |s| s.width())
            + self.clock.width()
    }
}

/// Where everything in the menu bar goes. Drawing, clicks and keyboard
/// navigation all read this one layout.
#[derive(Debug, Clone)]
pub struct MenuBarLayout {
    /// The Projects title carries the attention mark: a waiting project has
    /// no button on screen.
    pub projects_marked: bool,
    /// The project buttons that fit.
    pub projects: ProjectStrip,
    pub net: std::ops::Range<u16>,
    pub cpu: std::ops::Range<u16>,
    pub ram: std::ops::Range<u16>,
    pub clock: std::ops::Range<u16>,
}

impl MenuBarLayout {
    /// The navigation indices in the order Left/Right walk them: the menu
    /// titles, the project buttons on screen, then the indicators.
    pub fn nav_order(&self) -> Vec<usize> {
        (0..MENU_ITEM_COUNT)
            .chain(
                self.projects
                    .buttons
                    .iter()
                    .map(|button| PROJECT_BUTTON_BASE + button.index),
            )
            .chain(MENU_ITEM_COUNT..MENU_TOTAL_COUNT)
            .collect()
    }

    /// The project button at column `x`, as an index into the projects.
    pub fn project_at(&self, x: u16) -> Option<usize> {
        self.projects
            .buttons
            .iter()
            .find(|button| button.range().contains(&x))
            .map(|button| button.index)
    }
}

/// Lay the menu bar out in `area_width` columns.
pub fn menu_bar_layout(area_width: u16, params: &MenuRenderParams) -> MenuBarLayout {
    let indicators = IndicatorTexts::of(params);
    // The indicators keep their place; the project buttons get what is left
    // between them and the titles, one column clear of each.
    let fit = |marked: bool| {
        let titles_end = 1 + MenuLayout::with_mark(marked).total_width;
        let indicators_start = (area_width as usize)
            .saturating_sub(indicators.width())
            .max(titles_end);
        // The titles end with a two-column gap: the strip starts one in.
        let start = titles_end - 1;
        let width = indicators_start.saturating_sub(1).saturating_sub(start);
        (
            fit_project_strip(params.projects, start as u16, width),
            indicators_start as u16,
        )
    };
    let unseen_waiting = |strip: &ProjectStrip| {
        params.projects.iter().enumerate().any(|(index, project)| {
            project.attention && !strip.buttons.iter().any(|button| button.index == index)
        })
    };
    let (mut projects, mut net_start) = fit(false);
    let projects_marked = unseen_waiting(&projects);
    if projects_marked {
        (projects, net_start) = fit(true);
    }

    let net_end = net_start + (indicators.net_down.width() + indicators.net_up.width()) as u16;
    let cpu_end = net_end + indicators.cpu.width() as u16;
    let ram_end = cpu_end + indicators.ram.width() as u16;
    let clock_start = ram_end + indicators.battery.as_deref().map_or(0, |s| s.width()) as u16;
    let clock_end = clock_start + indicators.clock.width() as u16;
    MenuBarLayout {
        projects_marked,
        projects,
        net: net_start..net_end,
        cpu: net_end..cpu_end,
        ram: cpu_end..ram_end,
        clock: clock_start..clock_end,
    }
}

/// Spans filling the bar from column `from` up to column `to`.
fn pad(spans: &mut Vec<Span<'_>>, from: usize, to: usize) {
    if to > from {
        spans.push(Span::raw(" ".repeat(to - from)));
    }
}

/// Render top menu in Midnight Commander style
pub fn render_menu(frame: &mut Frame, area: Rect, params: &MenuRenderParams) {
    let bar = menu_bar_layout(area.width, params);
    PROJECTS_MARKED.store(bar.projects_marked, std::sync::atomic::Ordering::Relaxed);
    let indicators = IndicatorTexts::of(params);

    // Keyboard-selected style, shared by titles, buttons and indicators
    let selected_style = Style::default()
        .fg(params.theme.selected_fg)
        .bg(params.theme.selected_bg)
        .add_modifier(Modifier::BOLD);
    let is_selected = |index: usize| params.menu_open && params.selected_menu_item == Some(index);

    let mut spans = vec![Span::raw(" ")];
    let items = get_menu_items();
    for (i, item) in items.iter().enumerate() {
        let style = if is_selected(i) {
            selected_style
        } else {
            Style::default().fg(params.theme.accented_fg)
        };

        spans.push(Span::styled(item.as_str(), style));
        if i == PROJECTS_MENU_INDEX && bar.projects_marked {
            // A mark, not a colour: several themes (the default included)
            // give menu titles the warning colour already, and a badge
            // reads as the selected item.
            spans.push(Span::styled(projects_mark(), style));
        }
        // The gap after the last title is left to the padding below: the
        // project buttons start one column after it, not two.
        if i + 1 < items.len() {
            spans.push(Span::raw("  "));
        }
    }
    let mut column: usize = spans.iter().map(|s| s.width()).sum();

    // Project buttons: the current one in the menu titles' colour, the
    // others dimmed.
    for button in &bar.projects.buttons {
        pad(&mut spans, column, button.x as usize);
        let project = &params.projects[button.index];
        let style = if is_selected(PROJECT_BUTTON_BASE + button.index) {
            selected_style
        } else if project.current {
            Style::default().fg(params.theme.accented_fg)
        } else {
            Style::default().fg(params.theme.disabled)
        };
        spans.push(Span::styled(button.label.clone(), style));
        column = column.max(button.x as usize) + button.label.width();
    }
    if let Some((x, text)) = &bar.projects.overflow {
        pad(&mut spans, column, *x as usize);
        spans.push(Span::styled(
            text.clone(),
            Style::default().fg(params.theme.disabled),
        ));
        column = column.max(*x as usize) + text.width();
    }
    pad(&mut spans, column, bar.net.start as usize);

    let net_kbd = is_selected(INDICATOR_NET_INDEX);
    let pick = |selected: bool, style: Style| if selected { selected_style } else { style };
    spans.push(Span::styled(
        indicators.net_down,
        pick(net_kbd, Style::default().fg(params.theme.success)),
    ));
    spans.push(Span::styled(
        indicators.net_up,
        pick(net_kbd, Style::default().fg(params.theme.warning)),
    ));
    spans.push(Span::styled(
        indicators.cpu,
        pick(
            is_selected(INDICATOR_CPU_INDEX),
            Style::default().fg(resource_color(params.cpu_usage, params.theme)),
        ),
    ));
    spans.push(Span::styled(
        indicators.ram,
        pick(
            is_selected(INDICATOR_RAM_INDEX),
            Style::default().fg(resource_color(params.ram_percent, params.theme)),
        ),
    ));

    // Battery indicator (between RAM and clock) when available
    if let (Some(text), Some(b)) = (indicators.battery, params.battery) {
        let color = if b.charging {
            params.theme.success
        } else {
            resource_color(100u8.saturating_sub(b.percent), params.theme)
        };
        spans.push(Span::styled(text, Style::default().fg(color)));
    }

    spans.push(Span::styled(
        indicators.clock,
        pick(
            is_selected(INDICATOR_CLOCK_INDEX),
            Style::default()
                .fg(params.theme.fg)
                .add_modifier(Modifier::BOLD),
        ),
    ));

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

    #[test]
    fn the_projects_mark_widens_projects_and_shifts_the_items_after_it() {
        let plain = MenuLayout::build(false);
        let marked = MenuLayout::build(true);
        let extra = str_display_width(&projects_mark()) as u16;
        for i in 0..MENU_ITEM_COUNT {
            let width = plain.widths[i] + if i == PROJECTS_MENU_INDEX { extra } else { 0 };
            assert_eq!(marked.widths[i], width);
            let shift = if i > PROJECTS_MENU_INDEX { extra } else { 0 };
            assert_eq!(marked.x_positions[i], plain.x_positions[i] + shift);
        }
        assert_eq!(marked.total_width, plain.total_width + extra as usize);
    }

    fn params<'a>(theme: &'a Theme, projects: &'a [ProjectButton]) -> MenuRenderParams<'a> {
        MenuRenderParams {
            theme,
            selected_menu_item: None,
            menu_open: false,
            cpu_usage: 0,
            ram_percent: 0,
            ram_value: "1".to_string(),
            ram_unit: RamUnit::Gigabytes,
            net_down_rate: 0,
            net_up_rate: 0,
            battery: None,
            projects,
        }
    }

    fn project(name: &str, current: bool, attention: bool) -> ProjectButton {
        ProjectButton {
            name: name.to_string(),
            current,
            attention,
        }
    }

    #[test]
    fn project_buttons_sit_between_the_titles_and_the_indicators() {
        let theme = Theme::default();
        let projects = [project("one", true, false), project("two", false, false)];
        let bar = menu_bar_layout(200, &params(&theme, &projects));
        let titles_end = 1 + MenuLayout::with_mark(false).total_width;
        let first = &bar.projects.buttons[0];
        assert_eq!(
            first.x as usize,
            titles_end - 1,
            "one column after the titles"
        );
        let last = bar.projects.buttons.last().unwrap();
        assert!(last.range().end < bar.net.start, "clear of the indicators");
        assert_eq!(bar.project_at(first.x), Some(0));
        assert_eq!(bar.project_at(last.x), Some(1));
        assert!(!bar.projects_marked);

        let order = bar.nav_order();
        let projects_at = MENU_ITEM_COUNT;
        assert_eq!(order[projects_at - 1], OPTIONS_MENU_INDEX);
        assert_eq!(
            order[projects_at..projects_at + 2],
            [PROJECT_BUTTON_BASE, PROJECT_BUTTON_BASE + 1]
        );
        assert_eq!(order[projects_at + 2], INDICATOR_NET_INDEX);
    }

    #[test]
    fn a_waiting_project_without_a_button_marks_the_projects_title() {
        let theme = Theme::default();
        let projects = [project("one", true, false), project("two", false, true)];
        let wide = menu_bar_layout(200, &params(&theme, &projects));
        assert!(!wide.projects_marked, "its button shows the bell");

        // No room for buttons: the title carries the bell.
        let narrow_width = (1 + MenuLayout::with_mark(true).total_width) as u16 + 30;
        let narrow = menu_bar_layout(narrow_width, &params(&theme, &projects));
        assert!(narrow.projects.buttons.is_empty());
        assert!(narrow.projects_marked);
        assert_eq!(narrow.nav_order().len(), MENU_TOTAL_COUNT);
    }

    #[test]
    fn project_buttons_are_drawn_where_the_layout_puts_them() {
        use ratatui::{backend::TestBackend, Terminal};
        let theme = Theme::default();
        let projects = [project("nvn", true, false), project("zarab", false, false)];
        let params = params(&theme, &projects);
        let mut terminal = Terminal::new(TestBackend::new(160, 1)).unwrap();
        terminal
            .draw(|frame| render_menu(frame, frame.area(), &params))
            .unwrap();
        let row: String = (0..160)
            .map(|x| terminal.backend().buffer()[(x, 0)].symbol().to_string())
            .collect();
        let bar = menu_bar_layout(160, &params);
        for button in &bar.projects.buttons {
            let at: String = row
                .chars()
                .skip(button.x as usize)
                .take(button.label.chars().count())
                .collect();
            assert_eq!(at, button.label);
        }
        assert!(row.contains("[nvn] [zarab]"), "{row}");
        let titles_end = row.find(" [nvn]").unwrap();
        assert_ne!(
            &row[titles_end - 1..titles_end],
            " ",
            "one column after the titles: {row}"
        );
    }
}
