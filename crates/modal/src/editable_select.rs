//! Destination path modal: a path field whose dropdown is a directory tree.
//!
//! The field opens on a default path; its tree starts with quick
//! destinations (the other panels' directories) and follows the path once it
//! is edited (see [`crate::path_field`]). Below the field come optional
//! choice rows, checkboxes and the OK/Cancel buttons.

use anyhow::Result;
use crossterm::event::KeyCode;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use std::path::PathBuf;

use crate::base::{button_style, render_modal_block};
use crate::input_keys::{handle_input_key, InputKeyResult};
use crate::path_field::{FieldKey, PathField};

use termide_config::constants::{
    MODAL_BUTTON_SPACING, MODAL_MAX_WIDTH_PERCENTAGE_DEFAULT, MODAL_MIN_WIDTH_WIDE,
    MODAL_PADDING_WITH_DOUBLE_BORDER,
};
use termide_config::FileManagerKeybindings;
use termide_i18n as i18n;
use termide_theme::Theme;
use unicode_width::UnicodeWidthStr;

use crate::{centered_rect_with_size, Modal, ModalResult};

/// A quick destination offered at the top of the tree.
#[derive(Debug, Clone)]
pub struct SelectOption {
    /// The path written into the field when it is picked.
    pub value: String,
    /// Display text for the option
    pub display: String,
}

/// Focus area in the modal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Input,
    Choice(usize),
    Checkbox(usize),
    Buttons,
}

/// What a choice row's value does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChoiceKind {
    /// Read by the host when the modal closes.
    Plain,
    /// File extensions: picking one swaps it into the path's file name.
    Extension,
}

/// A labelled row picking one of a few values, cycled with Left/Right.
#[derive(Debug, Clone)]
struct ModalChoice {
    kind: ChoiceKind,
    label: String,
    options: Vec<String>,
    selected: usize,
}

impl ModalChoice {
    /// The row text before the value: the label and the left arrow.
    fn prefix(&self) -> String {
        format!("{}: ◀ ", self.label)
    }

    /// The whole row as rendered.
    fn text(&self) -> String {
        format!("{}{} ▶", self.prefix(), self.options[self.selected])
    }

    fn step(&mut self, forward: bool) {
        let len = self.options.len();
        self.selected = if forward {
            (self.selected + 1) % len
        } else {
            (self.selected + len - 1) % len
        };
    }
}

#[derive(Debug, Clone)]
struct ModalCheckbox {
    label: String,
    checked: bool,
    visible_when_primary_checked: bool,
}

/// Destination path modal: a path field with a directory tree dropdown.
#[derive(Debug)]
pub struct EditableSelectModal {
    title: String,
    prompt: String,
    field: PathField,
    /// Widest quick destination, for the modal's width.
    widest_option: usize,
    focus: FocusArea,
    selected_button: usize, // 0 = OK, 1 = Cancel
    last_buttons_area: Option<Rect>,
    checkboxes: Vec<ModalCheckbox>,
    last_checkbox_areas: Vec<(usize, Rect)>,
    choices: Vec<ModalChoice>,
    last_choice_areas: Vec<(usize, Rect)>,
}

impl EditableSelectModal {
    /// A modal asking for a path, starting at `default_value`, with
    /// `options` as the quick destinations.
    pub fn new(
        title: impl Into<String>,
        prompt: impl Into<String>,
        default_value: impl Into<String>,
        options: Vec<SelectOption>,
    ) -> Self {
        let widest_option = options
            .iter()
            .map(|o| o.value.width().max(o.display.width()))
            .max()
            .unwrap_or(0);
        let roots = options.into_iter().map(|o| o.value).collect();
        Self {
            title: title.into(),
            prompt: prompt.into(),
            field: PathField::new(default_value, roots),
            widest_option,
            focus: FocusArea::Input,
            selected_button: 0, // OK button selected by default
            last_buttons_area: None,
            checkboxes: Vec::new(),
            last_checkbox_areas: Vec::new(),
            choices: Vec::new(),
            last_choice_areas: Vec::new(),
        }
    }

    /// Offer `dirs` as the quick destinations instead of the options given
    /// at construction.
    pub fn set_quick_destinations(&mut self, dirs: Vec<String>) {
        self.widest_option = dirs.iter().map(|d| d.width()).max().unwrap_or(0);
        self.field.set_roots(dirs);
    }

