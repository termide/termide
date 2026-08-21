//! Row editor for the database panel: one field per column, with an explicit
//! NULL checkbox where the column accepts NULL.
//!
//! Opened from the grid (the row-detail key), it doubles as the row viewer —
//! every value is visible in full, not truncated to a column width — so the
//! copy actions stay on the button bar next to Save.
//!
//! Primary-key columns are shown but not editable: they are the address the
//! update is sent to, and changing one would move the row rather than edit it.

use anyhow::Result;
use crossterm::event::{KeyCode, MouseEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use termide_i18n as i18n;
use termide_theme::Theme;

use crate::{centered_rect_with_size, Modal, ModalResult};

/// A column offered to the row editor.
#[derive(Debug, Clone)]
pub struct DbRowEditColumn {
    pub name: String,
    /// Current value as text; `None` means the cell is NULL.
    pub value: Option<String>,
    /// Whether the column accepts NULL — no checkbox is offered otherwise.
    pub nullable: bool,
    /// Part of the primary key: displayed, never edited.
    pub is_key: bool,
}

/// What the user changed, in column order. Only fields that actually differ are
/// reported, so an untouched row saves nothing.
#[derive(Debug, Clone)]
pub struct DbRowEditResult {
    /// Column name → new value; `None` means NULL.
    pub changes: Vec<(String, Option<String>)>,
    /// A copy action was chosen instead of Save (`"tsv"`, `"json"`, `"insert"`).
    pub copy: Option<String>,
}

#[derive(Debug, Clone)]
struct Field {
    name: String,
    text: String,
    caret: usize,
    is_null: bool,
    nullable: bool,
    read_only: bool,
    original_text: String,
    original_null: bool,
}

impl Field {
    fn changed(&self) -> bool {
        if self.is_null != self.original_null {
            return true;
        }
        !self.is_null && self.text != self.original_text
    }
}

const BTN_SAVE: usize = 0;
const BTN_TSV: usize = 1;
const BTN_JSON: usize = 2;
const BTN_INSERT: usize = 3;
const BUTTON_COUNT: usize = 5;

/// Modal editor for one row of a table.
#[derive(Debug)]
pub struct DbRowEditModal {
    title: String,
    fields: Vec<Field>,
    /// Focused field index, or `fields.len()` for the button bar.
    focus: usize,
    button: usize,
    scroll: usize,
    /// Visible field rows from the last render, for scrolling and mouse.
    visible_rows: usize,
    read_only_notice: Option<String>,
}

impl DbRowEditModal {
    /// Build an editor for `columns`. `notice` explains why editing is
    /// unavailable (no primary key), which also makes every field read-only.
    pub fn new(
        title: impl Into<String>,
        columns: Vec<DbRowEditColumn>,
        notice: Option<String>,
    ) -> Self {
        let editable = notice.is_none();
        let fields = columns
            .into_iter()
            .map(|c| {
                let is_null = c.value.is_none();
                let text = c.value.unwrap_or_default();
                Field {
                    name: c.name,
                    caret: text.chars().count(),
                    original_text: text.clone(),
                    text,
                    is_null,
                    original_null: is_null,
                    nullable: c.nullable,
                    read_only: !editable || c.is_key,
                }
            })
            .collect();
        Self {
            title: title.into(),
            fields,
            focus: 0,
            button: BTN_SAVE,
            scroll: 0,
            visible_rows: 0,
            read_only_notice: notice,
        }
    }

    fn on_buttons(&self) -> bool {
        self.focus >= self.fields.len()
    }

    /// Whether anything would be written by Save.
    fn dirty(&self) -> bool {
        self.fields.iter().any(Field::changed)
    }

    fn collect(&self) -> DbRowEditResult {
        DbRowEditResult {
            changes: self
                .fields
                .iter()
                .filter(|f| f.changed())
                .map(|f| {
                    let value = if f.is_null {
                        None
                    } else {
                        Some(f.text.clone())
                    };
                    (f.name.clone(), value)
                })
                .collect(),
            copy: None,
        }
    }

    fn copy_result(id: &str) -> DbRowEditResult {
        DbRowEditResult {
            changes: Vec::new(),
            copy: Some(id.to_string()),
        }
    }

    /// Keep the focused field inside the visible window.
    fn scroll_into_view(&mut self) {
        if self.on_buttons() || self.visible_rows == 0 {
            return;
        }
        if self.focus < self.scroll {
            self.scroll = self.focus;
        } else if self.focus >= self.scroll + self.visible_rows {
            self.scroll = self.focus + 1 - self.visible_rows;
        }
    }

    fn focused_field_mut(&mut self) -> Option<&mut Field> {
        let idx = self.focus;
        self.fields.get_mut(idx).filter(|f| !f.read_only)
    }
}

/// Byte offset of character `index` in `text` (its length when past the end).
fn char_index_to_byte(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

impl Modal for DbRowEditModal {
    type Result = DbRowEditResult;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let t = i18n::t();
        let width = area.width.saturating_sub(8).clamp(40, 100);
        // Field rows + notice + buttons + borders.
        let wanted = self.fields.len() as u16 + 5 + u16::from(self.read_only_notice.is_some());
        let height = wanted.min(area.height.saturating_sub(4)).max(7);
        let modal = centered_rect_with_size(width, height, area);

        Clear.render(modal, buf);
        let base = Style::default().fg(theme.fg).bg(theme.bg);
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accented_fg))
            .title(Line::from(Span::styled(
                format!(" {} ", self.title),
                base.add_modifier(Modifier::BOLD),
            )))
            .style(base)
            .render(modal, buf);

        let inner = Rect {
            x: modal.x + 1,
            y: modal.y + 1,
            width: modal.width.saturating_sub(2),
            height: modal.height.saturating_sub(2),
        };

        let mut y = inner.y;
        if let Some(notice) = &self.read_only_notice {
            Paragraph::new(Line::from(Span::styled(
                notice.clone(),
                base.fg(theme.warning),
            )))
            .render(
                Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: 1,
                },
                buf,
            );
            y += 1;
        }

        // Two rows are reserved at the bottom: a blank separator and buttons.
        let rows_area_height = inner.height.saturating_sub(2).saturating_sub(y - inner.y);
        self.visible_rows = rows_area_height as usize;
        self.scroll_into_view();

        let name_width = self
            .fields
            .iter()
            .map(|f| f.name.chars().count())
            .max()
            .unwrap_or(0)
            .min(24);

        for (idx, field) in self
            .fields
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(self.visible_rows)
        {
            let row_y = y;
            y += 1;
            let is_focused = !self.on_buttons() && self.focus == idx;

            let mut spans = Vec::new();
            let label = format!("{:<width$} ", field.name, width = name_width);
            spans.push(Span::styled(label, base.add_modifier(Modifier::BOLD)));

            // NULL checkbox, only where the column accepts NULL.
            if field.nullable && !field.read_only {
                let mark = if field.is_null { "x" } else { " " };
                spans.push(Span::styled(
                    format!("[{mark}] {} ", t.db_edit_null_checkbox()),
                    base,
                ));
            }

            let value_style = if field.read_only {
                base.fg(theme.disabled)
            } else if is_focused {
                base.bg(theme.selected_bg)
            } else {
                base
            };
            let shown = if field.is_null {
                t.db_edit_null_value().to_string()
            } else {
                field.text.clone()
            };
            spans.push(Span::styled(shown, value_style));

            Paragraph::new(Line::from(spans)).render(
                Rect {
                    x: inner.x,
                    y: row_y,
                    width: inner.width,
                    height: 1,
                },
                buf,
            );
        }

        // Buttons.
        let labels = [
            t.db_edit_save().to_string(),
            t.db_copy_tsv().to_string(),
            t.db_copy_json().to_string(),
            t.db_copy_insert().to_string(),
            t.db_filter_cancel().to_string(),
        ];
        let mut spans = Vec::new();
        for (i, label) in labels.iter().enumerate() {
            let focused = self.on_buttons() && self.button == i;
            let mut style = base;
            if focused {
                style = style.add_modifier(Modifier::REVERSED);
            }
            if i == BTN_SAVE && !self.dirty() {
                style = style.fg(theme.disabled);
            }
            spans.push(Span::styled(format!("[ {label} ]"), style));
            spans.push(Span::raw("  "));
        }
        Paragraph::new(Line::from(spans))
            .alignment(Alignment::Center)
            .render(
                Rect {
                    x: inner.x,
                    y: inner.y + inner.height.saturating_sub(1),
                    width: inner.width,
                    height: 1,
                },
                buf,
            );
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let key = chord.raw;
        match key.code {
            KeyCode::Esc => return Ok(Some(ModalResult::Cancelled)),
            KeyCode::Tab | KeyCode::Down => {
                self.focus = (self.focus + 1).min(self.fields.len());
                self.scroll_into_view();
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.focus = self.focus.saturating_sub(1);
                self.scroll_into_view();
            }
            KeyCode::Enter => {
                if self.on_buttons() {
                    return Ok(Some(match self.button {
                        BTN_SAVE => ModalResult::Confirmed(self.collect()),
                        BTN_TSV => ModalResult::Confirmed(Self::copy_result("tsv")),
                        BTN_JSON => ModalResult::Confirmed(Self::copy_result("json")),
                        BTN_INSERT => ModalResult::Confirmed(Self::copy_result("insert")),
                        // BTN_CANCEL, and any future button, closes without
                        // writing.
                        _ => ModalResult::Cancelled,
                    }));
                }
                // From a field, Enter jumps to the button bar so Save is one
                // more Enter away — the same two-step commit as the grid.
                self.focus = self.fields.len();
                self.button = BTN_SAVE;
            }
            KeyCode::Left if self.on_buttons() => {
                self.button = self.button.saturating_sub(1);
            }
            KeyCode::Right if self.on_buttons() => {
                self.button = (self.button + 1).min(BUTTON_COUNT - 1);
            }
            KeyCode::Char(' ') if !self.on_buttons() => {
                // Space toggles NULL on a nullable field, and types a space
                // otherwise.
                let toggled = match self.focused_field_mut() {
                    Some(field) if field.nullable => {
                        field.is_null = !field.is_null;
                        true
                    }
                    _ => false,
                };
                if !toggled {
                    if let Some(field) = self.focused_field_mut() {
                        let byte = char_index_to_byte(&field.text, field.caret);
                        field.text.insert(byte, ' ');
                        field.caret += 1;
                    }
                }
            }
            KeyCode::Char(c) if !self.on_buttons() => {
                if let Some(field) = self.focused_field_mut() {
                    // Typing into a NULL field starts a real value.
                    field.is_null = false;
                    let byte = char_index_to_byte(&field.text, field.caret);
                    field.text.insert(byte, c);
                    field.caret += 1;
                }
            }
            KeyCode::Backspace if !self.on_buttons() => {
                if let Some(field) = self.focused_field_mut() {
                    if field.caret > 0 {
                        let start = char_index_to_byte(&field.text, field.caret - 1);
                        let end = char_index_to_byte(&field.text, field.caret);
                        field.text.replace_range(start..end, "");
                        field.caret -= 1;
                    }
                }
            }
            KeyCode::Delete if !self.on_buttons() => {
                if let Some(field) = self.focused_field_mut() {
                    let len = field.text.chars().count();
                    if field.caret < len {
                        let start = char_index_to_byte(&field.text, field.caret);
                        let end = char_index_to_byte(&field.text, field.caret + 1);
                        field.text.replace_range(start..end, "");
                    }
                }
            }
            KeyCode::Left => {
                if let Some(field) = self.focused_field_mut() {
                    field.caret = field.caret.saturating_sub(1);
                }
            }
            KeyCode::Right => {
                if let Some(field) = self.focused_field_mut() {
                    field.caret = (field.caret + 1).min(field.text.chars().count());
                }
            }
            KeyCode::Home => {
                if let Some(field) = self.focused_field_mut() {
                    field.caret = 0;
                }
            }
            KeyCode::End => {
                if let Some(field) = self.focused_field_mut() {
                    field.caret = field.text.chars().count();
                }
            }
            _ => {}
        }
        Ok(None)
    }

    fn handle_mouse(
        &mut self,
        _event: MouseEvent,
        _area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use termide_core::KeyChord;

    fn columns() -> Vec<DbRowEditColumn> {
        vec![
            DbRowEditColumn {
                name: "id".into(),
                value: Some("1".into()),
                nullable: false,
                is_key: true,
            },
            DbRowEditColumn {
                name: "name".into(),
                value: Some("alpha".into()),
                nullable: true,
                is_key: false,
            },
            DbRowEditColumn {
                name: "note".into(),
                value: None,
                nullable: true,
                is_key: false,
            },
        ]
    }

    fn press(modal: &mut DbRowEditModal, code: KeyCode) -> Option<ModalResult<DbRowEditResult>> {
        modal
            .handle_key(KeyChord::identity(KeyEvent::new(code, KeyModifiers::NONE)))
            .unwrap()
    }

    #[test]
    fn an_untouched_row_saves_nothing() {
        let mut modal = DbRowEditModal::new("row", columns(), None);
        modal.focus = modal.fields.len();
        let result = press(&mut modal, KeyCode::Enter);
        match result {
            Some(ModalResult::Confirmed(r)) => assert!(r.changes.is_empty(), "{:?}", r.changes),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn editing_a_field_reports_only_that_column() {
        let mut modal = DbRowEditModal::new("row", columns(), None);
        modal.focus = 1; // name
        press(&mut modal, KeyCode::Char('!'));
        modal.focus = modal.fields.len();
        match press(&mut modal, KeyCode::Enter) {
            Some(ModalResult::Confirmed(r)) => {
                assert_eq!(r.changes, vec![("name".to_string(), Some("alpha!".into()))]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    /// The NULL checkbox is how a value is cleared — an empty text field is an
    /// empty value, not NULL.
    #[test]
    fn space_toggles_null_on_a_nullable_field() {
        let mut modal = DbRowEditModal::new("row", columns(), None);
        modal.focus = 1; // name, nullable
        press(&mut modal, KeyCode::Char(' '));
        modal.focus = modal.fields.len();
        match press(&mut modal, KeyCode::Enter) {
            Some(ModalResult::Confirmed(r)) => {
                assert_eq!(r.changes, vec![("name".to_string(), None)]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    /// Typing into a NULL field turns it into a value.
    #[test]
    fn typing_clears_the_null_flag() {
        let mut modal = DbRowEditModal::new("row", columns(), None);
        modal.focus = 2; // note, currently NULL
        press(&mut modal, KeyCode::Char('h'));
        press(&mut modal, KeyCode::Char('i'));
        modal.focus = modal.fields.len();
        match press(&mut modal, KeyCode::Enter) {
            Some(ModalResult::Confirmed(r)) => {
                assert_eq!(r.changes, vec![("note".to_string(), Some("hi".into()))]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    /// Primary-key fields are the row's address: they render but never change.
    #[test]
    fn key_columns_are_read_only() {
        let mut modal = DbRowEditModal::new("row", columns(), None);
        modal.focus = 0; // id, part of the key
        press(&mut modal, KeyCode::Char('9'));
        assert_eq!(modal.fields[0].text, "1");
        assert!(!modal.fields[0].changed());
    }

    /// With a notice (no primary key) the whole row is read-only.
    #[test]
    fn a_notice_makes_every_field_read_only() {
        let mut modal = DbRowEditModal::new("row", columns(), Some("no key".into()));
        modal.focus = 1;
        press(&mut modal, KeyCode::Char('!'));
        assert!(!modal.dirty());
    }

    #[test]
    fn copy_buttons_report_their_action_instead_of_changes() {
        let mut modal = DbRowEditModal::new("row", columns(), None);
        modal.focus = modal.fields.len();
        modal.button = BTN_JSON;
        match press(&mut modal, KeyCode::Enter) {
            Some(ModalResult::Confirmed(r)) => {
                assert_eq!(r.copy.as_deref(), Some("json"));
                assert!(r.changes.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn escape_cancels() {
        let mut modal = DbRowEditModal::new("row", columns(), None);
        assert!(matches!(
            press(&mut modal, KeyCode::Esc),
            Some(ModalResult::Cancelled)
        ));
    }
}
