//! Keyboard handling and grid navigation for [`DbPanel`].

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use termide_core::{KeyChord, PanelEvent};
use termide_db::{Condition, FilterOp, SortDir, TypeCategory};
use termide_modal::{
    ActionButton, ActiveModal, DbFilterColumn, DbFilterModal, DbFilterResult, InfoActionModal,
};
use termide_state::PendingAction;

use crate::dropdown::DropdownKey;
use crate::filter::{label_for, op_from_label, operators_for, parse_value};
use crate::format::{row_to_insert, row_to_json, tsv_escape};
use crate::{DbPanel, Section};

impl DbPanel {
    pub(crate) fn handle_key_impl(&mut self, chord: KeyChord) -> Vec<PanelEvent> {
        let key = chord.raw;
        let code = key.code;

        // Drain any ready async results first so a held key acts on fresh data
        // (page turns apply as the DB responds, not only on the next tick).
        self.poll_async();

        // Refresh works regardless of focus / open dropdowns.
        if self.hotkeys.matches("refresh", &key) {
            self.refresh_catalog();
            return self.redraw();
        }

        // An open dropdown captures navigation.
        if self.db_dd.open {
            return match self.db_dd.handle_key(code, self.databases.len()) {
                DropdownKey::Pick(i) => {
                    if let Some(db) = self.databases.get(i).cloned() {
                        self.select_database(db);
                    }
                    self.redraw()
                }
                DropdownKey::Nav | DropdownKey::Closed => self.redraw(),
                DropdownKey::Unhandled => vec![],
            };
        }
        if self.table_dd.open {
            return match self.table_dd.handle_key(code, self.tables.len()) {
                DropdownKey::Pick(i) => {
                    if let Some(name) = self.tables.get(i).cloned() {
                        if self.selected_table.as_deref() != Some(name.as_str()) {
                            self.selected_table = Some(name);
                            self.reload_table();
                        }
                    }
                    self.redraw()
                }
                DropdownKey::Nav | DropdownKey::Closed => self.redraw(),
                DropdownKey::Unhandled => vec![],
            };
        }

        match code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.cycle_section();
                return self.redraw();
            }
            _ => {}
        }

        match self.section {
            Section::DbSelector => self.handle_db_selector_key(code),
            Section::TableSelector => self.handle_selector_key(code),
            Section::Grid => self.handle_grid_key(key),
        }
    }

    /// Move focus to the next zone (the DB selector exists only when the URL
    /// omitted a database).
    fn cycle_section(&mut self) {
        self.section = match self.section {
            Section::DbSelector => Section::TableSelector,
            Section::TableSelector => Section::Grid,
            Section::Grid => {
                if self.needs_db_pick {
                    Section::DbSelector
                } else {
                    Section::TableSelector
                }
            }
        };
    }

    fn handle_db_selector_key(&mut self, code: KeyCode) -> Vec<PanelEvent> {
        match code {
            KeyCode::Enter | KeyCode::Char(' ') => {
                if !self.databases.is_empty() {
                    let idx = self
                        .selected_db
                        .as_ref()
                        .and_then(|d| self.databases.iter().position(|n| n == d))
                        .unwrap_or(0);
                    self.db_dd.open_at(idx);
                }
                self.redraw()
            }
            KeyCode::Down => {
                self.section = Section::TableSelector;
                self.redraw()
            }
            _ => vec![],
        }
    }

    fn handle_selector_key(&mut self, code: KeyCode) -> Vec<PanelEvent> {
        match code {
            KeyCode::Enter | KeyCode::Char(' ') => {
                if !self.tables.is_empty() {
                    let idx = self
                        .selected_table
                        .as_ref()
                        .and_then(|t| self.tables.iter().position(|n| n == t))
                        .unwrap_or(0);
                    self.table_dd.open_at(idx);
                }
                self.redraw()
            }
            KeyCode::Down => {
                self.section = Section::Grid;
                self.redraw()
            }
            _ => vec![],
        }
    }

    fn handle_grid_key(&mut self, key: KeyEvent) -> Vec<PanelEvent> {
        if !self.is_connected() {
            return vec![];
        }

        // Configurable action hotkeys (see [database.keybindings]).
        if self.hotkeys.matches("sort", &key) {
            self.cycle_sort();
            return self.redraw();
        }
        if self.hotkeys.matches("filter", &key) {
            self.open_filter();
            return self.redraw();
        }
        if self.hotkeys.matches("clear_filter", &key) {
            return if self.clear_filters() {
                self.redraw()
            } else {
                vec![]
            };
        }
        if self.hotkeys.matches("detail", &key) {
            self.open_row_detail();
            return self.redraw();
        }
        if self.hotkeys.matches("copy_row", &key) {
            return self.copy(true);
        }

        // Fixed navigation keys.
        let changed = match key.code {
            KeyCode::Up => self.grid_up(),
            KeyCode::Down => self.grid_down(),
            KeyCode::Left => self.grid_left(),
            KeyCode::Right => self.grid_right(),
            KeyCode::PageDown => self.grid_page(true),
            KeyCode::PageUp => self.grid_page(false),
            KeyCode::Home => self.grid_home(),
            KeyCode::End => self.grid_end(),
            _ => false,
        };
        if changed {
            self.redraw()
        } else {
            vec![]
        }
    }

    /// Mouse: click the table selector to open it; click a column header to
    /// cycle its sort; click a data cell to move the cursor there.
    pub(crate) fn handle_mouse_impl(&mut self, event: MouseEvent) -> Vec<PanelEvent> {
        if event.kind != MouseEventKind::Down(MouseButton::Left) {
            return vec![];
        }
        let (row, col) = (event.row, event.column);

        // First item row sits just below the dropdown's top border.
        let list_top = self.geom.selector_y + 2;

        // Open DB dropdown: pick a database.
        if self.db_dd.open {
            if let Some(idx) = self.db_dd.index_at_row(row, list_top) {
                if let Some(db) = self.databases.get(idx).cloned() {
                    self.db_dd.open = false;
                    self.select_database(db);
                    return self.redraw();
                }
            }
            self.db_dd.open = false;
            return self.redraw();
        }
        // Open table dropdown: pick a table.
        if self.table_dd.open {
            if let Some(idx) = self.table_dd.index_at_row(row, list_top) {
                if let Some(name) = self.tables.get(idx).cloned() {
                    self.table_dd.open = false;
                    if self.selected_table.as_deref() != Some(name.as_str()) {
                        self.selected_table = Some(name);
                        self.reload_table();
                    }
                    return self.redraw();
                }
            }
            self.table_dd.open = false;
            return self.redraw();
        }

        // Click on the selector row → open the DB or table dropdown depending
        // on which chip was hit.
        if row == self.geom.selector_y {
            let on_table = !self.needs_db_pick || col >= self.geom.table_selector_x;
            if on_table {
                self.section = Section::TableSelector;
                if !self.tables.is_empty() {
                    let idx = self
                        .selected_table
                        .as_ref()
                        .and_then(|t| self.tables.iter().position(|n| n == t))
                        .unwrap_or(0);
                    self.table_dd.open_at(idx);
                }
            } else {
                self.section = Section::DbSelector;
                if !self.databases.is_empty() {
                    let idx = self
                        .selected_db
                        .as_ref()
                        .and_then(|d| self.databases.iter().position(|n| n == d))
                        .unwrap_or(0);
                    self.db_dd.open_at(idx);
                }
            }
            return self.redraw();
        }

        if !self.is_connected() {
            return vec![];
        }

        // Click on a column header → sort by that column.
        if Some(row) == self.geom.header_y {
            if let Some(col_idx) = self.column_at(col) {
                self.section = Section::Grid;
                self.cursor_col = col_idx;
                self.cycle_sort();
                return self.redraw();
            }
            return vec![];
        }

        // Click on a data cell → move the cursor there.
        if row >= self.geom.data_y0 {
            let vis = (row - self.geom.data_y0) as usize;
            let abs = self.row_scroll + vis;
            if abs < self.page.rows.len() {
                self.section = Section::Grid;
                self.cursor_row = abs;
                if let Some(col_idx) = self.column_at(col) {
                    self.cursor_col = col_idx;
                }
                return self.redraw();
            }
        }
        vec![]
    }

    /// Map a screen column to a grid column index using the captured layout.
    fn column_at(&self, x: u16) -> Option<usize> {
        self.geom
            .columns
            .iter()
            .find(|(_, start, end)| x >= *start && x < *end)
            .map(|(idx, _, _)| *idx)
    }

    // --- navigation primitives (scroll is recomputed in render) ---

    /// True while a page fetch is in flight. Page turns are suppressed until it
    /// lands, so holding a key can't race ahead of (or loop over) stale data.
    fn loading_page(&self) -> bool {
        self.page_rx.is_some()
    }

    fn grid_down(&mut self) -> bool {
        // While a page is loading the visible rows are about to be replaced —
        // freeze the cursor so held keys can't walk the stale page.
        if self.loading_page() {
            return false;
        }
        let rows = self.page.rows.len();
        if rows == 0 {
            return false;
        }
        if self.cursor_row + 1 < rows {
            self.cursor_row += 1;
            true
        } else if self.page.has_more && !self.loading_page() {
            self.offset += self.page_rows;
            self.cursor_row = 0;
            self.reload_page();
            true
        } else {
            false
        }
    }

    fn grid_up(&mut self) -> bool {
        if self.loading_page() {
            return false;
        }
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            true
        } else if self.offset > 0 && !self.loading_page() {
            self.offset = self.offset.saturating_sub(self.page_rows);
            // Land on the last row of the previous window once it loads.
            self.pending_bottom = true;
            self.cursor_row = 0;
            self.reload_page();
            true
        } else {
            false
        }
    }

    fn grid_left(&mut self) -> bool {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            true
        } else {
            false
        }
    }

    fn grid_right(&mut self) -> bool {
        let cols = self.col_count();
        if cols > 0 && self.cursor_col + 1 < cols {
            self.cursor_col += 1;
            true
        } else {
            false
        }
    }

    fn grid_page(&mut self, down: bool) -> bool {
        if self.loading_page() {
            return false;
        }
        // One window already equals one screen, so a page turn is a window turn.
        let rows = self.page.rows.len();
        if rows == 0 {
            return false;
        }
        if down {
            if self.page.has_more && !self.loading_page() {
                self.offset += self.page_rows;
                self.cursor_row = 0;
                self.reload_page();
                true
            } else if self.cursor_row != rows - 1 {
                self.cursor_row = rows - 1;
                true
            } else {
                false
            }
        } else if self.offset > 0 && !self.loading_page() {
            self.offset = self.offset.saturating_sub(self.page_rows);
            self.pending_bottom = true;
            self.cursor_row = 0;
            self.reload_page();
            true
        } else if self.cursor_row != 0 {
            self.cursor_row = 0;
            true
        } else {
            false
        }
    }

    fn grid_home(&mut self) -> bool {
        if self.loading_page() {
            return false;
        }
        self.cursor_col = 0;
        if self.offset > 0 && !self.loading_page() {
            self.offset = 0;
            self.cursor_row = 0;
            self.reload_page();
            true
        } else if self.cursor_row != 0 {
            self.cursor_row = 0;
            true
        } else {
            false
        }
    }

    fn grid_end(&mut self) -> bool {
        if self.loading_page() {
            return false;
        }
        let Some(total) = self.total_rows else {
            return false;
        };
        if total <= 0 {
            return false;
        }
        let last_offset = ((total as u64 - 1) / self.page_rows) * self.page_rows;
        if last_offset != self.offset && !self.loading_page() {
            self.offset = last_offset;
            self.pending_bottom = true;
            self.cursor_row = 0;
            self.reload_page();
            true
        } else {
            let last = self.page.rows.len().saturating_sub(1);
            if self.cursor_row != last {
                self.cursor_row = last;
                true
            } else {
                false
            }
        }
    }

    fn cycle_sort(&mut self) {
        let names = self.column_names();
        let Some(col) = names.get(self.cursor_col).cloned() else {
            return;
        };
        let current = self
            .order_by
            .first()
            .and_then(|(c, d)| if *c == col { Some(*d) } else { None });
        self.order_by = match current {
            None => vec![(col, SortDir::Asc)],
            Some(SortDir::Asc) => vec![(col, SortDir::Desc)],
            Some(SortDir::Desc) => Vec::new(),
        };
        self.offset = 0;
        self.cursor_row = 0;
        self.reload_page();
    }

    /// Text for the current cell (`whole_row == false`) or the whole row
    /// tab-separated (`whole_row == true`). `None` when there is no row/cell
    /// under the cursor.
    pub(crate) fn copy_text(&self, whole_row: bool) -> Option<String> {
        let row = self.page.rows.get(self.cursor_row)?;
        if whole_row {
            Some(
                row.iter()
                    .map(|v| tsv_escape(&v.display()))
                    .collect::<Vec<_>>()
                    .join("\t"),
            )
        } else {
            Some(row.get(self.cursor_col)?.display())
        }
    }

    fn copy(&self, whole_row: bool) -> Vec<PanelEvent> {
        let Some(text) = self.copy_text(whole_row) else {
            return vec![];
        };
        let t = termide_i18n::t();
        let message = if whole_row {
            t.db_copied_row()
        } else {
            t.db_copied_cell()
        }
        .to_string();
        vec![
            PanelEvent::CopyToClipboard(text),
            PanelEvent::SetStatusMessage {
                message,
                is_error: false,
            },
        ]
    }

    /// Build the row-detail modal for the current row: a key→value list plus
    /// copy-format buttons. The three copy formats are precomputed and carried
    /// in the `PendingAction` so the app can copy without calling back here.
    fn open_row_detail(&mut self) {
        let names = self.column_names();
        let Some(row) = self.page.rows.get(self.cursor_row) else {
            return;
        };
        let table = self.selected_table.clone().unwrap_or_default();

        let lines: Vec<(String, String)> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let v = row.get(i);
                let text = match v {
                    Some(v) if v.is_null() => "NULL".to_string(),
                    Some(v) => v.display(),
                    None => String::new(),
                };
                (name.clone(), text)
            })
            .collect();

        let tsv = row
            .iter()
            .map(|v| tsv_escape(&v.display()))
            .collect::<Vec<_>>()
            .join("\t");
        let json = row_to_json(&names, row);
        let insert = row_to_insert(&table, &names, row);

        let t = termide_i18n::t();
        let buttons = vec![
            ActionButton::new(t.db_copy_tsv(), "copy_tsv"),
            ActionButton::new(t.db_copy_json(), "copy_json"),
            ActionButton::new(t.db_copy_insert(), "copy_insert"),
            ActionButton::new(t.git_action_close(), "close"),
        ];
        let title = t.db_row_title_fmt(&table);
        let modal = InfoActionModal::new(title, lines, buttons);
        self.modal_request = Some((
            PendingAction::DbRowDetail { tsv, json, insert },
            ActiveModal::InfoAction(Box::new(modal)),
        ));
    }

    /// Open the per-column filter modal listing every column.
    fn open_filter(&mut self) {
        if self.columns.is_empty() {
            return;
        }
        let columns: Vec<DbFilterColumn> = self
            .columns
            .iter()
            .map(|c| {
                let operators: Vec<String> = operators_for(c.category)
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                // Pre-select from any existing condition on this column.
                let existing = self.filters.iter().find(|f| f.column == c.name);
                let op = existing.and_then(|f| {
                    let label = label_for(f.op);
                    operators.iter().position(|o| o == label)
                });
                let value = existing
                    .and_then(|f| f.value.as_ref())
                    .map(|v| v.display())
                    .unwrap_or_default();
                DbFilterColumn {
                    name: c.name.clone(),
                    operators,
                    op,
                    value,
                }
            })
            .collect();
        let modal = DbFilterModal::new(columns);
        self.modal_request = Some((
            PendingAction::DbFilter,
            ActiveModal::DbFilter(Box::new(modal)),
        ));
    }

    /// Clear all filters. Returns true if anything changed.
    fn clear_filters(&mut self) -> bool {
        if self.filters.is_empty() {
            return false;
        }
        self.filters.clear();
        self.offset = 0;
        self.cursor_row = 0;
        self.reload_all();
        true
    }

    /// Apply a result from the filter modal (called by the app on the active
    /// panel): replace the whole filter set with the modal's conditions.
    pub fn apply_filter_result(&mut self, r: DbFilterResult) {
        let mut filters = Vec::new();
        for c in r.conditions {
            let Some(op) = op_from_label(&c.op) else {
                continue;
            };
            let needs_value = !matches!(op, FilterOp::IsNull | FilterOp::IsNotNull);
            // An operator with no value isn't a usable condition — skip it
            // rather than send an invalid (e.g. `integer = ''`) query.
            if needs_value && c.value.trim().is_empty() {
                continue;
            }
            let value = needs_value.then(|| parse_value(self.category_of(&c.column), &c.value));
            filters.push(Condition {
                column: c.column,
                op,
                value,
            });
        }
        self.filters = filters;
        self.query_error = None;
        self.offset = 0;
        self.cursor_row = 0;
        self.reload_all();
    }

    /// Type category of a column by name (defaults to Text when unknown).
    fn category_of(&self, name: &str) -> TypeCategory {
        self.columns
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.category)
            .unwrap_or(TypeCategory::Text)
    }

    /// Standard "something changed" response: redraw + refresh the status bar.
    fn redraw(&self) -> Vec<PanelEvent> {
        vec![PanelEvent::NeedsRedraw, self.status_event()]
    }
}