    /// Take a relative path from `dir`.
    pub fn with_base_dir(mut self, dir: PathBuf) -> Self {
        self.field.set_base_dir(dir);
        self
    }

    /// The path names a file: picking a directory in the tree keeps the
    /// name after the last `/`.
    pub fn keeping_file_name(mut self) -> Self {
        self.field.set_keeps_name(true);
        self
    }

    /// Use the file manager's directory keys as configured.
    pub fn with_keybindings(mut self, keybindings: &FileManagerKeybindings) -> Self {
        self.field.set_keybindings(keybindings);
        self
    }

    /// Add a row under the input that picks one of `options`, starting at
    /// `selected`. Left/Right (or a click on an arrow) change it.
    pub fn with_choice(self, label: String, options: Vec<String>, selected: usize) -> Self {
        self.with_choice_of(ChoiceKind::Plain, label, options, selected)
    }

    /// Add a row picking the file extension of the path, starting at
    /// `selected`; changing it swaps the extension in the field.
    pub fn with_extension_choice(
        self,
        label: String,
        extensions: Vec<String>,
        selected: usize,
    ) -> Self {
        self.with_choice_of(ChoiceKind::Extension, label, extensions, selected)
    }

    fn with_choice_of(
        mut self,
        kind: ChoiceKind,
        label: String,
        options: Vec<String>,
        selected: usize,
    ) -> Self {
        if !options.is_empty() {
            self.choices.push(ModalChoice {
                kind,
                label,
                selected: selected.min(options.len() - 1),
                options,
            });
        }
        self
    }

    /// The index of the value the plain choice row shows, if there is one.
    pub fn choice_index(&self) -> Option<usize> {
        self.choices
            .iter()
            .find(|c| c.kind == ChoiceKind::Plain)
            .map(|c| c.selected)
    }

    /// Add an optional checkbox to the modal
    pub fn with_checkbox(mut self, label: String) -> Self {
        self.checkboxes.push(ModalCheckbox {
            label,
            checked: false,
            visible_when_primary_checked: false,
        });
        self
    }

    pub fn with_conditional_checkbox(mut self, label: String) -> Self {
        self.checkboxes.push(ModalCheckbox {
            label,
            checked: false,
            visible_when_primary_checked: true,
        });
        self
    }

    /// The text in the input, as it would be confirmed.
    pub fn value(&self) -> &str {
        self.field.text()
    }

    /// Whether the checkbox is checked
    pub fn is_checkbox_checked(&self) -> bool {
        self.checkboxes.first().is_some_and(|c| c.checked)
    }

    pub fn is_secondary_checkbox_checked(&self) -> bool {
        self.checkboxes.get(1).is_some_and(|c| c.checked)
    }

    fn visible_checkbox_indices(&self) -> Vec<usize> {
        self.checkboxes
            .iter()
            .enumerate()
            .filter(|(idx, checkbox)| {
                if *idx == 0 {
                    true
                } else {
                    !checkbox.visible_when_primary_checked || self.is_checkbox_checked()
                }
            })
            .map(|(idx, _)| idx)
            .collect()
    }

    /// The focus below choice row `after` (or below the input with `None`):
    /// the next choice row, a checkbox or the buttons.
    fn focus_below_choices(&self, after: Option<usize>) -> FocusArea {
        let next = after.map_or(0, |i| i + 1);
        if next < self.choices.len() {
            return FocusArea::Choice(next);
        }
        self.visible_checkbox_indices()
            .first()
            .map(|&i| FocusArea::Checkbox(i))
            .unwrap_or(FocusArea::Buttons)
    }

    /// The focus above the first checkbox: the last choice row or the input.
    fn focus_before_checkboxes(&self) -> FocusArea {
        match self.choices.len() {
            0 => FocusArea::Input,
            n => FocusArea::Choice(n - 1),
        }
    }

