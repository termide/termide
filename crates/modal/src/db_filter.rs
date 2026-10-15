//! Multi-column filter modal for the database viewer.
//!
//! One row per column — column name, an operator selectbox, and a value. Within
//! a row, ←/→ switch focus between the operator and the value (← from the start
//! of the value, as the value edits like any other field); ↑/↓ move between
//! rows. Enter on the operator opens a standard dropdown to pick it. Bottom
//! buttons: Apply / Clear filters / Cancel.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use termide_theme::Theme;

use crate::base::{render_input_field, render_modal_block, render_text_input};
use crate::input_keys::{handle_input_key, InputKeyResult};
use crate::{centered_rect_with_size, Modal, ModalResult, TextInputHandler};

/// A column the modal can filter on.
pub struct DbFilterColumn {
    pub name: String,
    /// Operator labels (a leading "no condition" sentinel is added internally).
    pub operators: Vec<String>,
    /// Pre-selected operator (index into `operators`), if a condition exists.
    pub op: Option<usize>,
    /// Pre-filled value.
    pub value: String,
}

/// One applied condition.
#[derive(Debug, Clone)]
pub struct DbFilterCondition {
    pub column: String,
    pub op: String,
    pub value: String,
}

/// Result of the filter modal: the full set of conditions (empty = clear all).
#[derive(Debug, Clone)]
pub struct DbFilterResult {
    pub conditions: Vec<DbFilterCondition>,
}

#[derive(Debug)]
struct Row {
    name: String,
    operators: Vec<String>,
    /// 0 = no condition; otherwise `operators[op_sel - 1]`.
    op_sel: usize,
    value: TextInputHandler,
}

