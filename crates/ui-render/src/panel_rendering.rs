//! Panel rendering functions.
//!
//! Provides functions to render expanded and collapsed panels.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Widget},
};

use termide_config::Config;
use termide_core::{Panel, PanelConfig, RenderContext, ThemeColors};
use termide_theme::Theme;

/// Parameters for rendering expanded panels.
#[derive(Clone, Copy)]
pub struct ExpandedPanelParams {
    pub tab_size: usize,
    pub word_wrap: bool,
    pub terminal_width: u16,
    pub terminal_height: u16,
}

/// Render collapsed panel (header only, 1 line).
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

    let y = area.y;

    // Left edge
    if area.width > 0 {
        buf[(area.x, y)].set_symbol("─").set_style(style);
    }

    // Buttons: [X][▶] if group_size > 1, else [X]
    let buttons = if group_size > 1 { "[X][▶]" } else { "[X]" };
    let buttons_width = buttons.chars().count() as u16;

    if area.width > 1 + buttons_width {
        buf.set_string(area.x + 1, y, buttons, style);
    }

    // Title
    let title_start = area.x + 1 + buttons_width;
    let title_text = format!(" {} ", title);
    let title_width = title_text.len() as u16;

    if area.width > title_start - area.x + title_width {
        buf.set_string(title_start, y, &title_text, style);
    }

    // Fill remaining with horizontal line
    let fill_start = title_start + title_width;
    for x in fill_start..area.right() {
        buf[(x, y)].set_symbol("─").set_style(style);
    }
}

/// Render expanded panel (full border with content).
#[allow(clippy::too_many_arguments)]
pub fn render_expanded_panel(
    panel: &mut Box<dyn Panel>,
    area: Rect,
    buf: &mut Buffer,
    is_focused: bool,
    panel_index: usize,
    theme: &Theme,
    config: &Config,
    params: ExpandedPanelParams,
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

    // Create title: [X][▼] Title (if group_size > 1) or [X] Title
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

    // Clear inner area before rendering content
    let clear_style = Style::default().bg(theme.bg);
    for y in inner.y..inner.y + inner.height {
        for x in inner.x..inner.x + inner.width {
            buf[(x, y)].reset();
            buf[(x, y)].set_style(clear_style);
        }
    }

    // Create RenderContext
    let colors = ThemeColors::from(theme);
    let panel_config = PanelConfig {
        tab_size: params.tab_size,
        word_wrap: params.word_wrap,
        show_line_numbers: true,
        show_hidden_files: false,
    };
    let ctx = RenderContext {
        theme: &colors,
        config: &panel_config,
        is_focused,
        panel_index,
        terminal_width: params.terminal_width,
        terminal_height: params.terminal_height,
    };

    // Prepare panel for rendering (update cached theme/config)
    panel.prepare_render(theme, config);

    // Render panel content
    panel.render(inner, buf, &ctx);
}
