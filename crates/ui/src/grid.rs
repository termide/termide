//! A 2D table drawn in pseudographics: a bold header row, a window of data
//! rows below it, and a cell cursor.
//!
//! Each column is a slot ` content ` (one space of padding on each side);
//! slots are separated by a `│` divider, so a slot highlighted edge to edge
//! reads as one cell bounded by the dividers. Column widths follow the header
//! and the rows in the window, capped at [`MAX_COL_WIDTH`]; longer values are
//! cut with an ellipsis.
//!
//! The widget does not own the data or decide what a window is: the host
//! hands it the rows it holds through [`GridRows`] — a page fetched from a
//! database, or a whole file in memory — and says where that window sits in
//! the full table through [`GridWindow`], for the vertical scrollbar. Moving
//! the cursor between rows, and paging, stay with the host; the widget keeps
//! the cursor and the scroll visible and records the layout for mouse
//! hit-testing in [`GridState`].

use std::borrow::Cow;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use termide_core::{ScrollBars, ThemeColors};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{ScrollBar, TextInput};

/// Widest a column grows, in display cells, before its values are cut.
pub const MAX_COL_WIDTH: usize = 40;
const MIN_COL_WIDTH: usize = 3;
/// Display width a column occupies besides its content: two padding spaces
/// and the `│` divider.
const COL_OVERHEAD: usize = 3;

/// One value as the grid shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridCell<'a> {
    pub text: Cow<'a, str>,
    /// Drawn dimmed: a placeholder such as `NULL` rather than a value.
    pub muted: bool,
}

impl<'a> GridCell<'a> {
    /// A plain value.
    pub fn new(text: impl Into<Cow<'a, str>>) -> Self {
        Self {
            text: text.into(),
            muted: false,
        }
    }

    /// A placeholder, drawn dimmed.
    pub fn muted(text: impl Into<Cow<'a, str>>) -> Self {
        Self {
            text: text.into(),
            muted: true,
        }
    }
}

/// The rows the host holds: the window the grid draws from.
pub trait GridRows {
    /// Number of rows in the window.
    fn row_count(&self) -> usize;
    /// The cell at `row` (within the window) and `col`; `None` draws blank.
    fn cell(&self, row: usize, col: usize) -> Option<GridCell<'_>>;
}

/// Where the rows handed to the grid sit in the whole table, for the vertical
/// scrollbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridWindow {
    /// Index of the window's first row in the whole table.
    pub offset: usize,
    /// Rows in the whole table; `None` while unknown, which hides the bar.
    pub total: Option<usize>,
}

/// Sort direction marked in a column's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnSort {
    Ascending,
    Descending,
}

impl ColumnSort {
    /// The arrow drawn after the column name.
    pub fn arrow(self) -> &'static str {
        match self {
            Self::Ascending => "↑",
            Self::Descending => "↓",
        }
    }
}

/// A cell being edited in place: the grid draws the text field in its slot.
pub struct GridEditor<'a> {
    /// Row within the window.
    pub row: usize,
    pub col: usize,
    pub input: &'a mut TextInput,
    /// The value is being written: the text is dimmed and no cursor drawn.
    pub saving: bool,
}

/// What a mouse position falls on, as of the last render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridHit {
    /// A column header.
    Header(usize),
    /// A drawn data row (within the window), and the column under the
    /// pointer — `None` right of the last column.
    Cell { row: usize, col: Option<usize> },
}

/// Layout of the last render, for hit-testing.
#[derive(Debug, Clone, Default)]
struct GridLayout {
    /// Y of the header row; `None` when the grid was not drawn.
    header_y: Option<u16>,
    /// Number of data rows drawn below the header.
    rows_drawn: usize,
    /// Per visible column: (column index, x start, x end exclusive).
    columns: Vec<(usize, u16, u16)>,
}

/// Cursor and scroll of a grid, kept by the host between frames.
#[derive(Debug, Clone, Default)]
pub struct GridState {
    /// Cursor row within the window.
    pub cursor_row: usize,
    pub cursor_col: usize,
    /// First window row on screen.
    pub row_scroll: usize,
    /// First column on screen.
    pub col_scroll: usize,
    visible_rows: usize,
    scrollbars: ScrollBars,
    layout: GridLayout,
}

impl GridState {
    /// Back to the first cell, scrolled to the start.
    pub fn reset(&mut self) {
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.row_scroll = 0;
        self.col_scroll = 0;
    }

    /// Data rows that fit on screen at the last render.
    pub fn visible_rows(&self) -> usize {
        self.visible_rows
    }

