//! Field-rendering helpers for the command-config modal: labeled input
//! fields and the group input with its dropdown indicator.

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    widgets::{Block, Borders, Paragraph, Widget},
};
use termide_theme::Theme;

use crate::base::render_text_input;
use crate::TextInputHandler;

use super::{CommandConfigModal, FocusArea};

impl CommandConfigModal {
    /// Render a labeled input field (3 rows: top padding, label+bordered
    /// input, bottom padding); returns the row its text is drawn in.
    pub(super) fn render_labeled_input_field(
        buf: &mut Buffer,
        area: Rect,
        label: &str,
        input: &mut TextInputHandler,
        is_focused: bool,
        theme: &Theme,
    ) -> Rect {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(15), Constraint::Min(1)])
            .split(area);

        // Render label (right-aligned, vertically centered)
        let label_para = Paragraph::new(label.to_string())
            .style(Style::default().fg(theme.fg))
            .alignment(Alignment::Right);
        let label_area = Rect {
            x: chunks[0].x,
            y: chunks[0].y + 1,
            width: chunks[0].width,
            height: 1,
        };
        label_para.render(label_area, buf);

        // Render input with border
        let border_style = if is_focused {
            Style::default().fg(theme.accented_fg)
        } else {
            Style::default().fg(theme.disabled)
        };

        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style);
        let input_inner = input_block.inner(chunks[1]);
        input_block.render(chunks[1], buf);

        let text_area = Rect {
            height: input_inner.height.min(1),
            ..input_inner
        };
        render_text_input(buf, text_area, input, is_focused, theme);
        text_area
    }

    /// Render group input field with dropdown indicator; returns the row its
    /// text is drawn in, left of the indicator.
    pub(super) fn render_group_field(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        label: &str,
        theme: &Theme,
    ) -> Rect {
        let is_focused = self.focus == FocusArea::Group;
        let has_groups = !self.group_suggestion.suggestions().is_empty();

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(15), Constraint::Min(1)])
            .split(area);

        // Label
        Paragraph::new(label.to_string())
            .style(Style::default().fg(theme.fg))
            .alignment(Alignment::Right)
            .render(
                Rect {
                    x: chunks[0].x,
                    y: chunks[0].y + 1,
                    width: chunks[0].width,
                    height: 1,
                },
                buf,
            );

        let input_area = chunks[1];
        let border_style = if is_focused {
            Style::default().fg(theme.accented_fg)
        } else {
            Style::default().fg(theme.disabled)
        };

        let borders = if self.group_suggestion.is_expanded() && has_groups {
            Borders::LEFT | Borders::TOP | Borders::RIGHT
        } else {
            Borders::ALL
        };

        let input_block = Block::default().borders(borders).border_style(border_style);
        let input_inner = input_block.inner(input_area);
        input_block.render(input_area, buf);

        let indicator_width = if has_groups { 2u16 } else { 0u16 };
        let text_width = input_inner.width.saturating_sub(indicator_width);

        let text_area = Rect::new(
            input_inner.x,
            input_inner.y,
            text_width,
            input_inner.height.min(1),
        );
        render_text_input(
            buf,
            text_area,
            self.group_suggestion.input_mut(),
            is_focused,
            theme,
        );

        if has_groups {
            let indicator_x = input_inner.x + input_inner.width.saturating_sub(1);
            let indicator_str = if self.group_suggestion.is_expanded() {
                "\u{25B2}"
            } else {
                "\u{25BC}"
            };
            buf.set_string(
                indicator_x,
                input_inner.y,
                indicator_str,
                Style::default().fg(theme.disabled),
            );
        }
        text_area
    }
}
