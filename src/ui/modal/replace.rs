use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Clear, Widget},
};

use super::{Modal, ModalResult};
use crate::theme::Theme;

/// Replace modal result
#[derive(Debug, Clone)]
pub struct ReplaceModalResult {
    pub find_query: String,
    pub replace_with: String,
    pub action: ReplaceAction,
}

/// Replace action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaceAction {
    /// Search and go to first match
    Search,
    /// Navigate to next match
    Next,
    /// Navigate to previous match
    Previous,
    /// Replace current match
    Replace,
    /// Replace all matches
    ReplaceAll,
}

/// Focus area in replace modal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    FindInput,
    ReplaceInput,
    Buttons,
}

/// Interactive replace modal with live preview and navigation
#[derive(Debug)]
pub struct ReplaceModal {
    find_input: String,
    find_cursor_pos: usize,
    replace_input: String,
    replace_cursor_pos: usize,
    focus: FocusArea,
    selected_button: usize, // 0 = Replace, 1 = Replace All, 2 = Previous, 3 = Next
    /// Match count display (e.g. "3 of 12")
    match_info: Option<(usize, usize)>, // (current, total)
    /// Last rendered areas for mouse handling
    last_button_areas: Vec<(Rect, usize)>, // (area, button_idx)
    last_close_button_area: Option<Rect>,
}

impl ReplaceModal {
    /// Create new replace modal
    pub fn new() -> Self {
        Self {
            find_input: String::new(),
            find_cursor_pos: 0,
            replace_input: String::new(),
            replace_cursor_pos: 0,
            focus: FocusArea::FindInput,
            selected_button: 3, // Next button selected by default
            match_info: None,
            last_button_areas: Vec::new(),
            last_close_button_area: None,
        }
    }

    /// Update match information (current index, total count)
    pub fn set_match_info(&mut self, current: usize, total: usize) {
        self.match_info = Some((current, total));
    }

    /// Set initial find text (e.g., from previous replace)
    pub fn set_find_input(&mut self, text: String) {
        self.find_cursor_pos = text.chars().count();
        self.find_input = text;
    }

    /// Set initial replace text (e.g., from previous replace)
    pub fn set_replace_input(&mut self, text: String) {
        self.replace_cursor_pos = text.chars().count();
        self.replace_input = text;
    }

    /// Calculate modal size
    fn calculate_modal_size(&self, screen_width: u16, screen_height: u16) -> (u16, u16) {
        // Compact modal: 2 input lines + buttons + match counter
        // "Find:    [____input____]"
        // "Replace: [____input____]"
        // "[ Replace ] [ Replace All ] [ ◄ Prev ] [ Next ► ] [3 of 12]"

        let min_width = 70; // Minimum width for comfortable use
        let max_width = (screen_width as f32 * 0.7) as u16;
        let width = min_width.min(max_width).min(screen_width);

        // Height: 1 (border) + 1 (find input) + 1 (replace input) + 1 (buttons+counter) + 1 (border)
        let height = 5;

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

impl Modal for ReplaceModal {
    type Result = ReplaceModalResult;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let (modal_width, modal_height) = self.calculate_modal_size(area.width, area.height);
        let modal_area = Self::top_center_rect(modal_width, modal_height, area);

        // Clear area
        Clear.render(modal_area, buf);

        // Create block with [X] close button on the left
        let title_with_close = " [X] Replace ";
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

        // Layout: [find input] [replace input] [buttons+counter]
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Find input line
                Constraint::Length(1), // Replace input line
                Constraint::Length(1), // Buttons line
            ])
            .split(inner);

        // === Render find input line ===
        let find_area = chunks[0];
        let find_label = "Find:    ";
        let find_label_width = find_label.len() as u16;

        buf.set_string(
            find_area.x,
            find_area.y,
            find_label,
            Style::default().fg(theme.bg),
        );

        // Find input field
        let find_input_x = find_area.x + find_label_width;
        let find_input_width = find_area.width.saturating_sub(find_label_width);

        let find_input_style = if matches!(self.focus, FocusArea::FindInput) {
            Style::default().fg(theme.fg).bg(theme.bg)
        } else {
            Style::default().fg(theme.bg)
        };

        // Draw find input text
        let visible_find = if self.find_input.len() as u16 > find_input_width {
            let start = self
                .find_input
                .len()
                .saturating_sub(find_input_width as usize);
            &self.find_input[start..]
        } else {
            &self.find_input
        };

        buf.set_string(find_input_x, find_area.y, visible_find, find_input_style);

