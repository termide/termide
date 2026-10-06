//! A question asked inside a panel: a short intent in the top border, an
//! optional dim detail block saying what exactly is being asked, and numbered
//! options in a small bordered card. Answered with the arrows and `Enter`, a
//! digit, or `Esc`; a single click moves the selection and a double click (or
//! `Enter`) confirms it. Two optional extra rows: one that takes an answer
//! typed by the user (`with_custom`), and one that cancels the whole thing
//! (`with_cancel`), which is also what `Esc` does.
//!
//! With `with_multi` several options can be picked: each shows a checkbox that
//! `Space`, `Enter` or its digit toggles, a typed answer counts as one more
//! pick, and a confirmation row submits them all. Options can carry a dim
//! description after their label (`with_descriptions`).
//!
//! One of several questions asked in a row can step between them
//! (`with_steps`): `←` goes back to the one before, `→` on to the next
//! already answered, and the answer given before can be shown picked
//! (`with_selected`, `with_checked`, `with_custom_answer`).
//!
//! For questions a panel raises on its own — an agent asking whether it may
//! run a command — this beats an app-wide modal: with several panels open a
//! modal does not say who is asking, a card sits in the panel that is.
//! Modals stay for choices the user starts.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use termide_core::ThemeColors;
use unicode_width::UnicodeWidthChar;

use crate::TextInput;

/// A collapsed detail block shows at most this many lines before it is folded
/// with an ellipsis; the same cap the input bar grows to.
const DETAIL_COLLAPSED_LINES: usize = 5;

/// What a key did to the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceAction {
    /// The selection moved, or typing went on.
    Handled,
    /// A fixed option was chosen, by `Enter` or its digit.
    Chosen(usize),
    /// The user typed an answer of their own and confirmed it.
    Custom(String),
    /// A multi-select form was confirmed: the options checked, in their
    /// order, and the answer typed, if any.
    Submitted {
        chosen: Vec<usize>,
        custom: Option<String>,
    },
    /// `Esc`, or the cancel row: the question is declined and whatever asked
    /// it should stop.
    Cancelled,
    /// `←` in a form that can step back: to the question before.
    Back,
    /// `→` in a form that can step on: to the next question.
    Forward,
    /// Not a form key.
    NotHandled,
}

/// One row of the card.
enum Row {
    Option(usize),
    Custom,
    Submit,
    Cancel,
}

pub struct ChoiceForm {
    title: String,
    /// The specifics of the question, shown dim under the title; folded to
    /// [`DETAIL_COLLAPSED_LINES`] unless expanded.
    detail: Option<String>,
    detail_expanded: bool,
    options: Vec<String>,
    /// A dim note after each option's label; empty for none.
    descriptions: Vec<String>,
    /// Label of the confirmation row, when several options can be picked.
    submit: Option<String>,
    /// Which options are checked, in a multi-select form.
    checked: Vec<bool>,
    /// The answer typed in a multi-select form, kept as one more pick.
    custom_answer: Option<String>,
    /// Label of the row that takes a typed answer, when offered.
    custom: Option<String>,
    /// Label of the row that cancels, when offered.
    cancel: Option<String>,
    /// Whether `←` and `→` step to the question before and after.
    back: bool,
    forward: bool,
    selected: usize,
    /// The answer being typed, once the custom row was chosen.
    typing: Option<TextInput>,
    drawn: Option<Rect>,
    /// Screen rect of the detail block from the last render, for click hits.
    detail_drawn: Option<Rect>,
    /// Screen row of the first option row from the last render, for click hits.
    rows_top: Option<u16>,
    /// The row each drawn line below `rows_top` belongs to, from the last
    /// render: a long option wraps onto several lines.
    line_rows: Vec<usize>,
}

impl ChoiceForm {
    #[must_use]
    pub fn new(title: impl Into<String>, options: Vec<String>) -> Self {
        Self {
            title: title.into(),
            detail: None,
            detail_expanded: false,
            checked: vec![false; options.len()],
            options,
            descriptions: Vec::new(),
            submit: None,
            custom_answer: None,
            custom: None,
            cancel: None,
            back: false,
            forward: false,
            selected: 0,
            typing: None,
            drawn: None,
            detail_drawn: None,
            rows_top: None,
            line_rows: Vec::new(),
        }
    }

