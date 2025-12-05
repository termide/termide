use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Widget},
};

use super::{Modal, ModalResult, TextInputHandler};
use crate::constants::{
    MODAL_BUTTON_SPACING, MODAL_MAX_WIDTH_PERCENTAGE_DEFAULT, MODAL_MIN_WIDTH_WIDE,
    MODAL_PADDING_WITH_DOUBLE_BORDER,
};
use crate::theme::Theme;
use crate::ui::centered_rect_with_size;

/// Select option for editable select modal
#[derive(Debug, Clone)]
pub struct SelectOption {
    /// Panel index or any other identifier
    #[allow(dead_code)]
    pub panel_index: usize,
    /// Value of the option (used for selection result)
    pub value: String,
    /// Display text for the option
    pub display: String,
}

/// Dropdown state for the editable select
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropdownState {
    /// Collapsed - only input field with arrow ▼ visible
    Collapsed,
    /// Expanded - input field + options list visible
    Expanded,
}

/// Focus area in the modal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Input,
    Buttons,
}

/// Editable select modal - combobox with editable input
#[derive(Debug)]
pub struct EditableSelectModal {
    title: String,
    prompt: String,
    input_handler: TextInputHandler,
    options: Vec<SelectOption>,
    selected_index: usize,
    state: DropdownState,
    saved_input: String,
    focus: FocusArea,
    selected_button: usize, // 0 = OK, 1 = Cancel
    // Areas for mouse handling
    last_modal_area: Option<Rect>,
    last_input_area: Option<Rect>,
    last_buttons_area: Option<Rect>,
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
        let selected_index = 0;

        Self {
            title: title.into(),
            prompt: prompt.into(),
            input_handler: TextInputHandler::with_default(default.clone()),
            options,
            selected_index,
            state: DropdownState::Collapsed, // Always start collapsed
            saved_input: default,            // Save for rollback
            focus: FocusArea::Input,
            selected_button: 0, // OK button selected by default
            last_modal_area: None,
            last_input_area: None,
            last_buttons_area: None,
        }
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

        // 3. Input field width (reserve space for arrow)
        let current_input_len = self.input_handler.text().chars().count() as u16;
        let min_input_width = current_input_len + 20;

        // 4. Options list width (only in Expanded state)
        let max_option_width = if self.state == DropdownState::Expanded {
            self.options
                .iter()
                .map(|s| {
                    // "▶ " prefix + display
                    2 + s.display.len()
                })
                .max()
                .unwrap_or(0) as u16
        } else {
            0
        };

        // 5. Buttons width: "[ OK ]    [ Cancel ]" = ~21 characters
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
        let list_height = if self.state == DropdownState::Expanded && !self.options.is_empty() {
            self.options.len().min(6) as u16 + 3 // Limit to 6 items + border + label
        } else {
            0
        };

        // 1 (top border) + prompt_lines + 3 (input) + list + 1 (buttons) + 1 (bottom border)
        let height = (1 + prompt_lines + 3 + list_height + 1 + 1).min(screen_height);

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

        // Clear the area
        Clear.render(modal_area, buf);

        // Create block with inverted colors
        let block = Block::default()
            .title(Span::styled(
                format!(" {} ", self.title),
                Style::default().fg(theme.bg).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.bg))
            .style(Style::default().bg(theme.fg));

        let inner = block.inner(modal_area);
        block.render(modal_area, buf);

        // Split into sections
        use ratatui::layout::{Constraint, Direction, Layout};

        let prompt_lines = if self.prompt.is_empty() {
            0
        } else {
            self.prompt.lines().count().max(1) as u16
        };

        let mut constraints = Vec::new();

        // Add prompt only if not empty
        if prompt_lines > 0 {
            constraints.push(Constraint::Length(prompt_lines)); // Prompt
        }

        constraints.push(Constraint::Length(3)); // Input field

        // Add list only in Expanded state
        if self.state == DropdownState::Expanded && !self.options.is_empty() {
            let list_height = self.options.len().min(6) as u16 + 3;
            constraints.push(Constraint::Length(list_height));
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
                .style(Style::default().fg(theme.bg));
            prompt.render(chunks[chunk_idx], buf);
            chunk_idx += 1;
        }