    /// Scrollbars drawn by the last render, for dragging their thumbs.
    pub fn scrollbars(&self) -> ScrollBars {
        self.scrollbars
    }

    /// Forget the last layout: the grid is not on screen this frame, so
    /// nothing should hit-test against it.
    pub fn hide(&mut self) {
        self.layout = GridLayout::default();
        self.scrollbars = ScrollBars::default();
    }

    /// Keep the cursor inside a window of `rows` rows and `cols` columns.
    pub fn clamp(&mut self, rows: usize, cols: usize) {
        self.cursor_row = self.cursor_row.min(rows.saturating_sub(1));
        self.cursor_col = self.cursor_col.min(cols.saturating_sub(1));
    }

    /// One column left; false at the first.
    pub fn move_left(&mut self) -> bool {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            true
        } else {
            false
        }
    }

    /// One column right within `cols` columns; false at the last.
    pub fn move_right(&mut self, cols: usize) -> bool {
        if self.cursor_col + 1 < cols {
            self.cursor_col += 1;
            true
        } else {
            false
        }
    }

    /// What the screen position (`x`, `y`) falls on.
    pub fn hit(&self, x: u16, y: u16) -> Option<GridHit> {
        let header_y = self.layout.header_y?;
        let col = self
            .layout
            .columns
            .iter()
            .find(|(_, start, end)| x >= *start && x < *end)
            .map(|(idx, _, _)| *idx);
        if y == header_y {
            return col.map(GridHit::Header);
        }
        let vis = y.checked_sub(header_y + 1)? as usize;
        (vis < self.layout.rows_drawn).then_some(GridHit::Cell {
            row: self.row_scroll + vis,
            col,
        })
    }

    /// Scroll so the cursor column fits within `avail` cells.
    fn follow_cursor_col(&mut self, widths: &[usize], avail: usize) {
        if self.cursor_col < self.col_scroll {
            self.col_scroll = self.cursor_col;
            return;
        }
        loop {
            let mut used = 0usize;
            let mut last_visible = self.col_scroll;
            for (j, w) in widths.iter().enumerate().skip(self.col_scroll) {
                let need = w + COL_OVERHEAD;
                if used + need > avail && j > self.col_scroll {
                    break;
                }
                used += need;
                last_visible = j;
            }
            if self.cursor_col <= last_visible || self.col_scroll >= widths.len().saturating_sub(1)
            {
                break;
            }
            self.col_scroll += 1;
        }
    }

    /// Scroll so the cursor row is among the `height` rows on screen.
    fn follow_cursor_row(&mut self, height: usize) {
        if self.cursor_row < self.row_scroll {
            self.row_scroll = self.cursor_row;
        } else if height > 0 && self.cursor_row >= self.row_scroll + height {
            self.row_scroll = self.cursor_row + 1 - height;
        }
    }
}

/// The grid widget for one frame.
pub struct Grid<'a, R: GridRows + ?Sized> {
    columns: &'a [String],
    rows: &'a R,
    window: GridWindow,
    sort: Option<(usize, ColumnSort)>,
    show_cursor: bool,
    editor: Option<GridEditor<'a>>,
    border_right_x: Option<u16>,
    border_bottom_y: Option<u16>,
}

impl<'a, R: GridRows + ?Sized> Grid<'a, R> {
    /// A grid of `columns` over `rows`, taken as the whole table until
    /// [`window`](Self::window) says otherwise.
    pub fn new(columns: &'a [String], rows: &'a R) -> Self {
        Self {
            columns,
            rows,
            window: GridWindow {
                offset: 0,
                total: Some(rows.row_count()),
            },
            sort: None,
            show_cursor: false,
            editor: None,
            border_right_x: None,
            border_bottom_y: None,
        }
    }

    /// Where `rows` sit in the whole table.
    pub fn window(mut self, window: GridWindow) -> Self {
        self.window = window;
        self
    }

    /// Mark column `col` as sorted.
    pub fn sort(mut self, sort: Option<(usize, ColumnSort)>) -> Self {
        self.sort = sort;
        self
    }

    /// Highlight the cursor row and cell (the grid has the focus).
    pub fn show_cursor(mut self, show: bool) -> Self {
        self.show_cursor = show;
        self
    }

    /// Draw an open cell editor in its slot.
    pub fn editor(mut self, editor: Option<GridEditor<'a>>) -> Self {
        self.editor = editor;
        self
    }

    /// Put the scrollbars on the panel frame at these coordinates when the
    /// chrome provides them; otherwise a content row and column are kept for
    /// them, so they never cover data.
    pub fn borders(mut self, right_x: Option<u16>, bottom_y: Option<u16>) -> Self {
        self.border_right_x = right_x;
        self.border_bottom_y = bottom_y;
        self
    }