impl Row {
    /// Number of selectable operator options (index 0 = "no condition").
    fn op_count(&self) -> usize {
        self.operators.len() + 1
    }
    fn op_label_at(&self, idx: usize) -> &str {
        if idx == 0 {
            "—"
        } else {
            self.operators
                .get(idx - 1)
                .map(|s| s.as_str())
                .unwrap_or("—")
        }
    }
    fn op_label(&self) -> &str {
        self.op_label_at(self.op_sel)
    }
    fn needs_value(&self) -> bool {
        !matches!(self.op_label(), "—" | "is null" | "is not null")
    }
    /// Whether the operator takes no value at all, so the field is blank and
    /// does not edit. "—" still shows what is typed (typing picks an
    /// operator).
    fn hides_value(&self) -> bool {
        matches!(self.op_label(), "is null" | "is not null")
    }
    /// Typing a value implies a condition: pick a default operator if none is
    /// set yet.
    fn imply_condition(&mut self) {
        if self.op_sel == 0 && !self.operators.is_empty() {
            self.op_sel = 1;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Operator,
    Value,
}

const BTN_APPLY: usize = 0;
const BTN_CLEAR: usize = 1;
const BTN_CANCEL: usize = 2;
const OP_CHIP_W: usize = 16;

/// Modal for editing per-column filter conditions.
#[derive(Debug)]
pub struct DbFilterModal {
    rows: Vec<Row>,
    /// Focused row index, or `rows.len()` for the button bar.
    focus: usize,
    field: Field,
    /// Operator dropdown open for the focused row.
    op_open: bool,
    op_cursor: usize,
    button: usize,
    scroll: usize,
}

impl DbFilterModal {
    pub fn new(columns: Vec<DbFilterColumn>) -> Self {
        let rows = columns
            .into_iter()
            .map(|c| Row {
                name: c.name,
                op_sel: c.op.map_or(0, |i| i + 1),
                operators: c.operators,
                value: TextInputHandler::with_text(c.value),
            })
            .collect();
        Self {
            rows,
            focus: 0,
            field: Field::Operator,
            op_open: false,
            op_cursor: 0,
            button: BTN_APPLY,
            scroll: 0,
        }
    }

    fn buttons_focused(&self) -> bool {
        self.focus >= self.rows.len()
    }

    fn collect(&self) -> ModalResult<DbFilterResult> {
        let conditions = self
            .rows
            .iter()
            .filter(|r| r.op_sel > 0)
            .map(|r| DbFilterCondition {
                column: r.name.clone(),
                op: r.op_label().to_string(),
                value: if r.needs_value() {
                    r.value.text().to_string()
                } else {
                    String::new()
                },
            })
            .collect();
        ModalResult::Confirmed(DbFilterResult { conditions })
    }
}

impl Modal for DbFilterModal {
    type Result = DbFilterResult;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let tr = termide_i18n::t();
        let width = 64u16.min(area.width.saturating_sub(2));
        let max_rows_h = area.height.saturating_sub(6).max(1);
        let rows_h = (self.rows.len() as u16).min(max_rows_h).max(1);
        let height = rows_h + 4;
        let modal_area = centered_rect_with_size(width, height, area);
        let inner = render_modal_block(modal_area, buf, tr.db_filter_title(), theme);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let base = Style::default().fg(theme.fg).bg(theme.bg);
        // Focus = inverse video (matches the grid's cell cursor).
        let focused = base.add_modifier(Modifier::REVERSED);

        let visible = (inner.height.saturating_sub(1) as usize).max(1); // last line = buttons
        let focus_row = self.focus.min(self.rows.len());
        if focus_row < self.rows.len() {
            if focus_row < self.scroll {
                self.scroll = focus_row;
            } else if focus_row >= self.scroll + visible {
                self.scroll = focus_row + 1 - visible;
            }
        }

        let name_w = self
            .rows
            .iter()
            .map(|r| UnicodeWidthStr::width(r.name.as_str()))
            .max()
            .unwrap_or(4)
            .clamp(4, 24);
        let op_x = inner.x + name_w as u16 + 2;
        let val_x = op_x + OP_CHIP_W as u16 + 1;

        let start = self.scroll.min(self.rows.len());
        let end = (start + visible).min(self.rows.len());
        let buttons_focused = self.buttons_focused();
        for (line, i) in (start..end).enumerate() {
            let r = &mut self.rows[i];
            let y = inner.y + line as u16;
            let row_focused = !buttons_focused && i == self.focus;
            let blanks = " ".repeat(inner.width as usize);
            buf.set_stringn(inner.x, y, &blanks, inner.width as usize, base);
            buf.set_stringn(inner.x, y, pad(&r.name, name_w), name_w, base);

            // Operator selectbox — same `[label ▼]` look as the panel's
            // InlineSelector (that widget lives in a crate that can't be a
            // dependency here without a cycle, so the format is mirrored).
            let op_focused = row_focused && self.field == Field::Operator;
            let arrow = if self.op_open && row_focused {
                "▼"
            } else {
                "▶"
            };
            let op_style = if op_focused { focused } else { base };
            // Highlight only the chip's own width, not the whole column slot.
            let chip = format!("[{} {}]", r.op_label(), arrow);
            buf.set_stringn(op_x, y, &chip, OP_CHIP_W, op_style);

            // Value field — drawn like any input, its cursor showing focus;
            // only meaningful when the operator needs one.
            let val_focused = row_focused && self.field == Field::Value;
            let vw = inner.width.saturating_sub(val_x - inner.x);
            let value_area = Rect::new(val_x, y, vw.min(28), 1);
            if r.hides_value() {
                render_input_field(
                    buf,
                    value_area.x,
                    y,
                    value_area.width,
                    "",
                    0,
                    None,
                    val_focused,
                    theme,
                );
            } else {
                render_text_input(buf, value_area, &mut r.value, val_focused, theme);
            }
        }

        // --- buttons ---
        let by = inner.y + inner.height.saturating_sub(1);
        let labels = [
            tr.db_filter_apply(),
            tr.db_filter_clear(),
            tr.db_filter_cancel(),
        ];
        let mut bx = inner.x;
        for (idx, lbl) in labels.iter().enumerate() {
            let chip = format!("[ {lbl} ]");
            let st = if self.buttons_focused() && self.button == idx {
                focused
            } else {
                base.add_modifier(Modifier::BOLD)
            };
            let (nx, _) = buf.set_stringn(
                bx,
                by,
                &chip,
                inner.width.saturating_sub(bx - inner.x) as usize,
                st,
            );
            bx = nx + 2;
        }

        // --- operator dropdown overlay (drawn last) ---
        if self.op_open && focus_row < self.rows.len() {
            let r = &self.rows[focus_row];
            let row_y = inner.y + (focus_row - start) as u16;
            for k in 0..r.op_count() {
                let y = row_y + 1 + k as u16;
                if y >= area.y + area.height {
                    break;
                }
                let st = if k == self.op_cursor { focused } else { base };
                let item = format!(" {} ", r.op_label_at(k));
                buf.set_stringn(op_x, y, pad(&item, OP_CHIP_W), OP_CHIP_W, st);
            }
        }
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let n = self.rows.len();

        // Operator dropdown captures navigation while open.
        if self.op_open {
            let count = self.rows.get(self.focus).map(|r| r.op_count()).unwrap_or(1);
            match chord.raw.code {
                KeyCode::Up => self.op_cursor = self.op_cursor.saturating_sub(1),
                KeyCode::Down => self.op_cursor = (self.op_cursor + 1).min(count.saturating_sub(1)),
                KeyCode::Enter => {
                    if let Some(r) = self.rows.get_mut(self.focus) {
                        r.op_sel = self.op_cursor;
                    }
                    self.op_open = false;
                }
                KeyCode::Esc => self.op_open = false,
                _ => {}
            }
            return Ok(None);
        }

        if !self.buttons_focused() && self.field == Field::Value && self.edit_value(chord.raw) {
            return Ok(None);
        }

        match chord.raw.code {
            KeyCode::Esc => return Ok(Some(ModalResult::Cancelled)),
            KeyCode::Up => self.focus = self.focus.saturating_sub(1),
            KeyCode::Down => self.focus = (self.focus + 1).min(n),
            KeyCode::Tab => {
                if self.buttons_focused() {
                    self.focus = 0;
                    self.field = Field::Operator;
                } else if self.field == Field::Operator {
                    self.field = Field::Value;
                } else {
                    self.field = Field::Operator;
                    self.focus = (self.focus + 1).min(n);
                }
            }
            KeyCode::Left => {
                if self.buttons_focused() {
                    self.button = self.button.saturating_sub(1);
                } else {
                    self.field = Field::Operator;
                }
            }
            KeyCode::Right => {
                if self.buttons_focused() {
                    self.button = (self.button + 1).min(BTN_CANCEL);
                } else {
                    self.field = Field::Value;
                }
            }
            KeyCode::Enter => {
                if self.buttons_focused() {
                    return Ok(Some(match self.button {
                        BTN_CANCEL => ModalResult::Cancelled,
                        BTN_CLEAR => ModalResult::Confirmed(DbFilterResult { conditions: vec![] }),
                        _ => self.collect(),
                    }));
                } else if self.field == Field::Operator {
                    self.op_cursor = self.rows.get(self.focus).map(|r| r.op_sel).unwrap_or(0);
                    self.op_open = true;
                } else {
                    return Ok(Some(self.collect()));
                }
            }
            _ => {}
        }
        Ok(None)
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        if !self.op_open && !self.buttons_focused() && self.field == Field::Value {
            if let Some(r) = self.rows.get_mut(self.focus) {
                r.imply_condition();
                if !r.hides_value() {
                    r.value.paste(text);
                }
                return true;
            }
        }
        false
    }
}

impl DbFilterModal {
    /// Give `key` to the focused value field; `false` when it is the modal's
    /// to handle — `Enter`, `Tab`, `Esc`, `↑`/`↓`, and `←` from the start of
    /// the value, which goes back to the operator.
    fn edit_value(&mut self, key: KeyEvent) -> bool {
        if matches!(
            key.code,
            KeyCode::Enter
                | KeyCode::Tab
                | KeyCode::BackTab
                | KeyCode::Esc
                | KeyCode::Up
                | KeyCode::Down
        ) {
            return false;
        }
        let Some(r) = self.rows.get_mut(self.focus) else {
            return false;
        };
        let typing =
            matches!(key.code, KeyCode::Char(_)) && !key.modifiers.contains(KeyModifiers::CONTROL);
        if typing {
            r.imply_condition();
        }
        if r.hides_value() {
            // Nothing to edit; `←` still goes back to the operator.
            return key.code != KeyCode::Left;
        }
        handle_input_key(&mut r.value, key) != InputKeyResult::NotHandled
    }
}

/// Pad/truncate to display width `w`.
fn pad(s: &str, w: usize) -> String {
    let sw = UnicodeWidthStr::width(s);
    if sw >= w {
        s.chars().take(w).collect()
    } else {
        format!("{s}{}", " ".repeat(w - sw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modal() -> DbFilterModal {
        DbFilterModal::new(vec![DbFilterColumn {
            name: "name".into(),
            operators: vec!["=".into(), "is null".into()],
            op: None,
            value: String::new(),
        }])
    }

    fn press(modal: &mut DbFilterModal, code: KeyCode, modifiers: KeyModifiers) {
        let key = KeyEvent::new(code, modifiers);
        modal
            .handle_key(termide_core::KeyChord::identity(key))
            .unwrap();
    }

    fn type_text(modal: &mut DbFilterModal, text: &str) {
        for ch in text.chars() {
            press(modal, KeyCode::Char(ch), KeyModifiers::NONE);
        }
    }

    /// The modal with the value field of its one row focused.
    fn in_value() -> DbFilterModal {
        let mut modal = modal();
        press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(modal.field, Field::Value);
        modal
    }

    #[test]
    fn the_value_edits_at_its_cursor_and_typing_picks_an_operator() {
        let mut modal = in_value();
        type_text(&mut modal, "alce");
        press(&mut modal, KeyCode::Left, KeyModifiers::NONE);
        press(&mut modal, KeyCode::Left, KeyModifiers::NONE);
        type_text(&mut modal, "i");
        assert_eq!(modal.rows[0].value.text(), "alice");
        assert_eq!(modal.rows[0].op_label(), "=");
    }

    #[test]
    fn a_selection_in_the_value_is_deleted_as_one() {
        let mut modal = in_value();
        type_text(&mut modal, "bobby");
        press(&mut modal, KeyCode::Left, KeyModifiers::SHIFT);
        press(&mut modal, KeyCode::Left, KeyModifiers::SHIFT);
        press(&mut modal, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(modal.rows[0].value.text(), "bob");
    }

    #[test]
    fn ctrl_v_does_not_type_a_v() {
        let mut modal = in_value();
        type_text(&mut modal, "x");
        // A stand-in clipboard, so the user's is never read or written.
        termide_ui::clipboard::use_fake_clipboard("PASTE");
        let clip = "PASTE";
        press(&mut modal, KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert_eq!(modal.rows[0].value.text(), format!("x{clip}"));
    }

    #[test]
    fn left_from_the_start_of_the_value_goes_back_to_the_operator() {
        let mut modal = in_value();
        type_text(&mut modal, "ab");
        press(&mut modal, KeyCode::Left, KeyModifiers::NONE);
        press(&mut modal, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(modal.field, Field::Value, "still walking the value");
        press(&mut modal, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(modal.field, Field::Operator);
    }

    #[test]
    fn a_paste_lands_at_the_cursor() {
        let mut modal = in_value();
        type_text(&mut modal, "ad");
        press(&mut modal, KeyCode::Left, KeyModifiers::NONE);
        assert!(modal.handle_paste("bc"));
        assert_eq!(modal.rows[0].value.text(), "abcd");
    }
}
