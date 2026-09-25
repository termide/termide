//! Modal dialog for adding bookmarks.
//!
//! Provides a modal with path, description, and group fields.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
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
use termide_ui::{SuggestionAction, SuggestionInput};

use crate::{centered_rect_with_size, Modal, ModalResult, TextInputHandler};

/// Result of bookmark add operation
#[derive(Debug, Clone)]
pub struct BookmarkAddResult {
    /// Path to bookmark
    pub path: String,
    /// Optional description
    pub description: Option<String>,
    /// Optional group
    pub group: Option<String>,
    /// Whether to save as project-local bookmark (.termide/)
    pub is_project: bool,
}

/// Focus area in the modal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Group,
    Description,
    Path,
    ProjectCheckbox,
    Buttons,
}

/// Bookmark add modal
#[derive(Debug)]
pub struct BookmarkAddModal {
    path_input: TextInputHandler,
    description_input: TextInputHandler,
    group_suggestion: SuggestionInput,
    project_checked: bool,
    focus: FocusArea,
    selected_button: usize, // 0 = Add, 1 = Cancel
    last_buttons_area: Option<Rect>,
    last_group_field_area: Option<Rect>,
    last_group_dropdown_area: Option<Rect>,
    last_checkbox_area: Option<Rect>,
}

impl BookmarkAddModal {
    /// Create a new bookmark add modal
    pub fn new(initial_path: Option<String>, existing_groups: Vec<String>) -> Self {
        let path = initial_path.unwrap_or_default();

        Self {
            path_input: TextInputHandler::with_default(path),
            description_input: TextInputHandler::new(),
            group_suggestion: SuggestionInput::new(existing_groups),
            project_checked: false,
            focus: FocusArea::Group,
            selected_button: 0,
            last_buttons_area: None,
            last_group_field_area: None,
            last_group_dropdown_area: None,
            last_checkbox_area: None,
        }
    }

    /// Pre-fill all fields for editing an existing bookmark
    pub fn with_values(
        mut self,
        path: &str,
        description: Option<&str>,
        group: Option<&str>,
        is_project: bool,
    ) -> Self {
        self.path_input = TextInputHandler::with_default(path.to_string());
        if let Some(desc) = description {
            self.description_input = TextInputHandler::with_default(desc.to_string());
        }
        if let Some(grp) = group {
            self.group_suggestion.set_text(grp);
        }
        self.project_checked = is_project;
        self
    }

    /// Calculate dynamic modal dimensions
    fn calculate_modal_size(&self, screen_width: u16, screen_height: u16) -> (u16, u16) {
        let t = i18n::t();

        // Calculate width based on content
        let title_width = t.bookmarks_add_title().len() as u16 + 4;
        let path_label_width = t.bookmarks_add_path().len() as u16 + 2;
        let desc_label_width = t.bookmarks_add_description().len() as u16 + 2;
        let group_label_width = t.bookmarks_add_group().len() as u16 + 2;

        let max_label_width = path_label_width
            .max(desc_label_width)
            .max(group_label_width);
        let input_width = 40u16; // Minimum input width

        let content_width = title_width.max(max_label_width + input_width).max(30);
        let total_width = content_width + MODAL_PADDING_WITH_DOUBLE_BORDER;

        let max_width = (screen_width as f32 * MODAL_MAX_WIDTH_PERCENTAGE_DEFAULT) as u16;
        let width = total_width
            .max(MODAL_MIN_WIDTH_WIDE)
            .min(max_width)
            .min(screen_width);

        // Calculate height
        // Border(1) + Group(3) + [Dropdown] + Description(3) + Path(3) + Checkbox(2) + Buttons(1) + Border(1)
        let suggestions = self.group_suggestion.suggestions();
        let dropdown_height = if self.group_suggestion.is_expanded() && !suggestions.is_empty() {
            suggestions.len().min(5) as u16 + 1
        } else {
            0
        };
        let height = (1 + 3 + dropdown_height + 3 + 3 + 2 + 1 + 1).min(screen_height);

        (width, height)
    }

    /// Get currently focused input handler (only for Path and Description)
    fn current_input(&mut self) -> Option<&mut TextInputHandler> {
        match self.focus {
            FocusArea::Path => Some(&mut self.path_input),
            FocusArea::Description => Some(&mut self.description_input),
            FocusArea::Group | FocusArea::ProjectCheckbox | FocusArea::Buttons => None,
        }
    }