    /// Draw into `area`: the header on its first row, data below.
    #[allow(clippy::needless_range_loop)]
    pub fn render(
        mut self,
        area: Rect,
        buf: &mut Buffer,
        state: &mut GridState,
        theme: &ThemeColors,
        is_focused: bool,
    ) {
        state.hide();
        if area.width == 0 || area.height == 0 {
            state.visible_rows = 0;
            return;
        }
        let base = Style::default().fg(theme.fg).bg(theme.bg);
        let names = self.columns;
        let total_cols = names.len();

        // Room for the sort arrow in the sorted column's header.
        let mut widths = self.column_widths();
        if let Some((idx, _)) = self.sort {
            if let Some(w) = widths.get_mut(idx) {
                *w = (*w + 2).min(MAX_COL_WIDTH);
            }
        }

        let total_col_w: usize = widths.iter().map(|w| w + COL_OVERHEAD).sum();
        let h_needed = total_col_w > area.width as usize;
        let h_reserve = usize::from(h_needed && self.border_bottom_y.is_none());

        let data_height = (area.height.saturating_sub(1) as usize).saturating_sub(h_reserve);
        let v_needed = matches!(self.window.total, Some(t) if t > data_height);
        let v_reserve = u16::from(v_needed && self.border_right_x.is_none());
        let content_width = area.width.saturating_sub(v_reserve);
        state.visible_rows = data_height;

        state.follow_cursor_row(data_height);
        state.follow_cursor_col(&widths, content_width as usize);

        let max_x = area.x + content_width;
        let border = base.fg(theme.border);

        // --- header row ---
        state.layout.header_y = Some(area.y);
        fill_line(buf, area.x, area.y, area.width, base);
        let mut x = area.x;
        for j in state.col_scroll..total_cols {
            if x >= max_x {
                break;
            }
            let mut label = names[j].clone();
            if let Some((idx, dir)) = self.sort {
                if idx == j {
                    label.push(' ');
                    label.push_str(dir.arrow());
                }
            }
            let slot = format!(" {} ", pad(&label, widths[j]));
            let slot_start = x;
            x = put(
                buf,
                x,
                area.y,
                max_x,
                &slot,
                base.add_modifier(Modifier::BOLD),
            );
            state.layout.columns.push((j, slot_start, x));
            x = put(buf, x, area.y, max_x, "│", border);
        }

        // --- data rows ---
        // Cursor row: normal colours, bold. Cursor cell: inverse video across
        // the whole slot, border to border.
        let row_count = self.rows.row_count();
        for vis in 0..data_height {
            let abs = state.row_scroll + vis;
            if abs >= row_count {
                break;
            }
            state.layout.rows_drawn = vis + 1;
            let y = area.y + 1 + vis as u16;
            let is_cur_row = self.show_cursor && abs == state.cursor_row;
            let row_style = if is_cur_row {
                base.add_modifier(Modifier::BOLD)
            } else {
                base
            };
            fill_line(buf, area.x, y, area.width, base);

            let mut x = area.x;
            for j in state.col_scroll..total_cols {
                if x >= max_x {
                    break;
                }
                if let Some(edit) = self.editor.as_mut().filter(|e| e.row == abs && e.col == j) {
                    // The editor owns the slot, without the inverse-video
                    // cell highlight that would hide its caret.
                    x = render_cell_editor(buf, x, y, max_x, widths[j], edit, row_style, theme);
                    x = put(buf, x, y, max_x, "│", border);
                    continue;
                }
                let cell = self.rows.cell(abs, j);
                let (text, muted) = match &cell {
                    Some(c) => (c.text.as_ref(), c.muted),
                    None => ("", false),
                };
                let is_cur_cell = is_cur_row && j == state.cursor_col;
                let slot = format!(" {} ", pad(text, widths[j]));
                let mut style = row_style;
                if muted && !is_cur_cell {
                    style = style.fg(theme.disabled);
                }
                if is_cur_cell {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                x = put(buf, x, y, max_x, &slot, style);
                x = put(buf, x, y, max_x, "│", border);
            }
        }

        // --- scrollbars, drawn last so they sit above the grid ---
        if v_needed {
            let offset = self.window.offset + state.row_scroll;
            let total = self.window.total.unwrap_or(0);
            let vx = self.border_right_x.unwrap_or(area.x + area.width - 1);
            state.scrollbars.vertical = ScrollBar::render_tracked(
                buf,
                vx,
                area.y + 1,
                data_height as u16,
                offset,
                data_height,
                total,
                theme,
                is_focused,
            );
        }
        if h_needed {
            // Count only fully visible columns, so a clipped rightmost column
            // still reports "more to the right" at col_scroll == 0.
            let mut used = 0usize;
            let mut visible_cols = 0usize;
            for w in widths.iter().skip(state.col_scroll) {
                let need = w + COL_OVERHEAD;
                if used + need > content_width as usize {
                    break;
                }
                used += need;
                visible_cols += 1;
            }
            let hy = self.border_bottom_y.unwrap_or(area.y + area.height - 1);
            state.scrollbars.horizontal = ScrollBar::render_horizontal_tracked(
                buf,
                area.x,
                hy,
                content_width,
                state.col_scroll,
                visible_cols.max(1),
                total_cols,
                theme,
                is_focused,
            );
        }
    }

    /// Per-column display widths from the header and the rows in the window.
    fn column_widths(&self) -> Vec<usize> {
        let mut widths: Vec<usize> = self
            .columns
            .iter()
            .map(|n| UnicodeWidthStr::width(n.as_str()).max(MIN_COL_WIDTH))
            .collect();
        for row in 0..self.rows.row_count() {
            for (j, w) in widths.iter_mut().enumerate() {
                if let Some(cell) = self.rows.cell(row, j) {
                    *w = (*w).max(UnicodeWidthStr::width(cell.text.as_ref()));
                }
            }
        }
        for w in widths.iter_mut() {
            *w = (*w).min(MAX_COL_WIDTH);
        }
        widths
    }
}

/// Fill a single row with spaces in `style` (background paint).
fn fill_line(buf: &mut Buffer, x: u16, y: u16, width: u16, style: Style) {
    let blanks = " ".repeat(width as usize);
    buf.set_stringn(x, y, &blanks, width as usize, style);
}

/// Write a string clipped to `max_x`; returns the new x.
fn put(buf: &mut Buffer, x: u16, y: u16, max_x: u16, s: &str, style: Style) -> u16 {
    if x >= max_x {
        return x;
    }
    let budget = (max_x - x) as usize;
    let (nx, _) = buf.set_stringn(x, y, s, budget, style);
    nx
}

/// Pad or cut `s` to display width `w`; a cut ends with an ellipsis.
fn pad(s: &str, w: usize) -> String {
    let sw = UnicodeWidthStr::width(s);
    if sw <= w {
        return format!("{s}{}", " ".repeat(w - sw));
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4]) as &str);
        if used + cw > w.saturating_sub(1) {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    // A wide character left out can leave the cut short of `w`.
    let ow = UnicodeWidthStr::width(out.as_str());
    if ow < w {
        out.push_str(&" ".repeat(w - ow));
    }
    out
}

