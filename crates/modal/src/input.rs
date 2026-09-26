//! Text input modal dialog.

use anyhow::Result;
use crossterm::event::KeyCode;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::base::{button_style, render_input_field, render_modal_block, screen_x_to_char_pos};
use crate::input_keys::{handle_input_key, InputKeyResult};

use termide_config::constants::MODAL_BUTTON_SPACING;
use termide_core::ThemeColors;
use termide_i18n as i18n;
use termide_theme::Theme;
use termide_ui::{CompletionItem, CompletionList};

use crate::{
    calculate_modal_width, centered_rect_with_size, max_line_width, Modal, ModalResult,
    ModalWidthConfig, TextInputHandler,
};

/// Focus area in the modal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Input,
    Checkbox(usize),
    Buttons,
}

/// Where an input modal's suggestions come from. The modal asks again every
/// time its text changes; `poll` lets a source take in work it runs in the
/// background, such as walking a project.
pub trait Suggest: Send {
    /// Suggestions for `text`, best first. A suggestion's `value` is what
    /// accepting it puts in the field (`Tab`) or submits (`Enter`).
    fn suggest(&mut self, text: &str) -> Vec<CompletionItem>;

    /// Take in background work that has finished; `true` when `suggest`
    /// may now answer differently.
    fn poll(&mut self) -> bool {
        false
    }
}

/// Rows the suggestion list always takes, so the modal keeps its size (and
/// its input its place) while the list grows and shrinks under typing.
const SUGGESTION_ROWS: usize = 8;

/// Width the modal gets for suggestions, typically paths.
const SUGGESTION_WIDTH: u16 = 72;

struct Suggestions {
    source: Box<dyn Suggest>,
    list: CompletionList,
}