    /// Move to next focus area
    fn next_focus(&mut self) {
        self.focus = match self.focus {
            FocusArea::Group => FocusArea::Description,
            FocusArea::Description => FocusArea::Path,
            FocusArea::Path => FocusArea::ProjectCheckbox,
            FocusArea::ProjectCheckbox => FocusArea::Buttons,
            FocusArea::Buttons => FocusArea::Group,
        };
        if self.focus != FocusArea::Group {
            self.group_suggestion.collapse();
        }
    }

    /// Move to previous focus area
    fn prev_focus(&mut self) {
        self.focus = match self.focus {
            FocusArea::Group => FocusArea::Buttons,
            FocusArea::Description => FocusArea::Group,
            FocusArea::Path => FocusArea::Description,
            FocusArea::ProjectCheckbox => FocusArea::Path,
            FocusArea::Buttons => FocusArea::ProjectCheckbox,
        };
        if self.focus != FocusArea::Group {
            self.group_suggestion.collapse();
        }
    }

    /// Render a labeled input field
    fn render_labeled_input_field(
        &self,
        buf: &mut Buffer,
        area: Rect,
        label: &str,
        input: &TextInputHandler,
        is_focused: bool,
        theme: &Theme,
    ) {
        // Split into label and input
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(15), Constraint::Min(1)])
            .split(area);

        // Render label (same style as InputModal's prompt)
        // Vertically center label in the 3-row height area
        let label_para = Paragraph::new(label.to_string())
            .style(Style::default().fg(theme.fg))
            .alignment(Alignment::Right);
        let label_area = Rect {
            x: chunks[0].x,
            y: chunks[0].y + 1, // Middle row of 3-row height
            width: chunks[0].width,
            height: 1,
        };
        label_para.render(label_area, buf);

        // Render input with border
        let border_style = if is_focused {
            Style::default().fg(theme.accented_fg)
        } else {
            Style::default().fg(theme.disabled)
        };

        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style);
        let input_inner = input_block.inner(chunks[1]);
        input_block.render(chunks[1], buf);

        // Render input content with cursor and selection
        render_input_field(
            buf,
            input_inner.x,
            input_inner.y,
            input_inner.width,
            input.text(),
            input.cursor_pos(),
            input.selection_range(),
            is_focused,
            theme,
        );
    }

    /// Render the group input field with dropdown toggle indicator
    fn render_group_field(&self, buf: &mut Buffer, area: Rect, label: &str, theme: &Theme) {
        let is_focused = self.focus == FocusArea::Group;
        let has_groups = !self.group_suggestion.suggestions().is_empty();

        // Split into label and input
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(15), Constraint::Min(1)])
            .split(area);

        // Render label
        let label_para = Paragraph::new(label.to_string())
            .style(Style::default().fg(theme.fg))
            .alignment(Alignment::Right);
        let label_area = Rect {
            x: chunks[0].x,
            y: chunks[0].y + 1,
            width: chunks[0].width,
            height: 1,
        };
        label_para.render(label_area, buf);

        // Calculate input area
        let input_area = chunks[1];
        let indicator = if has_groups {
            if self.group_suggestion.is_expanded() {
                "▲"
            } else {
                "▼"
            }
        } else {
            ""
        };

        let border_style = if is_focused {
            Style::default().fg(theme.accented_fg)
        } else {
            Style::default().fg(theme.disabled)
        };

        // Use different borders based on dropdown state
        let borders = if self.group_suggestion.is_expanded() && has_groups {
            Borders::LEFT | Borders::TOP | Borders::RIGHT // No bottom border when dropdown shown
        } else {
            Borders::ALL
        };

        let input_block = Block::default().borders(borders).border_style(border_style);
        let input_inner = input_block.inner(input_area);
        input_block.render(input_area, buf);

        // Calculate area for text input (excluding indicator)
        let indicator_width = if has_groups { 2u16 } else { 0u16 }; // " ▲" or " ▼"
        let text_width = input_inner.width.saturating_sub(indicator_width);

        // Render input content with cursor and selection
        let input = self.group_suggestion.input();
        render_input_field(
            buf,
            input_inner.x,
            input_inner.y,
            text_width,
            input.text(),
            input.cursor_pos(),
            input.selection_range(),
            is_focused,
            theme,
        );

        // Render indicator at right edge
        if has_groups {
            let indicator_x = input_inner.x + input_inner.width.saturating_sub(1);
            buf.set_string(
                indicator_x,
                input_inner.y,
                indicator,
                Style::default().fg(theme.disabled),
            );
        }
    }
}

