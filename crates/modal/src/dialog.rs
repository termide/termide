//! A dialog put together from parts: a message, a text field, a list to
//! tick or pick from, and a row of buttons. Each part but the buttons may
//! be left out; `Tab` goes from one part to the next.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use termide_theme::Theme;
use termide_ui::choice_form::wrap;
use termide_ui::path_utils::truncate_right;
use termide_ui::{checkbox, TextArea};

use crate::base::{button_style, render_modal_block};
use crate::{centered_rect_with_size, Modal, ModalResult};

/// The most message lines shown; the rest is cut.
const MAX_MESSAGE_LINES: usize = 10;
/// The most list rows shown before the list scrolls.
const MAX_LIST_ROWS: usize = 12;

/// A row of the dialog's list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DialogItem {
    pub label: String,
    /// Shown dim after the label.
    pub detail: String,
    /// Ticked to start with, in a list to tick.
    pub checked: bool,
}

/// What the list is for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DialogPick {
    /// Only to read.
    #[default]
    None,
    /// To pick one row from.
    One,
    /// To tick any rows of.
    Many,
}

/// The dialog's text field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DialogInput {
    /// Shown above the field.
    pub label: String,
    pub text: String,
    /// Its height in lines; one makes `Enter` press the first button.
    pub lines: u16,
}

/// What the dialog is made of.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DialogSpec {
    pub title: String,
    pub message: String,
    pub input: Option<DialogInput>,
    pub items: Vec<DialogItem>,
    pub pick: DialogPick,
    /// At least one; the first is pressed by `Enter` in the field or the
    /// list, and by `Ctrl+Enter` anywhere.
    pub buttons: Vec<String>,
}

/// What the dialog came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogAnswer {
    /// The button pressed, by its index.
    pub button: usize,
    /// The field's text, when there is a field.
    pub text: Option<String>,
    /// The rows ticked, in a list to tick.
    pub checked: Vec<usize>,
    /// The row picked, in a list to pick from.
    pub selected: Option<usize>,
}

/// A part of the dialog the keys can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Input,
    List,
    Buttons,
}

#[derive(Debug)]
pub struct DialogModal {
    spec: DialogSpec,
    field: TextArea,
    checked: Vec<bool>,
    /// The list row under the cursor.
    cursor: usize,
    list_scroll: usize,
    focus: Part,
    button: usize,
    /// Where the last render drew each part, for the mouse.
    input_area: Option<Rect>,
    list_area: Option<Rect>,
    button_areas: Vec<Rect>,
}

impl DialogModal {
    pub fn new(mut spec: DialogSpec) -> Self {
        if spec.buttons.is_empty() {
            spec.buttons.push("OK".to_string());
        }
        if let Some(input) = spec.input.as_mut() {
            input.lines = input.lines.clamp(1, 10);
        }
        let field = TextArea::with_text(spec.input.as_ref().map_or("", |i| i.text.as_str()));
        let checked = spec.items.iter().map(|item| item.checked).collect();
        let mut modal = Self {
            spec,
            field,
            checked,
            cursor: 0,
            list_scroll: 0,
            focus: Part::Buttons,
            button: 0,
            input_area: None,
            list_area: None,
            button_areas: Vec::new(),
        };
        modal.focus = modal.parts()[0];
        modal
    }

    /// The parts present, in the order `Tab` visits them.
    fn parts(&self) -> Vec<Part> {
        let mut parts = Vec::new();
        if self.spec.input.is_some() {
            parts.push(Part::Input);
        }
        if self.spec.pick != DialogPick::None && !self.spec.items.is_empty() {
            parts.push(Part::List);
        }
        parts.push(Part::Buttons);
        parts
    }

    fn step_focus(&mut self, back: bool) {
        let parts = self.parts();
        let at = parts.iter().position(|p| *p == self.focus).unwrap_or(0);
        let next = if back {
            (at + parts.len() - 1) % parts.len()
        } else {
            (at + 1) % parts.len()
        };
        self.focus = parts[next];
    }

