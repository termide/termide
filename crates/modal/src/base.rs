//! Common modal rendering utilities.
//!
//! Provides shared functionality for modal windows:
//! - Frame rendering with [X] close button
//! - Input field rendering with cursor
//! - Common positioning utilities
//! - Cursor navigation trait for search modals

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Clear, Widget},
};
use termide_theme::Theme;

/// Create a styled modal block with title.
///
/// This is the common style used across all modals:
/// - Inverted colors (bg on fg)
/// - Bold title with padding
/// - All borders
///
/// An empty (or whitespace-only) title skips the title span entirely
/// so the top border doesn't get a stray " " gap punched through the
/// box-drawing line.
pub fn create_modal_block(title: &str, theme: &Theme) -> Block<'static> {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accented_fg))
        .style(Style::default().bg(theme.bg));
    if title.trim().is_empty() {
        block
    } else {
        block.title(Span::styled(
            format!(" {} ", title),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ))
    }
}

/// Render a modal block and return its inner area.
///
/// Clears the area, renders the block, and returns the inner content area.
pub fn render_modal_block(area: Rect, buf: &mut Buffer, title: &str, theme: &Theme) -> Rect {
    Clear.render(area, buf);
    let block = create_modal_block(title, theme);
    let inner = block.inner(area);
    block.render(area, buf);
    inner
}

/// Create a style for a button based on selection state.
///
/// Selected buttons use inverted base colors (bg/fg) for maximum contrast.
/// Unselected buttons use the normal foreground color.
pub fn button_style(is_selected: bool, theme: &Theme) -> Style {
    if is_selected {
        Style::default()
            .fg(theme.bg)
            .bg(theme.fg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg)
    }
}

/// Render a text input field with cursor and selection support.
///
/// Parameters:
/// - `text`: Full text content
/// - `cursor_pos`: Cursor position in characters
/// - `selection_range`: Optional (start, end) selection in characters
#[allow(clippy::too_many_arguments)]
pub fn render_input_field(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    text: &str,
    cursor_pos: usize,
    selection_range: Option<(usize, usize)>,
    is_focused: bool,
    theme: &Theme,
) {
    use unicode_width::UnicodeWidthChar;

    let width = width as usize;
    if width == 0 {
        return;
    }

    let text_style = if is_focused {
        Style::default().fg(theme.fg).bg(theme.bg)
    } else {
        Style::default().fg(theme.fg)
    };
    let selection_style = Style::default().fg(theme.bg).bg(theme.fg);
    let cursor_style = selection_style;

    // Collect chars with their display widths
    let chars: Vec<(usize, char, usize)> = text
        .char_indices()
        .enumerate()
        .map(|(char_idx, (_byte_idx, c))| {
            let cw = UnicodeWidthChar::width(c).unwrap_or(1);
            (char_idx, c, cw)
        })
        .collect();

    let total_chars = chars.len();
    let total_display_width: usize = chars.iter().map(|(_, _, w)| w).sum();

    // Calculate scroll offset (how many chars to skip from start)
    let mut scroll_offset = 0;
    if total_display_width >= width {
        // Need to scroll - ensure cursor is visible
        let mut cursor_display_x = 0;
        for (char_idx, _, cw) in &chars {
            if *char_idx >= cursor_pos {
                break;
            }
            cursor_display_x += cw;
        }

        // If cursor would be past visible area, scroll
        if cursor_display_x >= width {
            let mut skipped_width = 0;
            for (char_idx, _, cw) in &chars {
                if cursor_display_x - skipped_width < width {
                    scroll_offset = *char_idx;
                    break;
                }
                skipped_width += cw;
            }
        }
    }

    // Render characters
    let mut screen_x = x;
    let field_end = x + width as u16;

    for (char_idx, c, cw) in chars.iter().skip(scroll_offset) {
        if screen_x >= field_end {
            break;
        }

        // Determine style for this character
        let is_selected = selection_range
            .map(|(start, end)| *char_idx >= start && *char_idx < end)
            .unwrap_or(false);
        let is_cursor = is_focused && *char_idx == cursor_pos;

        let style = if is_cursor || (is_selected && is_focused) {
            selection_style
        } else {
            text_style
        };

        buf.set_string(screen_x, y, c.to_string(), style);
        screen_x += *cw as u16;
    }

    // Render cursor at end if cursor is past last char
    if is_focused && cursor_pos >= total_chars && screen_x < field_end {
        let is_selected = selection_range
            .map(|(start, end)| cursor_pos >= start && cursor_pos < end)
            .unwrap_or(false);
        let style = if is_selected {
            selection_style
        } else {
            cursor_style
        };
        buf.set_string(screen_x, y, " ", style);
    }
}