impl Modal for BookmarkAddModal {
    type Result = BookmarkAddResult;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let t = i18n::t();

        // Calculate dimensions
        let (modal_width, modal_height) = self.calculate_modal_size(area.width, area.height);
        let modal_area = centered_rect_with_size(modal_width, modal_height, area);

        let inner = render_modal_block(modal_area, buf, t.bookmarks_add_title(), theme);

        // Calculate layout
        let suggestions = self.group_suggestion.suggestions();
        let dropdown_height = if self.group_suggestion.is_expanded() && !suggestions.is_empty() {
            suggestions.len().min(5) as u16 + 1
        } else {
            0
        };

        let mut constraints = vec![
            Constraint::Length(3), // Group
        ];
        if dropdown_height > 0 {
            constraints.push(Constraint::Length(dropdown_height));
        }
        constraints.extend([
            Constraint::Length(3), // Description
            Constraint::Length(3), // Path
            Constraint::Length(1), // Checkbox
            Constraint::Length(1), // Empty line
            Constraint::Length(1), // Buttons
        ]);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        let mut chunk_idx = 0;

        // Render Group field with dropdown indicator
        self.render_group_field(buf, chunks[chunk_idx], t.bookmarks_add_group(), theme);
        let group_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(15), Constraint::Min(1)])
            .split(chunks[chunk_idx]);
        self.last_group_field_area = Some(group_chunks[1]);
        chunk_idx += 1;

        // Render group dropdown if visible
        if dropdown_height > 0 {
            let dropdown_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(15), Constraint::Min(1)])
                .split(chunks[chunk_idx]);

            self.last_group_dropdown_area = Some(dropdown_chunks[1]);

            let selected_idx = self.group_suggestion.selected_index();
            let items: Vec<ListItem> = suggestions
                .iter()
                .enumerate()
                .map(|(idx, group)| {
                    // The row under the cursor is highlighted, so it needs no
                    // `▶` marker beside it; one column of padding stays.
                    let (prefix, style) = if idx == selected_idx {
                        (
                            " ",
                            Style::default()
                                .fg(theme.selected_fg)
                                .bg(theme.selected_bg)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else {
                        (" ", Style::default().fg(theme.fg))
                    };
                    ListItem::new(Line::from(Span::styled(
                        format!("{}{}", prefix, group),
                        style,
                    )))
                })
                .collect();

            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::LEFT | Borders::BOTTOM | Borders::RIGHT)
                        .border_style(Style::default().fg(theme.accented_fg)),
                )
                .style(Style::default().bg(theme.bg));
            list.render(dropdown_chunks[1], buf);
            chunk_idx += 1;
        } else {
            self.last_group_dropdown_area = None;
        }

        // Render Description field
        self.render_labeled_input_field(
            buf,
            chunks[chunk_idx],
            t.bookmarks_add_description(),
            &self.description_input,
            self.focus == FocusArea::Description,
            theme,
        );
        chunk_idx += 1;

        // Render Path field
        self.render_labeled_input_field(
            buf,
            chunks[chunk_idx],
            t.bookmarks_add_path(),
            &self.path_input,
            self.focus == FocusArea::Path,
            theme,
        );
        chunk_idx += 1;

        // Render project checkbox (aligned with input fields)
        {
            let cb_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(15), Constraint::Min(1)])
                .split(chunks[chunk_idx]);

            let checkbox_char = termide_ui::checkbox_mark(self.project_checked);
            let checkbox_style = if self.focus == FocusArea::ProjectCheckbox {
                Style::default().fg(theme.accented_fg)
            } else {
                Style::default().fg(theme.fg)
            };
            let checkbox_text = format!(" [{}] {}", checkbox_char, t.bookmarks_add_project());
            let checkbox = Paragraph::new(checkbox_text).style(checkbox_style);
            checkbox.render(cb_chunks[1], buf);
            self.last_checkbox_area = Some(cb_chunks[1]);
            chunk_idx += 1;
        }

        // Skip empty line
        chunk_idx += 1;

        // Render buttons
        let add_style = button_style(
            self.focus == FocusArea::Buttons && self.selected_button == 0,
            theme,
        );
        let cancel_style = button_style(
            self.focus == FocusArea::Buttons && self.selected_button == 1,
            theme,
        );

        let buttons = Line::from(vec![
            Span::styled(format!("[ {} ]", t.ui_ok()), add_style),
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
        // Escape to cancel
        if key.code == KeyCode::Esc {
            if self.group_suggestion.is_expanded() {
                // Rollback: restore saved input
                self.group_suggestion.rollback();
                return Ok(None);
            }
            return Ok(Some(ModalResult::Cancelled));
        }

        // Tab/Shift+Tab for navigation (or dropdown toggle on Group field)
        if key.code == KeyCode::Tab {
            // When focus is Group and groups exist: Tab toggles dropdown
            if self.focus == FocusArea::Group
                && !self.group_suggestion.suggestions().is_empty()
                && !key.modifiers.contains(KeyModifiers::SHIFT)
            {
                self.group_suggestion.toggle();
                return Ok(None);
            }
            // Otherwise: standard Tab navigation
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                self.prev_focus();
            } else {
                self.next_focus();
            }
            return Ok(None);
        }

        // Handle based on focus
        match self.focus {
            FocusArea::Path | FocusArea::Description => {
                // Try common input handling first
                if let Some(input) = self.current_input() {
                    match handle_input_key(input, key) {
                        InputKeyResult::Handled | InputKeyResult::TextModified => {
                            return Ok(None);
                        }
                        InputKeyResult::NotHandled => {}
                    }
                }

                // Modal-specific handling
                match key.code {
                    KeyCode::Down => {
                        self.next_focus();
                    }
                    KeyCode::Up => {
                        self.prev_focus();
                    }
                    KeyCode::Enter => {
                        // Move to next field
                        self.next_focus();
                    }
                    _ => {}
                }
                Ok(None)
            }
            FocusArea::Group => {
                // First try suggestion input handling (Up/Down when expanded, Enter when expanded)
                match self.group_suggestion.handle_key(key) {
                    SuggestionAction::Handled => return Ok(None),
                    SuggestionAction::Confirmed => return Ok(None), // Just collapsed
                    SuggestionAction::Cancelled => return Ok(None), // Already handled by Esc above
                    SuggestionAction::TextModified => return Ok(None),
                    SuggestionAction::NotHandled => {}
                }

                // Try common input handling
                match handle_input_key(self.group_suggestion.input_mut(), key) {
                    InputKeyResult::Handled | InputKeyResult::TextModified => {
                        return Ok(None);
                    }
                    InputKeyResult::NotHandled => {}
                }

                // Modal-specific handling
                match key.code {
                    KeyCode::Down => {
                        // Down arrow moves to next focus (Buttons)
                        self.next_focus();
                    }
                    KeyCode::Up => {
                        self.prev_focus();
                    }
                    KeyCode::Enter => {
                        self.next_focus();
                    }
                    _ => {}
                }
                Ok(None)
            }
            FocusArea::ProjectCheckbox => {
                match key.code {
                    KeyCode::Char(' ') => {
                        self.project_checked = !self.project_checked;
                    }
                    KeyCode::Down | KeyCode::Enter => {
                        self.next_focus();
                    }
                    KeyCode::Up => {
                        self.prev_focus();
                    }
                    _ => {}
                }
                Ok(None)
            }
            FocusArea::Buttons => {
                // Handle text input keys even when on buttons
                match handle_input_key(&mut self.path_input, key) {
                    InputKeyResult::Handled | InputKeyResult::TextModified => {
                        self.focus = FocusArea::Path;
                        return Ok(None);
                    }
                    InputKeyResult::NotHandled => {}
                }

                match key.code {
                    KeyCode::Left => {
                        self.selected_button = if self.selected_button == 0 { 1 } else { 0 };
                    }
                    KeyCode::Right => {
                        self.selected_button = if self.selected_button == 1 { 0 } else { 1 };
                    }
                    KeyCode::Up | KeyCode::BackTab => {
                        self.prev_focus();
                    }
                    KeyCode::Enter => {
                        if self.selected_button == 0 {
                            // Add button - validate and return result
                            let path = self.path_input.text().to_string();
                            if path.is_empty() {
                                // Can't add empty path
                                return Ok(None);
                            }

                            let description = {
                                let d = self.description_input.text();
                                if d.is_empty() {
                                    None
                                } else {
                                    Some(d.to_string())
                                }
                            };

                            let group = {
                                let g = self.group_suggestion.text();
                                if g.is_empty() {
                                    None
                                } else {
                                    Some(g.to_string())
                                }
                            };

                            return Ok(Some(ModalResult::Confirmed(BookmarkAddResult {
                                path,
                                description,
                                group,
                                is_project: self.project_checked,
                            })));
                        } else {
                            // Cancel button
                            return Ok(Some(ModalResult::Cancelled));
                        }
                    }
                    _ => {}
                }
                Ok(None)
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

        // Check if click is on group field (toggle dropdown)
        if let Some(group_area) = self.last_group_field_area {
            if mouse.row >= group_area.y
                && mouse.row < group_area.y + group_area.height
                && mouse.column >= group_area.x
                && mouse.column < group_area.x + group_area.width
            {
                // Click on group field - toggle dropdown if groups exist
                self.focus = FocusArea::Group;
                if !self.group_suggestion.suggestions().is_empty() {
                    self.group_suggestion.toggle();
                }
                return Ok(None);
            }
        }

        // Check if click is on group dropdown items
        if let Some(dropdown_area) = self.last_group_dropdown_area {
            if mouse.row >= dropdown_area.y
                && mouse.row < dropdown_area.y + dropdown_area.height
                && mouse.column >= dropdown_area.x
                && mouse.column < dropdown_area.x + dropdown_area.width
            {
                // Calculate which item was clicked (account for border)
                let relative_row = mouse.row.saturating_sub(dropdown_area.y);
                let item_index = relative_row as usize;

                let suggestions_len = self.group_suggestion.suggestions().len();
                if item_index < suggestions_len {
                    self.group_suggestion.select_and_confirm(item_index);
                }
                return Ok(None);
            }
        }

        // Check if click is on project checkbox
        if let Some(checkbox_area) = self.last_checkbox_area {
            if mouse.row >= checkbox_area.y
                && mouse.row < checkbox_area.y + checkbox_area.height
                && mouse.column >= checkbox_area.x
                && mouse.column < checkbox_area.x + checkbox_area.width
            {
                self.focus = FocusArea::ProjectCheckbox;
                self.project_checked = !self.project_checked;
                return Ok(None);
            }
        }

        // Check if we have stored buttons area
        let Some(buttons_area) = self.last_buttons_area else {
            return Ok(None);
        };

        // Check if click is within buttons area
        if mouse.row < buttons_area.y
            || mouse.row >= buttons_area.y + buttons_area.height
            || mouse.column < buttons_area.x
            || mouse.column >= buttons_area.x + buttons_area.width
        {
            return Ok(None);
        }

        // Calculate button positions (same logic as InputModal)
        let t = i18n::t();
        let ok_text = format!("[ {} ]", t.ui_ok());
        let cancel_text = format!("[ {} ]", t.ui_cancel());
        let total_text_width = ok_text.len() + MODAL_BUTTON_SPACING as usize + cancel_text.len();

        let start_col =
            buttons_area.x + (buttons_area.width.saturating_sub(total_text_width as u16)) / 2;
        let ok_end = start_col + ok_text.len() as u16;
        let cancel_start = ok_end + MODAL_BUTTON_SPACING;
        let cancel_end = cancel_start + cancel_text.len() as u16;

        // Determine which button was clicked
        if mouse.column >= start_col && mouse.column < ok_end {
            // OK button clicked
            self.focus = FocusArea::Buttons;
            self.selected_button = 0;
            // Execute OK action immediately
            let path = self.path_input.text().to_string();
            if path.is_empty() {
                return Ok(None);
            }

            let description = {
                let d = self.description_input.text();
                if d.is_empty() {
                    None
                } else {
                    Some(d.to_string())
                }
            };

            let group = {
                let g = self.group_suggestion.text();
                if g.is_empty() {
                    None
                } else {
                    Some(g.to_string())
                }
            };

            Ok(Some(ModalResult::Confirmed(BookmarkAddResult {
                path,
                description,
                group,
                is_project: self.project_checked,
            })))
        } else if mouse.column >= cancel_start && mouse.column < cancel_end {
            // Cancel button clicked
            self.focus = FocusArea::Buttons;
            self.selected_button = 1;
            Ok(Some(ModalResult::Cancelled))
        } else {
            Ok(None)
        }
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        match self.focus {
            FocusArea::Path => {
                self.path_input.paste(text);
                true
            }
            FocusArea::Description => {
                self.description_input.paste(text);
                true
            }
            FocusArea::Group => {
                self.group_suggestion.input_mut().paste(text);
                true
            }
            FocusArea::ProjectCheckbox | FocusArea::Buttons => false,
        }
    }
}
