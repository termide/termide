//! Table viewer for delimited text files (`.csv`, `.tsv`).
//!
//! Opened by View (`F3`) on such a file; the toggle-view key (or the `Edit`
//! status chip) swaps it in place for the source in the editor, and back. The
//! file is read and parsed on a background thread and held in memory; the
//! grid itself is [`termide_ui::Grid`].
//!
//! The delimiter and whether the first record names the columns are guessed
//! (see [`table`]) and can be switched from the status bar or the panel menu.
//! A click on a column header, or `s`, cycles the sort by that column —
//! ascending, descending, file order. `y` or Copy copies the cell under the
//! cursor; the panel menu copies the whole row. `Ctrl+R` re-reads the file.

mod table;

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
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
use termide_ui::{ColumnSort, Grid, GridCell, GridHit, GridRows, GridState};

use crate::table::Table;

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

    /// The file being read, while it is.
    loading: Option<Receiver<Result<Table, String>>>,
    table: Table,
    error: Option<String>,

    /// Data rows (indices into the records past the header) in display order.
    order: Vec<usize>,
    sort: Option<(usize, ColumnSort)>,
    grid: GridState,

    colors: ThemeColors,
    hotkeys: HotkeyTable,
    last_config_ptr: usize,
}

/// The data rows in display order, as the grid reads them.
struct Rows<'a> {
    records: &'a [StringRecord],
    order: &'a [usize],
}

impl GridRows for Rows<'_> {
    fn row_count(&self) -> usize {
        self.order.len()
    }

    fn cell(&self, row: usize, col: usize) -> Option<GridCell<'_>> {
        let record = self.records.get(*self.order.get(row)?)?;
        Some(GridCell::new(table::one_line(record.get(col)?)))
    }
}

