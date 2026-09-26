//! Theme selection dropdown with live preview.
//!
//! Simple list of theme names. Live preview is handled by applying
//! the theme on cursor navigation (see menu_actions.rs).

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::Widget,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem},
};
use unicode_width::UnicodeWidthStr;

use termide_core::ThemeColors;
use termide_theme::Theme;
use termide_ui::ScrollBar;

use crate::dropdown::ListGeometry;

/// Theme dropdown with live preview on cursor navigation
pub struct ThemeDropdown<'a> {
    /// Theme names to display
    theme_names: &'a [String],
    /// Selected item index
    selected: usize,
    /// X position
    x: u16,
    /// Y position
    y: u16,
    /// App theme for borders
    app_theme: &'a Theme,
}

const MAX_VISIBLE: usize = 25;

fn theme_dropdown_width(theme_names: &[String]) -> u16 {
    let max_name_len = theme_names.iter().map(|n| n.width()).max().unwrap_or(10);
    // "▶ " + name + padding
    (max_name_len + 4).min(30) as u16
}

/// On-screen geometry of a [`ThemeDropdown`] listing `theme_names`, requested
/// at (`x`, `y`) with `selected` highlighted and fitted to `screen`.
pub fn theme_dropdown_geometry(
    theme_names: &[String],
    selected: usize,
    x: u16,
    y: u16,
    screen: Rect,
) -> ListGeometry {
    ListGeometry::compute(
        theme_dropdown_width(theme_names),
        theme_names.len(),
        MAX_VISIBLE,
        selected,
        x,
        y,
        screen,
    )
}

impl<'a> ThemeDropdown<'a> {
    pub fn new(
        theme_names: &'a [String],
        selected: usize,
        x: u16,
        y: u16,
        app_theme: &'a Theme,
    ) -> Self {
        Self {
            theme_names,
            selected,
            x,
            y,
            app_theme,
        }
    }

    /// Get the width of this dropdown
    pub fn width(&self) -> u16 {
        theme_dropdown_width(self.theme_names)
    }

    /// Get the height of this dropdown
    pub fn height(&self) -> u16 {
        let items_count = self.theme_names.len().min(MAX_VISIBLE);
        (items_count + 2) as u16 // +2 for borders
    }

    pub fn render(&self, buf: &mut Buffer) {
        if self.theme_names.is_empty() {
            return;
        }

        let total = self.theme_names.len();
        let geometry =
            theme_dropdown_geometry(self.theme_names, self.selected, self.x, self.y, buf.area);
        let area = geometry.area;
        let Rect {
            x,
            y,
            width,
            height,
        } = area;
        let scroll_offset = geometry.scroll_offset;
        let visible_count = geometry.visible_count();

        // Clear area under dropdown
        Clear.render(area, buf);

        // Build list items - use panel colors (not modal colors)
        let visible_end = (scroll_offset + visible_count).min(total);
        let visible_items = &self.theme_names[scroll_offset..visible_end];

        let items: Vec<ListItem> = visible_items
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let actual_index = scroll_offset + i;
                let is_selected = actual_index == self.selected;

                // Panel-style colors: normal text on panel background, selection highlighted
                let item_style = if is_selected {
                    Style::default()
                        .fg(self.app_theme.selected_fg)
                        .bg(self.app_theme.selected_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(self.app_theme.fg).bg(self.app_theme.bg)
                };

                // Build line with padding
                let content = format!(" {}", name);
                let padding_len = (width as usize).saturating_sub(content.width() + 2);
                let padded = if padding_len > 0 {
                    format!("{}{}", content, " ".repeat(padding_len))
                } else {
                    content
                };

                ListItem::new(Line::from(Span::styled(padded, item_style)))
            })
            .collect();

        // Use modal-style colors: bg background, accented_fg for border
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(self.app_theme.accented_fg))
                .style(Style::default().bg(self.app_theme.bg)),
        );

        list.render(area, buf);

        // Render scrollbar on right edge (inside border)
        let theme_colors = ThemeColors::from(self.app_theme);
        ScrollBar::render(
            buf,
            x + width - 1,            // Right border position
            y + 1,                    // Inside top border
            height.saturating_sub(2), // Inside borders
            scroll_offset,
            visible_count,
            total,
            &theme_colors,
            true, // Dropdown is always focused when visible
        );
    }
}

#[cfg(test)]
mod tests {
    use super::ThemeDropdown;
    use ratatui::{buffer::Buffer, layout::Rect};
    use termide_theme::Theme;

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("theme-{i:02}")).collect()
    }

    // Regression for #25: a theme list taller than the terminal must clamp to
    // the screen instead of rendering past the bottom (which panics ratatui).
    // The crash report had an 88x24 area with 38 themes.
    #[test]
    fn render_does_not_overflow_short_terminal() {
        let theme = Theme::get_by_name("default");
        let names = names(38);
        let mut buf = Buffer::empty(Rect::new(0, 0, 88, 24));
        // Selected near the end exercises the scroll window at its limit.
        ThemeDropdown::new(&names, 37, 20, 2, theme).render(&mut buf);
        // First item too — no underflow in the scroll math.
        let mut buf2 = Buffer::empty(Rect::new(0, 0, 88, 24));
        ThemeDropdown::new(&names, 0, 20, 2, theme).render(&mut buf2);
    }

    #[test]
    fn render_handles_tiny_terminal() {
        let theme = Theme::get_by_name("default");
        let names = names(38);
        for h in [1u16, 2, 3] {
            let mut buf = Buffer::empty(Rect::new(0, 0, 40, h));
            ThemeDropdown::new(&names, 20, 0, 0, theme).render(&mut buf);
        }
    }
}
