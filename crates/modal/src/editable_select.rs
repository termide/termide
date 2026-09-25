//! Editable select (combobox) modal dialog.
//!
//! Provides a dropdown-style modal with an editable text input field.
//! Users can type to filter options or select from the dropdown list.
//!
//! Features:
//! - Text input with cursor navigation
//! - Dropdown list with keyboard navigation
//! - Real-time filtering of options
//! - Tab completion support

use anyhow::Result;
use crossterm::event::KeyCode;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Widget},
};

use crate::base::{button_style, render_input_field, render_modal_block};
use crate::input_keys::{handle_input_key, InputKeyResult};

use termide_config::constants::{
    MODAL_BUTTON_SPACING, MODAL_MAX_WIDTH_PERCENTAGE_DEFAULT, MODAL_MIN_WIDTH_WIDE,
    MODAL_PADDING_WITH_DOUBLE_BORDER,
};
use termide_i18n as i18n;
use termide_theme::Theme;
use termide_ui::path_utils::truncate_right;
use termide_ui::{SuggestionAction, SuggestionInput};

use crate::{centered_rect_with_size, Modal, ModalResult};

/// Select option for editable select modal
#[derive(Debug, Clone)]
pub struct SelectOption {
    /// Value of the option (used for selection result)
    pub value: String,
    /// Display text for the option
    pub display: String,
}

/// Focus area in the modal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Input,
    Checkbox(usize),
    Buttons,
}

#[derive(Debug, Clone)]
struct ModalCheckbox {
    label: String,
    checked: bool,
    visible_when_primary_checked: bool,
}

/// Editable select modal - combobox with editable input
#[derive(Debug)]
pub struct EditableSelectModal {
    title: String,
    prompt: String,
    suggestion_input: SuggestionInput,
    options: Vec<SelectOption>, // Keep for display text mapping
    focus: FocusArea,
    selected_button: usize, // 0 = OK, 1 = Cancel
    // Areas for mouse handling
    last_modal_area: Option<Rect>,
    last_input_area: Option<Rect>,
    last_dropdown_area: Option<Rect>,
    last_buttons_area: Option<Rect>,
    checkboxes: Vec<ModalCheckbox>,
    last_checkbox_areas: Vec<(usize, Rect)>,
}

