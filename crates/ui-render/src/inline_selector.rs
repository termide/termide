//! Inline selector widget for dropdown-style selectors.

use ratatui::{
    buffer::Buffer,
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use termide_core::ThemeColors;

/// Inline selector widget that renders as `[label ▶]` or `[label ▼]`
pub struct InlineSelector<'a> {
    label: &'a str,
    is_open: bool,
    is_focused: bool,
    theme: &'a ThemeColors,
}

impl<'a> InlineSelector<'a> {
    /// Create a new inline selector
    pub fn new(label: &'a str, is_open: bool, is_focused: bool, theme: &'a ThemeColors) -> Self {
        Self {
            label,
            is_open,
            is_focused,
            theme,
        }
    }

    /// The width the selector takes when its label is drawn whole:
    /// `[` + label + ` ` + arrow + `]`.
    pub fn full_width(&self) -> u16 {
        (self.label.width() + 4) as u16
    }

    /// Render the selector at the given position
    /// Returns the rendered width
    pub fn render(&self, x: u16, y: u16, max_width: u16, buf: &mut Buffer) -> u16 {
        // Arrow: ▶/► when collapsed, ▼ when expanded
        const ARROW_CLOSED: &str = if cfg!(windows) { "►" } else { "▶" };
        let arrow = if self.is_open { "▼" } else { ARROW_CLOSED };

        let style = if self.is_focused {
            // Inverted cursor style
            Style::default()
                .fg(self.theme.bg)
                .bg(self.theme.fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.theme.fg)
        };

        // Truncate label if needed: "[" + label + " " + arrow + "]" = 4 extra chars
        let max_label_width = max_width.saturating_sub(4) as usize;
        let truncated_label = if self.label.width() > max_label_width {
            let mut end = 0;
            let mut w = 0;
            for ch in self.label.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if w + cw > max_label_width {
                    break;
                }
                w += cw;
                end += ch.len_utf8();
            }
            &self.label[..end]
        } else {
            self.label
        };

        let text = format!("[{} {}]", truncated_label, arrow);
        let text_width = text.width() as u16;
        buf.set_string(x, y, &text, style);

        text_width
    }
}

/// The most the first of two selectors sharing a row of `total` cells may take
/// when the second, drawn after a two-cell gap, needs `second_width`: whatever
/// the second leaves, but never less than half the row — so a short second
/// label gives its room to the first, and two long labels split the row evenly.
pub fn leading_selector_max_width(total: u16, second_width: u16) -> u16 {
    (total / 2).max(total.saturating_sub(second_width + 2))
}

#[cfg(test)]
mod tests {
    use super::leading_selector_max_width;

    #[test]
    fn a_short_second_selector_leaves_its_room_to_the_first() {
        // `[main ▶]` is 8 cells: the first gets the 40 − 8 − 2 cells left.
        assert_eq!(leading_selector_max_width(40, 8), 30);
    }

    #[test]
    fn two_long_selectors_split_the_row_in_half() {
        assert_eq!(leading_selector_max_width(40, 35), 20);
        assert_eq!(leading_selector_max_width(5, 8), 2);
    }
}