    /// Step choice row `index`; an extension row swaps the path's extension.
    fn step_choice(&mut self, index: usize, forward: bool) {
        let Some(choice) = self.choices.get_mut(index) else {
            return;
        };
        let old = choice.options[choice.selected].clone();
        choice.step(forward);
        if choice.kind != ChoiceKind::Extension {
            return;
        }
        let new = choice.options[choice.selected].clone();
        let mut known = choice.options.clone();
        // The longest extension first, so `.tar.gz` wins over `.gz`.
        known.sort_by_key(|e| std::cmp::Reverse(e.len()));
        let text = self.field.text();
        let stem = known
            .iter()
            .chain(std::iter::once(&old))
            .find_map(|ext| text.strip_suffix(ext.as_str()))
            .unwrap_or(text);
        self.field.set_text(format!("{stem}{new}"));
    }

    /// Confirm the typed value; an empty one cancels.
    fn confirm(&self) -> ModalResult<String> {
        let text = self.field.text();
        if text.is_empty() {
            ModalResult::Cancelled
        } else {
            ModalResult::Confirmed(text.to_string())
        }
    }

    fn next_visible_checkbox_after(&self, current: usize) -> Option<usize> {
        let visible = self.visible_checkbox_indices();
        visible
            .iter()
            .position(|idx| *idx == current)
            .and_then(|pos| visible.get(pos + 1).copied())
    }

    fn previous_visible_checkbox_before(&self, current: usize) -> Option<usize> {
        let visible = self.visible_checkbox_indices();
        visible
            .iter()
            .position(|idx| *idx == current)
            .and_then(|pos| {
                pos.checked_sub(1)
                    .and_then(|prev| visible.get(prev).copied())
            })
    }

    fn prompt_lines(&self) -> u16 {
        if self.prompt.is_empty() {
            0
        } else {
            self.prompt.lines().count().max(1) as u16
        }
    }

    /// Calculate dynamic modal width and height
    fn calculate_modal_size(&self, screen_width: u16, screen_height: u16) -> (u16, u16) {
        let title_width = self.title.width() as u16 + 2;
        let prompt_width = self.prompt.lines().map(|l| l.width()).max().unwrap_or(0) as u16;
        // The field fits the widest quick destination and the first value,
        // and its arrow.
        let input_width = self.widest_option.max(self.field.text().width()) as u16 + 5;
        // Buttons width: "[ OK ]    [ Cancel ]" = ~21 characters
        let buttons_width = 21;
        let choice_width = self
            .choices
            .iter()
            .map(|c| {
                let widest = c.options.iter().map(|o| o.width()).max();
                (c.prefix().width() + widest.unwrap_or(0) + 2) as u16
            })
            .max()
            .unwrap_or(0);

        let content_width = title_width
            .max(prompt_width)
            .max(input_width)
            .max(buttons_width)
            .max(choice_width);
        let total_width = content_width + MODAL_PADDING_WITH_DOUBLE_BORDER;
        let max_width = (screen_width as f32 * MODAL_MAX_WIDTH_PERCENTAGE_DEFAULT) as u16;
        let width = total_width
            .max(MODAL_MIN_WIDTH_WIDE)
            .min(max_width)
            .min(screen_width);

        // Borders, prompt, field (and tree), choices, checkboxes, buttons.
        let height = 1
            + self.prompt_lines()
            + self.field.height()
            + self.choices.len() as u16
            + self.visible_checkbox_indices().len() as u16
            + 1
            + 1;
        (width, height.min(screen_height))
    }

    fn ok_or_cancel(&self, button: usize) -> ModalResult<String> {
        if button == 0 {
            self.confirm()
        } else {
            ModalResult::Cancelled
        }
    }

    /// Type `key` into the field from a row below it: focus returns there.
    fn type_into_field(&mut self, key: crossterm::event::KeyEvent) -> bool {
        match handle_input_key(self.field.input_mut(), key) {
            InputKeyResult::Handled => {
                self.focus = FocusArea::Input;
                true
            }
            InputKeyResult::TextModified => {
                self.focus = FocusArea::Input;
                self.field.text_edited();
                true
            }
            InputKeyResult::NotHandled => false,
        }
    }
}

impl Modal for EditableSelectModal {
    type Result = String;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let (modal_width, modal_height) = self.calculate_modal_size(area.width, area.height);
        let modal_area = centered_rect_with_size(modal_width, modal_height, area);
        let inner = render_modal_block(modal_area, buf, &self.title, theme);