    /// Show `detail` dim under the title, saying what exactly is being asked.
    /// Blank text adds no block.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        self.detail = (!detail.trim().is_empty()).then_some(detail);
        self
    }

    /// A dim note after each option's label, in the options' order; a blank
    /// one shows nothing.
    #[must_use]
    pub fn with_descriptions(mut self, descriptions: Vec<String>) -> Self {
        self.descriptions = descriptions;
        self
    }

    /// Let several options be picked: each gets a checkbox, and a row named
    /// `submit` confirms the picks.
    #[must_use]
    pub fn with_multi(mut self, submit: impl Into<String>) -> Self {
        self.submit = Some(submit.into());
        self
    }

    /// Offer a row where the user types an answer; `label` names it.
    #[must_use]
    pub fn with_custom(mut self, label: impl Into<String>) -> Self {
        self.custom = Some(label.into());
        self
    }

    /// Offer a row that cancels (the same as `Esc`); `label` names it.
    #[must_use]
    pub fn with_cancel(mut self, label: impl Into<String>) -> Self {
        self.cancel = Some(label.into());
        self
    }

    /// Let `←` step back and `→` step on, reported as
    /// [`ChoiceAction::Back`] and [`ChoiceAction::Forward`].
    #[must_use]
    pub fn with_steps(mut self, back: bool, forward: bool) -> Self {
        self.back = back;
        self.forward = forward;
        self
    }

    /// Start with the selection on row `index`; the rows offered so far
    /// count, so call it after the builders that add them.
    #[must_use]
    pub fn with_selected(mut self, index: usize) -> Self {
        if index < self.row_count() {
            self.selected = index;
        }
        self
    }

    /// Start with these options checked, in a multi-select form.
    #[must_use]
    pub fn with_checked(mut self, indices: &[usize]) -> Self {
        for &index in indices {
            if let Some(checked) = self.checked.get_mut(index) {
                *checked = true;
            }
        }
        self
    }

    /// Start with an answer of the user's own already given: shown in its
    /// row, picked in a multi-select form, and where editing it starts.
    #[must_use]
    pub fn with_custom_answer(mut self, answer: impl Into<String>) -> Self {
        let answer = answer.into();
        self.custom_answer = (!answer.trim().is_empty()).then_some(answer);
        self
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    #[must_use]
    pub fn options(&self) -> &[String] {
        &self.options
    }

    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Whether option `index` is checked in a multi-select form.
    #[must_use]
    pub fn is_checked(&self, index: usize) -> bool {
        self.checked.get(index).copied().unwrap_or(false)
    }

    /// The answer being typed, while the custom row is active.
    #[must_use]
    pub fn typed(&self) -> Option<&str> {
        self.typing.as_ref().map(TextInput::text)
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = (0..self.options.len()).map(Row::Option).collect();
        if self.custom.is_some() {
            rows.push(Row::Custom);
        }
        if self.submit.is_some() {
            rows.push(Row::Submit);
        }
        if self.cancel.is_some() {
            rows.push(Row::Cancel);
        }
        rows
    }

    fn row_count(&self) -> usize {
        self.options.len()
            + usize::from(self.custom.is_some())
            + usize::from(self.submit.is_some())
            + usize::from(self.cancel.is_some())
    }

    fn is_multi(&self) -> bool {
        self.submit.is_some()
    }

    /// The detail wrapped to `inner` columns, folded to
    /// [`DETAIL_COLLAPSED_LINES`] (the last line ellipsised) unless expanded.
    /// Empty when there is no detail.
    fn detail_lines(&self, inner: usize) -> Vec<String> {
        let Some(detail) = &self.detail else {
            return Vec::new();
        };
        let mut lines = wrap(detail, inner);
        if !self.detail_expanded && lines.len() > DETAIL_COLLAPSED_LINES {
            lines.truncate(DETAIL_COLLAPSED_LINES);
            if let Some(last) = lines.last_mut() {
                let budget = inner.saturating_sub(1);
                while width_of(last) > budget {
                    last.pop();
                }
                last.push('…');
            }
        }
        lines
    }

    /// The lines of row `index` at `inner` columns: its number, checkbox and
    /// label, then its description, wrapped under the label. Each line is the
    /// text drawn in the row's style and the description part drawn after it.
    fn row_lines(&self, index: usize, row: &Row, inner: usize) -> Vec<(String, String)> {
        let label = match row {
            Row::Option(option) => self.options[*option].as_str(),
            Row::Custom => self.custom.as_deref().unwrap_or(""),
            Row::Submit => self.submit.as_deref().unwrap_or(""),
            Row::Cancel => self.cancel.as_deref().unwrap_or(""),
        };
        // A multi-select form's picks show their checkbox before the label.
        let checkbox = |on: bool| if on { "[✓] " } else { "[ ] " };
        let mark = match row {
            Row::Option(option) if self.is_multi() => checkbox(self.checked[*option]),
            Row::Custom if self.is_multi() => checkbox(self.custom_answer.is_some()),
            _ => "",
        };
        let body = match (row, &self.typing, &self.custom_answer) {
            (Row::Custom, Some(input), _) => format!("{label}: {}▏", input.text()),
            (Row::Custom, None, Some(kept)) => format!("{label}: {kept}"),
            _ => label.to_string(),
        };
        let description = match row {
            Row::Option(option) => self.descriptions.get(*option).map_or("", |d| d.trim()),
            _ => "",
        };
        let prefix = format!(" {}. {mark}", index + 1);
        let indent = width_of(&prefix);
        let room = inner.saturating_sub(indent).max(1);

        // Pack the label's words, then the description's, two columns apart.
        let words = body
            .split_whitespace()
            .map(|w| (w, false))
            .chain(description.split_whitespace().map(|w| (w, true)));
        let mut lines: Vec<(String, String)> = Vec::new();
        let (mut text, mut note, mut used) = (String::new(), String::new(), 0usize);
        for (raw, is_note) in words {
            for word in hard_break(raw, room) {
                let w = width_of(&word);
                let mut gap = match (used, is_note && note.is_empty()) {
                    (0, _) => 0,
                    (_, true) => 2,
                    _ => 1,
                };
                if used > 0 && used + gap + w > room {
                    lines.push((std::mem::take(&mut text), std::mem::take(&mut note)));
                    used = 0;
                    gap = 0;
                }
                let part = if is_note { &mut note } else { &mut text };
                part.push_str(&" ".repeat(gap));
                part.push_str(&word);
                used += gap + w;
            }
        }
        if used > 0 || lines.is_empty() {
            lines.push((text, note));
        }
        for (n, (text, _)) in lines.iter_mut().enumerate() {
            let lead = if n == 0 {
                prefix.clone()
            } else {
                " ".repeat(indent)
            };
            text.insert_str(0, &lead);
        }
        lines
    }

    pub fn select_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn select_down(&mut self) {
        self.selected = (self.selected + 1).min(self.row_count().saturating_sub(1));
    }

    /// Move the selection to `index` without confirming it (a single click).
    pub fn select(&mut self, index: usize) {
        if index < self.row_count() {
            self.selected = index;
        }
    }

    /// Confirm the row at `index`, the same as `Enter` on it (a double click).
    pub fn activate_at(&mut self, index: usize) -> ChoiceAction {
        self.activate(index)
    }

    /// Act on the row at `index`: a fixed option is chosen (toggled in a
    /// multi-select form), the custom row starts typing, the confirmation row
    /// submits the picks, the cancel row cancels.
    fn activate(&mut self, index: usize) -> ChoiceAction {
        let rows = self.rows();
        let Some(row) = rows.get(index) else {
            return ChoiceAction::Handled;
        };
        self.selected = index;
        match row {
            Row::Option(option) if self.is_multi() => {
                self.checked[*option] = !self.checked[*option];
                ChoiceAction::Handled
            }
            Row::Option(option) => ChoiceAction::Chosen(*option),
            Row::Custom => {
                // Editing a kept answer starts from its text.
                let mut input = TextInput::new();
                for c in self.custom_answer.as_deref().unwrap_or("").chars() {
                    input.insert(c);
                }
                self.typing = Some(input);
                ChoiceAction::Handled
            }
            Row::Submit => {
                let chosen: Vec<usize> = (0..self.options.len())
                    .filter(|&option| self.checked[option])
                    .collect();
                if chosen.is_empty() && self.custom_answer.is_none() {
                    // Nothing picked yet: nothing to submit.
                    return ChoiceAction::Handled;
                }
                ChoiceAction::Submitted {
                    chosen,
                    custom: self.custom_answer.clone(),
                }
            }
            Row::Cancel => ChoiceAction::Cancelled,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ChoiceAction {
        if let Some(input) = &mut self.typing {
            // Typing the custom answer: Enter confirms, Esc goes back to the
            // rows, the rest edits the line.
            return match key.code {
                KeyCode::Enter => {
                    let text = input.text().trim().to_string();
                    if self.submit.is_some() {
                        // A multi-select form keeps the answer as one more
                        // pick; an emptied one is dropped.
                        self.typing = None;
                        self.custom_answer = (!text.is_empty()).then_some(text);
                        ChoiceAction::Handled
                    } else if text.is_empty() {
                        ChoiceAction::Handled
                    } else {
                        self.typing = None;
                        ChoiceAction::Custom(text)
                    }
                }
                KeyCode::Esc => {
                    self.typing = None;
                    ChoiceAction::Handled
                }
                KeyCode::Backspace => {
                    input.backspace();
                    ChoiceAction::Handled
                }
                KeyCode::Delete => {
                    input.delete();
                    ChoiceAction::Handled
                }
                KeyCode::Left => {
                    input.move_left();
                    ChoiceAction::Handled
                }
                KeyCode::Right => {
                    input.move_right();
                    ChoiceAction::Handled
                }
                KeyCode::Home => {
                    input.move_home();
                    ChoiceAction::Handled
                }
                KeyCode::End => {
                    input.move_end();
                    ChoiceAction::Handled
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    input.insert(c);
                    ChoiceAction::Handled
                }
                _ => ChoiceAction::NotHandled,
            };
        }
        match key.code {
            KeyCode::Up => {
                self.select_up();
                ChoiceAction::Handled
            }
            KeyCode::Down => {
                self.select_down();
                ChoiceAction::Handled
            }
            KeyCode::Enter if self.row_count() > 0 => self.activate(self.selected),
            KeyCode::Char(' ') if self.is_multi() => match self.rows().get(self.selected) {
                Some(Row::Option(_) | Row::Custom) => self.activate(self.selected),
                _ => ChoiceAction::Handled,
            },
            KeyCode::Char(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                if index < self.row_count() {
                    self.activate(index)
                } else {
                    ChoiceAction::Handled
                }
            }
            KeyCode::Esc => ChoiceAction::Cancelled,
            KeyCode::Left if self.back => ChoiceAction::Back,
            KeyCode::Right if self.forward => ChoiceAction::Forward,
            _ => ChoiceAction::NotHandled,
        }
    }

    /// Rows the card takes at `width`: a border above and below around the
    /// detail block (when present, with a blank line under it) and the lines
    /// of every entry, a long one wrapped.
    #[must_use]
    pub fn height(&self, width: u16) -> u16 {
        let inner = (width as usize).saturating_sub(2);
        let detail = self.detail_lines(inner).len();
        let separator = usize::from(detail > 0);
        let rows: usize = self
            .rows()
            .iter()
            .enumerate()
            .map(|(index, row)| self.row_lines(index, row, inner).len())
            .sum();
        (2 + detail + separator + rows) as u16
    }

    /// Draw the card filling `area` (use [`ChoiceForm::height`] rows). The
    /// title sits in the top border; the detail, when present, shows dim under
    /// it. The selected row is highlighted in the selection colours while the
    /// panel is focused, in bold otherwise, so an unfocused panel still shows
    /// what it is asking. While an answer is being typed, its row shows the
    /// text and a cursor.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, colors: &ThemeColors, focused: bool) {
        self.detail_drawn = None;
        self.rows_top = None;
        if area.width < 4 || area.height < 3 {
            self.drawn = None;
            return;
        }
        let border = Style::default().fg(if focused {
            colors.border_focused
        } else {
            colors.border
        });
        let text = Style::default().fg(colors.fg).bg(colors.bg);
        let dim = Style::default().fg(colors.disabled).bg(colors.bg);
        let width = area.width as usize;
        let inner = width.saturating_sub(2);
        let right = area.x + area.width - 1;
        let bottom = area.y + area.height - 1;

        buf.set_string(area.x, area.y, " ".repeat(width), text);
        buf.set_string(area.x, area.y, "┌", border);
        for x in area.x + 1..right {
            buf[(x, area.y)].set_symbol("─").set_style(border);
        }
        buf.set_string(right, area.y, "┐", border);
        let title = format!(" {} ", self.title);
        buf.set_stringn(
            area.x + 1,
            area.y,
            &title,
            width.saturating_sub(2),
            Style::default().fg(colors.fg).add_modifier(Modifier::BOLD),
        );
        buf.set_string(area.x, bottom, " ".repeat(width), text);
        buf.set_string(area.x, bottom, "└", border);
        for x in area.x + 1..right {
            buf[(x, bottom)].set_symbol("─").set_style(border);
        }
        buf.set_string(right, bottom, "┘", border);

        // Everything between the borders, clipped to what fits.
        let mut y = area.y + 1;
        let side = |buf: &mut Buffer, y: u16| {
            buf.set_string(area.x, y, " ".repeat(width), text);
            buf.set_string(area.x, y, "│", border);
            buf.set_string(right, y, "│", border);
        };

        let detail = self.detail_lines(inner);
        if !detail.is_empty() {
            let first = y;
            for line in &detail {
                if y >= bottom {
                    break;
                }
                side(buf, y);
                buf.set_stringn(area.x + 1, y, line, inner, dim);
                y += 1;
            }
            self.detail_drawn = Some(Rect {
                x: area.x,
                y: first,
                width: area.width,
                height: y - first,
            });
            // A blank line divides the detail from the options.
            if y < bottom {
                side(buf, y);
                y += 1;
            }
        }

        self.rows_top = Some(y);
        self.line_rows.clear();
        'rows: for (index, row) in self.rows().iter().enumerate() {
            let selected = index == self.selected;
            let style = match (selected, focused) {
                (true, true) => Style::default()
                    .fg(colors.selection_fg)
                    .bg(colors.selection_bg),
                (true, false) => text.add_modifier(Modifier::BOLD),
                (false, _) => text,
            };
            // An option's description follows its label, dim unless the row
            // is highlighted.
            let note_style = if selected && focused { style } else { dim };
            for (line, note) in self.row_lines(index, row, inner) {
                if y >= bottom {
                    break 'rows;
                }
                side(buf, y);
                let used = width_of(&line).min(inner);
                buf.set_stringn(area.x + 1, y, &line, inner, style);
                if !note.is_empty() {
                    buf.set_stringn(area.x + 1 + used as u16, y, &note, inner - used, note_style);
                }
                self.line_rows.push(index);
                y += 1;
            }
        }
        self.drawn = Some(area);
    }

    /// A single click: move the selection to the row under `(x, y)`, or fold
    /// or unfold the detail block, without confirming anything. Returns whether
    /// the click landed on the card.
    pub fn click_select(&mut self, x: u16, y: u16) -> bool {
        if self.toggle_detail_at(x, y) {
            return true;
        }
        match self.hit(x, y) {
            Some(index) => {
                self.select(index);
                true
            }
            None => self.contains(x, y),
        }
    }

    /// A double click: confirm the row under `(x, y)`, the same as choosing it
    /// with `Enter`. A click on the detail folds or unfolds it instead.
    pub fn click_confirm(&mut self, x: u16, y: u16) -> ChoiceAction {
        if self.toggle_detail_at(x, y) {
            return ChoiceAction::Handled;
        }
        match self.hit(x, y) {
            Some(index) => self.activate(index),
            None => ChoiceAction::NotHandled,
        }
    }

    /// Fold or unfold the detail block if `(x, y)` is on it; whether it was.
    fn toggle_detail_at(&mut self, x: u16, y: u16) -> bool {
        let Some(rect) = self.detail_drawn else {
            return false;
        };
        let inside =
            x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height;
        if inside {
            self.detail_expanded = !self.detail_expanded;
        }
        inside
    }

    /// Whether `(x, y)` is anywhere inside the card, from the last render.
    #[must_use]
    fn contains(&self, x: u16, y: u16) -> bool {
        self.drawn.is_some_and(|rect| {
            x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
        })
    }

    /// The row under a click at `(x, y)`, from the last render.
    #[must_use]
    pub fn hit(&self, x: u16, y: u16) -> Option<usize> {
        let rect = self.drawn?;
        let top = self.rows_top?;
        let inside = x >= rect.x && x < rect.x + rect.width && y >= top;
        if !inside {
            return None;
        }
        self.line_rows.get((y - top) as usize).copied()
    }
}

