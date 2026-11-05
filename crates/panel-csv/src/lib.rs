//! Table viewer for delimited text files (`.csv`, `.tsv`).
//!
//! Opened by View (`F3`) on such a file; the toggle-view key (or the `Edit`
//! status chip) swaps it in place for the source in the editor, and back. The
//! grid itself is [`termide_ui::Grid`].
//!
//! The file is never held in memory: a background pass indexes where its
//! records start (see [`index`]), and only the window of rows around the
//! screen is read from disk. Rows show as soon as the first ones are
//! indexed; the count grows while the rest are.
//!
//! The delimiter and whether the first record names the columns are guessed
//! (see [`table`]) and can be switched from the status bar or the panel menu.
//! A click on a column header, or `s`, cycles the sort by that column —
//! ascending, descending, file order; a sort reads the column once more in
//! the background and keeps the file order on screen until it is done. `y`
//! or Copy copies the cell under the cursor; the panel menu copies the whole
//! row. `Ctrl+R` re-reads the file.

mod index;
mod table;

use std::any::Any;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use csv::StringRecord;
use termide_core::{
    CommandResult, Config, HotkeyTable, KeyChord, Panel, PanelCommand, PanelEvent, PanelState,
    RenderContext, ScrollAxis, SegmentKind, StatusSegment, Theme, ThemeColors, WidthPreference,
};
use termide_ui::{ColumnSort, Grid, GridCell, GridHit, GridRows, GridState, GridWindow};

use crate::index::{Index, SortJob, SortSpec, Update};

/// Rows moved per mouse wheel notch.
const WHEEL_ROWS: usize = 3;

/// Table viewer for one delimited file.
pub struct CsvPanel {
    path: PathBuf,
    title: String,

    /// Delimiter chosen by the user; `None` follows the guess.
    delimiter: Option<u8>,
    /// Header choice by the user; `None` follows the guess.
    header: Option<bool>,

    /// The indexer, while it runs.
    indexing: Option<Receiver<Update>>,
    index: Index,
    error: Option<String>,

    /// The sort asked for.
    sort: Option<(usize, ColumnSort)>,
    /// The sort being worked out, while it is.
    sorting: Option<SortJob>,
    /// The sort on screen: the record offsets of the data rows in its order.
    sorted: Option<Sorted>,

    /// Cursor row and first row on screen, among all data rows.
    cursor: usize,
    top: usize,
    /// Data rows read from disk around the screen.
    window: Window,
    grid: GridState,

    colors: ThemeColors,
    hotkeys: HotkeyTable,
    last_config_ptr: usize,
}

struct Sorted {
    by: (usize, ColumnSort),
    offsets: Vec<u64>,
}

/// Data rows read from disk: the `span` asked for, and the records it held
/// (fewer when the file ended earlier than the index said).
#[derive(Default)]
struct Window {
    span: Range<usize>,
    records: Vec<StringRecord>,
}

/// The rows on screen, as the grid reads them.
struct Rows<'a>(&'a [StringRecord]);

impl GridRows for Rows<'_> {
    fn row_count(&self) -> usize {
        self.0.len()
    }

    fn cell(&self, row: usize, col: usize) -> Option<GridCell<'_>> {
        Some(GridCell::new(table::one_line(self.0.get(row)?.get(col)?)))
    }
}

impl CsvPanel {
    /// Open `path` in the table viewer; indexing starts at once, in the
    /// background.
    pub fn new(path: PathBuf) -> Self {
        let title = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        let mut panel = Self {
            path,
            title,
            delimiter: None,
            header: None,
            indexing: None,
            index: Index::default(),
            error: None,
            sort: None,
            sorting: None,
            sorted: None,
            cursor: 0,
            top: 0,
            window: Window::default(),
            grid: GridState::default(),
            colors: ThemeColors::default(),
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
        };
        panel.start_indexing();
        panel
    }

    /// (Re)index the file with the current delimiter, and redo the sort on
    /// it: the offsets of the old pass may no longer hold.
    fn start_indexing(&mut self) {
        self.index = Index::default();
        self.window = Window::default();
        self.sorted = None;
        self.error = None;
        match index::spawn_index(self.path.clone(), self.delimiter) {
            Ok(rx) => self.indexing = Some(rx),
            Err(e) => self.error = Some(e.to_string()),
        }
        self.start_sort();
    }