impl CsvPanel {
    /// Open `path` in the table viewer; reading starts at once, in the
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
            loading: None,
            table: Table::default(),
            error: None,
            order: Vec::new(),
            sort: None,
            grid: GridState::default(),
            colors: ThemeColors::default(),
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
        };
        panel.start_loading();
        panel
    }

    /// (Re)read the file on a background thread with the current delimiter.
    fn start_loading(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let path = self.path.clone();
        let delimiter = self.delimiter;
        let spawned = std::thread::Builder::new()
            .name("termide-csv-load".into())
            .spawn(move || {
                let _ = tx.send(table::load(&path, delimiter));
            });
        match spawned {
            Ok(_) => self.loading = Some(rx),
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Take the read table if it is ready; true when something changed.
    fn poll_loading(&mut self) -> bool {
        let Some(rx) = &self.loading else {
            return false;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("the reading thread stopped".to_string())
            }
        };
        self.loading = None;
        match result {
            Ok(table) => {
                self.table = table;
                self.error = None;
                self.rebuild_order();
            }
            Err(e) => self.error = Some(e),
        }
        true
    }

    /// Whether the first record names the columns.
    fn has_header(&self) -> bool {
        self.header.unwrap_or(self.table.header_guess)
    }

    /// Records holding data: all but the header.
    fn data(&self) -> &[StringRecord] {
        data_records(&self.table, self.has_header())
    }

    fn column_names(&self) -> Vec<String> {
        let header = self
            .has_header()
            .then(|| self.table.records.first())
            .flatten();
        table::column_names(header, self.table.width)
    }

    /// Recompute the display order for the current sort, keeping the cursor
    /// on the window position it had.
    fn rebuild_order(&mut self) {
        let data = self.data();
        let mut order: Vec<usize> = (0..data.len()).collect();
        if let Some((col, dir)) = self.sort {
            let field = |i: usize| data[i].get(col).unwrap_or("");
            // Stable, so equal values keep their file order either way.
            order.sort_by(|&a, &b| {
                let ord = table::compare_fields(field(a), field(b));
                match dir {
                    ColumnSort::Ascending => ord,
                    ColumnSort::Descending => ord.reverse(),
                }
            });
        }
        self.order = order;
        self.grid.clamp(self.order.len(), self.table.width);
    }

    /// Cycle the sort of column `col`: ascending, descending, file order.
    fn cycle_sort(&mut self, col: usize) {
        self.sort = match self.sort {
            Some((c, ColumnSort::Ascending)) if c == col => Some((col, ColumnSort::Descending)),
            Some((c, ColumnSort::Descending)) if c == col => None,
            _ => Some((col, ColumnSort::Ascending)),
        };
        self.rebuild_order();
    }

    fn toggle_header(&mut self) {
        self.header = Some(!self.has_header());
        if self.sort.is_some_and(|(c, _)| c >= self.table.width) {
            self.sort = None;
        }
        self.rebuild_order();
    }

    /// Re-read the file split by the next delimiter.
    fn cycle_delimiter(&mut self) {
        let current = self.delimiter.unwrap_or(self.table.delimiter);
        self.delimiter = Some(table::next_delimiter(current));
        // The columns change with the delimiter: a sort by one would land on
        // whatever falls in its place.
        self.sort = None;
        self.grid.reset();
        self.start_loading();
    }

    /// Re-read the file, keeping the delimiter, the sort and the cursor.
    fn refresh(&mut self) {
        self.error = None;
        self.start_loading();
    }

    /// The record under the cursor.
    fn current_record(&self) -> Option<&StringRecord> {
        let index = *self.order.get(self.grid.cursor_row)?;
        self.data().get(index)
    }

    fn copy_cell(&self) -> Vec<PanelEvent> {
        let Some(text) = self
            .current_record()
            .and_then(|r| r.get(self.grid.cursor_col))
        else {
            return vec![];
        };
        copied(text.to_string(), termide_i18n::t().csv_copied_cell())
    }

    /// The row under the cursor, tab-separated, ready to paste into a
    /// spreadsheet.
    fn copy_row(&self) -> Vec<PanelEvent> {
        let Some(record) = self.current_record() else {
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
        let last = self.order.len().saturating_sub(1);
        let row = self.grid.cursor_row.saturating_add_signed(delta).min(last);
        let moved = row != self.grid.cursor_row;
        self.grid.cursor_row = row;
        moved
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

/// The records of `table` holding data: all but the header, when it has one.
fn data_records(table: &Table, header: bool) -> &[StringRecord] {
    let skip = usize::from(header).min(table.records.len());
    &table.records[skip..]
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
        if self.table.records.is_empty() {
            if self.loading.is_some() {
                self.center_message(buf, area, t.csv_loading(), base.fg(self.colors.info));
            }
            return;
        }

        let names = self.column_names();
        let rows = Rows {
            records: data_records(&self.table, self.has_header()),
            order: &self.order,
        };
        Grid::new(&names, &rows)
            .sort(self.sort)
            .show_cursor(ctx.is_focused)
            .borders(ctx.border_right_x, ctx.border_bottom_y)
            .render(area, buf, &mut self.grid, &self.colors, ctx.is_focused);

        // A re-read keeps the old table on screen until the new one lands.
        if self.loading.is_some() {
            let label = format!(" {} ", t.csv_loading());
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
        if self.poll_loading() {
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
                let moved = (self.grid.cursor_row, self.grid.cursor_col) != (0, 0);
                self.grid.cursor_row = 0;
                self.grid.cursor_col = 0;
                moved
            }
            KeyCode::End => self.move_rows(isize::MAX),
            KeyCode::Left => self.grid.move_left(),
            KeyCode::Right => self.grid.move_right(self.table.width),
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
            MouseEventKind::ScrollRight => self.grid.move_right(self.table.width),
            MouseEventKind::ScrollLeft => self.grid.move_left(),
            MouseEventKind::Down(MouseButton::Left) => {
                match self.grid.hit(event.column, event.row) {
                    Some(GridHit::Header(col)) => {
                        self.grid.cursor_col = col;
                        self.cycle_sort(col);
                        true
                    }
                    Some(GridHit::Cell { row, col }) => {
                        self.grid.cursor_row = row;
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
                if let Some(text) = self
                    .current_record()
                    .and_then(|r| r.get(self.grid.cursor_col))
                {
                    let _ = termide_ui::clipboard::copy(text);
                }
                CommandResult::Handled(true)
            }
            PanelCommand::GetScrollBars => CommandResult::ScrollBars(self.grid.scrollbars()),
            // A thumb drag moves the cursor with it: the grid keeps the
            // cursor in view, so moving only the scroll would snap back.
            PanelCommand::SetScrollOffset { axis, offset } => {
                match axis {
                    ScrollAxis::Vertical => {
                        self.grid.cursor_row = offset.min(self.order.len().saturating_sub(1));
                        self.grid.row_scroll = self.grid.cursor_row;
                    }
                    ScrollAxis::Horizontal => {
                        self.grid.cursor_col = offset.min(self.table.width.saturating_sub(1));
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
        let delimiter = self.delimiter.unwrap_or(self.table.delimiter);
        let rows = self.order.len();
        let row = if rows == 0 {
            0
        } else {
            self.grid.cursor_row + 1
        };
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
            StatusSegment::new(format!("{row}/{rows}"), SegmentKind::Value),
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
        for _ in 0..200 {
            panel.poll_loading();
            if panel.loading.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the file was not read");
    }

    fn press(panel: &mut CsvPanel, code: KeyCode) -> Vec<PanelEvent> {
        panel.handle_key(KeyChord::identity(KeyEvent::from(code)))
    }

    fn column(panel: &CsvPanel, col: usize) -> Vec<String> {
        let rows = Rows {
            records: panel.data(),
            order: &panel.order,
        };
        (0..rows.row_count())
            .map(|r| rows.cell(r, col).unwrap().text.into_owned())
            .collect()
    }

    #[test]
    fn a_file_with_a_header_names_its_columns() {
        let (_dir, panel) = open("id;name\n2;b\n1;a\n", "t.csv");
        assert_eq!(panel.column_names(), vec!["id", "name"]);
        assert_eq!(column(&panel, 1), vec!["b", "a"]);
        assert_eq!(table::delimiter_label(panel.table.delimiter), ";");
    }

    #[test]
    fn the_header_switch_turns_the_first_record_into_data() {
        let (_dir, mut panel) = open("id,name\n1,a\n", "t.csv");
        panel.handle_status_action("toggle_header");
        assert_eq!(panel.column_names(), vec!["A", "B"]);
        assert_eq!(column(&panel, 0), vec!["id", "1"]);
    }

    #[test]
    fn s_cycles_the_sort_of_the_cursor_column() {
        // `""` is an empty field; a blank line would be no record at all.
        let (_dir, mut panel) = open("n\n10\n9\n\"\"\n100\n", "t.csv");
        press(&mut panel, KeyCode::Char('s'));
        assert_eq!(column(&panel, 0), vec!["9", "10", "100", ""]);
        press(&mut panel, KeyCode::Char('s'));
        assert_eq!(column(&panel, 0), vec!["", "100", "10", "9"]);
        press(&mut panel, KeyCode::Char('s'));
        assert_eq!(column(&panel, 0), vec!["10", "9", "", "100"]);
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
        assert_eq!(panel.table.width, 3);
        panel.handle_status_action("cycle_delimiter");
        settle(&mut panel);
        // `;` was guessed; the next one is Tab, which splits nothing.
        assert_eq!(panel.table.delimiter, b'\t');
        assert_eq!(panel.table.width, 1);
        panel.handle_status_action("cycle_delimiter");
        settle(&mut panel);
        assert_eq!(panel.table.delimiter, b'|');
        panel.handle_status_action("cycle_delimiter");
        settle(&mut panel);
        assert_eq!(panel.table.delimiter, b',');
        assert_eq!(panel.column_names(), vec!["a;b;c", "B"]);
    }

    #[test]
    fn navigation_stays_within_the_table() {
        let (_dir, mut panel) = open("a,b\n1,2\n3,4\n5,6\n", "t.csv");
        press(&mut panel, KeyCode::End);
        assert_eq!(panel.grid.cursor_row, 2);
        press(&mut panel, KeyCode::Down);
        assert_eq!(panel.grid.cursor_row, 2);
        press(&mut panel, KeyCode::Right);
        press(&mut panel, KeyCode::Right);
        assert_eq!(panel.grid.cursor_col, 1);
        press(&mut panel, KeyCode::Home);
        assert_eq!((panel.grid.cursor_row, panel.grid.cursor_col), (0, 0));
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
            terminal_width: 30,
            terminal_height: 6,
            border_right_x: None,
            border_bottom_y: None,
        };
        panel.render(area, &mut buf, &ctx);
        let line: String = (0..30).map(|x| buf[(x, 1)].symbol()).collect();
        assert!(line.starts_with(" 1   │ alpha │"), "{line:?}");

        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        panel.handle_mouse(click(8, 2), area);
        assert_eq!((panel.grid.cursor_row, panel.grid.cursor_col), (1, 1));
        panel.handle_mouse(click(8, 0), area);
        assert_eq!(panel.sort, Some((1, ColumnSort::Ascending)));
    }
}