        let prompt_lines = self.prompt_lines();
        let mut constraints = Vec::new();
        if prompt_lines > 0 {
            constraints.push(Constraint::Length(prompt_lines));
        }
        constraints.push(Constraint::Length(self.field.height()));
        for _ in &self.choices {
            constraints.push(Constraint::Length(1));
        }
        let visible_checkboxes = self.visible_checkbox_indices();
        for _ in &visible_checkboxes {
            constraints.push(Constraint::Length(1));
        }
        constraints.push(Constraint::Length(1)); // Buttons

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);
        let mut chunk_idx = 0;

        if prompt_lines > 0 {
            Paragraph::new(self.prompt.clone())
                .alignment(Alignment::Left)
                .style(Style::default().fg(theme.fg))
                .render(chunks[chunk_idx], buf);
            chunk_idx += 1;
        }

        self.field.render(
            chunks[chunk_idx],
            buf,
            theme,
            self.focus == FocusArea::Input,
        );
        chunk_idx += 1;

        self.last_choice_areas.clear();
        for (i, choice) in self.choices.iter().enumerate() {
            let style = if self.focus == FocusArea::Choice(i) {
                Style::default().fg(theme.accented_fg).bg(theme.bg)
            } else {
                Style::default().fg(theme.fg).bg(theme.bg)
            };
            Paragraph::new(choice.text())
                .style(style)
                .alignment(Alignment::Left)
                .render(chunks[chunk_idx], buf);
            self.last_choice_areas.push((i, chunks[chunk_idx]));
            chunk_idx += 1;
        }

        self.last_checkbox_areas.clear();
        for checkbox_idx in visible_checkboxes {
            let checkbox = &self.checkboxes[checkbox_idx];
            let checkbox_char = termide_ui::checkbox_mark(checkbox.checked);
            let checkbox_style = if self.focus == FocusArea::Checkbox(checkbox_idx) {
                Style::default().fg(theme.accented_fg).bg(theme.bg)
            } else {
                Style::default().fg(theme.fg).bg(theme.bg)
            };
            Paragraph::new(format!("[{}] {}", checkbox_char, checkbox.label))
                .style(checkbox_style)
                .alignment(Alignment::Left)
                .render(chunks[chunk_idx], buf);
            self.last_checkbox_areas
                .push((checkbox_idx, chunks[chunk_idx]));
            chunk_idx += 1;
        }

        let t = i18n::t();
        let ok_style = button_style(
            self.focus == FocusArea::Buttons && self.selected_button == 0,
            theme,
        );
        let cancel_style = button_style(
            self.focus == FocusArea::Buttons && self.selected_button == 1,
            theme,
        );
        let buttons = Line::from(vec![
            Span::styled(format!("[ {} ]", t.ui_ok()), ok_style),
            Span::raw("    "),
            Span::styled(format!("[ {} ]", t.ui_cancel()), cancel_style),
        ]);
        Paragraph::new(buttons)
            .alignment(Alignment::Center)
            .render(chunks[chunk_idx], buf);
        self.last_buttons_area = Some(chunks[chunk_idx]);
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let key = chord.raw;
        // An open tree takes every key, Esc included, until it closes.
        if self.field.is_open() {
            self.focus = FocusArea::Input;
        }

        match self.focus {
            FocusArea::Input => {
                if self.field.handle_key(chord) == FieldKey::Handled {
                    return Ok(None);
                }
                match key.code {
                    KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
                    KeyCode::Down => {
                        self.focus = self.focus_below_choices(None);
                        Ok(None)
                    }
                    KeyCode::Enter => Ok(Some(self.confirm())),
                    _ => Ok(None),
                }
            }
            FocusArea::Choice(index) => {
                match key.code {
                    KeyCode::Esc => return Ok(Some(ModalResult::Cancelled)),
                    KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') => {
                        self.step_choice(index, key.code != KeyCode::Left);
                        return Ok(None);
                    }
                    KeyCode::Up | KeyCode::BackTab => {
                        self.focus = match index {
                            0 => FocusArea::Input,
                            i => FocusArea::Choice(i - 1),
                        };
                        return Ok(None);
                    }
                    KeyCode::Down | KeyCode::Tab => {
                        self.focus = self.focus_below_choices(Some(index));
                        return Ok(None);
                    }
                    KeyCode::Enter => return Ok(Some(self.confirm())),
                    _ => {}
                }
                // Typing goes back to the input, as from a checkbox.
                self.type_into_field(key);
                Ok(None)
            }
            FocusArea::Checkbox(checkbox_idx) => {
                if key.code == KeyCode::Esc {
                    return Ok(Some(ModalResult::Cancelled));
                }
                // Space toggles checkbox — must come before typing, which
                // would eat it
                if key.code == KeyCode::Char(' ') {
                    if let Some(checkbox) = self.checkboxes.get_mut(checkbox_idx) {
                        checkbox.checked = !checkbox.checked;
                    }
                    return Ok(None);
                }
                if self.type_into_field(key) {
                    return Ok(None);
                }
                match key.code {
                    KeyCode::Up | KeyCode::BackTab => {
                        self.focus = self
                            .previous_visible_checkbox_before(checkbox_idx)
                            .map(FocusArea::Checkbox)
                            .unwrap_or_else(|| self.focus_before_checkboxes());
                        Ok(None)
                    }
                    KeyCode::Down | KeyCode::Tab => {
                        self.focus = self
                            .next_visible_checkbox_after(checkbox_idx)
                            .map(FocusArea::Checkbox)
                            .unwrap_or(FocusArea::Buttons);
                        Ok(None)
                    }
                    KeyCode::Enter => Ok(Some(self.confirm())),
                    _ => Ok(None),
                }
            }
            FocusArea::Buttons => {
                if key.code == KeyCode::Esc {
                    return Ok(Some(ModalResult::Cancelled));
                }
                if self.type_into_field(key) {
                    return Ok(None);
                }
                match key.code {
                    KeyCode::Left | KeyCode::Right => {
                        self.selected_button = 1 - self.selected_button;
                        Ok(None)
                    }
                    KeyCode::Up | KeyCode::BackTab => {
                        self.focus = match self.visible_checkbox_indices().into_iter().last() {
                            Some(idx) => FocusArea::Checkbox(idx),
                            None => self.focus_before_checkboxes(),
                        };
                        Ok(None)
                    }
                    KeyCode::Enter => Ok(Some(self.ok_or_cancel(self.selected_button))),
                    _ => Ok(None),
                }
            }
        }
    }

    fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        use crossterm::event::{MouseButton, MouseEventKind};

        if self.field.handle_mouse(&mouse) {
            self.focus = FocusArea::Input;
            return Ok(None);
        }
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return Ok(None);
        }
        let point = (mouse.column, mouse.row).into();

        // The choice row: the left arrow steps back, anywhere else forward.
        let choice_hit = self
            .last_choice_areas
            .iter()
            .find(|(_, area)| area.contains(point))
            .map(|&(i, area)| (i, area));
        if let Some((index, area)) = choice_hit {
            self.focus = FocusArea::Choice(index);
            // The prefix ends with "◀ ".
            let arrow = area.x + (self.choices[index].prefix().width() as u16).saturating_sub(2);
            self.step_choice(index, mouse.column != arrow);
            return Ok(None);
        }

        if let Some(&(checkbox_idx, _)) = self
            .last_checkbox_areas
            .iter()
            .find(|(_, area)| area.contains(point))
        {
            self.focus = FocusArea::Checkbox(checkbox_idx);
            if let Some(checkbox) = self.checkboxes.get_mut(checkbox_idx) {
                checkbox.checked = !checkbox.checked;
            }
            return Ok(None);
        }

        if let Some(buttons_area) = self.last_buttons_area {
            if buttons_area.contains(point) {
                // Buttons are centered: "[ OK ]    [ Cancel ]"
                let t = i18n::t();
                let ok_width = format!("[ {} ]", t.ui_ok()).width() as u16;
                let cancel_width = format!("[ {} ]", t.ui_cancel()).width() as u16;
                let total = ok_width + MODAL_BUTTON_SPACING + cancel_width;
                let start = buttons_area.x + buttons_area.width.saturating_sub(total) / 2;
                let cancel_start = start + ok_width + MODAL_BUTTON_SPACING;
                let button = if (start..start + ok_width).contains(&mouse.column) {
                    0
                } else if (cancel_start..cancel_start + cancel_width).contains(&mouse.column) {
                    1
                } else {
                    return Ok(None);
                };
                self.focus = FocusArea::Buttons;
                self.selected_button = button;
                return Ok(Some(self.ok_or_cancel(button)));
            }
        }

        Ok(None)
    }

    /// A paste goes into the input, as typing does: focus returns to it
    /// from the choice row, a checkbox or the buttons.
    fn handle_paste(&mut self, text: &str) -> bool {
        self.field.paste(text);
        self.focus = FocusArea::Input;
        true
    }
}