        // Draw cursor if find input is focused
        if matches!(self.focus, FocusArea::FindInput) {
            let cursor_screen_pos =
                find_input_x + (visible_find.len() as u16).min(find_input_width - 1);
            if cursor_screen_pos < find_input_x + find_input_width {
                buf[(cursor_screen_pos, find_area.y)].set_style(
                    Style::default()
                        .bg(theme.bg)
                        .fg(theme.fg)
                        .add_modifier(Modifier::REVERSED),
                );
            }
        }

        // === Render replace input line ===
        let replace_area = chunks[1];
        let replace_label = "Replace: ";
        let replace_label_width = replace_label.len() as u16;

        buf.set_string(
            replace_area.x,
            replace_area.y,
            replace_label,
            Style::default().fg(theme.bg),
        );

        // Replace input field
        let replace_input_x = replace_area.x + replace_label_width;
        let replace_input_width = replace_area.width.saturating_sub(replace_label_width);

        let replace_input_style = if matches!(self.focus, FocusArea::ReplaceInput) {
            Style::default().fg(theme.fg).bg(theme.bg)
        } else {
            Style::default().fg(theme.bg)
        };

        // Draw replace input text
        let visible_replace = if self.replace_input.len() as u16 > replace_input_width {
            let start = self
                .replace_input
                .len()
                .saturating_sub(replace_input_width as usize);
            &self.replace_input[start..]
        } else {
            &self.replace_input
        };

        buf.set_string(
            replace_input_x,
            replace_area.y,
            visible_replace,
            replace_input_style,
        );

        // Draw cursor if replace input is focused
        if matches!(self.focus, FocusArea::ReplaceInput) {
            let cursor_screen_pos =
                replace_input_x + (visible_replace.len() as u16).min(replace_input_width - 1);
            if cursor_screen_pos < replace_input_x + replace_input_width {
                buf[(cursor_screen_pos, replace_area.y)].set_style(
                    Style::default()
                        .bg(theme.bg)
                        .fg(theme.fg)
                        .add_modifier(Modifier::REVERSED),
                );
            }
        }

        // === Render buttons and counter ===
        let buttons_area = chunks[2];

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
        let buttons = vec![("Replace", 0), ("All", 1), ("◄ Prev", 2), ("Next ►", 3)];

        let buttons_focused = matches!(self.focus, FocusArea::Buttons);
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
            x_offset += button_width + 1;
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>> {
        match self.focus {
            FocusArea::FindInput => self.handle_find_input_key(key),
            FocusArea::ReplaceInput => self.handle_replace_input_key(key),
            FocusArea::Buttons => self.handle_buttons_key(key),
        }
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
                if !self.find_input.is_empty() {
                    let action = match idx {
                        0 => ReplaceAction::Replace,
                        1 => ReplaceAction::ReplaceAll,
                        2 => ReplaceAction::Previous,
                        _ => ReplaceAction::Next,
                    };
                    return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                        find_query: self.find_input.clone(),
                        replace_with: self.replace_input.clone(),
                        action,
                    })));
                }
            }
        }

        Ok(None)
    }
}

