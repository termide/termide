//! Rendering for [`DbPanel`]: the table selector, and the data grid of the
//! loaded page through [`termide_ui::Grid`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use termide_db::{DbValue, Page, SortDir};
use termide_ui::{ColumnSort, Grid, GridCell, GridEditor, GridRows, GridWindow};
use termide_ui_render::InlineSelector;

use crate::{ConnState, DbPanel, Section};

/// The loaded page as grid rows: NULL is drawn dimmed.
struct PageRows<'a>(&'a Page);

impl GridRows for PageRows<'_> {
    fn row_count(&self) -> usize {
        self.0.rows.len()
    }

    fn cell(&self, row: usize, col: usize) -> Option<GridCell<'_>> {
        let value = self.0.rows.get(row)?.get(col)?;
        Some(match value {
            DbValue::Null => GridCell::muted("NULL"),
            DbValue::Text(s) => GridCell::new(s.as_str()),
            v => GridCell::new(v.display()),
        })
    }
}

impl DbPanel {
    pub(crate) fn render_content(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        is_focused: bool,
        border_right_x: Option<u16>,
        border_bottom_y: Option<u16>,
    ) {
        // The grid is drawn only in the connected state; until then nothing
        // should hit-test against an old layout.
        self.grid.hide();
        if area.width < 4 || area.height < 2 {
            return;
        }
        let theme = self.cached_theme;
        let base = Style::default().fg(theme.fg).bg(theme.bg);

        self.selector_y = area.y;

        // --- selector row (bracketed chips, like the git-status selectors) ---
        let tr = termide_i18n::t();
        fill_line(buf, area.x, area.y, area.width, base);
        let mut sx = area.x;
        if self.needs_db_pick {
            let db_label = self
                .selected_db
                .clone()
                .unwrap_or_else(|| tr.db_no_database().to_string());
            let focused = is_focused && self.section == Section::DbSelector;
            let chip = InlineSelector::new(&db_label, self.db_dd.open, focused, &theme);
            let used = chip.render(sx, area.y, area.width / 2, buf);
            sx += used + 1;
        }
        self.table_selector_x = sx;
        let table_label = self
            .selected_table
            .clone()
            .unwrap_or_else(|| tr.db_no_table().to_string());
        let focused = is_focused && self.section == Section::TableSelector;
        let chip = InlineSelector::new(&table_label, self.table_dd.open, focused, &theme);
        chip.render(sx, area.y, area.width.saturating_sub(sx - area.x), buf);

        // --- body area below selector ---
        let body = Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: area.height.saturating_sub(1),
        };

        // Body — no early returns, so the dropdown overlay below always draws.
        match &self.conn {
            ConnState::Connecting(_) => {
                self.center_message(buf, body, tr.db_connecting(), base.fg(theme.info));
            }
            ConnState::Failed(msg) => {
                self.center_message(
                    buf,
                    body,
                    &tr.db_connection_failed_fmt(msg),
                    base.fg(theme.error),
                );
            }
            ConnState::Connected(_) => {
                if self.needs_db_pick && self.selected_db.is_none() {
                    self.center_message(buf, body, tr.db_select_database(), base.fg(theme.info));
                } else if self.selected_table.is_none() {
                    self.center_message(buf, body, tr.db_no_tables(), base);
                } else {
                    self.render_grid(buf, body, is_focused, border_right_x, border_bottom_y);
                }
            }
        }

        // Dropdown overlay drawn last so it sits above everything.
        if self.db_dd.open {
            self.db_dd.render(buf, area, &self.databases, &theme);
        } else if self.table_dd.open {
            let tarea = Rect {
                x: self.table_selector_x,
                y: area.y,
                width: area.width.saturating_sub(self.table_selector_x - area.x),
                height: area.height,
            };
            self.table_dd.render(buf, tarea, &self.tables, &theme);
        }
    }

    fn render_grid(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        is_focused: bool,
        border_right_x: Option<u16>,
        border_bottom_y: Option<u16>,
    ) {
        let theme = self.cached_theme;
        let base = Style::default().fg(theme.fg).bg(theme.bg);
        let names = self.column_names();
        if names.is_empty() {
            self.center_message(buf, area, termide_i18n::t().db_loading(), base);
            return;
        }

        let sort = self.order_by.first().and_then(|(c, d)| {
            let idx = names.iter().position(|n| n == c)?;
            let dir = match d {
                SortDir::Asc => ColumnSort::Ascending,
                SortDir::Desc => ColumnSort::Descending,
            };
            Some((idx, dir))
        });
        let editor = self.edit.as_mut().map(|e| GridEditor {
            row: e.row,
            col: e.col,
            input: &mut e.input,
            saving: e.saving,
        });
        let rows = PageRows(&self.page);
        Grid::new(&names, &rows)
            .window(GridWindow {
                offset: self.offset as usize,
                total: self.total_rows.map(|t| t.max(0) as usize),
            })
            .sort(sort)
            .show_cursor(is_focused && self.section == Section::Grid)
            .editor(editor)
            .borders(border_right_x, border_bottom_y)
            .render(area, buf, &mut self.grid, &theme, is_focused);

        // Keep the fetch window equal to the visible height: one fetched window
        // is exactly one screen, so we page instead of scrolling within a
        // buffer. Re-fetch when the viewport height changes (resize).
        let want = self.grid.visible_rows().max(1) as u64;
        if want != self.page_rows {
            self.page_rows = want;
            self.reload_page();
        }

        if self.loading {
            let style = base.fg(theme.info);
            let label = format!(" {} ", termide_i18n::t().db_loading());
            buf.set_stringn(area.x, area.y, &label, area.width as usize, style);
        }
    }

    fn center_message(&self, buf: &mut Buffer, area: Rect, msg: &str, style: Style) {
        if area.height == 0 {
            return;
        }
        let y = area.y + area.height / 2;
        let w = UnicodeWidthStr::width(msg).min(area.width as usize);
        let x = area.x + (area.width.saturating_sub(w as u16)) / 2;
        buf.set_stringn(x, y, msg, area.width as usize, style);
    }
}

/// Fill a single row with spaces in `style` (background paint).
fn fill_line(buf: &mut Buffer, x: u16, y: u16, width: u16, style: Style) {
    let blanks = " ".repeat(width as usize);
    buf.set_stringn(x, y, &blanks, width as usize, style);
}