impl EditableSelectModal {
    /// Create a new editable select modal
    pub fn new(
        title: impl Into<String>,
        prompt: impl Into<String>,
        default_value: impl Into<String>,
        options: Vec<SelectOption>,
    ) -> Self {
        let default = default_value.into();
        // Extract values from options for SuggestionInput
        let suggestion_values: Vec<String> = options.iter().map(|o| o.value.clone()).collect();

        Self {
            title: title.into(),
            prompt: prompt.into(),
            suggestion_input: SuggestionInput::with_text(default, suggestion_values),
            options,
            focus: FocusArea::Input,
            selected_button: 0, // OK button selected by default
            last_modal_area: None,
            last_input_area: None,
            last_dropdown_area: None,
            last_buttons_area: None,
            checkboxes: Vec::new(),
            last_checkbox_areas: Vec::new(),
        }
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

    /// Calculate dynamic modal width and height
    fn calculate_modal_size(&self, screen_width: u16, screen_height: u16) -> (u16, u16) {
        // 1. Title width
        let title_width = self.title.len() as u16 + 2;

        // 2. Prompt width
        let prompt_max_line_width = if self.prompt.is_empty() {
            0
        } else {
            self.prompt
                .lines()
                .map(|line| line.len())
                .max()
                .unwrap_or(0) as u16
        };

        // 3. Calculate max option width ALWAYS (stable width regardless of state)
        let max_option_len = self
            .options
            .iter()
            .map(|s| s.value.chars().count().max(s.display.chars().count()))
            .max()
            .unwrap_or(0) as u16;

        // 4. Input field width based on max option (not current input)
        let min_input_width = max_option_len + 5; // +5 for arrow and padding

        // 5. Options list width, with one column of padding (the row under
        // the cursor is inverted, so it needs no `▶` marker)
        let max_option_width = max_option_len + 1;

        // 6. Buttons width: "[ OK ]    [ Cancel ]" = ~21 characters
        let buttons_width = 21;

        // Take maximum
        let content_width = title_width
            .max(prompt_max_line_width)
            .max(min_input_width)
            .max(max_option_width)
            .max(buttons_width);

        // Add padding and borders
        let total_width = content_width + MODAL_PADDING_WITH_DOUBLE_BORDER;

        // Apply width constraints
        let max_width = (screen_width as f32 * MODAL_MAX_WIDTH_PERCENTAGE_DEFAULT) as u16;
        let width = total_width
            .max(MODAL_MIN_WIDTH_WIDE)
            .min(max_width)
            .min(screen_width);

        // Calculate height
        let prompt_lines = if self.prompt.is_empty() {
            0
        } else {
            self.prompt.lines().count().max(1) as u16
        };
        let list_height = if self.suggestion_input.is_expanded() && !self.options.is_empty() {
            self.options.len().min(6) as u16 + 1 // Limit to 6 items + bottom border
        } else {
            0
        };

        // 1 (top border) + prompt_lines + 3 (input) + list + checkbox(0 or 1) + 1 (buttons) + 1 (bottom border)
        let checkbox_height = self.visible_checkbox_indices().len() as u16;
        let height =
            (1 + prompt_lines + 3 + list_height + checkbox_height + 1 + 1).min(screen_height);

        (width, height)
    }
}

impl Modal for EditableSelectModal {
    type Result = String;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Calculate dynamic dimensions
        let (modal_width, modal_height) = self.calculate_modal_size(area.width, area.height);

        // Create centered area
        let modal_area = centered_rect_with_size(modal_width, modal_height, area);

        // Save modal area for mouse handling
        self.last_modal_area = Some(modal_area);

        let inner = render_modal_block(modal_area, buf, &self.title, theme);

        // Split into sections
        let prompt_lines = if self.prompt.is_empty() {
            0
        } else {
            self.prompt.lines().count().max(1) as u16
        };

