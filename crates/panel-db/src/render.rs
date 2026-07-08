//! Rendering for [`DbPanel`]: table selector, column headers (with the active
//! sort arrow) and the 2D data grid with a cell cursor.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthStr;

use termide_db::SortDir;
use termide_ui_render::InlineSelector;

use crate::{ConnState, DbPanel, Section};

const MAX_COL_WIDTH: usize = 40;
const MIN_COL_WIDTH: usize = 3;
/// Display width a column occupies on screen: ` content ` slot + "│" divider.
const COL_OVERHEAD: usize = 3;

impl DbPanel {
    pub(crate) fn render_content(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        is_focused: bool,
        border_right_x: Option<u16>,
        border_bottom_y: Option<u16>,
    ) {
        if area.width < 4 || area.height < 2 {
            return;
        }
        let theme = self.cached_theme;
        let base = Style::default().fg(theme.fg).bg(theme.bg);

        // Reset mouse hit-test geometry for this frame.
        self.geom.selector_y = area.y;
        self.geom.header_y = None;
        self.geom.data_y0 = area.y + 2;
        self.geom.columns.clear();

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
        self.geom.table_selector_x = sx;
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
                x: self.geom.table_selector_x,
                y: area.y,
                width: area
                    .width
                    .saturating_sub(self.geom.table_selector_x - area.x),
                height: area.height,
            };
            self.table_dd.render(buf, tarea, &self.tables, &theme);
        }
    }

    #[allow(clippy::needless_range_loop)]
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

        // Column widths from the visible window sample. Reserve room for the
        // sort arrow in the sorted column's header.
        let mut widths = self.column_widths(&names);
        let sorted = self.order_by.first().cloned();
        if let Some((c, _)) = &sorted {
            if let Some(idx) = names.iter().position(|n| n == c) {
                widths[idx] = (widths[idx] + 2).min(MAX_COL_WIDTH);
            }
        }

        // Scrollbars sit on the panel's actual frame when the chrome provides
        // its coordinates; otherwise they fall back to the content edge and a
        // content column/row is reserved so they don't overlap data.
        let total_cols = names.len();
        let total_col_w: usize = widths.iter().map(|w| w + COL_OVERHEAD).sum();
        let h_needed = total_col_w > area.width as usize;
        let h_reserve = if h_needed && border_bottom_y.is_none() {
            1
        } else {
            0
        };

        // Header occupies row 0; data fills the rest (minus a reserved
        // horizontal-scrollbar row only in the fallback case).
        let data_rows_region = area.height.saturating_sub(1) as usize;
        let data_height = data_rows_region.saturating_sub(h_reserve);
        let v_needed = matches!(self.total_rows, Some(t) if (t as usize) > data_height);
        let v_reserve = if v_needed && border_right_x.is_none() {
            1
        } else {
            0
        };
        let content_width = area.width.saturating_sub(v_reserve);

        self.visible_rows = data_height;

        // Keep the fetch window equal to the visible height: one fetched window
        // is exactly one screen, so we page instead of scrolling within a
        // buffer. Re-fetch when the viewport height changes (resize).
        let want = data_height.max(1) as u64;
        if want != self.page_rows {
            self.page_rows = want;
            self.reload_page();
        }

        // Vertical scroll within the window: keep the cursor row visible.
        if self.cursor_row < self.row_scroll {
            self.row_scroll = self.cursor_row;
        } else if data_height > 0 && self.cursor_row >= self.row_scroll + data_height {
            self.row_scroll = self.cursor_row + 1 - data_height;
        }

        // Horizontal scroll: keep the cursor column visible within content_width.
        self.adjust_col_scroll(&widths, content_width as usize);

        let max_x = area.x + content_width;
        let border = base.fg(theme.border);

        // Each column is a slot ` content ` (one space of padding on each side);
        // slots are separated by a "│" divider, so a slot highlighted edge-to-edge
        // reads as one cell bounded by the dividers.

        // --- header row ---
        self.geom.header_y = Some(area.y);
        self.geom.data_y0 = area.y + 1;
        fill_line(buf, area.x, area.y, area.width, base);
        let mut x = area.x;
        for j in self.col_scroll..names.len() {
            if x >= max_x {
                break;
            }
            let mut label = names[j].clone();
            if let Some((c, d)) = &sorted {
                if *c == names[j] {
                    label.push(' ');
                    label.push_str(if *d == SortDir::Asc { "↑" } else { "↓" });
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
            self.geom.columns.push((j, slot_start, x));
            x = put(buf, x, area.y, max_x, "│", border);
        }

        // --- data rows ---
        // Selected row: normal colours but bold. Selected cell: inverse video
        // across the whole slot (border to border).
        let rows = &self.page.rows;
        for vis in 0..data_height {
            let abs = self.row_scroll + vis;
            if abs >= rows.len() {
                break;
            }
            let y = area.y + 1 + vis as u16;
            let is_cur_row = is_focused && self.section == Section::Grid && abs == self.cursor_row;
            let row_style = if is_cur_row {
                base.add_modifier(Modifier::BOLD)
            } else {
                base
            };
            fill_line(buf, area.x, y, area.width, base);

            let row = &rows[abs];
            let mut x = area.x;
            for j in self.col_scroll..names.len() {
                if x >= max_x {
                    break;
                }
                let value = row.get(j);
                let (text, is_null) = match value {
                    Some(v) if v.is_null() => ("NULL".to_string(), true),
                    Some(v) => (v.display(), false),
                    None => (String::new(), false),
                };
                let slot = format!(" {} ", pad(&text, widths[j]));
                let is_cur_cell = is_cur_row && j == self.cursor_col;
                let mut style = row_style;
                if is_null && !is_cur_cell {
                    style = style.fg(theme.disabled);
                }
                if is_cur_cell {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                x = put(buf, x, y, max_x, &slot, style);
                x = put(buf, x, y, max_x, "│", border);
            }
        }

        // --- scrollbars (drawn last so they sit above the grid) ---
        let sb_color = if is_focused {
            theme.border_focused
        } else {
            theme.disabled
        };
        if v_needed {
            // Position across the whole table: window offset + in-window scroll.
            let offset = self.offset as usize + self.row_scroll;
            let total = self.total_rows.unwrap_or(0) as usize;
            let vx = border_right_x.unwrap_or(area.x + area.width - 1);
            render_vscrollbar(
                buf,
                vx,
                area.y + 1,
                data_height as u16,
                offset,
                data_height,
                total,
                sb_color,
            );
        }
        if h_needed {
            // Count only fully-visible columns so a partially-clipped rightmost
            // column still reports "more to the right" at col_scroll == 0.
            let mut used = 0usize;
            let mut visible_cols = 0usize;
            for w in widths.iter().skip(self.col_scroll) {
                let need = w + COL_OVERHEAD;
                if used + need > content_width as usize {
                    break;
                }
                used += need;
                visible_cols += 1;
            }
            let visible_cols = visible_cols.max(1);
            let hy = border_bottom_y.unwrap_or(area.y + area.height - 1);
            render_hscrollbar(
                buf,
                area.x,
                hy,
                content_width,
                self.col_scroll,
                visible_cols,
                total_cols,
                sb_color,
            );
        }

        if self.loading {
            let style = base.fg(theme.info);
            let label = format!(" {} ", termide_i18n::t().db_loading());
            buf.set_stringn(area.x, area.y, &label, area.width as usize, style);
        }
    }

    /// Compute per-column display widths from header + the visible window.
    fn column_widths(&self, names: &[String]) -> Vec<usize> {
        let mut widths: Vec<usize> = names
            .iter()
            .map(|n| UnicodeWidthStr::width(n.as_str()).max(MIN_COL_WIDTH))
            .collect();
        for row in &self.page.rows {
            for (j, w) in widths.iter_mut().enumerate() {
                if let Some(v) = row.get(j) {
                    let text = if v.is_null() {
                        "NULL".to_string()
                    } else {
                        v.display()
                    };
                    let cw = UnicodeWidthStr::width(text.as_str());
                    if cw > *w {
                        *w = cw;
                    }
                }
            }
        }
        for w in widths.iter_mut() {
            *w = (*w).min(MAX_COL_WIDTH);
        }
        widths
    }

    fn adjust_col_scroll(&mut self, widths: &[usize], avail: usize) {
        if self.cursor_col < self.col_scroll {
            self.col_scroll = self.cursor_col;
            return;
        }
        // Grow col_scroll until the cursor column fits within `avail`.
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

/// Vertical scrollbar on column `x`, rows `y0..y0+height`. Matches the shared
/// [`termide_ui::ScrollBar`] look (`│` track, `▌` thumb). No-op when all rows fit.
#[allow(clippy::too_many_arguments)]
fn render_vscrollbar(
    buf: &mut Buffer,
    x: u16,
    y0: u16,
    height: u16,
    offset: usize,
    visible: usize,
    total: usize,
    color: Color,
) {
    if total <= visible || height == 0 {
        return;
    }
    let style = Style::default().fg(color);
    let thumb = ((height as f32 * (visible as f32 / total as f32)).max(1.0)) as u16;
    let max_scroll = total.saturating_sub(visible);
    let ratio = if max_scroll > 0 {
        offset.min(max_scroll) as f32 / max_scroll as f32
    } else {
        0.0
    };
    let pos = ((height.saturating_sub(thumb)) as f32 * ratio) as u16;
    for i in 0..height {
        let sym = if i >= pos && i < pos + thumb {
            "▌"
        } else {
            "│"
        };
        buf[(x, y0 + i)].set_symbol(sym).set_style(style);
    }
}

/// Horizontal scrollbar on row `y`, columns `x0..x0+width`, sized in list units
/// (table columns). `─` track, `━` thumb. No-op when everything fits.
#[allow(clippy::too_many_arguments)]
fn render_hscrollbar(
    buf: &mut Buffer,
    x0: u16,
    y: u16,
    width: u16,
    offset: usize,
    visible: usize,
    total: usize,
    color: Color,
) {
    if total <= visible || width == 0 {
        return;
    }
    let style = Style::default().fg(color);
    let thumb = ((width as f32 * (visible as f32 / total as f32)).max(1.0)) as u16;
    let max_scroll = total.saturating_sub(visible);
    let ratio = if max_scroll > 0 {
        offset.min(max_scroll) as f32 / max_scroll as f32
    } else {
        0.0
    };
    let pos = ((width.saturating_sub(thumb)) as f32 * ratio) as u16;
    for i in 0..width {
        let sym = if i >= pos && i < pos + thumb {
            "━"
        } else {
            "─"
        };
        buf[(x0 + i, y)].set_symbol(sym).set_style(style);
    }
}

/// Fill a single row with spaces in `style` (background paint).
fn fill_line(buf: &mut Buffer, x: u16, y: u16, width: u16, style: Style) {
    let blanks = " ".repeat(width as usize);
    buf.set_stringn(x, y, &blanks, width as usize, style);
}

/// Write a string clipped to `max_x`; returns the new x cursor.
fn put(buf: &mut Buffer, x: u16, y: u16, max_x: u16, s: &str, style: Style) -> u16 {
    if x >= max_x {
        return x;
    }
    let budget = (max_x - x) as usize;
    let (nx, _) = buf.set_stringn(x, y, s, budget, style);
    nx
}

/// Pad/truncate `s` to display width `w` (truncation adds an ellipsis).
fn pad(s: &str, w: usize) -> String {
    let sw = UnicodeWidthStr::width(s);
    if sw == w {
        s.to_string()
    } else if sw < w {
        format!("{s}{}", " ".repeat(w - sw))
    } else {
        // Truncate by chars to fit w-1, add ellipsis.
        let mut out = String::new();
        let mut used = 0usize;
        for ch in s.chars() {
            let cw = UnicodeWidthStr::width(ch.to_string().as_str());
            if used + cw > w.saturating_sub(1) {
                break;
            }
            out.push(ch);
            used += cw;
        }
        out.push('…');
        // Pad if the ellipsis left us short.
        let ow = UnicodeWidthStr::width(out.as_str());
        if ow < w {
            out.push_str(&" ".repeat(w - ow));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn vscrollbar_draws_thumb_and_track() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 10));
        render_vscrollbar(&mut buf, 2, 0, 10, 0, 10, 100, Color::White);
        let col: String = (0..10).map(|y| buf[(2, y)].symbol().to_string()).collect();
        assert!(col.contains('▌'), "thumb present: {col:?}");
        assert!(col.contains('│'), "track present: {col:?}");
    }

    #[test]
    fn scrollbars_are_noop_when_all_fit() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        render_vscrollbar(&mut buf, 2, 0, 5, 0, 5, 5, Color::White);
        render_hscrollbar(&mut buf, 0, 4, 20, 0, 12, 12, Color::White);
        let col: String = (0..5).map(|y| buf[(2, y)].symbol().to_string()).collect();
        let row: String = (0..20).map(|x| buf[(x, 4)].symbol().to_string()).collect();
        assert!(!col.contains('▌') && !col.contains('│'), "no vbar: {col:?}");
        assert!(!row.contains('━'), "no hbar: {row:?}");
    }

    #[test]
    fn hscrollbar_draws_thumb_and_track() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        render_hscrollbar(&mut buf, 0, 0, 20, 0, 3, 12, Color::White);
        let row: String = (0..20).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(row.contains('━'), "thumb present: {row:?}");
        assert!(row.contains('─'), "track present: {row:?}");
    }
}