    fn answer(&self, button: usize) -> ModalResult<DialogAnswer> {
        let ticking = self.spec.pick == DialogPick::Many;
        ModalResult::Confirmed(DialogAnswer {
            button,
            text: self.spec.input.as_ref().map(|_| self.field.text()),
            checked: if ticking {
                (0..self.checked.len())
                    .filter(|&i| self.checked[i])
                    .collect()
            } else {
                Vec::new()
            },
            selected: (self.spec.pick == DialogPick::One && !self.spec.items.is_empty())
                .then_some(self.cursor),
        })
    }

    fn list_rows(&self) -> usize {
        self.spec.items.len().min(MAX_LIST_ROWS)
    }

    fn move_cursor(&mut self, delta: isize) {
        let last = self.spec.items.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
        let rows = self.list_rows().max(1);
        if self.cursor < self.list_scroll {
            self.list_scroll = self.cursor;
        } else if self.cursor >= self.list_scroll + rows {
            self.list_scroll = self.cursor + 1 - rows;
        }
    }

    fn toggle(&mut self, row: usize) {
        if self.spec.pick == DialogPick::Many {
            if let Some(checked) = self.checked.get_mut(row) {
                *checked = !*checked;
            }
        }
    }

    /// The message wrapped to `width`, cut to its most lines.
    fn message_lines(&self, width: usize) -> Vec<String> {
        if self.spec.message.trim().is_empty() {
            return Vec::new();
        }
        let mut lines = wrap(&self.spec.message, width);
        if lines.len() > MAX_MESSAGE_LINES {
            lines.truncate(MAX_MESSAGE_LINES);
            if let Some(last) = lines.last_mut() {
                last.push('…');
            }
        }
        lines
    }

    /// The modal's width and height on a screen of `area`.
    fn size(&self, area: Rect) -> (u16, u16) {
        let content = self
            .spec
            .items
            .iter()
            .map(|item| item.label.width() + item.detail.width() + 8)
            .chain(std::iter::once(self.spec.title.width() + 4))
            .chain(std::iter::once(
                self.spec
                    .buttons
                    .iter()
                    .map(|b| b.width() + 5)
                    .sum::<usize>(),
            ))
            .max()
            .unwrap_or(0);
        let width = (content as u16 + 4)
            .clamp(50, 90)
            .min(area.width.saturating_sub(2).max(20));
        let inner = width.saturating_sub(4) as usize;
        let mut height = 2 + 1; // borders, the buttons
        let message = self.message_lines(inner).len() as u16;
        if message > 0 {
            height += message + 1;
        }
        if let Some(input) = &self.spec.input {
            height += u16::from(!input.label.is_empty()) + input.lines + 2 + 1;
        }
        if !self.spec.items.is_empty() {
            height += self.list_rows() as u16 + 1;
        }
        (width, height.min(area.height))
    }

    fn render_field(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let focused = self.focus == Part::Input;
        let border = Style::default().fg(if focused {
            theme.accented_fg
        } else {
            theme.disabled
        });
        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(border);
        let inner = block.inner(area);
        ratatui::widgets::Widget::render(block, area, buf);
        self.input_area = Some(inner);
        self.field.ensure_cursor_visible(inner.height as usize);
        let cursor = self.field.cursor();
        let selection = self.field.selection_range();
        let scroll = self.field.scroll_offset();
        for (i, line) in self
            .field
            .lines()
            .iter()
            .enumerate()
            .skip(scroll)
            .take(inner.height as usize)
        {
            let y = inner.y + (i - scroll) as u16;
            // A one-line field keeps the cursor in sight by its tail.
            let skip = if self.spec.input.as_ref().is_some_and(|f| f.lines == 1) {
                cursor
                    .col
                    .saturating_sub(inner.width.saturating_sub(1) as usize)
            } else {
                0
            };
            let mut x = inner.x;
            let len = line.chars().count();
            for (col, ch) in line.chars().enumerate().skip(skip) {
                let width = ch.width().unwrap_or(1) as u16;
                if x + width > inner.right() {
                    break;
                }
                let selected = selection.is_some_and(|(start, end)| {
                    (i, col) >= (start.row, start.col) && (i, col) < (end.row, end.col)
                });
                let style = if focused && i == cursor.row && col == cursor.col {
                    Style::default().fg(theme.bg).bg(theme.fg)
                } else if selected {
                    Style::default().fg(theme.selected_fg).bg(theme.selected_bg)
                } else {
                    Style::default().fg(theme.fg)
                };
                buf.set_string(x, y, ch.to_string(), style);
                x += width;
            }
            if focused && i == cursor.row && cursor.col >= len && x < inner.right() {
                buf.set_string(x, y, " ", Style::default().bg(theme.fg));
            }
        }
    }