    /// Work out the sort asked for in the background, or drop the one on
    /// screen when none is.
    fn start_sort(&mut self) {
        self.sorting = None;
        let Some((column, dir)) = self.sort else {
            if self.sorted.take().is_some() {
                self.window = Window::default();
            }
            return;
        };
        let spec = SortSpec {
            delimiter: self.delimiter.unwrap_or(self.index.delimiter),
            skip_first: self.has_header(),
            column,
            descending: dir == ColumnSort::Descending,
        };
        match index::spawn_sort(self.path.clone(), spec) {
            Ok(job) => self.sorting = Some(job),
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Take in what the indexer and the sort have done; true when something
    /// changed.
    fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Some(rx) = &self.indexing {
            let update = match rx.try_recv() {
                Ok(update) => update,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => Update::Failed("the indexer stopped".into()),
            };
            changed = true;
            match update {
                Update::Done => self.indexing = None,
                Update::Failed(e) => {
                    self.indexing = None;
                    self.error = Some(e);
                }
                update => {
                    self.index.apply(update);
                }
            }
        }
        if let Some(job) = &self.sorting {
            let result = match job.result.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err("the sort stopped".into())),
            };
            if let Some(result) = result {
                changed = true;
                self.sorting = None;
                match (result, self.sort) {
                    (Ok(offsets), Some(by)) => {
                        self.sorted = Some(Sorted { by, offsets });
                        self.window = Window::default();
                    }
                    (Ok(_), None) => {}
                    (Err(e), _) => self.error = Some(e),
                }
            }
        }
        changed
    }

    /// Whether the first record names the columns.
    fn has_header(&self) -> bool {
        self.header
            .unwrap_or_else(|| self.index.first.as_ref().is_some_and(table::guess_header))
    }

    /// Data rows: in the sort on screen, or all records but the header.
    fn row_count(&self) -> usize {
        match &self.sorted {
            Some(sorted) => sorted.offsets.len(),
            None => self
                .index
                .records
                .saturating_sub(usize::from(self.has_header())),
        }
    }

    fn column_names(&self) -> Vec<String> {
        let header = self
            .has_header()
            .then_some(self.index.first.as_ref())
            .flatten();
        table::column_names(header, self.index.width)
    }

    /// Have data rows `rows` read, with as many again on either side so
    /// that scrolling a little reads nothing. A window that holds them
    /// already is kept.
    fn load_window(&mut self, rows: Range<usize>) {
        let total = self.row_count();
        let rows = rows.start.min(total)..rows.end.min(total);
        let span = &self.window.span;
        if span.start <= rows.start && rows.end <= span.end {
            return;
        }
        let margin = rows.len().max(1);
        let span = rows.start.saturating_sub(margin)..(rows.end + margin).min(total);
        let read = match &self.sorted {
            Some(sorted) => index::read_at(
                &self.path,
                self.index.delimiter,
                &sorted.offsets[span.clone()],
            ),
            None => {
                let skip = usize::from(self.has_header());
                self.index.read(&self.path, span.start + skip, span.len())
            }
        };
        match read {
            Ok(records) => self.window = Window { span, records },
            Err(e) => self.error = Some(e),
        }
    }

    /// Data row `row`, read in if it is not in the window.
    fn record(&mut self, row: usize) -> Option<StringRecord> {
        self.load_window(row..row + 1);
        self.window
            .records
            .get(row.checked_sub(self.window.span.start)?)
            .cloned()
    }

    /// Cycle the sort of column `col`: ascending, descending, file order.
    fn cycle_sort(&mut self, col: usize) {
        self.sort = match self.sort {
            Some((c, ColumnSort::Ascending)) if c == col => Some((col, ColumnSort::Descending)),
            Some((c, ColumnSort::Descending)) if c == col => None,
            _ => Some((col, ColumnSort::Ascending)),
        };
        self.start_sort();
    }

    fn toggle_header(&mut self) {
        self.header = Some(!self.has_header());
        self.window = Window::default();
        // The header leaves the data rows or joins them: sort them again.
        self.sorted = None;
        self.start_sort();
    }

    /// Re-read the file split by the next delimiter.
    fn cycle_delimiter(&mut self) {
        let current = self.delimiter.unwrap_or(self.index.delimiter);
        self.delimiter = Some(table::next_delimiter(current));
        // The columns change with the delimiter: a sort by one would land on
        // whatever falls in its place.
        self.sort = None;
        self.cursor = 0;
        self.top = 0;
        self.grid.reset();
        self.start_indexing();
    }

    /// Re-read the file, keeping the delimiter, the sort and the cursor.
    fn refresh(&mut self) {
        self.start_indexing();
    }

    fn copy_cell(&mut self) -> Vec<PanelEvent> {
        let col = self.grid.cursor_col;
        let Some(text) = self
            .record(self.cursor)
            .and_then(|r| r.get(col).map(str::to_string))
        else {
            return vec![];
        };
        copied(text, termide_i18n::t().csv_copied_cell())
    }

    /// The row under the cursor, tab-separated, ready to paste into a
    /// spreadsheet.
    fn copy_row(&mut self) -> Vec<PanelEvent> {
        let Some(record) = self.record(self.cursor) else {
            return vec![];
        };
        let text = record
            .iter()
            .map(|f| f.replace(['\t', '\n', '\r'], " "))
            .collect::<Vec<_>>()
            .join("\t");
        copied(text, termide_i18n::t().csv_copied_row())
    }

    fn move_rows(&mut self, delta: isize) -> bool {
        let last = self.row_count().saturating_sub(1);
        let row = self.cursor.saturating_add_signed(delta).min(last);
        let moved = row != self.cursor;
        self.cursor = row;
        moved
    }

    /// Keep the cursor among the `page` rows on screen from `top`; once the
    /// count is final, keep both within it.
    fn follow_cursor(&mut self, page: usize) {
        let total = self.row_count();
        if self.indexing.is_none() {
            self.cursor = self.cursor.min(total.saturating_sub(1));
            self.top = self.top.min(total.saturating_sub(page));
        }
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + page {
            self.top = self.cursor + 1 - page;
        }
    }

    fn redraw() -> Vec<PanelEvent> {
        vec![PanelEvent::NeedsRedraw]
    }

    fn center_message(&self, buf: &mut Buffer, area: Rect, msg: &str, style: Style) {
        let y = area.y + area.height / 2;
        let w = UnicodeWidthStr::width(msg).min(area.width as usize);
        let x = area.x + (area.width.saturating_sub(w as u16)) / 2;
        buf.set_stringn(x, y, msg, area.width as usize, style);
    }
}