impl std::fmt::Debug for Suggestions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Suggestions")
            .field("items", &self.list.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
struct ModalCheckbox {
    label: String,
    checked: bool,
    visible_when_primary_checked: bool,
}

/// Text input modal window
#[derive(Debug)]
pub struct InputModal {
    title: String,
    prompt: String,
    input_handler: TextInputHandler,
    focus: FocusArea,
    selected_button: usize, // 0 = OK, 1 = Cancel
    last_buttons_area: Option<Rect>,
    last_input_area: Option<Rect>,
    checkboxes: Vec<ModalCheckbox>,
    last_checkbox_areas: Vec<(usize, Rect)>,
    /// Whether to mask input with asterisks (password mode).
    is_password: bool,
    /// A list under the input that follows the typed text.
    suggestions: Option<Suggestions>,
}

impl InputModal {
    /// Create a new input modal window
    pub fn new(title: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            prompt: prompt.into(),
            input_handler: TextInputHandler::new(),
            focus: FocusArea::Input,
            selected_button: 0, // OK button selected by default
            last_buttons_area: None,
            last_input_area: None,
            checkboxes: Vec::new(),
            last_checkbox_areas: Vec::new(),
            is_password: false,
            suggestions: None,
        }
    }

    /// Create with default value
    pub fn with_default(
        title: impl Into<String>,
        prompt: impl Into<String>,
        default: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            prompt: prompt.into(),
            input_handler: TextInputHandler::with_default(default),
            focus: FocusArea::Input,
            selected_button: 0, // OK button selected by default
            last_buttons_area: None,
            last_input_area: None,
            checkboxes: Vec::new(),
            last_checkbox_areas: Vec::new(),
            is_password: false,
            suggestions: None,
        }
    }

    /// Mask the input field, showing asterisks instead of the typed text
    /// (passphrase / password entry).
    pub fn password(mut self) -> Self {
        self.is_password = true;
        self
    }

    /// Show suggestions from `source` under the input.
    pub fn with_suggestions(mut self, source: Box<dyn Suggest>) -> Self {
        self.set_suggestions(source);
        self
    }

    /// Show suggestions from `source` under the input, for a modal built
    /// elsewhere.
    pub fn set_suggestions(&mut self, source: Box<dyn Suggest>) {
        self.suggestions = Some(Suggestions {
            source,
            list: CompletionList::new(Vec::new())
                .with_max_rows(SUGGESTION_ROWS)
                .below_input(),
        });
        self.refresh_suggestions();
    }

    /// Let the suggestion source take in background work; `true` when the
    /// list changed and the modal needs a redraw.
    pub fn poll_suggestions(&mut self) -> bool {
        let Some(suggestions) = self.suggestions.as_mut() else {
            return false;
        };
        if !suggestions.source.poll() {
            return false;
        }
        self.refresh_suggestions();
        true
    }

    /// Ask the source again for the current text; the best one is selected.
    fn refresh_suggestions(&mut self) {
        if let Some(suggestions) = self.suggestions.as_mut() {
            let items = suggestions.source.suggest(self.input_handler.text());
            suggestions.list.set_items(items);
            suggestions.list.select(0);
        }
    }

    /// The selected suggestion's value, while the list shows any.
    fn selected_suggestion(&self) -> Option<String> {
        self.suggestions
            .as_ref()
            .and_then(|s| s.list.selected_item())
            .map(|item| item.value.clone())
    }

    /// Submit the selected suggestion, else the typed text (an empty field
    /// cancels).
    fn confirm(&self) -> ModalResult<String> {
        if let Some(value) = self.selected_suggestion() {
            return ModalResult::Confirmed(value);
        }
        if self.input_handler.is_empty() {
            ModalResult::Cancelled
        } else {
            ModalResult::Confirmed(self.input_handler.text().to_string())
        }
    }

    /// The keys the suggestion list takes while the input has focus and the
    /// list shows something: `Up`/`Down` move, `Tab` puts the selection in
    /// the field (a directory then lists its contents). `None` for any other
    /// key.
    fn handle_suggestion_key(&mut self, code: KeyCode) -> Option<()> {
        if self.focus != FocusArea::Input {
            return None;
        }
        let suggestions = self.suggestions.as_mut().filter(|s| !s.list.is_empty())?;
        match code {
            KeyCode::Up => suggestions.list.select_up(),
            KeyCode::Down => suggestions.list.select_down(),
            KeyCode::Tab => {
                let value = suggestions.list.selected_item()?.value.clone();
                self.input_handler.set_text(value);
            }
            _ => return None,
        }
        Some(())
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

    /// Add a checkbox that only appears when the primary checkbox is checked.
    pub fn with_conditional_checkbox(mut self, label: String) -> Self {
        self.checkboxes.push(ModalCheckbox {
            label,
            checked: false,
            visible_when_primary_checked: true,
        });
        self
    }

    /// Whether the checkbox is checked
    pub fn is_checkbox_checked(&self) -> bool {
        self.checkboxes.first().is_some_and(|c| c.checked)
    }

    /// Whether the secondary conditional checkbox is checked
    pub fn is_secondary_checkbox_checked(&self) -> bool {
        self.checkboxes.get(1).is_some_and(|c| c.checked)
    }

    /// Calculate dynamic modal width and height
    fn calculate_modal_size(&self, screen_width: u16, screen_height: u16) -> (u16, u16) {
        let title_width = self.title.len() as u16 + 2;
        let prompt_width = max_line_width(&self.prompt);
        let buttons_width = 21u16; // "[ OK ]    [ Cancel ]"
        let input_width = self.input_handler.text().chars().count() as u16 + 20;
        let suggestion_width = if self.suggestions.is_some() {
            SUGGESTION_WIDTH
        } else {
            0
        };
        let checkbox_width = self
            .checkboxes
            .iter()
            .map(|c| max_line_width(&format!(" [✓] {}", c.label)))
            .max()
            .unwrap_or(0);

        let width = calculate_modal_width(
            [
                title_width,
                prompt_width,
                buttons_width,
                input_width,
                checkbox_width,
                suggestion_width,
            ]
            .into_iter(),
            screen_width,
            ModalWidthConfig {
                wide: self.suggestions.is_some(),
                double_border: true,
            },
        );

        // Calculate height: border + prompt + input(3) + checkbox(0 or 1) + buttons + border
        let prompt_lines = if self.prompt.is_empty() {
            0
        } else {
            self.prompt.lines().count().max(1) as u16
        };
        let checkbox_height = if self.visible_checkbox_indices().is_empty() {
            0
        } else {
            self.visible_checkbox_indices().len() as u16 + 1
        };
        let height = (1 + prompt_lines + 3 + self.suggestion_rows() + checkbox_height + 1 + 1)
            .min(screen_height);

        (width, height)
    }

    fn suggestion_rows(&self) -> u16 {
        if self.suggestions.is_some() {
            SUGGESTION_ROWS as u16
        } else {
            0
        }
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

    fn first_visible_checkbox(&self) -> Option<usize> {
        self.visible_checkbox_indices().into_iter().next()
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

    /// Move focus to next element
    fn focus_next(&mut self) {
        self.focus = match self.focus {
            FocusArea::Input => {
                if let Some(idx) = self.first_visible_checkbox() {
                    FocusArea::Checkbox(idx)
                } else {
                    FocusArea::Buttons
                }
            }
            FocusArea::Checkbox(current) => self
                .next_visible_checkbox_after(current)
                .map(FocusArea::Checkbox)
                .unwrap_or(FocusArea::Buttons),
            FocusArea::Buttons => FocusArea::Input,
        };
    }

    /// Move focus to previous element
    fn focus_prev(&mut self) {
        self.focus = match self.focus {
            FocusArea::Input => FocusArea::Buttons,
            FocusArea::Checkbox(current) => self
                .previous_visible_checkbox_before(current)
                .map(FocusArea::Checkbox)
                .unwrap_or(FocusArea::Input),
            FocusArea::Buttons => {
                if let Some(idx) = self.visible_checkbox_indices().into_iter().last() {
                    FocusArea::Checkbox(idx)
                } else {
                    FocusArea::Input
                }
            }
        };
    }
}

impl InputModal {
    fn handle_key_inner(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<String>>> {
        let key = chord.raw;
        // Escape always cancels
        if key.code == KeyCode::Esc {
            return Ok(Some(ModalResult::Cancelled));
        }

        if self.handle_suggestion_key(key.code).is_some() {
            return Ok(None);
        }

        // Tab navigation (works from any focus)
        if key.code == KeyCode::Tab {
            self.focus_next();
            return Ok(None);
        }
        if key.code == KeyCode::BackTab {
            self.focus_prev();
            return Ok(None);
        }

        match self.focus {
            FocusArea::Input => {
                // Try common input handling first
                match handle_input_key(&mut self.input_handler, key) {
                    InputKeyResult::Handled | InputKeyResult::TextModified => {
                        return Ok(None);
                    }
                    InputKeyResult::NotHandled => {}
                }

                // Modal-specific handling
                match key.code {
                    KeyCode::Down => {
                        self.focus_next();
                        Ok(None)
                    }
                    KeyCode::Enter => {
                        // Confirm input (or cancel if empty)
                        Ok(Some(self.confirm()))
                    }
                    _ => Ok(None),
                }
            }
            FocusArea::Checkbox(checkbox_idx) => {
                // Space toggles checkbox — must come before handle_input_key which would eat it
                if key.code == KeyCode::Char(' ') {
                    if let Some(checkbox) = self.checkboxes.get_mut(checkbox_idx) {
                        checkbox.checked = !checkbox.checked;
                    }
                    return Ok(None);
                }

                // Handle text input keys for quick typing
                match handle_input_key(&mut self.input_handler, key) {
                    InputKeyResult::Handled | InputKeyResult::TextModified => {
                        self.focus = FocusArea::Input;
                        return Ok(None);
                    }
                    InputKeyResult::NotHandled => {}
                }

                match key.code {
                    KeyCode::Up => {
                        self.focus = self
                            .previous_visible_checkbox_before(checkbox_idx)
                            .map(FocusArea::Checkbox)
                            .unwrap_or(FocusArea::Input);
                        Ok(None)
                    }
                    KeyCode::Down => {
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
                // Handle navigation keys first (before input handler steals Left/Right)
                match key.code {
                    KeyCode::Left => {
                        // Move to previous button (wrap around)
                        self.selected_button = if self.selected_button == 0 { 1 } else { 0 };
                        return Ok(None);
                    }
                    KeyCode::Right => {
                        // Move to next button (wrap around)
                        self.selected_button = if self.selected_button == 1 { 0 } else { 1 };
                        return Ok(None);
                    }
                    KeyCode::Up => {
                        self.focus_prev();
                        return Ok(None);
                    }
                    KeyCode::Enter => {
                        // Execute selected button action
                        return if self.selected_button == 0 {
                            // OK button
                            if self.input_handler.is_empty() {
                                Ok(Some(ModalResult::Cancelled))
                            } else {
                                Ok(Some(ModalResult::Confirmed(
                                    self.input_handler.text().to_string(),
                                )))
                            }
                        } else {
                            // Cancel button
                            Ok(Some(ModalResult::Cancelled))
                        };
                    }
                    _ => {}
                }

                // Handle text input keys — typing switches back to input
                match handle_input_key(&mut self.input_handler, key) {
                    InputKeyResult::Handled | InputKeyResult::TextModified => {
                        self.focus = FocusArea::Input;
                        return Ok(None);
                    }
                    InputKeyResult::NotHandled => {}
                }

                Ok(None)
            }
        }
    }
}

impl Modal for InputModal {
    type Result = String;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Calculate dynamic dimensions
        let (modal_width, modal_height) = self.calculate_modal_size(area.width, area.height);

        // Create centered area
        let modal_area = centered_rect_with_size(modal_width, modal_height, area);

        let inner = render_modal_block(modal_area, buf, &self.title, theme);

        // Split into prompt (if not empty), input, checkbox (if present), and buttons
        let prompt_lines = if self.prompt.is_empty() {
            0
        } else {
            self.prompt.lines().count().max(1) as u16
        };

        let mut constraints = Vec::new();
        if prompt_lines > 0 {
            constraints.push(Constraint::Length(prompt_lines)); // Prompt
        }
        constraints.push(Constraint::Length(3)); // Input
        let suggestion_rows = self.suggestion_rows();
        if suggestion_rows > 0 {
            constraints.push(Constraint::Length(suggestion_rows));
        }
        let visible_checkboxes = self.visible_checkbox_indices();
        for _ in &visible_checkboxes {
            constraints.push(Constraint::Length(1));
        }
        if !visible_checkboxes.is_empty() {
            constraints.push(Constraint::Length(1)); // Empty line
        }
        constraints.push(Constraint::Length(1)); // Buttons

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        let mut chunk_idx = 0;

        // Render prompt if not empty
        if prompt_lines > 0 {
            let prompt = Paragraph::new(self.prompt.clone())
                .alignment(Alignment::Left)
                .style(Style::default().fg(theme.fg));
            prompt.render(chunks[chunk_idx], buf);
            chunk_idx += 1;
        }

        // Render input field with border
        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accented_fg));
        let input_inner = input_block.inner(chunks[chunk_idx]);
        input_block.render(chunks[chunk_idx], buf);

        // Save input area for mouse handling
        self.last_input_area = Some(input_inner);

        // Render input content with cursor and selection
        // In password mode, display asterisks instead of actual characters
        let display_text;
        let text = if self.is_password {
            display_text = "*".repeat(self.input_handler.text().chars().count());
            &display_text
        } else {
            self.input_handler.text()
        };

        render_input_field(
            buf,
            input_inner.x,
            input_inner.y,
            input_inner.width,
            text,
            self.input_handler.cursor_pos(),
            self.input_handler.selection_range(),
            self.focus == FocusArea::Input,
            theme,
        );
        chunk_idx += 1;

        if suggestion_rows > 0 {
            if let Some(suggestions) = self.suggestions.as_mut() {
                let colors = ThemeColors::from(theme);
                suggestions.list.render(chunks[chunk_idx], buf, &colors);
            }
            chunk_idx += 1;
        }

        // Render checkbox if present
        self.last_checkbox_areas.clear();
        if !visible_checkboxes.is_empty() {
            for checkbox_idx in visible_checkboxes {
                let checkbox = &self.checkboxes[checkbox_idx];
                let checkbox_char = termide_ui::checkbox_mark(checkbox.checked);
                let checkbox_style = if self.focus == FocusArea::Checkbox(checkbox_idx) {
                    Style::default().fg(theme.accented_fg).bg(theme.bg)
                } else {
                    Style::default().fg(theme.fg).bg(theme.bg)
                };
                let checkbox_text = format!(" [{}] {}", checkbox_char, checkbox.label);
                let paragraph = Paragraph::new(checkbox_text)
                    .style(checkbox_style)
                    .alignment(Alignment::Left);
                paragraph.render(chunks[chunk_idx], buf);
                self.last_checkbox_areas
                    .push((checkbox_idx, chunks[chunk_idx]));
                chunk_idx += 1;
            }
            chunk_idx += 1; // empty line
        } else {
            self.last_checkbox_areas.clear();
        }

        // Render buttons
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

        let buttons_paragraph = Paragraph::new(buttons).alignment(Alignment::Center);
        buttons_paragraph.render(chunks[chunk_idx], buf);

        // Save buttons area for mouse handling
        self.last_buttons_area = Some(chunks[chunk_idx]);
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let before = self.input_handler.text().to_string();
        let result = self.handle_key_inner(chord)?;
        if result.is_none() && self.input_handler.text() != before {
            self.refresh_suggestions();
        }
        Ok(result)
    }

    fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        use crossterm::event::{MouseButton, MouseEventKind};

        if let Some(suggestions) = self.suggestions.as_mut() {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    suggestions.list.select_up();
                    return Ok(None);
                }
                MouseEventKind::ScrollDown => {
                    suggestions.list.select_down();
                    return Ok(None);
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(index) = suggestions.list.hit(mouse.column, mouse.row) {
                        suggestions.list.select(index);
                        self.focus = FocusArea::Input;
                        return Ok(Some(self.confirm()));
                    }
                }
                _ => {}
            }
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Check if click is on input field
                if let Some(input_area) = self.last_input_area {
                    if mouse.column >= input_area.x
                        && mouse.column < input_area.x + input_area.width
                        && mouse.row == input_area.y
                    {
                        self.focus = FocusArea::Input;
                        let click_x = (mouse.column - input_area.x) as usize;
                        let char_pos = screen_x_to_char_pos(self.input_handler.text(), click_x);
                        self.input_handler.set_cursor_with_selection_start(char_pos);
                        return Ok(None);
                    }
                }

                // Check if click is on checkbox
                for (checkbox_idx, checkbox_area) in &self.last_checkbox_areas {
                    if mouse.row >= checkbox_area.y
                        && mouse.row < checkbox_area.y + checkbox_area.height
                        && mouse.column >= checkbox_area.x
                        && mouse.column < checkbox_area.x + checkbox_area.width
                    {
                        self.focus = FocusArea::Checkbox(*checkbox_idx);
                        if let Some(checkbox) = self.checkboxes.get_mut(*checkbox_idx) {
                            checkbox.checked = !checkbox.checked;
                        }
                        return Ok(None);
                    }
                }

                // Check if click is on buttons
                let Some(buttons_area) = self.last_buttons_area else {
                    return Ok(None);
                };

                if mouse.row < buttons_area.y
                    || mouse.row >= buttons_area.y + buttons_area.height
                    || mouse.column < buttons_area.x
                    || mouse.column >= buttons_area.x + buttons_area.width
                {
                    return Ok(None);
                }

                // Calculate button positions
                let t = i18n::t();
                let ok_text = format!("[ {} ]", t.ui_ok());
                let cancel_text = format!("[ {} ]", t.ui_cancel());
                let total_text_width =
                    ok_text.len() + MODAL_BUTTON_SPACING as usize + cancel_text.len();

                let start_col = buttons_area.x
                    + (buttons_area.width.saturating_sub(total_text_width as u16)) / 2;
                let ok_end = start_col + ok_text.len() as u16;
                let cancel_start = ok_end + MODAL_BUTTON_SPACING;
                let cancel_end = cancel_start + cancel_text.len() as u16;

                if mouse.column >= start_col && mouse.column < ok_end {
                    // OK button clicked
                    self.focus = FocusArea::Buttons;
                    self.selected_button = 0;
                    if self.input_handler.is_empty() {
                        return Ok(Some(ModalResult::Cancelled));
                    } else {
                        return Ok(Some(ModalResult::Confirmed(
                            self.input_handler.text().to_string(),
                        )));
                    }
                } else if mouse.column >= cancel_start && mouse.column < cancel_end {
                    // Cancel button clicked
                    self.focus = FocusArea::Buttons;
                    self.selected_button = 1;
                    return Ok(Some(ModalResult::Cancelled));
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Extend selection during drag on input field
                if let Some(input_area) = self.last_input_area {
                    if mouse.row == input_area.y {
                        let drag_x = if mouse.column < input_area.x {
                            0
                        } else {
                            (mouse.column - input_area.x) as usize
                        };
                        let char_pos = screen_x_to_char_pos(self.input_handler.text(), drag_x);
                        self.input_handler.extend_selection_to(char_pos);
                    }
                }
            }
            _ => {}
        }

        Ok(None)
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        self.input_handler.paste(text);
        self.refresh_suggestions();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditional_checkbox_is_hidden_until_primary_is_checked() {
        let mut modal = InputModal::with_default("Copy", "Prompt", "dest")
            .with_checkbox("Create symlink".to_string())
            .with_conditional_checkbox("Use relative target".to_string());

        assert_eq!(modal.visible_checkbox_indices(), vec![0]);
        modal.checkboxes[0].checked = true;
        assert_eq!(modal.visible_checkbox_indices(), vec![0, 1]);
    }

    #[test]
    fn secondary_checkbox_state_is_reported() {
        let mut modal = InputModal::with_default("Copy", "Prompt", "dest")
            .with_checkbox("Create symlink".to_string())
            .with_conditional_checkbox("Use relative target".to_string());

        modal.checkboxes[0].checked = true;
        modal.checkboxes[1].checked = true;
        assert!(modal.is_checkbox_checked());
        assert!(modal.is_secondary_checkbox_checked());
    }

    /// Suggests the words that start with the typed text; `poll` adds one
    /// more word, as a background walk finishing would.
    struct Words {
        words: Vec<&'static str>,
        late: Option<&'static str>,
    }

    impl Suggest for Words {
        fn suggest(&mut self, text: &str) -> Vec<CompletionItem> {
            self.words
                .iter()
                .filter(|w| !text.is_empty() && w.starts_with(text))
                .map(|w| CompletionItem::new(*w))
                .collect()
        }

        fn poll(&mut self) -> bool {
            match self.late.take() {
                Some(word) => {
                    self.words.push(word);
                    true
                }
                None => false,
            }
        }
    }

    fn words_modal() -> InputModal {
        InputModal::new("Open", "").with_suggestions(Box::new(Words {
            words: vec!["src/", "src/main.rs", "Cargo.toml"],
            late: Some("src/lib.rs"),
        }))
    }

    fn press(modal: &mut InputModal, code: KeyCode) -> Option<ModalResult<String>> {
        let key = crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE);
        modal
            .handle_key(termide_core::KeyChord::identity(key))
            .unwrap()
    }

    fn type_text(modal: &mut InputModal, text: &str) {
        for ch in text.chars() {
            press(modal, KeyCode::Char(ch));
        }
    }

    fn confirmed(result: Option<ModalResult<String>>) -> Option<String> {
        match result {
            Some(ModalResult::Confirmed(value)) => Some(value),
            _ => None,
        }
    }

    #[test]
    fn suggestions_follow_the_text_and_enter_takes_the_selected_one() {
        let mut modal = words_modal();
        type_text(&mut modal, "src");
        press(&mut modal, KeyCode::Down);
        assert_eq!(
            confirmed(press(&mut modal, KeyCode::Enter)).as_deref(),
            Some("src/main.rs")
        );
    }

    #[test]
    fn with_nothing_suggested_enter_submits_the_typed_text() {
        let mut modal = words_modal();
        type_text(&mut modal, "https://example.com");
        assert_eq!(
            confirmed(press(&mut modal, KeyCode::Enter)).as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn tab_puts_the_selection_in_the_field_and_asks_again() {
        let mut modal = words_modal();
        type_text(&mut modal, "Ca");
        press(&mut modal, KeyCode::Tab);
        assert_eq!(modal.input_handler.text(), "Cargo.toml");
        assert_eq!(modal.focus, FocusArea::Input, "Tab completed, focus stayed");

        let mut empty = words_modal();
        press(&mut empty, KeyCode::Tab);
        assert_eq!(empty.focus, FocusArea::Buttons, "no list: Tab moves focus");
    }

    #[test]
    fn poll_takes_in_background_results() {
        let mut modal = words_modal();
        type_text(&mut modal, "src/l");
        assert!(modal.selected_suggestion().is_none());
        assert!(modal.poll_suggestions());
        assert_eq!(modal.selected_suggestion().as_deref(), Some("src/lib.rs"));
        assert!(!modal.poll_suggestions(), "nothing new");
    }

    #[test]
    fn the_modal_keeps_its_height_while_the_list_changes() {
        let mut modal = words_modal();
        let empty = modal.calculate_modal_size(120, 40);
        type_text(&mut modal, "s");
        assert_eq!(modal.calculate_modal_size(120, 40), empty);
        assert_eq!(
            empty.1,
            InputModal::new("Open", "").calculate_modal_size(120, 40).1 + SUGGESTION_ROWS as u16
        );
    }
}