    fn render_list(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        self.list_area = Some(area);
        let focused = self.focus == Part::List;
        for (line, row) in (self.list_scroll..self.spec.items.len())
            .take(area.height as usize)
            .enumerate()
        {
            let y = area.y + line as u16;
            let item = &self.spec.items[row];
            let under = focused && row == self.cursor;
            let base = if under {
                Style::default().fg(theme.selected_fg).bg(theme.selected_bg)
            } else {
                Style::default().fg(theme.fg)
            };
            for x in area.left()..area.right() {
                buf[(x, y)].set_style(base);
            }
            let mark = match self.spec.pick {
                DialogPick::Many => format!("{} ", checkbox(self.checked[row])),
                DialogPick::One if row == self.cursor => "› ".to_string(),
                _ => "  ".to_string(),
            };
            let room = area.width as usize;
            let text = truncate_right(&format!("{mark}{}", item.label), room);
            buf.set_stringn(area.x, y, &text, room, base);
            let used = text.width();
            if !item.detail.is_empty() && used + 3 < room {
                let dim = if under { base } else { base.fg(theme.disabled) };
                let detail = truncate_right(&item.detail, room - used - 2);
                buf.set_stringn(area.x + used as u16 + 2, y, &detail, room - used - 2, dim);
            }
        }
    }

    fn render_buttons(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let labels: Vec<String> = self
            .spec
            .buttons
            .iter()
            .map(|label| format!("[ {label} ]"))
            .collect();
        let total: usize = labels.iter().map(|l| l.width()).sum::<usize>() + 3 * labels.len();
        let mut x = area.x + (area.width as usize).saturating_sub(total) as u16 / 2;
        self.button_areas.clear();
        for (index, label) in labels.iter().enumerate() {
            let selected = self.focus == Part::Buttons && index == self.button;
            let width = (label.width() as u16).min(area.right().saturating_sub(x));
            buf.set_stringn(
                x,
                area.y,
                label,
                width as usize,
                button_style(selected, theme),
            );
            self.button_areas.push(Rect::new(x, area.y, width, 1));
            x = (x + width + 3).min(area.right());
        }
    }
}