        // Render input field with arrow at right edge
        let input_inner_width = chunks[chunk_idx].width.saturating_sub(4); // 2 border + 2 padding
        let arrow_char = match self.state {
            DropdownState::Collapsed => "▼",
            DropdownState::Expanded => "▲",
        };

        let text_before = self.input_handler.text_before_cursor();
        let text_after = self.input_handler.text_after_cursor();

        // Calculate padding to push arrow to the right
        let text_len = (text_before.chars().count() + 1 + text_after.chars().count()) as u16;
        let padding_len = input_inner_width.saturating_sub(text_len + 1) as usize; // -1 for arrow

        let input_line = Line::from(vec![
            Span::styled(text_before, Style::default().fg(theme.bg)),
            Span::styled("█", Style::default().fg(theme.success)),
            Span::styled(text_after, Style::default().fg(theme.bg)),
            Span::styled(" ".repeat(padding_len), Style::default()),
            Span::styled(arrow_char, Style::default().fg(theme.disabled)),
        ]);

        // Choose borders based on state: in Expanded, remove bottom border for visual unity
        let input_borders = if self.state == DropdownState::Expanded && !self.options.is_empty() {
            Borders::LEFT | Borders::TOP | Borders::RIGHT // No bottom border
        } else {
            Borders::ALL
        };

        let input_paragraph = Paragraph::new(input_line)
            .block(
                Block::default()
                    .borders(input_borders)
                    .border_style(Style::default().fg(theme.success)),
            )
            .style(Style::default().bg(theme.fg));
        input_paragraph.render(chunks[chunk_idx], buf);

        // Save input area for mouse handling
        self.last_input_area = Some(chunks[chunk_idx]);
        chunk_idx += 1;

        // Render options list only in Expanded state
        if self.state == DropdownState::Expanded && !self.options.is_empty() {
            let items: Vec<ListItem> = self
                .options
                .iter()
                .enumerate()
                .map(|(idx, option)| {
                    let prefix = if idx == self.selected_index {
                        "▶ "
                    } else {
                        "  "
                    };

                    let style = if idx == self.selected_index {
                        Style::default()
                            .fg(theme.fg)
                            .bg(theme.accented_fg)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.bg)
                    };

                    // Truncate long text
                    let display_text = if option.display.len() > (modal_width as usize - 10) {
                        format!("{}...", &option.display[..(modal_width as usize - 13)])
                    } else {
                        option.display.clone()
                    };

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
                        .border_style(Style::default().fg(theme.success)),
                )
                .style(Style::default().bg(theme.fg));

            list.render(chunks[chunk_idx], buf);
            chunk_idx += 1;
        }

        // Render buttons
        let t = crate::i18n::t();

        let ok_style = if self.focus == FocusArea::Buttons && self.selected_button == 0 {
            Style::default()
                .fg(theme.fg)
                .bg(theme.accented_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.accented_fg)
        };

