use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Clear, Widget},
};

use super::{Modal, ModalResult, TextInputHandler};
use crate::theme::Theme;

/// Search modal result
#[derive(Debug, Clone)]
pub struct SearchModalResult {
    pub query: String,
    pub action: SearchAction,
}

/// Search action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchAction {
    /// Search and go to first match
    Search,
    /// Navigate to next match
    Next,
    /// Navigate to previous match
    Previous,
    /// Close modal with selection active
    CloseWithSelection,
}

/// Focus area in search modal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Input,
}

/// Interactive search modal with live preview and navigation
#[derive(Debug)]
pub struct SearchModal {
    input_handler: TextInputHandler,
    focus: FocusArea,
    selected_button: usize, // 0 = Previous, 1 = Next
    /// Match count display (e.g. "3 of 12")
    match_info: Option<(usize, usize)>, // (current, total)
    /// Last rendered areas for mouse handling
    last_button_areas: Vec<(Rect, usize)>, // (area, button_idx)
    last_close_button_area: Option<Rect>,
}

impl SearchModal {
    /// Create new search modal
    pub fn new(_prompt: impl Into<String>) -> Self {
        Self {
            input_handler: TextInputHandler::new(),
            focus: FocusArea::Input,
            selected_button: 1, // Next button selected by default
            match_info: None,
            last_button_areas: Vec::new(),
            last_close_button_area: None,
        }
    }

    /// Update match information (current index, total count)
    pub fn set_match_info(&mut self, current: usize, total: usize) {
        self.match_info = Some((current, total));
    }

    /// Set initial input text (e.g., from previous search)
    pub fn set_input(&mut self, text: String) {
        self.input_handler = TextInputHandler::with_default(text);
    }

    /// Calculate modal size
    fn calculate_modal_size(&self, screen_width: u16, screen_height: u16) -> (u16, u16) {
        // Compact modal: input + buttons + match counter
        // "Search: [____input____] [ ◄ Prev ] [ Next ► ] [3 of 12]"

        let min_width = 60; // Minimum width for comfortable use
        let max_width = (screen_width as f32 * 0.6) as u16;
        let width = min_width.min(max_width).min(screen_width);

        // Height: 1 (border) + 1 (prompt+input) + 1 (buttons+counter) + 1 (border)
        let height = 4;

        (width, height.min(screen_height))
    }

    /// Create a rectangle at top center
    fn top_center_rect(width: u16, height: u16, r: Rect) -> Rect {
        let horizontal_margin = r.width.saturating_sub(width) / 2;
        let vertical_margin = 2; // 2 lines from top

        let vertical_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(vertical_margin),
                Constraint::Length(height),
                Constraint::Min(0),
            ])
            .split(r);

        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(horizontal_margin),
                Constraint::Length(width),
                Constraint::Length(horizontal_margin),
            ])
            .split(vertical_layout[1])[1]
    }
}

impl Modal for SearchModal {
    type Result = SearchModalResult;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let (modal_width, modal_height) = self.calculate_modal_size(area.width, area.height);
        let modal_area = Self::top_center_rect(modal_width, modal_height, area);

        // Clear area
        Clear.render(modal_area, buf);

        // Create block with [X] close button on the left
        let title_with_close = " [X] Search ";
        let block = Block::default()
            .title(Span::styled(
                title_with_close,
                Style::default().fg(theme.bg).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.bg))
            .style(Style::default().bg(theme.fg));

        // Calculate close button area (the [X] at the beginning of title)
        let close_x = modal_area.x + 1; // Position after space: " [X]"
        self.last_close_button_area = Some(Rect {
            x: close_x,
            y: modal_area.y,
            width: 3,
            height: 1,
        });

        let inner = block.inner(modal_area);
        block.render(modal_area, buf);