/// Render a labeled input field.
#[allow(clippy::too_many_arguments)]
pub fn render_labeled_input(
    buf: &mut Buffer,
    area: Rect,
    label: &str,
    text: &str,
    cursor_pos: usize,
    selection_range: Option<(usize, usize)>,
    is_focused: bool,
    theme: &Theme,
) {
    let label_width = label.len() as u16;

    // Render label
    buf.set_string(area.x, area.y, label, Style::default().fg(theme.fg));

    // Render input field
    let input_x = area.x + label_width;
    let input_width = area.width.saturating_sub(label_width);

    render_input_field(
        buf,
        input_x,
        area.y,
        input_width,
        text,
        cursor_pos,
        selection_range,
        is_focused,
        theme,
    );
}

/// Result of checking mouse click position in a modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseClickResult {
    /// Click was outside the modal area (should close)
    OutsideModal,
    /// Click was outside the list area (ignore)
    OutsideList,
    /// Click was on a valid list item at the given index
    OnListItem(usize),
}

/// Check mouse click position relative to modal and list areas.
///
/// This is a common pattern for search modals that display a list of results.
/// Returns the appropriate action based on click position.
///
/// # Arguments
/// * `mouse_col`, `mouse_row` - Mouse click coordinates
/// * `modal_area` - Optional modal area for outside-click detection
/// * `list_area` - Optional list area for item click detection
/// * `scroll_offset` - Current scroll offset in the list
/// * `lines_per_item` - Number of visual lines per list item (default 1)
pub fn check_mouse_click(
    mouse_col: u16,
    mouse_row: u16,
    modal_area: Option<Rect>,
    list_area: Option<Rect>,
    scroll_offset: usize,
) -> MouseClickResult {
    check_mouse_click_with_item_height(
        mouse_col,
        mouse_row,
        modal_area,
        list_area,
        scroll_offset,
        1,
    )
}

/// Check mouse click with custom item height (lines per item).
///
/// Use this when list items span multiple lines.
pub fn check_mouse_click_with_item_height(
    mouse_col: u16,
    mouse_row: u16,
    modal_area: Option<Rect>,
    list_area: Option<Rect>,
    scroll_offset: usize,
    lines_per_item: usize,
) -> MouseClickResult {
    // Check if click is outside modal - close it
    if let Some(modal_area) = modal_area {
        if mouse_col < modal_area.x
            || mouse_col >= modal_area.x + modal_area.width
            || mouse_row < modal_area.y
            || mouse_row >= modal_area.y + modal_area.height
        {
            return MouseClickResult::OutsideModal;
        }
    }

    let Some(list_area) = list_area else {
        return MouseClickResult::OutsideList;
    };

    // Check if click is within list area
    if mouse_row < list_area.y
        || mouse_row >= list_area.y + list_area.height
        || mouse_col < list_area.x
        || mouse_col >= list_area.x + list_area.width
    {
        return MouseClickResult::OutsideList;
    }

    // Calculate which item was clicked
    let relative_row = (mouse_row - list_area.y) as usize;
    let clicked_index = scroll_offset + relative_row / lines_per_item.max(1);

    MouseClickResult::OnListItem(clicked_index)
}

/// Trait for cursor navigation in search modals.
///
/// Provides default implementations for common navigation patterns
/// (up, down, home, end) with scroll adjustment.
pub trait CursorNavigation {
    /// Get total number of results.
    fn results_len(&self) -> usize;

    /// Get current cursor position.
    fn cursor(&self) -> usize;

    /// Set cursor position.
    fn set_cursor(&mut self, pos: usize);

    /// Get current scroll offset.
    fn scroll_offset(&self) -> usize;

    /// Set scroll offset.
    fn set_scroll_offset(&mut self, offset: usize);

    /// Get maximum visible results count.
    fn max_visible(&self) -> usize;

    /// Move cursor up by one.
    fn cursor_up(&mut self) {
        if self.cursor() > 0 {
            self.set_cursor(self.cursor() - 1);
            self.adjust_scroll();
        }
    }

    /// Move cursor down by one.
    fn cursor_down(&mut self) {
        if self.cursor() < self.results_len().saturating_sub(1) {
            self.set_cursor(self.cursor() + 1);
            self.adjust_scroll();
        }
    }

    /// Move cursor to first result.
    fn cursor_home(&mut self) {
        self.set_cursor(0);
        self.set_scroll_offset(0);
    }

    /// Move cursor to last result.
    fn cursor_end(&mut self) {
        self.set_cursor(self.results_len().saturating_sub(1));
        self.adjust_scroll();
    }

    /// Adjust scroll offset to keep cursor visible.
    fn adjust_scroll(&mut self) {
        let max_visible = self.max_visible();
        self.set_scroll_offset(termide_ui::ensure_offset_visible(
            self.scroll_offset(),
            self.cursor(),
            max_visible,
        ));
    }

    /// Move cursor up by page (max_visible items).
    fn cursor_page_up(&mut self) {
        for _ in 0..self.max_visible() {
            self.cursor_up();
        }
    }