impl Modal for DialogModal {
    type Result = DialogAnswer;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let (width, height) = self.size(area);
        let modal = centered_rect_with_size(width, height, area);
        let inner = render_modal_block(modal, buf, &self.spec.title, theme);
        let inner = Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(2),
            ..inner
        };
        let bottom = inner.bottom();
        let mut y = inner.y;
        for line in self.message_lines(inner.width as usize) {
            if y >= bottom {
                break;
            }
            buf.set_stringn(
                inner.x,
                y,
                &line,
                inner.width as usize,
                Style::default().fg(theme.fg),
            );
            y += 1;
        }
        if y > inner.y {
            y += 1;
        }
        self.input_area = None;
        if let Some(input) = self.spec.input.clone() {
            if !input.label.is_empty() && y < bottom {
                buf.set_stringn(
                    inner.x,
                    y,
                    &input.label,
                    inner.width as usize,
                    Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
                );
                y += 1;
            }
            let field = Rect::new(inner.x, y, inner.width, input.lines + 2);
            if field.bottom() < bottom {
                self.render_field(field, buf, theme);
            }
            y += input.lines + 3;
        }
        self.list_area = None;
        if !self.spec.items.is_empty() && y < bottom {
            let rows = (self.list_rows() as u16).min(bottom.saturating_sub(y + 1));
            self.render_list(Rect::new(inner.x, y, inner.width, rows), buf, theme);
        }
        let buttons = Rect::new(inner.x, bottom.saturating_sub(1), inner.width, 1);
        self.render_buttons(buttons, buf, theme);
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let key = chord.raw;
        if key.code == KeyCode::Esc {
            return Ok(Some(ModalResult::Cancelled));
        }
        if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(Some(self.answer(0)));
        }
        match key.code {
            KeyCode::Tab => {
                self.step_focus(false);
                return Ok(None);
            }
            KeyCode::BackTab => {
                self.step_focus(true);
                return Ok(None);
            }
            _ => {}
        }
        match self.focus {
            Part::Input => {
                let one_line = self.spec.input.as_ref().is_some_and(|f| f.lines == 1);
                match key.code {
                    KeyCode::Enter if one_line => return Ok(Some(self.answer(0))),
                    KeyCode::Enter => self.field.insert_newline(),
                    KeyCode::Up | KeyCode::Down if one_line => {}
                    _ => {
                        termide_ui::edit_text_area(&mut self.field, key);
                    }
                }
            }
            Part::List => match key.code {
                KeyCode::Up => self.move_cursor(-1),
                KeyCode::Down => self.move_cursor(1),
                KeyCode::PageUp => self.move_cursor(-(MAX_LIST_ROWS as isize)),
                KeyCode::PageDown => self.move_cursor(MAX_LIST_ROWS as isize),
                KeyCode::Home => self.move_cursor(isize::MIN / 2),
                KeyCode::End => self.move_cursor(isize::MAX / 2),
                KeyCode::Char(' ') => self.toggle(self.cursor),
                KeyCode::Enter => return Ok(Some(self.answer(0))),
                _ => {}
            },
            Part::Buttons => match key.code {
                KeyCode::Left => self.button = self.button.saturating_sub(1),
                KeyCode::Right => {
                    self.button = (self.button + 1).min(self.spec.buttons.len() - 1);
                }
                KeyCode::Enter | KeyCode::Char(' ') => return Ok(Some(self.answer(self.button))),
                _ => {}
            },
        }
        Ok(None)
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let at = (mouse.column, mouse.row).into();
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = self.button_areas.iter().position(|r| r.contains(at)) {
                    return Ok(Some(self.answer(index)));
                }
                if let Some(area) = self.input_area.filter(|r| r.contains(at)) {
                    self.focus = Part::Input;
                    let row = self.field.scroll_offset() + (mouse.row - area.y) as usize;
                    let row = row.min(self.field.line_count().saturating_sub(1));
                    self.field
                        .mouse_press(row, (mouse.column - area.x) as usize);
                }
                if let Some(area) = self.list_area.filter(|r| r.contains(at)) {
                    let row = self.list_scroll + (mouse.row - area.y) as usize;
                    if row < self.spec.items.len() {
                        if self.parts().contains(&Part::List) {
                            self.focus = Part::List;
                        }
                        self.cursor = row;
                        self.toggle(row);
                    }
                }
            }
            MouseEventKind::ScrollUp if self.list_area.is_some_and(|r| r.contains(at)) => {
                self.list_scroll = self.list_scroll.saturating_sub(1);
            }
            MouseEventKind::ScrollDown if self.list_area.is_some_and(|r| r.contains(at)) => {
                let max = self.spec.items.len().saturating_sub(self.list_rows());
                self.list_scroll = (self.list_scroll + 1).min(max);
            }
            _ => {}
        }
        Ok(None)
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        if self.focus != Part::Input {
            return false;
        }
        let one_line = self.spec.input.as_ref().is_some_and(|f| f.lines == 1);
        if one_line {
            self.field.insert_str(&text.replace(['\r', '\n'], " "));
        } else {
            self.field.insert_str(text);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn spec() -> DialogSpec {
        DialogSpec {
            title: "🧩 p4 · Submit".into(),
            message: "Submit these files?".into(),
            input: Some(DialogInput {
                label: "Description".into(),
                text: String::new(),
                lines: 3,
            }),
            items: vec![
                DialogItem {
                    label: "a.c".into(),
                    detail: "edit".into(),
                    checked: true,
                },
                DialogItem {
                    label: "b.c".into(),
                    detail: "add".into(),
                    checked: true,
                },
            ],
            pick: DialogPick::Many,
            buttons: vec!["Submit".into(), "Cancel".into()],
        }
    }

    fn press(modal: &mut DialogModal, code: KeyCode, mods: KeyModifiers) -> Option<DialogAnswer> {
        let chord = termide_core::KeyChord::new(
            KeyEvent::new(code, mods),
            &termide_core::KeyNormalizer::default(),
        );
        match modal.handle_key(chord).unwrap() {
            Some(ModalResult::Confirmed(answer)) => Some(answer),
            _ => None,
        }
    }

    fn typed(modal: &mut DialogModal, text: &str) {
        for c in text.chars() {
            press(modal, KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    #[test]
    fn the_field_the_ticks_and_the_button_come_back_together() {
        let mut modal = DialogModal::new(spec());
        typed(&mut modal, "Fix");
        assert!(
            press(&mut modal, KeyCode::Enter, KeyModifiers::NONE).is_none(),
            "Enter makes a new line in a field of several"
        );
        typed(&mut modal, "it");
        press(&mut modal, KeyCode::Tab, KeyModifiers::NONE);
        press(&mut modal, KeyCode::Down, KeyModifiers::NONE);
        press(&mut modal, KeyCode::Char(' '), KeyModifiers::NONE);
        let answer = press(&mut modal, KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(
            answer,
            DialogAnswer {
                button: 0,
                text: Some("Fix\nit".into()),
                checked: vec![0],
                selected: None,
            }
        );
    }

    #[test]
    fn tab_reaches_the_buttons_and_any_of_them_answers() {
        let mut modal = DialogModal::new(spec());
        press(&mut modal, KeyCode::Tab, KeyModifiers::NONE);
        press(&mut modal, KeyCode::Tab, KeyModifiers::NONE);
        press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        let answer = press(&mut modal, KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(answer.button, 1);
        press(&mut modal, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(modal.focus, Part::Input, "Tab goes round");
        assert_eq!(
            press(&mut modal, KeyCode::Enter, KeyModifiers::CONTROL).map(|a| a.button),
            Some(0)
        );
    }

    #[test]
    fn a_list_to_pick_from_answers_the_row_and_a_one_line_field_enter() {
        let mut modal = DialogModal::new(DialogSpec {
            input: Some(DialogInput {
                lines: 1,
                ..DialogInput::default()
            }),
            pick: DialogPick::One,
            ..spec()
        });
        typed(&mut modal, "x");
        press(&mut modal, KeyCode::Tab, KeyModifiers::NONE);
        press(&mut modal, KeyCode::Down, KeyModifiers::NONE);
        press(&mut modal, KeyCode::BackTab, KeyModifiers::NONE);
        let answer = press(&mut modal, KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(answer.selected, Some(1));
        assert_eq!(answer.text.as_deref(), Some("x"));
        assert!(answer.checked.is_empty());
    }

    #[test]
    fn a_dialog_of_a_message_alone_starts_on_its_buttons() {
        let mut modal = DialogModal::new(DialogSpec {
            title: "t".into(),
            message: "Hello".into(),
            ..DialogSpec::default()
        });
        assert_eq!(modal.spec.buttons, ["OK"]);
        let answer = press(&mut modal, KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert_eq!(answer.text, None);
        assert_eq!(answer.selected, None);
    }

    #[test]
    fn it_draws_every_part_and_a_click_presses_a_button() {
        let mut modal = DialogModal::new(spec());
        let area = Rect::new(0, 0, 80, 30);
        let mut buf = Buffer::empty(area);
        modal.render(area, &mut buf, &Theme::default());
        let screen: String = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        for part in [
            "Submit these files?",
            "Description",
            "[✓] a.c",
            "edit",
            "[ Cancel ]",
        ] {
            assert!(screen.contains(part), "{part} missing:\n{screen}");
        }
        let cancel = modal.button_areas[1];
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: cancel.x + 1,
            row: cancel.y,
            modifiers: KeyModifiers::NONE,
        };
        let answer = modal.handle_mouse(click, area).unwrap();
        assert!(matches!(
            answer,
            Some(ModalResult::Confirmed(DialogAnswer { button: 1, .. }))
        ));
    }
}