impl ReplaceModal {
    fn handle_find_input_key(
        &mut self,
        key: KeyEvent,
    ) -> Result<Option<ModalResult<ReplaceModalResult>>> {
        match (key.code, key.modifiers) {
            // Tab - move to replace input / trigger next
            (KeyCode::Tab, KeyModifiers::NONE) => {
                if !self.find_input.is_empty() {
                    // If there's text, navigate to next match
                    return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                        find_query: self.find_input.clone(),
                        replace_with: self.replace_input.clone(),
                        action: ReplaceAction::Next,
                    })));
                } else {
                    // Otherwise move focus to replace field
                    self.focus = FocusArea::ReplaceInput;
                }
            }
            // Shift+Tab - trigger previous
            (KeyCode::BackTab, _) => {
                if !self.find_input.is_empty() {
                    return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                        find_query: self.find_input.clone(),
                        replace_with: self.replace_input.clone(),
                        action: ReplaceAction::Previous,
                    })));
                }
            }
            // Enter - replace current and move to next
            (KeyCode::Enter, KeyModifiers::NONE) => {
                if !self.find_input.is_empty() {
                    return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                        find_query: self.find_input.clone(),
                        replace_with: self.replace_input.clone(),
                        action: ReplaceAction::Replace,
                    })));
                }
            }
            // Esc - cancel
            (KeyCode::Esc, KeyModifiers::NONE) => {
                return Ok(Some(ModalResult::Cancelled));
            }
            // F3 - next match
            (KeyCode::F(3), KeyModifiers::NONE) => {
                if !self.find_input.is_empty() {
                    return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                        find_query: self.find_input.clone(),
                        replace_with: self.replace_input.clone(),
                        action: ReplaceAction::Next,
                    })));
                }
            }
            // Shift+F3 - previous match
            (KeyCode::F(3), KeyModifiers::SHIFT) => {
                if !self.find_input.is_empty() {
                    return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                        find_query: self.find_input.clone(),
                        replace_with: self.replace_input.clone(),
                        action: ReplaceAction::Previous,
                    })));
                }
            }
            // Ctrl+R - replace current
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                if !self.find_input.is_empty() {
                    return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                        find_query: self.find_input.clone(),
                        replace_with: self.replace_input.clone(),
                        action: ReplaceAction::Replace,
                    })));
                }
            }
            // Ctrl+Alt+R - replace all
            (KeyCode::Char('r'), modifiers)
                if modifiers.contains(KeyModifiers::CONTROL)
                    && modifiers.contains(KeyModifiers::ALT) =>
            {
                if !self.find_input.is_empty() {
                    return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                        find_query: self.find_input.clone(),
                        replace_with: self.replace_input.clone(),
                        action: ReplaceAction::ReplaceAll,
                    })));
                }
            }
            // Backspace - delete character
            (KeyCode::Backspace, KeyModifiers::NONE) => {
                if self.find_cursor_pos > 0 {
                    let chars: Vec<char> = self.find_input.chars().collect();
                    self.find_input = chars[..self.find_cursor_pos - 1]
                        .iter()
                        .chain(chars[self.find_cursor_pos..].iter())
                        .collect();
                    self.find_cursor_pos -= 1;

                    // Trigger live search
                    if !self.find_input.is_empty() {
                        return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                            find_query: self.find_input.clone(),
                            replace_with: self.replace_input.clone(),
                            action: ReplaceAction::Search,
                        })));
                    }
                }
            }
            // Delete - delete character at cursor
            (KeyCode::Delete, KeyModifiers::NONE) => {
                let chars: Vec<char> = self.find_input.chars().collect();
                if self.find_cursor_pos < chars.len() {
                    self.find_input = chars[..self.find_cursor_pos]
                        .iter()
                        .chain(chars[self.find_cursor_pos + 1..].iter())
                        .collect();

                    // Trigger live search
                    if !self.find_input.is_empty() {
                        return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                            find_query: self.find_input.clone(),
                            replace_with: self.replace_input.clone(),
                            action: ReplaceAction::Search,
                        })));
                    }
                }
            }
            // Left - move cursor left
            (KeyCode::Left, KeyModifiers::NONE) => {
                if self.find_cursor_pos > 0 {
                    self.find_cursor_pos -= 1;
                }
            }
            // Right - move cursor right
            (KeyCode::Right, KeyModifiers::NONE) => {
                let chars_len = self.find_input.chars().count();
                if self.find_cursor_pos < chars_len {
                    self.find_cursor_pos += 1;
                }
            }
            // Home - move to start
            (KeyCode::Home, KeyModifiers::NONE) => {
                self.find_cursor_pos = 0;
            }
            // End - move to end
            (KeyCode::End, KeyModifiers::NONE) => {
                self.find_cursor_pos = self.find_input.chars().count();
            }
            // Down - move to replace input
            (KeyCode::Down, KeyModifiers::NONE) => {
                self.focus = FocusArea::ReplaceInput;
            }
            // Character input - insert character and trigger live search
            (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                let chars: Vec<char> = self.find_input.chars().collect();
                self.find_input = chars[..self.find_cursor_pos]
                    .iter()
                    .chain(std::iter::once(&ch))
                    .chain(chars[self.find_cursor_pos..].iter())
                    .collect();
                self.find_cursor_pos += 1;

                // Trigger live search
                return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                    find_query: self.find_input.clone(),
                    replace_with: self.replace_input.clone(),
                    action: ReplaceAction::Search,
                })));
            }
            _ => {}
        }

        Ok(None)
    }

    fn handle_replace_input_key(
        &mut self,
        key: KeyEvent,
    ) -> Result<Option<ModalResult<ReplaceModalResult>>> {
        match (key.code, key.modifiers) {
            // Tab - move to buttons
            (KeyCode::Tab, KeyModifiers::NONE) => {
                self.focus = FocusArea::Buttons;
            }
            // Shift+Tab - move back to find
            (KeyCode::BackTab, _) => {
                self.focus = FocusArea::FindInput;
            }
            // Enter - replace current
            (KeyCode::Enter, KeyModifiers::NONE) => {
                if !self.find_input.is_empty() {
                    return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                        find_query: self.find_input.clone(),
                        replace_with: self.replace_input.clone(),
                        action: ReplaceAction::Replace,
                    })));
                }
            }
            // Esc - cancel
            (KeyCode::Esc, KeyModifiers::NONE) => {
                return Ok(Some(ModalResult::Cancelled));
            }
            // F3 - next match
            (KeyCode::F(3), KeyModifiers::NONE) => {
                if !self.find_input.is_empty() {
                    return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                        find_query: self.find_input.clone(),
                        replace_with: self.replace_input.clone(),
                        action: ReplaceAction::Next,
                    })));
                }
            }
            // Shift+F3 - previous match
            (KeyCode::F(3), KeyModifiers::SHIFT) => {
                if !self.find_input.is_empty() {
                    return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                        find_query: self.find_input.clone(),
                        replace_with: self.replace_input.clone(),
                        action: ReplaceAction::Previous,
                    })));
                }
            }
            // Ctrl+R - replace current
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                if !self.find_input.is_empty() {
                    return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                        find_query: self.find_input.clone(),
                        replace_with: self.replace_input.clone(),
                        action: ReplaceAction::Replace,
                    })));
                }
            }
            // Ctrl+Alt+R - replace all
            (KeyCode::Char('r'), modifiers)
                if modifiers.contains(KeyModifiers::CONTROL)
                    && modifiers.contains(KeyModifiers::ALT) =>
            {
                if !self.find_input.is_empty() {
                    return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                        find_query: self.find_input.clone(),
                        replace_with: self.replace_input.clone(),
                        action: ReplaceAction::ReplaceAll,
                    })));
                }
            }
            // Backspace - delete character
            (KeyCode::Backspace, KeyModifiers::NONE) => {
                if self.replace_cursor_pos > 0 {
                    let chars: Vec<char> = self.replace_input.chars().collect();
                    self.replace_input = chars[..self.replace_cursor_pos - 1]
                        .iter()
                        .chain(chars[self.replace_cursor_pos..].iter())
                        .collect();
                    self.replace_cursor_pos -= 1;
                }
            }
            // Delete - delete character at cursor
            (KeyCode::Delete, KeyModifiers::NONE) => {
                let chars: Vec<char> = self.replace_input.chars().collect();
                if self.replace_cursor_pos < chars.len() {
                    self.replace_input = chars[..self.replace_cursor_pos]
                        .iter()
                        .chain(chars[self.replace_cursor_pos + 1..].iter())
                        .collect();
                }
            }
            // Left - move cursor left
            (KeyCode::Left, KeyModifiers::NONE) => {
                if self.replace_cursor_pos > 0 {
                    self.replace_cursor_pos -= 1;
                }
            }
            // Right - move cursor right
            (KeyCode::Right, KeyModifiers::NONE) => {
                let chars_len = self.replace_input.chars().count();
                if self.replace_cursor_pos < chars_len {
                    self.replace_cursor_pos += 1;
                }
            }
            // Home - move to start
            (KeyCode::Home, KeyModifiers::NONE) => {
                self.replace_cursor_pos = 0;
            }
            // End - move to end
            (KeyCode::End, KeyModifiers::NONE) => {
                self.replace_cursor_pos = self.replace_input.chars().count();
            }
            // Up - move back to find input
            (KeyCode::Up, KeyModifiers::NONE) => {
                self.focus = FocusArea::FindInput;
            }
            // Down - move to buttons
            (KeyCode::Down, KeyModifiers::NONE) => {
                self.focus = FocusArea::Buttons;
            }
            // Character input - insert character (no live search trigger)
            (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                let chars: Vec<char> = self.replace_input.chars().collect();
                self.replace_input = chars[..self.replace_cursor_pos]
                    .iter()
                    .chain(std::iter::once(&ch))
                    .chain(chars[self.replace_cursor_pos..].iter())
                    .collect();
                self.replace_cursor_pos += 1;
            }
            _ => {}
        }

        Ok(None)
    }

    fn handle_buttons_key(
        &mut self,
        key: KeyEvent,
    ) -> Result<Option<ModalResult<ReplaceModalResult>>> {
        match key.code {
            KeyCode::Left => {
                self.selected_button = self.selected_button.saturating_sub(1);
            }
            KeyCode::Right => {
                self.selected_button = (self.selected_button + 1).min(3);
            }
            KeyCode::Up => {
                self.focus = FocusArea::ReplaceInput;
            }
            KeyCode::Enter => {
                if !self.find_input.is_empty() {
                    let action = match self.selected_button {
                        0 => ReplaceAction::Replace,
                        1 => ReplaceAction::ReplaceAll,
                        2 => ReplaceAction::Previous,
                        _ => ReplaceAction::Next,
                    };
                    return Ok(Some(ModalResult::Confirmed(ReplaceModalResult {
                        find_query: self.find_input.clone(),
                        replace_with: self.replace_input.clone(),
                        action,
                    })));
                }
            }
            KeyCode::Esc => {
                return Ok(Some(ModalResult::Cancelled));
            }
            KeyCode::Tab => {
                self.focus = FocusArea::FindInput;
            }
            _ => {}
        }

        Ok(None)
    }
}