/// Draw an open cell editor into its slot.
///
/// The visible part scrolls with the cursor so a value longer than the column
/// stays editable; the cursor cell and the selection are inverse video, as in
/// any other text field. While the value is being saved the text is dimmed and
/// no cursor is drawn.
#[allow(clippy::too_many_arguments)]
fn render_cell_editor(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    max_x: u16,
    width: usize,
    edit: &mut GridEditor<'_>,
    row_style: Style,
    theme: &ThemeColors,
) -> u16 {
    let base = if edit.saving {
        row_style.fg(theme.disabled)
    } else {
        row_style.fg(theme.fg).bg(theme.selection_bg)
    };
    let marked = base.add_modifier(Modifier::REVERSED);
    let scroll = edit
        .input
        .follow_scroll(width.min(usize::from(u16::MAX)) as u16);

    let mut next_x = put(buf, x, y, max_x, " ", base);
    let text_start = next_x;
    next_x = put(buf, next_x, y, max_x, &" ".repeat(width), base);
    let text_end = (text_start as usize + width).min(max_x as usize) as u16;

    let cursor = edit.input.cursor_pos();
    let selection = edit.input.selection_range();
    let show_marks = !edit.saving;
    let mut col = text_start;
    let mut shown = scroll;
    for (i, c) in edit.input.text().chars().enumerate().skip(scroll) {
        let w = UnicodeWidthChar::width(c).unwrap_or(1).max(1) as u16;
        if col + w > text_end {
            break;
        }
        let selected = selection.is_some_and(|(start, end)| i >= start && i < end);
        let style = if show_marks && (i == cursor || selected) {
            marked
        } else {
            base
        };
        buf.set_string(col, y, c.to_string(), style);
        col += w;
        shown = i + 1;
    }
    // A cursor past the last character sits on the blank after it.
    if show_marks
        && cursor >= shown
        && cursor == edit.input.text().chars().count()
        && col < text_end
    {
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_style(marked);
        }
    }

    put(buf, next_x, y, max_x, " ", base)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rows(Vec<Vec<&'static str>>);

    impl GridRows for Rows {
        fn row_count(&self) -> usize {
            self.0.len()
        }
        fn cell(&self, row: usize, col: usize) -> Option<GridCell<'_>> {
            let text = *self.0.get(row)?.get(col)?;
            Some(if text == "NULL" {
                GridCell::muted(text)
            } else {
                GridCell::new(text)
            })
        }
    }

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    fn line(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol())
            .collect::<String>()
    }

    fn draw(
        cols: &[String],
        rows: &Rows,
        state: &mut GridState,
        width: u16,
        height: u16,
    ) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        Grid::new(cols, rows).show_cursor(true).render(
            area,
            &mut buf,
            state,
            &ThemeColors::default(),
            true,
        );
        buf
    }

    #[test]
    fn draws_header_and_rows_in_slots_between_dividers() {
        let cols = names(&["id", "name"]);
        let rows = Rows(vec![vec!["1", "alpha"], vec!["2", "NULL"]]);
        let mut state = GridState::default();
        let buf = draw(&cols, &rows, &mut state, 20, 4);
        assert_eq!(line(&buf, 0), " id  │ name  │      ");
        assert_eq!(line(&buf, 1), " 1   │ alpha │      ");
        assert_eq!(line(&buf, 2), " 2   │ NULL  │      ");
        assert!(buf[(1, 1)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(7, 1)].modifier.contains(Modifier::REVERSED));
        assert_eq!(state.visible_rows(), 3);
    }

    #[test]
    fn a_long_value_is_cut_with_an_ellipsis() {
        assert_eq!(pad("abcdef", 4), "abc…");
        assert_eq!(pad("ab", 4), "ab  ");
        // A wide character that does not fit leaves the cut one cell short.
        assert_eq!(pad("日本語", 4), "日… ");
    }

    #[test]
    fn the_cursor_column_scrolls_into_view() {
        let cols = names(&["aaaa", "bbbb", "cccc", "dddd"]);
        let rows = Rows(vec![vec!["1", "2", "3", "4"]]);
        let mut state = GridState {
            cursor_col: 3,
            ..GridState::default()
        };
        let buf = draw(&cols, &rows, &mut state, 16, 3);
        assert!(state.col_scroll > 0);
        assert!(line(&buf, 0).contains("dddd"));
        assert!(state.scrollbars().horizontal.is_some());
    }

    #[test]
    fn hit_maps_screen_positions_to_headers_and_cells() {
        let cols = names(&["id", "name"]);
        let rows = Rows(vec![vec!["1", "alpha"], vec!["2", "beta"]]);
        let mut state = GridState::default();
        draw(&cols, &rows, &mut state, 20, 6);
        assert_eq!(state.hit(7, 0), Some(GridHit::Header(1)));
        assert_eq!(
            state.hit(1, 2),
            Some(GridHit::Cell {
                row: 1,
                col: Some(0)
            })
        );
        // Right of the last column: the row, no column.
        assert_eq!(state.hit(19, 1), Some(GridHit::Cell { row: 0, col: None }));
        // Below the last drawn row.
        assert_eq!(state.hit(1, 4), None);
        state.hide();
        assert_eq!(state.hit(7, 0), None);
    }

    #[test]
    fn the_sorted_column_carries_its_arrow() {
        let cols = names(&["id"]);
        let rows = Rows(vec![vec!["1"]]);
        let area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(area);
        let mut state = GridState::default();
        Grid::new(&cols, &rows)
            .sort(Some((0, ColumnSort::Descending)))
            .render(area, &mut buf, &mut state, &ThemeColors::default(), true);
        // Two cells are kept for the arrow on top of the name's width.
        assert!(line(&buf, 0).starts_with(" id ↓  │"));
    }

    #[test]
    fn the_editor_draws_the_typed_text_in_its_slot() {
        let cols = names(&["name"]);
        let rows = Rows(vec![vec!["alpha"]]);
        let mut input = TextInput::with_text("new");
        let area = Rect::new(0, 0, 12, 2);
        let mut buf = Buffer::empty(area);
        let mut state = GridState::default();
        Grid::new(&cols, &rows)
            .show_cursor(true)
            .editor(Some(GridEditor {
                row: 0,
                col: 0,
                input: &mut input,
                saving: false,
            }))
            .render(area, &mut buf, &mut state, &ThemeColors::default(), true);
        assert!(line(&buf, 1).starts_with(" new   │"));
        // The caret past the end is the inverse-video blank after the text.
        assert!(buf[(4, 1)].modifier.contains(Modifier::REVERSED));
    }
}