#[cfg(test)]
impl EditableSelectModal {
    /// The field's text area as last drawn.
    fn field_text_area(&self) -> Rect {
        self.field.text_area().expect("the field is drawn")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> termide_core::KeyChord {
        termide_core::KeyChord::identity(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn press(modal: &mut EditableSelectModal, code: KeyCode) -> Option<ModalResult<String>> {
        modal.handle_key(key(code)).unwrap()
    }

    fn type_text(modal: &mut EditableSelectModal, text: &str) {
        for c in text.chars() {
            press(modal, KeyCode::Char(c));
        }
    }

    fn levels() -> EditableSelectModal {
        EditableSelectModal::new("Pack", "", "a.zip", vec![]).with_choice(
            "Level".to_string(),
            vec!["None".into(), "Fast".into(), "Normal".into()],
            2,
        )
    }

    /// A scratch directory, removed when dropped.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A directory with `src/bin`, `docs` and `.git` in it.
    fn project() -> Scratch {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("termide-path-field-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["src/bin", "docs", ".git"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        Scratch(dir)
    }

    fn option(path: &std::path::Path) -> SelectOption {
        let value = format!("{}/", path.display());
        SelectOption {
            display: value.clone(),
            value,
        }
    }

    /// The tree's rows as drawn, one string per line, trailing blanks cut.
    fn screen(modal: &mut EditableSelectModal) -> String {
        let area = Rect::new(0, 0, 100, 40);
        let mut buf = Buffer::empty(area);
        modal.render(area, &mut buf, &Theme::default());
        (0..area.height)
            .map(|y| {
                let line: String = (0..area.width).map(|x| buf[(x, y)].symbol()).collect();
                line.trim_end().to_string() + "\n"
            })
            .collect()
    }

    #[test]
    fn a_bracketed_paste_lands_in_the_input() {
        let mut modal = levels();
        modal.focus = FocusArea::Choice(0);
        assert!(modal.handle_paste("/tmp/b\n"));
        assert_eq!(modal.value(), "a.zip/tmp/b");
        assert_eq!(modal.focus, FocusArea::Input);
    }

    #[test]
    fn conditional_checkbox_is_hidden_until_primary_is_checked() {
        let mut modal = EditableSelectModal::new("Copy", "Prompt", "dest", vec![])
            .with_checkbox("Create symlink".to_string())
            .with_conditional_checkbox("Use relative target".to_string());

        assert_eq!(modal.visible_checkbox_indices(), vec![0]);
        modal.checkboxes[0].checked = true;
        assert_eq!(modal.visible_checkbox_indices(), vec![0, 1]);
    }

    #[test]
    fn the_choice_row_is_reached_from_the_input_and_cycled() {
        let mut modal = levels();
        assert_eq!(modal.choice_index(), Some(2));
        press(&mut modal, KeyCode::Down);
        assert_eq!(modal.focus, FocusArea::Choice(0));
        press(&mut modal, KeyCode::Right);
        assert_eq!(modal.choice_index(), Some(0), "Right wraps around");
        press(&mut modal, KeyCode::Left);
        assert_eq!(modal.choice_index(), Some(2), "Left wraps back");
        press(&mut modal, KeyCode::Down);
        assert_eq!(modal.focus, FocusArea::Buttons);
        press(&mut modal, KeyCode::Up);
        assert_eq!(modal.focus, FocusArea::Choice(0));
        let result = press(&mut modal, KeyCode::Enter);
        assert!(matches!(result, Some(ModalResult::Confirmed(ref v)) if v == "a.zip"));
    }

    #[test]
    fn clicking_the_choice_row_steps_it() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let mut modal = levels();
        let area = Rect::new(0, 0, 60, 20);
        modal.render(area, &mut Buffer::empty(area), &Theme::default());
        let row = modal.last_choice_areas[0].1;
        let click = |column| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row: row.y,
            modifiers: KeyModifiers::NONE,
        };
        let left_arrow = row.x + "Level: ".len() as u16;
        modal.handle_mouse(click(left_arrow), area).unwrap();
        assert_eq!(modal.choice_index(), Some(1));
        modal.handle_mouse(click(left_arrow + 3), area).unwrap();
        assert_eq!(modal.choice_index(), Some(2));
        assert_eq!(modal.focus, FocusArea::Choice(0));
    }

    #[test]
    fn the_extension_row_swaps_the_extension_in_the_path() {
        let mut modal = EditableSelectModal::new("Pack", "", "/d/a.b.zip", vec![])
            .keeping_file_name()
            .with_extension_choice(
                "Format".to_string(),
                vec![".zip".into(), ".tar.gz".into(), ".gz".into()],
                0,
            )
            .with_choice("Level".to_string(), vec!["Fast".into()], 0);
        press(&mut modal, KeyCode::Down);
        press(&mut modal, KeyCode::Right);
        assert_eq!(modal.value(), "/d/a.b.tar.gz");
        press(&mut modal, KeyCode::Right);
        assert_eq!(
            modal.value(),
            "/d/a.b.gz",
            "the longest known extension goes"
        );
        // The level row is the plain choice the host reads.
        assert_eq!(modal.choice_index(), Some(0));
        press(&mut modal, KeyCode::Down);
        assert_eq!(modal.focus, FocusArea::Choice(1));
    }

    #[test]
    fn secondary_checkbox_state_is_reported() {
        let mut modal = EditableSelectModal::new("Copy", "Prompt", "dest", vec![])
            .with_checkbox("Create symlink".to_string())
            .with_conditional_checkbox("Use relative target".to_string());

        modal.checkboxes[0].checked = true;
        modal.checkboxes[1].checked = true;
        assert!(modal.is_checkbox_checked());
        assert!(modal.is_secondary_checkbox_checked());
    }

    #[test]
    fn a_click_places_the_cursor_and_a_drag_selects_in_the_input() {
        use crate::base::test_mouse::{drag, press as down};
        let mut modal = levels();
        let area = Rect::new(0, 0, 60, 20);
        modal.render(area, &mut Buffer::empty(area), &Theme::default());
        modal.focus = FocusArea::Buttons;
        let field = modal.field_text_area();
        modal
            .handle_mouse(down(field.x + 1, field.y), area)
            .unwrap();
        assert_eq!(modal.focus, FocusArea::Input);
        assert_eq!(modal.field.input_mut().cursor_pos(), 1);
        modal
            .handle_mouse(drag(field.x + 3, field.y), area)
            .unwrap();
        assert_eq!(modal.field.input_mut().selected_text(), Some(".z"));
        // Past the left side the selection reaches the start of the text.
        modal.handle_mouse(drag(0, field.y + 2), area).unwrap();
        assert_eq!(modal.field.input_mut().selected_text(), Some("a"));
        assert!(!modal.field.is_open());
    }

    #[test]
    fn tab_opens_the_quick_destinations_and_down_writes_them_into_the_field() {
        let a = project();
        let b = project();
        let default = format!("{}/", a.path().display());
        let mut modal = EditableSelectModal::new(
            "Copy",
            "",
            default.clone(),
            vec![option(a.path()), option(b.path())],
        );
        press(&mut modal, KeyCode::Tab);
        assert!(modal.field.is_open());
        // A root too long for its row loses its start, not its end.
        let drawn = screen(&mut modal);
        let b_name = b.path().file_name().unwrap().to_string_lossy();
        assert!(drawn.contains(&format!("{b_name}/│")), "{drawn}");

        press(&mut modal, KeyCode::Down);
        assert_eq!(modal.value(), default);
        press(&mut modal, KeyCode::Down);
        assert_eq!(modal.value(), format!("{}/", b.path().display()));
        // Right opens the root in place; Down steps into it.
        press(&mut modal, KeyCode::Right);
        press(&mut modal, KeyCode::Down);
        assert_eq!(modal.value(), format!("{}/docs/", b.path().display()));

        // Enter closes the tree keeping the path; a second one confirms.
        assert!(press(&mut modal, KeyCode::Enter).is_none());
        assert!(!modal.field.is_open());
        let result = press(&mut modal, KeyCode::Enter);
        assert!(
            matches!(result, Some(ModalResult::Confirmed(ref v)) if *v == format!("{}/docs/", b.path().display()))
        );
    }

    #[test]
    fn esc_closes_the_tree_restoring_the_path_and_a_second_esc_cancels() {
        let a = project();
        let default = format!("{}/", a.path().display());
        let mut modal = EditableSelectModal::new("Copy", "", default.clone(), vec![]);
        press(&mut modal, KeyCode::Tab);
        press(&mut modal, KeyCode::Down);
        press(&mut modal, KeyCode::Down);
        assert_ne!(modal.value(), default);
        assert!(press(&mut modal, KeyCode::Esc).is_none());
        assert_eq!(modal.value(), default);
        assert!(!modal.field.is_open());
        assert!(matches!(
            press(&mut modal, KeyCode::Esc),
            Some(ModalResult::Cancelled)
        ));
    }

    #[test]
    fn an_edited_path_lists_its_directory_filtered_by_the_name_typed() {
        let a = project();
        let default = format!("{}/", a.path().display());
        let mut modal = EditableSelectModal::new("Copy", "", default, vec![option(a.path())]);
        press(&mut modal, KeyCode::Tab);
        type_text(&mut modal, "dc");
        let drawn = screen(&mut modal);
        assert!(drawn.contains("▶ docs"), "{drawn}");
        assert!(!drawn.contains("▶ src"), "{drawn}");

        // A name starting with `.` lists hidden directories.
        for _ in 0..2 {
            press(&mut modal, KeyCode::Backspace);
        }
        type_text(&mut modal, ".g");
        let drawn = screen(&mut modal);
        assert!(drawn.contains("▶ .git"), "{drawn}");

        // From the tree, a letter is typed into the field.
        press(&mut modal, KeyCode::Down); // `..`
        press(&mut modal, KeyCode::Down);
        type_text(&mut modal, "x");
        assert!(modal.value().ends_with("/.git/x"), "{}", modal.value());
    }

    #[test]
    fn picking_a_directory_keeps_the_file_name() {
        let a = project();
        let mut modal = EditableSelectModal::new(
            "Copy",
            "",
            format!("{}/report.pdf", a.path().display()),
            vec![],
        )
        .keeping_file_name();
        press(&mut modal, KeyCode::Tab);
        // `..`, docs, src
        press(&mut modal, KeyCode::Down);
        press(&mut modal, KeyCode::Down);
        assert_eq!(
            modal.value(),
            format!("{}/docs/report.pdf", a.path().display())
        );
    }

    #[test]
    fn directories_are_created_in_the_tree_and_letters_stay_typing() {
        let a = project();
        let mut modal =
            EditableSelectModal::new("Copy", "", format!("{}/", a.path().display()), vec![]);
        press(&mut modal, KeyCode::Tab);
        press(&mut modal, KeyCode::Down);
        press(&mut modal, KeyCode::Down); // docs
                                          // `D` (create_dir as a letter) is typed, not a shortcut.
        type_text(&mut modal, "D");
        assert!(modal.value().ends_with("/docs/D"), "{}", modal.value());
        // The tree now lists `docs`; F7 on its `..` row creates in it.
        press(&mut modal, KeyCode::Down);
        press(&mut modal, KeyCode::F(7));
        type_text(&mut modal, "out");
        press(&mut modal, KeyCode::Enter);
        let out = a.path().join("docs/out");
        assert!(out.is_dir());
        assert_eq!(modal.value(), format!("{}/", out.display()));
        // Delete takes the empty directory away again.
        press(&mut modal, KeyCode::Delete);
        assert!(!out.exists());
    }

    #[test]
    fn toggle_hidden_shows_hidden_directories_in_the_tree() {
        let a = project();
        let mut modal =
            EditableSelectModal::new("Copy", "", format!("{}/", a.path().display()), vec![]);
        press(&mut modal, KeyCode::Tab);
        assert!(!screen(&mut modal).contains(".git"));
        press(&mut modal, KeyCode::Down);
        press(&mut modal, KeyCode::Char('.'));
        assert!(screen(&mut modal).contains("▶ .git"));
    }
}