        let has_prompt = prompt_lines > 0;
        let has_list = self.suggestion_input.is_expanded() && !self.options.is_empty();
        let list_height = if has_list {
            self.options.len().min(6) as u16 + 1
        } else {
            0
        };
        let mut constraints = Vec::new();
        if has_prompt {
            constraints.push(Constraint::Length(prompt_lines));
        }
        constraints.push(Constraint::Length(3)); // Input
        if has_list {
            constraints.push(Constraint::Length(list_height));
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

        // Render prompt if not empty
        if prompt_lines > 0 {
            let prompt = Paragraph::new(self.prompt.clone())
                .alignment(Alignment::Left)
                .style(Style::default().fg(theme.fg));
            prompt.render(chunks[chunk_idx], buf);
            chunk_idx += 1;
        }

        // Render input field with arrow at right edge
        let arrow_char = if self.suggestion_input.is_expanded() {
            "▲"
        } else {
            "▼"
        };

        // Choose borders based on state: in Expanded, remove bottom border for visual unity
        let input_borders = if self.suggestion_input.is_expanded() && !self.options.is_empty() {
            Borders::LEFT | Borders::TOP | Borders::RIGHT // No bottom border
        } else {
            Borders::ALL
        };

        let input_block = Block::default()
            .borders(input_borders)
            .border_style(Style::default().fg(theme.accented_fg));
        let input_inner = input_block.inner(chunks[chunk_idx]);
        input_block.render(chunks[chunk_idx], buf);

        // Save input area for mouse handling (the full bordered area)
        self.last_input_area = Some(chunks[chunk_idx]);

        // Calculate area for text input (excluding arrow)
        let arrow_width = 2u16; // space + arrow
        let text_width = input_inner.width.saturating_sub(arrow_width);

        // Render input content with cursor and selection
        let input = self.suggestion_input.input();
        render_input_field(
            buf,
            input_inner.x,
            input_inner.y,
            text_width,
            input.text(),
            input.cursor_pos(),
            input.selection_range(),
            self.focus == FocusArea::Input,
            theme,
        );

        // Render arrow at right edge
        let arrow_x = input_inner.x + input_inner.width.saturating_sub(1);
        buf.set_string(
            arrow_x,
            input_inner.y,
            arrow_char,
            Style::default().fg(theme.disabled),
        );

        chunk_idx += 1;

        // Render options list only in Expanded state
        if self.suggestion_input.is_expanded() && !self.options.is_empty() {
            // Save dropdown area for mouse handling
            self.last_dropdown_area = Some(chunks[chunk_idx]);

            let selected_idx = self.suggestion_input.selected_index();
            let viewport = self.suggestion_input.viewport();
            let scroll_offset = viewport.offset;
            let max_visible = self.suggestion_input.max_visible();

            // Only render items within the visible viewport range
            let visible_end = (scroll_offset + max_visible).min(self.options.len());
            let items: Vec<ListItem> = self
                .options
                .iter()
                .enumerate()
                .skip(scroll_offset)
                .take(visible_end - scroll_offset)
                .map(|(idx, option)| {
                    let prefix = " ";

                    let style = if idx == selected_idx {
                        Style::default()
                            .fg(theme.bg)
                            .bg(theme.fg)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.fg)
                    };

                    // Truncate long text
                    let max_text_width = modal_width as usize - 10;
                    let display_text = truncate_right(&option.display, max_text_width);

                    ListItem::new(Line::from(vec![
                        Span::styled(prefix, style),
                        Span::styled(display_text, style),
                    ]))
                })
                .collect();

            // Remove top border and title for visual unity with input field
            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::LEFT | Borders::BOTTOM | Borders::RIGHT) // No top border
                        .border_style(Style::default().fg(theme.accented_fg)),
                )
                .style(Style::default().bg(theme.bg));