fn copied(text: String, message: &str) -> Vec<PanelEvent> {
    vec![
        PanelEvent::CopyToClipboard(text),
        PanelEvent::SetStatusMessage {
            message: message.to_string(),
            is_error: false,
        },
    ]
}

impl Panel for CsvPanel {
    fn name(&self) -> &'static str {
        "csv"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferWide
    }

    fn prepare_render(&mut self, theme: &Theme, config: &Arc<Config>) {
        self.colors = ThemeColors::from(theme);
        let ptr = Arc::as_ptr(config) as usize;
        if self.last_config_ptr != ptr {
            self.last_config_ptr = ptr;
            let mut t = HotkeyTable::new();
            t.insert("toggle_view", &config.viewer.keybindings.toggle_view);
            self.hotkeys = t;
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        self.grid.hide();
        if area.width == 0 || area.height == 0 {
            return;
        }
        let base = Style::default().fg(self.colors.fg).bg(self.colors.bg);
        buf.set_style(area, base);

        // Rows on screen: as many as the last frame had room for, or at most
        // the area below the header before the first one.
        let page = match self.grid.visible_rows() {
            0 => area.height.saturating_sub(1) as usize,
            rows => rows,
        }
        .max(1);
        // Ask for the whole area, not just the rows the last frame showed:
        // a frame grown taller still finds its rows.
        let height = area.height as usize;
        if self.error.is_none() {
            self.follow_cursor(page);
            self.load_window(self.top..self.top + height);
        }

        if let Some(err) = &self.error {
            let msg = format!(" Cannot open: {err}");
            buf.set_stringn(
                area.x,
                area.y,
                &msg,
                area.width as usize,
                base.fg(self.colors.error),
            );
            return;
        }
        let t = termide_i18n::t();
        let total = self.row_count();
        if self.index.records == 0 {
            if self.indexing.is_some() {
                self.center_message(buf, area, t.csv_loading(), base.fg(self.colors.info));
            }
            return;
        }

        let skip = self.top.saturating_sub(self.window.span.start);
        let records = &self.window.records;
        let shown = &records[skip.min(records.len())..(skip + height).min(records.len())];

        // The grid sees the screen's rows as its window, the cursor in it.
        self.grid.cursor_row = self.cursor - self.top;
        self.grid.row_scroll = 0;
        let names = self.column_names();
        Grid::new(&names, &Rows(shown))
            .window(GridWindow {
                offset: self.top,
                total: Some(total),
            })
            .sort(self.sorted.as_ref().map(|s| s.by))
            .show_cursor(ctx.is_focused)
            .borders(ctx.border_right_x, ctx.border_bottom_y)
            .render(area, buf, &mut self.grid, &self.colors, ctx.is_focused);
        // Fewer rows fitted than were assumed: the grid scrolled to keep
        // the cursor in view; take that scroll as the new top.
        self.top += self.grid.row_scroll;
        self.grid.row_scroll = 0;

        let busy = if self.indexing.is_some() {
            Some(t.csv_loading())
        } else if self.sorting.is_some() {
            Some(t.csv_sorting())
        } else {
            None
        };
        if let Some(busy) = busy {
            let label = format!(" {busy} ");
            buf.set_stringn(
                area.x,
                area.y,
                &label,
                area.width as usize,
                base.fg(self.colors.info),
            );
        }
    }

    fn tick(&mut self) -> Vec<PanelEvent> {
        if self.poll() {
            Self::redraw()
        } else {
            vec![]
        }
    }

    fn handle_key(&mut self, chord: KeyChord) -> Vec<PanelEvent> {
        // No text input here: every key is a shortcut, matched on the
        // layout-normalized form so it works on any keyboard layout.
        let key = chord.canonical;
        if self.hotkeys.matches("toggle_view", &key) {
            return vec![PanelEvent::SwapActiveToText(self.path.clone())];
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('r') {
            self.refresh();
            return Self::redraw();
        }
        if ctrl && key.code == KeyCode::Char('c') {
            return self.copy_cell();
        }
        if ctrl {
            return vec![];
        }
        let page = self.grid.visible_rows().max(1) as isize;
        let changed = match key.code {
            KeyCode::Up => self.move_rows(-1),
            KeyCode::Down => self.move_rows(1),
            KeyCode::PageUp => self.move_rows(-page),
            KeyCode::PageDown => self.move_rows(page),
            KeyCode::Home => {
                let moved = (self.cursor, self.grid.cursor_col) != (0, 0);
                self.cursor = 0;
                self.grid.cursor_col = 0;
                moved
            }
            KeyCode::End => self.move_rows(isize::MAX),
            KeyCode::Left => self.grid.move_left(),
            KeyCode::Right => self.grid.move_right(self.index.width),
            KeyCode::Char('s') => {
                self.cycle_sort(self.grid.cursor_col);
                true
            }
            KeyCode::Char('y') => return self.copy_cell(),
            _ => false,
        };
        if changed {
            Self::redraw()
        } else {
            vec![]
        }
    }

    fn handle_scroll(&mut self, delta: i32, _panel_area: Rect) -> Vec<PanelEvent> {
        if self.move_rows(delta as isize) {
            Self::redraw()
        } else {
            vec![]
        }
    }

    fn handle_mouse(&mut self, event: MouseEvent, _panel_area: Rect) -> Vec<PanelEvent> {
        let changed = match event.kind {
            MouseEventKind::ScrollDown => self.move_rows(WHEEL_ROWS as isize),
            MouseEventKind::ScrollUp => self.move_rows(-(WHEEL_ROWS as isize)),
            MouseEventKind::ScrollRight => self.grid.move_right(self.index.width),
            MouseEventKind::ScrollLeft => self.grid.move_left(),
            MouseEventKind::Down(MouseButton::Left) => {
                match self.grid.hit(event.column, event.row) {
                    Some(GridHit::Header(col)) => {
                        self.grid.cursor_col = col;
                        self.cycle_sort(col);
                        true
                    }
                    Some(GridHit::Cell { row, col }) => {
                        self.cursor = self.top + row;
                        if let Some(col) = col {
                            self.grid.cursor_col = col;
                        }
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        };
        if changed {
            Self::redraw()
        } else {
            vec![]
        }
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            PanelCommand::Copy => {
                let col = self.grid.cursor_col;
                if let Some(record) = self.record(self.cursor) {
                    if let Some(text) = record.get(col) {
                        let _ = termide_ui::clipboard::copy(text);
                    }
                }
                CommandResult::Handled(true)
            }
            PanelCommand::GetScrollBars => CommandResult::ScrollBars(self.grid.scrollbars()),
            // A thumb drag moves the cursor with it: the grid keeps the
            // cursor in view, so moving only the scroll would snap back.
            PanelCommand::SetScrollOffset { axis, offset } => {
                match axis {
                    ScrollAxis::Vertical => {
                        self.cursor = offset.min(self.row_count().saturating_sub(1));
                        self.top = self.cursor;
                    }
                    ScrollAxis::Horizontal => {
                        self.grid.cursor_col = offset.min(self.index.width.saturating_sub(1));
                        self.grid.col_scroll = self.grid.cursor_col;
                    }
                }
                CommandResult::NeedsRedraw(true)
            }
            PanelCommand::Cut | PanelCommand::Paste => CommandResult::Handled(false),
            _ => CommandResult::None,
        }
    }

    fn status_segments(&self) -> Vec<StatusSegment> {
        if self.error.is_some() {
            return vec![];
        }
        let sep = || StatusSegment::new(" │ ", SegmentKind::Label);
        let delimiter = self.delimiter.unwrap_or(self.index.delimiter);
        let rows = self.row_count();
        let row = if rows == 0 { 0 } else { self.cursor + 1 };
        // More rows are on their way while the file is being indexed.
        let more = if self.indexing.is_some() { "…" } else { "" };
        // Same field order as the editor and the other viewers: View, Edit.
        vec![
            StatusSegment::new(" ", SegmentKind::Label),
            StatusSegment::new("View: ", SegmentKind::Label),
            StatusSegment::new("Table", SegmentKind::Value),
            sep(),
            StatusSegment::clickable("Edit: ", SegmentKind::Label, "edit_source"),
            StatusSegment::clickable("No", SegmentKind::Active, "edit_source"),
            sep(),
            StatusSegment::clickable("Delimiter: ", SegmentKind::Label, "cycle_delimiter"),
            StatusSegment::clickable(
                table::delimiter_label(delimiter),
                SegmentKind::Active,
                "cycle_delimiter",
            ),
            sep(),
            StatusSegment::clickable("Header: ", SegmentKind::Label, "toggle_header"),
            StatusSegment::clickable(
                if self.has_header() { "Yes" } else { "No" },
                SegmentKind::Active,
                "toggle_header",
            ),
            sep(),
            StatusSegment::new("Row: ", SegmentKind::Label),
            StatusSegment::new(format!("{row}/{rows}{more}"), SegmentKind::Value),
        ]
    }

    fn handle_status_action(&mut self, action: &str) -> Vec<PanelEvent> {
        match action {
            "edit_source" => vec![PanelEvent::SwapActiveToText(self.path.clone())],
            "cycle_delimiter" => {
                self.cycle_delimiter();
                Self::redraw()
            }
            "toggle_header" => {
                self.toggle_header();
                Self::redraw()
            }
            "copy_row" => self.copy_row(),
            _ => vec![],
        }
    }

    fn context_menu_items(&self) -> Vec<(String, String)> {
        if self.error.is_some() {
            return vec![];
        }
        let t = termide_i18n::t();
        vec![
            (t.csv_menu_copy_row().to_string(), "copy_row".into()),
            (
                t.csv_menu_change_delimiter().to_string(),
                "cycle_delimiter".into(),
            ),
            (
                t.csv_menu_toggle_header().to_string(),
                "toggle_header".into(),
            ),
        ]
    }

    fn reload(&mut self) -> anyhow::Result<()> {
        self.refresh();
        Ok(())
    }

    fn to_state(&self, _project_dir: &Path) -> Option<PanelState> {
        Some(PanelState::Csv {
            path: self.path.clone(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn get_working_directory(&self) -> Option<PathBuf> {
        self.path.parent().map(|p| p.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn open(content: &str, name: &str) -> (tempfile::TempDir, CsvPanel) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, content).unwrap();
        let mut panel = CsvPanel::new(path);
        settle(&mut panel);
        (dir, panel)
    }

    fn settle(panel: &mut CsvPanel) {
        for _ in 0..400 {
            panel.poll();
            if panel.indexing.is_none() && panel.sorting.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the file was not read");
    }

    fn render(panel: &mut CsvPanel, area: Rect, buf: &mut Buffer) {
        let colors = ThemeColors::default();
        let ctx = RenderContext {
            theme: &colors,
            config: &termide_core::PanelConfig {
                tab_size: 4,
                word_wrap: false,
                show_line_numbers: false,
                show_hidden_files: false,
            },
            is_focused: true,
            panel_index: 0,
            terminal_width: area.width,
            terminal_height: area.height,
            border_right_x: None,
            border_bottom_y: None,
        };
        panel.render(area, buf, &ctx);
    }

    fn press(panel: &mut CsvPanel, code: KeyCode) -> Vec<PanelEvent> {
        panel.handle_key(KeyChord::identity(KeyEvent::from(code)))
    }

    fn column(panel: &mut CsvPanel, col: usize) -> Vec<String> {
        (0..panel.row_count())
            .map(|r| panel.record(r).unwrap()[col].to_string())
            .collect()
    }

    #[test]
    fn a_file_with_a_header_names_its_columns() {
        let (_dir, mut panel) = open("id;name\n2;b\n1;a\n", "t.csv");
        assert_eq!(panel.column_names(), vec!["id", "name"]);
        assert_eq!(column(&mut panel, 1), vec!["b", "a"]);
        assert_eq!(table::delimiter_label(panel.index.delimiter), ";");
    }

    #[test]
    fn the_header_switch_turns_the_first_record_into_data() {
        let (_dir, mut panel) = open("id,name\n1,a\n", "t.csv");
        panel.handle_status_action("toggle_header");
        assert_eq!(panel.column_names(), vec!["A", "B"]);
        assert_eq!(column(&mut panel, 0), vec!["id", "1"]);
    }

    #[test]
    fn s_cycles_the_sort_of_the_cursor_column() {
        // `""` is an empty field; a blank line would be no record at all.
        let (_dir, mut panel) = open("n\n10\n9\n\"\"\n100\n", "t.csv");
        press(&mut panel, KeyCode::Char('s'));
        settle(&mut panel);
        assert_eq!(column(&mut panel, 0), vec!["9", "10", "100", ""]);
        press(&mut panel, KeyCode::Char('s'));
        settle(&mut panel);
        assert_eq!(column(&mut panel, 0), vec!["", "100", "10", "9"]);
        press(&mut panel, KeyCode::Char('s'));
        settle(&mut panel);
        assert_eq!(column(&mut panel, 0), vec!["10", "9", "", "100"]);
        assert!(panel.sorted.is_none());
    }

    #[test]
    fn copying_takes_the_cell_or_the_row_under_the_cursor() {
        let (_dir, mut panel) = open("a,b\n1,\"x\ty\"\n", "t.csv");
        press(&mut panel, KeyCode::Right);
        match press(&mut panel, KeyCode::Char('y')).as_slice() {
            [PanelEvent::CopyToClipboard(text), PanelEvent::SetStatusMessage { .. }] => {
                assert_eq!(text, "x\ty")
            }
            other => panic!("unexpected {other:?}"),
        }
        match panel.handle_status_action("copy_row").as_slice() {
            [PanelEvent::CopyToClipboard(text), ..] => assert_eq!(text, "1\tx y"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn the_delimiter_switch_rereads_the_file_split_differently() {
        let (_dir, mut panel) = open("a;b;c\n1;2;3,5\n", "t.csv");
        assert_eq!(panel.index.width, 3);
        panel.handle_status_action("cycle_delimiter");
        settle(&mut panel);
        // `;` was guessed; the next one is Tab, which splits nothing.
        assert_eq!(panel.index.delimiter, b'\t');
        assert_eq!(panel.index.width, 1);
        panel.handle_status_action("cycle_delimiter");
        settle(&mut panel);
        assert_eq!(panel.index.delimiter, b'|');
        panel.handle_status_action("cycle_delimiter");
        settle(&mut panel);
        assert_eq!(panel.index.delimiter, b',');
        assert_eq!(panel.column_names(), vec!["a;b;c", "B"]);
    }

    #[test]
    fn navigation_stays_within_the_table() {
        let (_dir, mut panel) = open("a,b\n1,2\n3,4\n5,6\n", "t.csv");
        press(&mut panel, KeyCode::End);
        assert_eq!(panel.cursor, 2);
        press(&mut panel, KeyCode::Down);
        assert_eq!(panel.cursor, 2);
        press(&mut panel, KeyCode::Right);
        press(&mut panel, KeyCode::Right);
        assert_eq!(panel.grid.cursor_col, 1);
        press(&mut panel, KeyCode::Home);
        assert_eq!((panel.cursor, panel.grid.cursor_col), (0, 0));
    }

    #[test]
    fn toggle_view_and_the_edit_chip_swap_to_the_source() {
        let (_dir, mut panel) = open("a\n1\n", "t.csv");
        assert!(matches!(
            panel.handle_status_action("edit_source").as_slice(),
            [PanelEvent::SwapActiveToText(_)]
        ));
    }

    #[test]
    fn a_missing_file_reports_the_error() {
        let mut panel = CsvPanel::new(PathBuf::from("/nonexistent/x.csv"));
        settle(&mut panel);
        assert!(panel.error.is_some());
        assert!(panel.status_segments().is_empty());
    }

    #[test]
    fn to_state_keeps_the_path() {
        let (_dir, panel) = open("a\n", "t.csv");
        match panel.to_state(Path::new("/")) {
            Some(PanelState::Csv { path }) => assert_eq!(path, panel.path),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn renders_the_table_and_hit_tests_clicks() {
        let (_dir, mut panel) = open("id,name\n1,alpha\n2,beta\n", "t.csv");
        let area = Rect::new(0, 0, 30, 6);
        let mut buf = Buffer::empty(area);
        render(&mut panel, area, &mut buf);
        let line: String = (0..30).map(|x| buf[(x, 1)].symbol()).collect();
        assert!(line.starts_with(" 1   │ alpha │"), "{line:?}");

        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        panel.handle_mouse(click(8, 2), area);
        assert_eq!((panel.cursor, panel.grid.cursor_col), (1, 1));
        panel.handle_mouse(click(8, 0), area);
        assert_eq!(panel.sort, Some((1, ColumnSort::Ascending)));
    }

    #[test]
    fn only_the_rows_around_the_screen_are_read() {
        let text: String = std::iter::once("n,sq\n".to_string())
            .chain((0..5000).map(|i| format!("{i},{}\n", i * i)))
            .collect();
        let (_dir, mut panel) = open(&text, "t.csv");
        assert_eq!(panel.row_count(), 5000);
        let area = Rect::new(0, 0, 30, 11);
        let mut buf = Buffer::empty(area);
        render(&mut panel, area, &mut buf);
        assert_eq!(panel.window.span, 0..22, "the screen and a screen past it");

        press(&mut panel, KeyCode::End);
        render(&mut panel, area, &mut buf);
        assert_eq!(panel.cursor, 4999);
        assert_eq!(panel.top, 4999 + 1 - panel.grid.visible_rows());
        assert!(panel.window.records.len() <= 40);
        let last: String = (0..30).map(|x| buf[(x, 10)].symbol()).collect();
        assert!(last.starts_with(" 4999 │ 24990001 │"), "{last:?}");

        // A drag of the scrollbar thumb lands the cursor mid-file.
        panel.handle_command(PanelCommand::SetScrollOffset {
            axis: ScrollAxis::Vertical,
            offset: 2500,
        });
        render(&mut panel, area, &mut buf);
        let first: String = (0..30).map(|x| buf[(x, 1)].symbol()).collect();
        assert!(first.starts_with(" 2500 │ 6250000 │"), "{first:?}");
    }

    #[test]
    fn a_sorted_view_reads_its_rows_in_the_sorted_order() {
        let text: String = std::iter::once("n\n".to_string())
            .chain((0..1000).map(|i| format!("{}\n", (i * 7919) % 1000)))
            .collect();
        let (_dir, mut panel) = open(&text, "t.csv");
        press(&mut panel, KeyCode::Char('s'));
        press(&mut panel, KeyCode::Char('s'));
        settle(&mut panel);
        assert_eq!(
            panel.sorted.as_ref().unwrap().by,
            (0, ColumnSort::Descending)
        );
        press(&mut panel, KeyCode::End);
        assert_eq!(&panel.record(panel.cursor).unwrap()[0], "0");
        assert_eq!(&panel.record(0).unwrap()[0], "999");
    }
}