        let cancel_style = if self.focus == FocusArea::Buttons && self.selected_button == 1 {
            Style::default()
                .fg(theme.fg)
                .bg(theme.accented_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.accented_fg)
        };

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

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>> {
        // Escape handling depends on state
        if key.code == KeyCode::Esc {
            match self.state {
                DropdownState::Expanded => {
                    // Collapse and rollback changes
                    self.input_handler = TextInputHandler::with_default(self.saved_input.clone());
                    self.state = DropdownState::Collapsed;
                    self.focus = FocusArea::Input; // Return focus to input
                    return Ok(None);
                }
                DropdownState::Collapsed => {
                    // Cancel operation
                    return Ok(Some(ModalResult::Cancelled));
                }
            }
        }

        match self.focus {
            FocusArea::Input => {
                match key.code {
                    KeyCode::Tab => {
                        // Toggle Collapsed <-> Expanded
                        match self.state {
                            DropdownState::Collapsed => {
                                // Save current input for rollback
                                self.saved_input = self.input_handler.text().to_string();
                                self.state = DropdownState::Expanded;
                            }
                            DropdownState::Expanded => {
                                // Collapse
                                self.state = DropdownState::Collapsed;
                            }
                        }
                        Ok(None)
                    }
                    KeyCode::Down => {
                        match self.state {
                            DropdownState::Collapsed => {
                                // Move focus to buttons
                                self.focus = FocusArea::Buttons;
                                Ok(None)
                            }
                            DropdownState::Expanded => {
                                // Navigate in list
                                if !self.options.is_empty()
                                    && self.selected_index < self.options.len().saturating_sub(1)
                                {
                                    self.selected_index += 1;
                                    // Update input with selected value
                                    self.input_handler = TextInputHandler::with_default(
                                        self.options[self.selected_index].value.clone(),
                                    );
                                }
                                Ok(None)
                            }
                        }
                    }
                    KeyCode::Up => {
                        // Only work in Expanded state for list navigation
                        if self.state == DropdownState::Expanded
                            && !self.options.is_empty()
                            && self.selected_index > 0
                        {
                            self.selected_index -= 1;
                            // Update input with selected value
                            self.input_handler = TextInputHandler::with_default(
                                self.options[self.selected_index].value.clone(),
                            );
                        }
                        Ok(None)
                    }
                    KeyCode::Enter => {
                        match self.state {
                            DropdownState::Collapsed => {
                                // Confirm current value
                                if self.input_handler.is_empty() {
                                    Ok(Some(ModalResult::Cancelled))
                                } else {
                                    Ok(Some(ModalResult::Confirmed(
                                        self.input_handler.text().to_string(),
                                    )))
                                }
                            }
                            DropdownState::Expanded => {
                                // Select from list and collapse
                                if !self.options.is_empty() {
                                    self.input_handler = TextInputHandler::with_default(
                                        self.options[self.selected_index].value.clone(),
                                    );
                                    self.state = DropdownState::Collapsed;
                                }
                                Ok(None)
                            }
                        }
                    }
                    KeyCode::Char(c) => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            return Ok(None);
                        }
                        // Allow typing
                        self.input_handler.insert_char(c);
                        Ok(None)
                    }
                    KeyCode::Backspace => {
                        self.input_handler.backspace();
                        Ok(None)
                    }
                    KeyCode::Delete => {
                        self.input_handler.delete();
                        Ok(None)
                    }
                    KeyCode::Left => {
                        self.input_handler.move_left();
                        Ok(None)
                    }
                    KeyCode::Right => {
                        self.input_handler.move_right();
                        Ok(None)
                    }
                    KeyCode::Home => {
                        self.input_handler.move_home();
                        Ok(None)
                    }
                    KeyCode::End => {
                        self.input_handler.move_end();
                        Ok(None)
                    }
                    _ => Ok(None),
                }
            }
            FocusArea::Buttons => {
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
                    KeyCode::Up => {
                        // Move focus back to input
                        self.focus = FocusArea::Input;
                        Ok(None)
                    }
                    KeyCode::Enter => {
                        // Execute selected button action
                        if self.selected_button == 0 {
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
                        }
                    }
                    KeyCode::Char(c) => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            return Ok(None);
                        }
                        // Switch back to input and insert character
                        self.focus = FocusArea::Input;
                        self.input_handler.insert_char(c);
                        Ok(None)
                    }
                    KeyCode::Backspace => {
                        // Switch back to input and delete character
                        self.focus = FocusArea::Input;
                        self.input_handler.backspace();
                        Ok(None)
                    }
                    KeyCode::Delete => {
                        // Switch back to input and delete character
                        self.focus = FocusArea::Input;
                        self.input_handler.delete();
                        Ok(None)
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
                    match self.state {
                        DropdownState::Collapsed => {
                            // Save current input for rollback
                            self.saved_input = self.input_handler.text().to_string();
                            self.state = DropdownState::Expanded;
                        }
                        DropdownState::Expanded => {
                            // Collapse
                            self.state = DropdownState::Collapsed;
                        }
                    }
                    return Ok(None);
                }
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
                let t = crate::i18n::t();
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
                    // Execute Cancel action immediately
                    return Ok(Some(ModalResult::Cancelled));
                }
            }
        }

        Ok(None)
    }
}