        // Layout: [prompt+input] [buttons+counter]
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Input line
                Constraint::Length(1), // Buttons line
            ])
            .split(inner);

        // === Render input line ===
        let input_area = chunks[0];

        // Render input field (full width, no prompt)
        let input_x = input_area.x;
        let input_width = input_area.width;

        // Input background (only if focused)
        let input_style = if matches!(self.focus, FocusArea::Input) {
            Style::default().fg(theme.fg).bg(theme.bg)
        } else {
            Style::default().fg(theme.bg)
        };

        // Draw input text
        let input_text = self.input_handler.text();
        let visible_input = if input_text.len() as u16 > input_width {
            let start = input_text.len().saturating_sub(input_width as usize);
            &input_text[start..]
        } else {
            input_text
        };

        buf.set_string(input_x, input_area.y, visible_input, input_style);

        // Draw cursor if input is focused
        if matches!(self.focus, FocusArea::Input) {
            let cursor_screen_pos = input_x + (visible_input.len() as u16).min(input_width - 1);
            if cursor_screen_pos < input_x + input_width {
                buf[(cursor_screen_pos, input_area.y)].set_style(
                    Style::default()
                        .bg(theme.bg)
                        .fg(theme.fg)
                        .add_modifier(Modifier::REVERSED),
                );
            }
        }

        // === Render buttons and counter ===
        let buttons_area = chunks[1];

        // Match counter on the right
        let match_text = if let Some((current, total)) = self.match_info {
            if total == 0 {
                "No matches".to_string()
            } else {
                format!("{} of {}", current + 1, total)
            }
        } else {
            String::new()
        };

        let match_text_width = match_text.len() as u16;
        if match_text_width > 0 && buttons_area.width > match_text_width {
            let match_x = buttons_area.x + buttons_area.width - match_text_width;
            buf.set_string(
                match_x,
                buttons_area.y,
                &match_text,
                Style::default().fg(theme.bg),
            );
        }

        // Buttons on the left
        let buttons = vec![("◄ Prev", 0), ("Next ►", 1)];

        let buttons_focused = false; // Buttons are not focusable in search modal
        let mut x_offset = buttons_area.x;
        self.last_button_areas.clear();

        for (label, idx) in buttons {
            let is_selected = buttons_focused && self.selected_button == idx;
            let button_style = if is_selected {
                Style::default()
                    .fg(theme.fg)
                    .bg(theme.bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.bg)
            };

            let button_text = if is_selected {
                format!("[ {} ]", label)
            } else {
                format!("  {}  ", label)
            };

            let button_width = button_text.len() as u16;

            // Save button area for mouse handling
            self.last_button_areas.push((
                Rect {
                    x: x_offset,
                    y: buttons_area.y,
                    width: button_width,
                    height: 1,
                },
                idx,
            ));

            buf.set_string(x_offset, buttons_area.y, &button_text, button_style);
            x_offset += button_width + 2;
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>> {
        match self.focus {
            FocusArea::Input => match (key.code, key.modifiers) {
                // Tab - move to buttons / trigger next
                (KeyCode::Tab, KeyModifiers::NONE) => {
                    // Trigger next search
                    if !self.input_handler.is_empty() {
                        return Ok(Some(ModalResult::Confirmed(SearchModalResult {
                            query: self.input_handler.text().to_string(),
                            action: SearchAction::Next,
                        })));
                    }
                }
                // Shift+Tab - trigger previous
                (KeyCode::BackTab, _) => {
                    if !self.input_handler.is_empty() {
                        return Ok(Some(ModalResult::Confirmed(SearchModalResult {
                            query: self.input_handler.text().to_string(),
                            action: SearchAction::Previous,
                        })));
                    }
                }
                // Enter - close modal with selection
                (KeyCode::Enter, KeyModifiers::NONE) => {
                    if !self.input_handler.is_empty() {
                        return Ok(Some(ModalResult::Confirmed(SearchModalResult {
                            query: self.input_handler.text().to_string(),
                            action: SearchAction::CloseWithSelection,
                        })));
                    }
                }
                // Esc - cancel
                (KeyCode::Esc, KeyModifiers::NONE) => {
                    return Ok(Some(ModalResult::Cancelled));
                }
                // F3 - next match
                (KeyCode::F(3), KeyModifiers::NONE) => {
                    if !self.input_handler.is_empty() {
                        return Ok(Some(ModalResult::Confirmed(SearchModalResult {
                            query: self.input_handler.text().to_string(),
                            action: SearchAction::Next,
                        })));
                    }
                }
                // Shift+F3 - previous match
                (KeyCode::F(3), KeyModifiers::SHIFT) => {
                    if !self.input_handler.is_empty() {
                        return Ok(Some(ModalResult::Confirmed(SearchModalResult {
                            query: self.input_handler.text().to_string(),
                            action: SearchAction::Previous,
                        })));
                    }
                }
                // Backspace - delete character
                (KeyCode::Backspace, KeyModifiers::NONE) => {
                    if self.input_handler.backspace() {
                        // Trigger live search
                        if !self.input_handler.is_empty() {
                            return Ok(Some(ModalResult::Confirmed(SearchModalResult {
                                query: self.input_handler.text().to_string(),
                                action: SearchAction::Search,
                            })));
                        }
                    }
                }
                // Delete - delete character at cursor
                (KeyCode::Delete, KeyModifiers::NONE) => {
                    if self.input_handler.delete() {
                        // Trigger live search
                        if !self.input_handler.is_empty() {
                            return Ok(Some(ModalResult::Confirmed(SearchModalResult {
                                query: self.input_handler.text().to_string(),
                                action: SearchAction::Search,
                            })));
                        }
                    }
                }
                // Left - move cursor left
                (KeyCode::Left, KeyModifiers::NONE) => {
                    self.input_handler.move_left();
                }
                // Right - move cursor right
                (KeyCode::Right, KeyModifiers::NONE) => {
                    self.input_handler.move_right();
                }
                // Home - move to start
                (KeyCode::Home, KeyModifiers::NONE) => {
                    self.input_handler.move_home();
                }
                // End - move to end
                (KeyCode::End, KeyModifiers::NONE) => {
                    self.input_handler.move_end();
                }
                // Character input - insert character and trigger live search
                (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                    self.input_handler.insert_char(ch);

                    // Trigger live search
                    return Ok(Some(ModalResult::Confirmed(SearchModalResult {
                        query: self.input_handler.text().to_string(),
                        action: SearchAction::Search,
                    })));
                }
                _ => {}
            },
        }

        Ok(None)
    }

    fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        use crossterm::event::MouseEventKind;

        // Only handle left clicks
        if !matches!(
            mouse.kind,
            MouseEventKind::Down(crossterm::event::MouseButton::Left)
        ) {
            return Ok(None);
        }

        let click_pos = (mouse.column, mouse.row);

        // Check if clicked on close button [X]
        if let Some(close_area) = self.last_close_button_area {
            if click_pos.0 >= close_area.x
                && click_pos.0 < close_area.x + close_area.width
                && click_pos.1 == close_area.y
            {
                return Ok(Some(ModalResult::Cancelled));
            }
        }

        // Check if clicked on any button
        for (area, idx) in &self.last_button_areas {
            if click_pos.0 >= area.x && click_pos.0 < area.x + area.width && click_pos.1 == area.y {
                // Trigger corresponding action
                if !self.input_handler.is_empty() {
                    let action = match idx {
                        0 => SearchAction::Previous,
                        _ => SearchAction::Next,
                    };
                    return Ok(Some(ModalResult::Confirmed(SearchModalResult {
                        query: self.input_handler.text().to_string(),
                        action,
                    })));
                }
            }
        }

        Ok(None)
    }
}