            list.render(chunks[chunk_idx], buf);
            chunk_idx += 1;
        } else {
            self.last_dropdown_area = None;
        }

        // Render checkbox if present
        self.last_checkbox_areas.clear();
        for checkbox_idx in visible_checkboxes {
            let checkbox = &self.checkboxes[checkbox_idx];
            let checkbox_char = if checkbox.checked { "x" } else { " " };
            let checkbox_style = if self.focus == FocusArea::Checkbox(checkbox_idx) {
                Style::default().fg(theme.accented_fg).bg(theme.bg)
            } else {
                Style::default().fg(theme.fg).bg(theme.bg)
            };
            let checkbox_text = format!("[{}] {}", checkbox_char, checkbox.label);
            let checkbox = Paragraph::new(checkbox_text)
                .style(checkbox_style)
                .alignment(Alignment::Left);
            checkbox.render(chunks[chunk_idx], buf);
            self.last_checkbox_areas
                .push((checkbox_idx, chunks[chunk_idx]));
            chunk_idx += 1;
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
        let key = chord.raw;
        // Escape handling depends on state
        if key.code == KeyCode::Esc {
            if self.suggestion_input.is_expanded() {
                // Collapse and rollback changes
                self.suggestion_input.rollback();
                self.focus = FocusArea::Input;
                return Ok(None);
            } else {
                // Cancel operation
                return Ok(Some(ModalResult::Cancelled));
            }
        }

        match self.focus {
            FocusArea::Input => {
                // First try suggestion input handling (Tab, Up/Down when expanded, Enter when expanded)
                match self.suggestion_input.handle_key(key) {
                    SuggestionAction::Handled => return Ok(None),
                    SuggestionAction::Confirmed => return Ok(None), // Just collapsed, don't confirm modal
                    SuggestionAction::Cancelled => return Ok(None), // Already handled by Esc above
                    SuggestionAction::TextModified => return Ok(None),
                    SuggestionAction::NotHandled => {}
                }

                // Try common input handling
                match handle_input_key(self.suggestion_input.input_mut(), key) {
                    InputKeyResult::Handled | InputKeyResult::TextModified => {
                        return Ok(None);
                    }
                    InputKeyResult::NotHandled => {}
                }

                // Modal-specific handling
                match key.code {
                    KeyCode::Down => {
                        // Move focus down (only when collapsed)
                        if let Some(idx) = self.first_visible_checkbox() {
                            self.focus = FocusArea::Checkbox(idx);
                        } else {
                            self.focus = FocusArea::Buttons;
                        }
                        Ok(None)
                    }
                    KeyCode::Tab => {
                        if let Some(idx) = self.first_visible_checkbox() {
                            self.focus = FocusArea::Checkbox(idx);
                        } else {
                            self.focus = FocusArea::Buttons;
                        }
                        Ok(None)
                    }
                    KeyCode::Enter => {
                        // Confirm current value (only when collapsed)
                        let text = self.suggestion_input.text();
                        if text.is_empty() {
                            Ok(Some(ModalResult::Cancelled))
                        } else {
                            Ok(Some(ModalResult::Confirmed(text.to_string())))
                        }
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
                match handle_input_key(self.suggestion_input.input_mut(), key) {
                    InputKeyResult::Handled | InputKeyResult::TextModified => {
                        self.focus = FocusArea::Input;
                        return Ok(None);
                    }
                    InputKeyResult::NotHandled => {}
                }

                match key.code {
                    KeyCode::Up | KeyCode::BackTab => {
                        self.focus = self
                            .previous_visible_checkbox_before(checkbox_idx)
                            .map(FocusArea::Checkbox)
                            .unwrap_or(FocusArea::Input);
                        Ok(None)
                    }
                    KeyCode::Down | KeyCode::Tab => {
                        self.focus = self
                            .next_visible_checkbox_after(checkbox_idx)
                            .map(FocusArea::Checkbox)
                            .unwrap_or(FocusArea::Buttons);
                        Ok(None)
                    }
                    KeyCode::Enter => {
                        let text = self.suggestion_input.text();
                        if text.is_empty() {
                            Ok(Some(ModalResult::Cancelled))
                        } else {
                            Ok(Some(ModalResult::Confirmed(text.to_string())))
                        }
                    }
                    _ => Ok(None),
                }
            }
            FocusArea::Buttons => {
                // Handle text input keys even when on buttons
                match handle_input_key(self.suggestion_input.input_mut(), key) {
                    InputKeyResult::Handled | InputKeyResult::TextModified => {
                        // Switch back to input when typing
                        self.focus = FocusArea::Input;
                        return Ok(None);
                    }
                    InputKeyResult::NotHandled => {}
                }

                match key.code {
                    KeyCode::Left => {
                        // Move to previous button (wrap around)
                        self.selected_button = if self.selected_button == 0 { 1 } else { 0 };
                        Ok(None)
                    }
                    KeyCode::Right => {
                        // Move to next button (wrap around)
                        self.selected_button = if self.selected_button == 1 { 0 } else { 1 };
                        Ok(None)
                    }
                    KeyCode::Up | KeyCode::BackTab => {
                        if let Some(idx) = self.visible_checkbox_indices().into_iter().last() {
                            self.focus = FocusArea::Checkbox(idx);
                        } else {
                            self.focus = FocusArea::Input;
                        }
                        Ok(None)
                    }
                    KeyCode::Enter => {
                        // Execute selected button action
                        if self.selected_button == 0 {
                            // OK button
                            let text = self.suggestion_input.text();
                            if text.is_empty() {
                                Ok(Some(ModalResult::Cancelled))
                            } else {
                                Ok(Some(ModalResult::Confirmed(text.to_string())))
                            }
                        } else {
                            // Cancel button
                            Ok(Some(ModalResult::Cancelled))
                        }
                    }
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
        use crossterm::event::MouseEventKind;

        if self.suggestion_input.is_expanded() {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    for _ in 0..3 {
                        self.suggestion_input.select_up();
                    }
                    return Ok(None);
                }
                MouseEventKind::ScrollDown => {
                    for _ in 0..3 {
                        self.suggestion_input.select_down();
                    }
                    return Ok(None);
                }
                _ => {}
            }
        }

        // Only handle left button press
        if mouse.kind != MouseEventKind::Down(crossterm::event::MouseButton::Left) {
            return Ok(None);
        }

        // Check for click on dropdown arrow
        if let Some(input_area) = self.last_input_area {
            // Check if click is within input area
            if mouse.row >= input_area.y
                && mouse.row < input_area.y + input_area.height
                && mouse.column >= input_area.x
                && mouse.column < input_area.x + input_area.width
            {
                // Arrow is at the right edge (last 3 columns: padding + arrow + border)
                // Check if click is in the arrow region (rightmost 3-4 characters of input area)
                let arrow_start = input_area.x + input_area.width.saturating_sub(3);
                if mouse.column >= arrow_start {
                    // Toggle dropdown state
                    self.suggestion_input.toggle();
                    return Ok(None);
                }
            }
        }

        // Check for click on dropdown items
        if let Some(dropdown_area) = self.last_dropdown_area {
            if mouse.row >= dropdown_area.y
                && mouse.row < dropdown_area.y + dropdown_area.height
                && mouse.column >= dropdown_area.x
                && mouse.column < dropdown_area.x + dropdown_area.width
            {
                // Calculate which item was clicked (account for border and scroll offset)
                let relative_row = mouse.row.saturating_sub(dropdown_area.y);
                // First row is inside the list (no top border in this design)
                // Add scroll offset to get the actual item index
                let scroll_offset = self.suggestion_input.viewport().offset;
                let item_index = scroll_offset + relative_row as usize;

                if item_index < self.options.len() {
                    self.suggestion_input.select_and_confirm(item_index);
                }
                return Ok(None);
            }
        }

        // Check for click on checkbox
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

        // Check for click on buttons
        if let Some(buttons_area) = self.last_buttons_area {
            // Check if click is within buttons area
            if mouse.row >= buttons_area.y
                && mouse.row < buttons_area.y + buttons_area.height
                && mouse.column >= buttons_area.x
                && mouse.column < buttons_area.x + buttons_area.width
            {
                // Calculate button positions
                // Buttons are centered: "[ OK ]    [ Cancel ]"
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

                // Determine which button was clicked
                if mouse.column >= start_col && mouse.column < ok_end {
                    // OK button clicked
                    self.focus = FocusArea::Buttons;
                    self.selected_button = 0;
                    // Execute OK action immediately
                    let text = self.suggestion_input.text();
                    if text.is_empty() {
                        return Ok(Some(ModalResult::Cancelled));
                    } else {
                        return Ok(Some(ModalResult::Confirmed(text.to_string())));
                    }
                } else if mouse.column >= cancel_start && mouse.column < cancel_end {
                    // Cancel button clicked
                    self.focus = FocusArea::Buttons;
                    self.selected_button = 1;
                    // Execute Cancel action immediately
                    return Ok(Some(ModalResult::Cancelled));
                }
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn secondary_checkbox_state_is_reported() {
        let mut modal = EditableSelectModal::new("Copy", "Prompt", "dest", vec![])
            .with_checkbox("Create symlink".to_string())
            .with_conditional_checkbox("Use relative target".to_string());

        modal.checkboxes[0].checked = true;
        modal.checkboxes[1].checked = true;
        assert!(modal.is_checkbox_checked());
        assert!(modal.is_secondary_checkbox_checked());
    }
}