/// Wrap `text` to `width` columns: pack whitespace-separated words, hard-break
/// a word longer than the line, and start a new line on each existing newline.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        let mut cur_w = 0usize;
        for raw in para.split_whitespace() {
            for word in hard_break(raw, width) {
                let ww = width_of(&word);
                if cur.is_empty() {
                    cur = word;
                    cur_w = ww;
                } else if cur_w + 1 + ww <= width {
                    cur.push(' ');
                    cur.push_str(&word);
                    cur_w += 1 + ww;
                } else {
                    lines.push(std::mem::take(&mut cur));
                    cur = word;
                    cur_w = ww;
                }
            }
        }
        lines.push(cur);
    }
    lines
}

/// Split `word` into pieces each fitting `width` display columns, breaking mid
/// character run only when the word itself is longer than a whole line.
fn hard_break(word: &str, width: usize) -> Vec<String> {
    if width_of(word) <= width {
        return vec![word.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for ch in word.chars() {
        let cw = ch.width().unwrap_or(0);
        if cur_w + cw > width && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(ch);
        cur_w += cw;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Display width of `s` in terminal columns.
fn width_of(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> ChoiceForm {
        ChoiceForm::new(
            "Agent wants to run bash:",
            vec!["Allow once".into(), "Allow always".into(), "Deny".into()],
        )
        .with_detail("git push")
    }

    #[test]
    fn arrows_digits_enter_and_esc_answer() {
        let mut form = ChoiceForm::new(
            "Agent wants to run bash:",
            vec!["Allow once".into(), "Allow always".into(), "Deny".into()],
        );
        assert_eq!(
            form.handle_key(KeyEvent::from(KeyCode::Down)),
            ChoiceAction::Handled
        );
        assert_eq!(
            form.handle_key(KeyEvent::from(KeyCode::Enter)),
            ChoiceAction::Chosen(1)
        );
        assert_eq!(
            form.handle_key(KeyEvent::from(KeyCode::Char('3'))),
            ChoiceAction::Chosen(2)
        );
        assert_eq!(
            form.handle_key(KeyEvent::from(KeyCode::Char('9'))),
            ChoiceAction::Handled
        );
        assert_eq!(
            form.handle_key(KeyEvent::from(KeyCode::Esc)),
            ChoiceAction::Cancelled
        );
        assert_eq!(
            form.handle_key(KeyEvent::from(KeyCode::Char('x'))),
            ChoiceAction::NotHandled
        );
        // No detail: three rows and two borders.
        assert_eq!(form.height(50), 5);
    }

    #[test]
    fn arrows_step_only_where_offered_and_a_preset_answer_shows() {
        let left = KeyEvent::from(KeyCode::Left);
        let right = KeyEvent::from(KeyCode::Right);
        let mut plain = form();
        assert_eq!(plain.handle_key(left), ChoiceAction::NotHandled);
        assert_eq!(plain.handle_key(right), ChoiceAction::NotHandled);

        let mut stepping = form().with_steps(true, false);
        assert_eq!(stepping.handle_key(left), ChoiceAction::Back);
        assert_eq!(stepping.handle_key(right), ChoiceAction::NotHandled);
        let mut stepping = form().with_steps(false, true);
        assert_eq!(stepping.handle_key(right), ChoiceAction::Forward);

        // While an answer is typed, the arrows move the cursor.
        let mut typing = form()
            .with_custom("Other")
            .with_custom_answer("my words")
            .with_steps(true, true);
        typing = typing.with_selected(3);
        assert_eq!(typing.selected(), 3);
        typing.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(typing.typed(), Some("my words"));
        assert_eq!(typing.handle_key(left), ChoiceAction::Handled);

        let multi = ChoiceForm::new("Pick", vec!["a".into(), "b".into(), "c".into()])
            .with_multi("Submit")
            .with_checked(&[0, 2]);
        assert!(multi.is_checked(0) && !multi.is_checked(1) && multi.is_checked(2));
    }

    #[test]
    fn renders_the_intent_the_detail_and_the_options() {
        let mut form = form();
        // Two borders, one detail line, a blank divider and three options.
        assert_eq!(form.height(50), 7);
        let area = Rect::new(2, 3, 50, form.height(50));
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 12));
        form.render(area, &mut buf, &ThemeColors::default(), true);
        let row =
            |y: u16| -> String { (0..60).map(|x| buf[(x, y)].symbol().to_string()).collect() };
        assert!(row(3).contains("Agent wants to run bash:"), "{}", row(3));
        assert!(row(4).contains("git push"), "detail: {}", row(4));
        assert!(row(6).contains("1. Allow once"), "{}", row(6));
        assert!(row(8).contains("3. Deny"), "{}", row(8));
    }

    #[test]
    fn a_single_click_selects_and_a_double_click_confirms() {
        let mut form = form();
        let area = Rect::new(2, 3, 50, form.height(50));
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 12));
        form.render(area, &mut buf, &ThemeColors::default(), true);
        // Options start at row 6 (title, detail, blank divider above them).
        assert_eq!(form.hit(10, 6), Some(0));
        assert_eq!(form.hit(10, 8), Some(2));
        assert_eq!(form.hit(10, 3), None, "the border is not an option");

        // A single click only moves the selection.
        assert!(form.click_select(10, 8));
        assert_eq!(form.selected(), 2);
        // A double click on the selected row confirms it.
        assert_eq!(form.click_confirm(10, 8), ChoiceAction::Chosen(2));
    }

    #[test]
    fn clicking_the_detail_folds_and_unfolds_it() {
        let long = (1..=8)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut form =
            ChoiceForm::new("Agent wants to run bash:", vec!["Allow".into()]).with_detail(long);
        // Collapsed: five detail lines, a divider, one option, two borders.
        assert_eq!(form.height(50), 9);
        let area = Rect::new(2, 3, 50, form.height(50));
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 20));
        form.render(area, &mut buf, &ThemeColors::default(), true);

        // A click on the detail expands it to all eight lines.
        assert!(form.click_select(5, 4));
        assert_eq!(form.height(50), 12);
        // A click on it again folds it back.
        let area = Rect::new(2, 3, 50, form.height(50));
        form.render(area, &mut buf, &ThemeColors::default(), true);
        assert!(form.click_select(5, 4));
        assert_eq!(form.height(50), 9);
    }

    fn multi() -> ChoiceForm {
        ChoiceForm::new(
            "Which crates?",
            vec!["core".into(), "ui".into(), "app".into()],
        )
        .with_custom("Type your own")
        .with_multi("Done")
    }

    #[test]
    fn a_multi_select_form_toggles_and_submits_the_picks() {
        let mut form = multi();
        // Nothing picked: the confirmation row does nothing.
        assert_eq!(
            form.handle_key(KeyEvent::from(KeyCode::Char('5'))),
            ChoiceAction::Handled
        );
        // A digit, Space and Enter toggle an option instead of choosing it.
        assert_eq!(
            form.handle_key(KeyEvent::from(KeyCode::Char('3'))),
            ChoiceAction::Handled
        );
        assert!(form.is_checked(2));
        form.select(0);
        form.handle_key(KeyEvent::from(KeyCode::Char(' ')));
        form.handle_key(KeyEvent::from(KeyCode::Down));
        form.handle_key(KeyEvent::from(KeyCode::Enter));
        form.handle_key(KeyEvent::from(KeyCode::Enter));
        assert!(form.is_checked(0) && !form.is_checked(1) && form.is_checked(2));
        assert_eq!(
            form.handle_key(KeyEvent::from(KeyCode::Char('5'))),
            ChoiceAction::Submitted {
                chosen: vec![0, 2],
                custom: None
            }
        );
    }

    #[test]
    fn a_multi_select_form_keeps_a_typed_answer_as_a_pick() {
        let mut form = multi();
        form.handle_key(KeyEvent::from(KeyCode::Char('4')));
        for c in "docs".chars() {
            form.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        // Enter keeps the answer rather than submitting it.
        assert_eq!(
            form.handle_key(KeyEvent::from(KeyCode::Enter)),
            ChoiceAction::Handled
        );
        let area = Rect::new(0, 0, 40, form.height(40));
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 8));
        form.render(area, &mut buf, &ThemeColors::default(), true);
        let row =
            |y: u16| -> String { (0..40).map(|x| buf[(x, y)].symbol().to_string()).collect() };
        assert!(row(1).contains("1. [ ] core"), "{}", row(1));
        assert!(row(4).contains("4. [✓] Type your own: docs"), "{}", row(4));
        assert!(row(5).contains("5. Done"), "{}", row(5));
        assert_eq!(
            form.handle_key(KeyEvent::from(KeyCode::Char('5'))),
            ChoiceAction::Submitted {
                chosen: vec![],
                custom: Some("docs".into())
            }
        );
    }

    #[test]
    fn a_description_follows_its_label() {
        let mut form = ChoiceForm::new("Approach?", vec!["Channel".into(), "Slot".into()])
            .with_descriptions(vec!["like permissions".into(), String::new()]);
        let area = Rect::new(0, 0, 40, form.height(40));
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 4));
        form.render(area, &mut buf, &ThemeColors::default(), true);
        let row =
            |y: u16| -> String { (0..40).map(|x| buf[(x, y)].symbol().to_string()).collect() };
        assert!(
            row(1).contains("1. Channel  like permissions"),
            "{}",
            row(1)
        );
        assert!(row(2).contains("2. Slot "), "{}", row(2));
    }

    #[test]
    fn a_long_option_wraps_under_its_label() {
        let mut form = ChoiceForm::new(
            "Approach?",
            vec!["Keep the channel open between turns".into(), "Slot".into()],
        )
        .with_descriptions(vec!["like the permission prompt".into(), String::new()]);
        // Two borders, three lines of the first option and one of the second.
        assert_eq!(form.height(24), 7);
        let area = Rect::new(0, 0, 24, form.height(24));
        let mut buf = Buffer::empty(Rect::new(0, 0, 24, 7));
        form.render(area, &mut buf, &ThemeColors::default(), true);
        let row =
            |y: u16| -> String { (0..24).map(|x| buf[(x, y)].symbol().to_string()).collect() };
        // No line runs into the right border, and continuations sit under the
        // label rather than under the number.
        for y in 1..6 {
            assert!(row(y).ends_with('│'), "{}", row(y));
        }
        assert!(row(1).starts_with("│ 1. Keep the channel"), "{}", row(1));
        assert!(row(2).starts_with("│    open between"), "{}", row(2));
        assert!(row(4).contains("permission prompt"), "{}", row(4));
        assert!(row(5).contains("2. Slot"), "{}", row(5));
        // Every line of a wrapped option hits that option.
        assert_eq!(form.hit(5, 3), Some(0));
        assert_eq!(form.hit(5, 5), Some(1));
        assert_eq!(form.hit(5, 6), None);
    }

    #[test]
    fn wrap_breaks_on_spaces_and_newlines() {
        assert_eq!(wrap("hello world", 5), vec!["hello", "world"]);
        assert_eq!(wrap("a\nb", 10), vec!["a", "b"]);
        // A word longer than the width is hard-broken.
        assert_eq!(wrap("abcdef", 3), vec!["abc", "def"]);
    }
}