    /// Move cursor down by page (max_visible items).
    fn cursor_page_down(&mut self) {
        for _ in 0..self.max_visible() {
            self.cursor_down();
        }
    }
}

/// Convert a screen X-offset inside a rendered single-line input field to
/// the corresponding character (grapheme-agnostic, char-wise) position in
/// `text`, accounting for double-width characters. Click past the end of
/// the text returns the text length.
pub fn screen_x_to_char_pos(text: &str, screen_x: usize) -> usize {
    use unicode_width::UnicodeWidthChar;
    let mut width = 0;
    for (i, c) in text.chars().enumerate() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(1);
        if width + cw > screen_x {
            return i;
        }
        width += cw;
    }
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_mouse_click_outside_modal() {
        let modal_area = Some(Rect::new(10, 10, 50, 30));
        let list_area = Some(Rect::new(12, 15, 46, 20));

        // Click outside modal bounds
        assert_eq!(
            check_mouse_click(5, 5, modal_area, list_area, 0),
            MouseClickResult::OutsideModal
        );
        assert_eq!(
            check_mouse_click(70, 20, modal_area, list_area, 0),
            MouseClickResult::OutsideModal
        );
    }

    #[test]
    fn test_check_mouse_click_outside_list() {
        let modal_area = Some(Rect::new(10, 10, 50, 30));
        let list_area = Some(Rect::new(12, 15, 46, 20));

        // Click inside modal but outside list area
        assert_eq!(
            check_mouse_click(11, 11, modal_area, list_area, 0),
            MouseClickResult::OutsideList
        );
    }

    #[test]
    fn test_check_mouse_click_on_list_item() {
        let modal_area = Some(Rect::new(10, 10, 50, 30));
        let list_area = Some(Rect::new(12, 15, 46, 20));

        // Click on first item
        assert_eq!(
            check_mouse_click(20, 15, modal_area, list_area, 0),
            MouseClickResult::OnListItem(0)
        );

        // Click on third item
        assert_eq!(
            check_mouse_click(20, 17, modal_area, list_area, 0),
            MouseClickResult::OnListItem(2)
        );

        // Click with scroll offset
        assert_eq!(
            check_mouse_click(20, 15, modal_area, list_area, 5),
            MouseClickResult::OnListItem(5)
        );
    }

    #[test]
    fn test_check_mouse_click_no_list_area() {
        let modal_area = Some(Rect::new(10, 10, 50, 30));

        assert_eq!(
            check_mouse_click(20, 20, modal_area, None, 0),
            MouseClickResult::OutsideList
        );
    }

    /// Test struct implementing CursorNavigation
    struct TestNav {
        cursor: usize,
        scroll: usize,
        len: usize,
        max_visible: usize,
    }

    impl CursorNavigation for TestNav {
        fn results_len(&self) -> usize {
            self.len
        }
        fn cursor(&self) -> usize {
            self.cursor
        }
        fn set_cursor(&mut self, pos: usize) {
            self.cursor = pos;
        }
        fn scroll_offset(&self) -> usize {
            self.scroll
        }
        fn set_scroll_offset(&mut self, offset: usize) {
            self.scroll = offset;
        }
        fn max_visible(&self) -> usize {
            self.max_visible
        }
    }

    #[test]
    fn test_cursor_navigation_up_down() {
        let mut nav = TestNav {
            cursor: 5,
            scroll: 0,
            len: 20,
            max_visible: 10,
        };

        nav.cursor_up();
        assert_eq!(nav.cursor(), 4);

        nav.cursor_down();
        assert_eq!(nav.cursor(), 5);
    }

    #[test]
    fn test_cursor_navigation_bounds() {
        let mut nav = TestNav {
            cursor: 0,
            scroll: 0,
            len: 5,
            max_visible: 10,
        };

        // Can't go below 0
        nav.cursor_up();
        assert_eq!(nav.cursor(), 0);

        // Can't go past len - 1
        nav.cursor = 4;
        nav.cursor_down();
        assert_eq!(nav.cursor(), 4);
    }

    #[test]
    fn test_cursor_navigation_home_end() {
        let mut nav = TestNav {
            cursor: 5,
            scroll: 3,
            len: 20,
            max_visible: 10,
        };

        nav.cursor_home();
        assert_eq!(nav.cursor(), 0);
        assert_eq!(nav.scroll_offset(), 0);

        nav.cursor_end();
        assert_eq!(nav.cursor(), 19);
    }

    #[test]
    fn test_cursor_navigation_scroll_adjustment() {
        let mut nav = TestNav {
            cursor: 15,
            scroll: 5,
            len: 20,
            max_visible: 5,
        };

        // Cursor should trigger scroll adjustment
        nav.adjust_scroll();
        assert_eq!(nav.scroll_offset(), 11); // cursor(15) - max_visible(5) + 1 = 11
    }
}
