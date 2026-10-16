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
    let scroll = termide_ui::text_utils::input_scroll_offset(text, cursor_pos, width as usize);
    render_input_field_scrolled(
        buf,
        x,
        y,
        width,
        text,
        cursor_pos,
        selection_range,
        is_focused,
        theme,
        scroll,
    );
}

/// [`render_input_field`] with the first `scroll` characters of `text` scrolled
/// out of view, for a field that keeps its scroll between frames (see
/// [`TextInput::follow_scroll`](termide_ui::TextInput::follow_scroll)).
#[allow(clippy::too_many_arguments)]
pub fn render_input_field_scrolled(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    text: &str,
    cursor_pos: usize,
    selection_range: Option<(usize, usize)>,
    is_focused: bool,
    theme: &Theme,
    scroll_offset: usize,
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

/// Draw `input` as a single-line field in the one-row `area`, keeping the
/// scroll it had while its cursor stays in view (see
/// [`TextInput::follow_scroll`](termide_ui::TextInput::follow_scroll)), so a
/// click mapped through [`field_char_at`] lands where it points.
pub fn render_text_input(
    buf: &mut Buffer,
    area: Rect,
    input: &mut crate::TextInputHandler,
    is_focused: bool,
    theme: &Theme,
) {
    let scroll = input.follow_scroll(area.width);
    render_input_field_scrolled(
        buf,
        area.x,
        area.y,
        area.width,
        input.text(),
        input.cursor_pos(),
        input.selection_range(),
        is_focused,
        theme,
        scroll,
    );
}

/// The character of `input` under screen column `column` of the field
/// [`render_text_input`] drew in `area`. Left of the field it is the character
/// just scrolled out, so a drag past the edge keeps scrolling.
pub fn field_char_at(input: &crate::TextInputHandler, area: Rect, column: u16) -> usize {
    input.char_at_column(column.checked_sub(area.x).map(usize::from))
}

/// A left press inside `area`, the one-row field [`render_text_input`] last
/// drew for `input`, places its cursor on the character under the pointer and
/// anchors a selection there; a quick second press selects the word there and
/// a third the whole text (see
/// [`TextInput::mouse_press`](termide_ui::TextInput::mouse_press)). Returns
/// whether the press landed in the field, so the caller focuses it and
/// remembers that a drag now belongs to it.
pub fn press_field(
    input: &mut crate::TextInputHandler,
    area: Option<Rect>,
    mouse: &crossterm::event::MouseEvent,
) -> bool {
    press_field_box(input, area, None, mouse)
}

/// [`press_field`] for a field whose text in `text` sits in a box `frame`
/// drawn round it — its border, its padding: a press anywhere on the box
/// counts as one on the text. On the rows above and below the text it lands
/// on the text's row, left of the text at the start of what is shown, right
/// of it at the end. Without a `frame` only the text counts.
pub fn press_field_box(
    input: &mut crate::TextInputHandler,
    text: Option<Rect>,
    frame: Option<Rect>,
    mouse: &crossterm::event::MouseEvent,
) -> bool {
    use crossterm::event::{MouseButton, MouseEventKind};
    let Some(text) = text else {
        return false;
    };
    let point = (mouse.column, mouse.row).into();
    if mouse.kind != MouseEventKind::Down(MouseButton::Left)
        || !(text.contains(point) || frame.is_some_and(|frame| frame.contains(point)))
    {
        return false;
    }
    let x = mouse
        .column
        .saturating_sub(text.x)
        .min(text.width.saturating_sub(1));
    let pos = input.char_at_column(Some(usize::from(x)));
    input.mouse_press(pos);
    true
}

/// The box a one-row field drawn in `text` has when a one-cell border goes
/// all round it, for [`press_field_box`]. With `right_edge` false the box
/// stops at the text's right end, leaving the cells past it — a dropdown
/// arrow, the border beside it — to their own clicks.
pub fn field_box(text: Option<Rect>, right_edge: bool) -> Option<Rect> {
    text.map(|text| {
        Rect::new(
            text.x.saturating_sub(1),
            text.y.saturating_sub(1),
            text.width + if right_edge { 2 } else { 1 },
            text.height + 2,
        )
    })
}

/// A left drag that began with a [`press_field`] on the field in `area`:
/// extends the selection to the character under the pointer, whatever row it
/// is on. Past either side of the field it keeps going to the text's start or
/// end, scrolling the field along.
pub fn drag_field(
    input: &mut crate::TextInputHandler,
    area: Option<Rect>,
    mouse: &crossterm::event::MouseEvent,
) {
    if let Some(area) = area {
        let pos = field_char_at(input, area, mouse.column);
        input.extend_selection_to(pos);
    }
}

/// Left-button mouse events for tests.
#[cfg(test)]
pub(crate) mod test_mouse {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    fn event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    pub(crate) fn press(column: u16, row: u16) -> MouseEvent {
        event(MouseEventKind::Down(MouseButton::Left), column, row)
    }

    pub(crate) fn drag(column: u16, row: u16) -> MouseEvent {
        event(MouseEventKind::Drag(MouseButton::Left), column, row)
    }

    pub(crate) fn release(column: u16, row: u16) -> MouseEvent {
        event(MouseEventKind::Up(MouseButton::Left), column, row)
    }
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

/// Whether `mouse` is a left press outside `modal_area`, the frame the modal
/// last drew: the click that dismisses it, as `Esc` does. `false` before the
/// first render, when there is no frame to be outside of.
#[must_use]
pub fn is_click_outside(mouse: &crossterm::event::MouseEvent, modal_area: Option<Rect>) -> bool {
    use crossterm::event::{MouseButton, MouseEventKind};
    mouse.kind == MouseEventKind::Down(MouseButton::Left)
        && modal_area.is_some_and(|area| !area.contains((mouse.column, mouse.row).into()))
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

    /// A press on a field's border lands on the text: above and below on the
    /// column it points at, left of the text at the start of what is shown,
    /// right of it at the end; outside the box it is not the field's.
    #[test]
    fn a_press_on_a_field_border_places_the_cursor() {
        use super::test_mouse::press;
        let text = Some(Rect::new(5, 5, 10, 1));
        let frame = field_box(text, true);
        assert_eq!(frame, Some(Rect::new(4, 4, 12, 3)));
        let mut input = crate::TextInputHandler::with_text("hello");
        // Each press lands on another character, so none reads as a double
        // click.
        let at = |input: &mut crate::TextInputHandler, column, row| {
            press_field_box(input, text, frame, &press(column, row)).then(|| input.cursor_pos())
        };
        assert_eq!(at(&mut input, 4, 5), Some(0));
        assert_eq!(at(&mut input, 7, 4), Some(2));
        assert_eq!(at(&mut input, 8, 6), Some(3));
        assert_eq!(at(&mut input, 15, 5), Some(5));
        assert_eq!(at(&mut input, 3, 5), None);
        assert_eq!(at(&mut input, 7, 7), None);
        // Without a box only the text counts.
        assert!(!press_field_box(&mut input, text, None, &press(4, 5)));

        // Scrolled: the borders reach the first and last characters shown.
        let mut long = crate::TextInputHandler::with_text("x".repeat(30));
        long.follow_scroll(10);
        let first = long.scroll();
        assert!(first > 0);
        assert_eq!(at(&mut long, 4, 5), Some(first));
        assert_eq!(at(&mut long, 15, 6), Some(30));
        // The box without its right edge leaves the right border alone.
        let combo = field_box(text, false);
        assert!(!press_field_box(&mut input, text, combo, &press(15, 5)));
        assert!(press_field_box(&mut input, text, combo, &press(4, 6)));
    }
}
