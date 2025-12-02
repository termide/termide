use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Widget},
};

use crate::panels::Panel;
use crate::state::AppState;
use crate::theme::Theme;

/// Рендеринг свёрнутой панели (только заголовок, 1 строка)
/// Формат: ─[X][▶] Title ───────────── (если group_size > 1)
///         ─[X] Title ───────────────── (если group_size == 1)
pub fn render_collapsed_panel(
    panel: &dyn Panel,
    area: Rect,
    buf: &mut Buffer,
    is_focused: bool,
    theme: &Theme,
    group_size: usize,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let title = panel.title();
    let style = if is_focused {
        Style::default()
            .fg(theme.accented_fg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.disabled)
    };

    // Рендерим только первую строку
    let y = area.y;

    // Левый край: ─
    if area.width > 0 {
        buf[(area.x, y)].set_symbol("─").set_style(style);
    }

    // Кнопки: [X][▶] если group_size > 1, иначе только [X]
    let buttons = if group_size > 1 { "[X][▶]" } else { "[X]" };
    let buttons_width = buttons.chars().count() as u16;

    if area.width > 1 + buttons_width {
        buf.set_string(area.x + 1, y, buttons, style);
    }

    // Заголовок
    let title_start = area.x + 1 + buttons_width;
    let title_text = format!(" {} ", title);
    let title_width = title_text.len() as u16;

    if area.width > title_start - area.x + title_width {
        buf.set_string(title_start, y, &title_text, style);
    }

    // Заполнение оставшейся части горизонтальной линией ─
    let fill_start = title_start + title_width;
    for x in fill_start..area.right() {
        buf[(x, y)].set_symbol("─").set_style(style);
    }
}

/// Рендеринг развёрнутой панели (полный бордюр)
/// Формат:
/// ┌[X][▼] Title ────────┐ (если group_size > 1)
/// │  содержимое...      │
/// └─────────────────────┘
/// или
/// ┌[X] Title ───────────┐ (если group_size == 1)
/// │  содержимое...      │
/// └─────────────────────┘
pub fn render_expanded_panel(
    panel: &mut Box<dyn Panel>,
    area: Rect,
    buf: &mut Buffer,
    is_focused: bool,
    panel_index: usize,
    state: &AppState,
    group_size: usize,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let title = panel.title();
    let style = if is_focused {
        Style::default()
            .fg(state.theme.accented_fg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(state.theme.disabled)
    };

    // Создать блок с модифицированным заголовком: [X][▼] Title (если group_size > 1)
    // или [X] Title (если group_size == 1)
    let title_text = if group_size > 1 {
        format!("[X][▼] {} ", title)
    } else {
        format!("[X] {} ", title)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(style)
        .title(Span::styled(title_text, style));

    let inner = block.inner(area);
    block.render(area, buf);

    // Рендерим содержимое панели внутри блока
    panel.render(inner, buf, is_focused, panel_index, state);
}
