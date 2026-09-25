//! Information display modal with action buttons.

use anyhow::Result;
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use crate::base::render_modal_block;
use unicode_width::UnicodeWidthStr;

use termide_config::constants::{
    MODAL_MAX_WIDTH_PERCENTAGE_WIDE, MODAL_MIN_VALUE_WIDTH, MODAL_MIN_WIDTH_WIDE, SPINNER_FRAMES,
    SPINNER_FRAMES_COUNT,
};
use termide_i18n as i18n;
use termide_theme::Theme;

use crate::{centered_rect_with_size, Modal, ModalResult};

/// Action button definition
#[derive(Debug, Clone)]
pub struct ActionButton {
    /// Button label
    pub label: String,
    /// Action identifier returned when button is clicked
    pub action: String,
}

impl ActionButton {
    /// Create a new action button
    pub fn new(label: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            action: action.into(),
        }
    }
}

/// Permission access level for the current user
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermAccess {
    /// Root user — can change all bits
    Root,
    /// File owner — can change all except own r/w (locked ON)
    Owner,
    /// Not owner/root — read-only view, all bits locked
    ReadOnly,
}

/// Unix file permissions state (9 bits: rwx for user/group/others)
#[derive(Debug, Clone)]
pub struct PermissionsState {
    bits: [bool; 9],
    locked: [bool; 9],
    cursor_row: usize,
    cursor_col: usize,
    original_mode: u32,
    applied_mode: u32,
}

impl PermissionsState {
    /// Create from Unix mode bits with access control
    pub fn from_mode(mode: u32, access: PermAccess) -> Self {
        let locked = match access {
            PermAccess::Root => [false; 9],
            PermAccess::Owner => [
                true, true, false, // owner r/w locked, x free
                false, false, false, // group all free
                false, false, false, // others all free
            ],
            PermAccess::ReadOnly => [true; 9],
        };
        Self {
            bits: [
                mode & 0o400 != 0,
                mode & 0o200 != 0,
                mode & 0o100 != 0,
                mode & 0o040 != 0,
                mode & 0o020 != 0,
                mode & 0o010 != 0,
                mode & 0o004 != 0,
                mode & 0o002 != 0,
                mode & 0o001 != 0,
            ],
            locked,
            cursor_row: 0,
            cursor_col: 0,
            original_mode: mode,
            applied_mode: mode,
        }
    }

    /// Convert bits back to Unix mode, preserving setuid/setgid/sticky bits
    pub fn to_mode(&self) -> u32 {
        let mut mode = self.original_mode & !0o777;
        for (i, &bit) in self.bits.iter().enumerate() {
            if bit {
                mode |= 1 << (8 - i);
            }
        }
        mode
    }

    /// Return new mode if changed since last apply, and mark as applied
    pub fn take_pending_mode(&mut self) -> Option<u32> {
        let mode = self.to_mode();
        if mode != self.applied_mode {
            self.applied_mode = mode;
            Some(mode)
        } else {
            None
        }
    }

    fn toggle_current(&mut self) {
        let idx = self.cursor_row * 3 + self.cursor_col;
        if !self.locked[idx] {
            self.bits[idx] = !self.bits[idx];
        }
    }
}

/// Focus area within the InfoActionModal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Permissions,
    Buttons,
}

/// Information modal window with action buttons
#[derive(Debug)]
pub struct InfoActionModal {
    title: String,
    lines: Vec<(String, String)>,
    buttons: Vec<ActionButton>,
    selected_button: usize,
    spinner_frame: usize,
    last_button_areas: Vec<Rect>,
    /// Git operation in progress (action name like "push" or "pull")
    operation_in_progress: Option<String>,
    /// Optional permissions editor
    permissions: Option<PermissionsState>,
    /// Current focus area
    focus: FocusArea,
    /// Saved permission checkbox areas for mouse handling (9 cells)
    last_perm_areas: Vec<Rect>,
}

impl InfoActionModal {
    /// Create a new information modal window with tabular data and action buttons
    pub fn new(
        title: impl Into<String>,
        lines: Vec<(String, String)>,
        buttons: Vec<ActionButton>,
    ) -> Self {
        Self {
            title: title.into(),
            lines,
            buttons,
            selected_button: 0,
            spinner_frame: 0,
            last_button_areas: Vec::new(),
            operation_in_progress: None,
            permissions: None,
            focus: FocusArea::Buttons,
            last_perm_areas: Vec::new(),
        }
    }

    /// Add permissions editor widget (Unix mode bits with access control)
    pub fn with_permissions(mut self, mode: u32, access: PermAccess) -> Self {
        self.permissions = Some(PermissionsState::from_mode(mode, access));
        self
    }

    /// Take pending permission change (returns new mode if changed since last call)
    pub fn take_pending_permission_change(&mut self) -> Option<u32> {
        self.permissions
            .as_mut()
            .and_then(|p| p.take_pending_mode())
    }

    /// Set the initially selected button index
    pub fn with_selected_button(mut self, index: usize) -> Self {
        self.selected_button = index.min(self.buttons.len().saturating_sub(1));
        self
    }

    /// Update a specific field value by key
    pub fn update_value(&mut self, key: &str, new_value: String) {
        if let Some(line) = self.lines.iter_mut().find(|(k, _)| k == key) {
            line.1 = new_value;
        }
    }

    /// Set operation in progress (for animated button)
    pub fn set_operation_in_progress(&mut self, action: Option<String>) {
        self.operation_in_progress = action;
    }

    /// Check if operation is in progress
    pub fn is_operation_in_progress(&self) -> bool {
        self.operation_in_progress.is_some()
    }

    /// Advance the spinner frame counter (for animation)
    pub fn advance_spinner(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES_COUNT;
    }

    /// Get the current spinner character
    fn get_spinner_char(&self) -> &str {
        SPINNER_FRAMES[self.spinner_frame]
    }

    /// Wrap text to fit within max_width, breaking on delimiters
    fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
        if max_width == 0 {
            return vec![text.to_string()];
        }

        let text_width = text.width();
        if text_width <= max_width {
            return vec![text.to_string()];
        }

        let mut lines = Vec::new();
        let mut current_line = String::new();
        let mut current_width = 0;

        let parts: Vec<&str> = if text.contains('/') || text.contains('\\') {
            text.split_inclusive(&['/', '\\'][..]).collect()
        } else {
            text.split_inclusive(' ').collect()
        };

        for part in parts {
            let part_width = part.width();

            if part_width > max_width {
                if !current_line.is_empty() {
                    lines.push(current_line.clone());
                    current_line.clear();
                    current_width = 0;
                }

                for ch in part.chars() {
                    let ch_width = ch.to_string().width();
                    if current_width + ch_width > max_width {
                        lines.push(current_line.clone());
                        current_line.clear();
                        current_width = 0;
                    }
                    current_line.push(ch);
                    current_width += ch_width;
                }
            } else if current_width + part_width > max_width {
                if !current_line.is_empty() {
                    lines.push(current_line.clone());
                }
                current_line = part.to_string();
                current_width = part_width;
            } else {
                current_line.push_str(part);
                current_width += part_width;
            }
        }

        if !current_line.is_empty() {
            lines.push(current_line);
        }

        if lines.is_empty() {
            lines.push(String::new());
        }

        lines
    }

    /// Calculate dynamic modal width based on content size
    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        let max_key_len = self
            .lines
            .iter()
            .map(|(key, _)| key.width())
            .max()
            .unwrap_or(0);

        let t = i18n::t();
        let max_value_len = self
            .lines
            .iter()
            .map(|(_, value)| {
                if value.contains(t.file_info_calculating()) {
                    value.width() + 2
                } else {
                    value.width()
                }
            })
            .max()
            .unwrap_or(0);

        // Calculate buttons width (including spacing)
        let buttons_width: usize = self
            .buttons
            .iter()
            .map(|b| b.label.width() + 4) // "[ label ]" + space
            .sum::<usize>()
            + self.buttons.len().saturating_sub(1) * 2; // spacing between buttons

        let content_width = 6 + max_key_len + 2 + max_value_len;
        let buttons_row_width = buttons_width + 4; // padding

        let required_width = content_width.max(buttons_row_width);

        let max_width = (screen_width as f32 * MODAL_MAX_WIDTH_PERCENTAGE_WIDE) as u16;
        (required_width as u16)
            .max(MODAL_MIN_WIDTH_WIDE)
            .min(max_width)
            .min(screen_width)
    }

    fn select_next_button(&mut self) {
        if !self.buttons.is_empty() {
            self.selected_button = (self.selected_button + 1) % self.buttons.len();
        }
    }

    fn select_prev_button(&mut self) {
        if !self.buttons.is_empty() {
            if self.selected_button == 0 {
                self.selected_button = self.buttons.len() - 1;
            } else {
                self.selected_button -= 1;
            }
        }
    }
}

/// Result from InfoActionModal
#[derive(Debug, Clone)]
pub enum InfoActionResult {
    /// User selected an action button
    Action(String),
    /// User closed the modal
    Closed,
    /// User cancelled an in-progress operation
    CancelOperation,
}

impl Modal for InfoActionModal {
    type Result = InfoActionResult;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let modal_width = self.calculate_modal_width(area.width);

        let max_key_len = self
            .lines
            .iter()
            .map(|(key, _)| key.width())
            .max()
            .unwrap_or(0);

        let available_value_width = modal_width
            .saturating_sub(6)
            .saturating_sub(max_key_len as u16)
            .saturating_sub(2)
            .max(MODAL_MIN_VALUE_WIDTH as u16) as usize;

        let t = i18n::t();
        let mut total_data_lines = 0;
        for (_, value) in &self.lines {
            let display_value = if value.contains(t.file_info_calculating()) {
                format!("{} {}", self.get_spinner_char(), value)
            } else {
                value.clone()
            };
            let wrapped = Self::wrap_text(&display_value, available_value_width);
            total_data_lines += wrapped.len();
        }

        // Permissions widget height: empty line + header + separator + 3 rows = 6
        let perm_height = if self.permissions.is_some() { 6u16 } else { 0 };

        // Height: top border + empty + data + perm + empty + buttons + bottom border
        let modal_height = (total_data_lines as u16) + perm_height + 5;

        let modal_area = centered_rect_with_size(modal_width, modal_height, area);

        let inner = render_modal_block(modal_area, buf, &self.title, theme);

        let mut constraints = vec![
            Constraint::Length(1),                       // top padding
            Constraint::Length(total_data_lines as u16), // data
        ];
        if self.permissions.is_some() {
            constraints.push(Constraint::Length(perm_height)); // permissions
        }
        constraints.push(Constraint::Length(1)); // bottom padding
        constraints.push(Constraint::Length(1)); // buttons

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        // Render tabular data
        let mut text_lines = Vec::new();
        for (key, value) in &self.lines {
            let padding = " ".repeat(max_key_len - key.width());

            let display_value = if value.contains(t.file_info_calculating()) {
                format!("{} {}", self.get_spinner_char(), value)
            } else {
                value.clone()
            };

            let wrapped_values = Self::wrap_text(&display_value, available_value_width);

            if !wrapped_values.is_empty() {
                let spans = if key.is_empty() {
                    vec![
                        Span::styled(
                            format!("  {}{}", key, padding),
                            Style::default()
                                .fg(theme.accented_fg)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "),
                        Span::styled(wrapped_values[0].clone(), Style::default().fg(theme.fg)),
                    ]
                } else {
                    vec![
                        Span::styled(
                            format!("  {}{}", key, padding),
                            Style::default()
                                .fg(theme.accented_fg)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(": "),
                        Span::styled(wrapped_values[0].clone(), Style::default().fg(theme.fg)),
                    ]
                };
                text_lines.push(Line::from(spans));

                let indent = " ".repeat(max_key_len + 4);
                for wrapped_line in wrapped_values.iter().skip(1) {
                    text_lines.push(Line::from(vec![Span::styled(
                        format!("{}{}", indent, wrapped_line),
                        Style::default().fg(theme.fg),
                    )]));
                }
            }
        }

        let data = Paragraph::new(text_lines).alignment(Alignment::Left);
        data.render(chunks[1], buf);

        // Render permissions widget if present
        let mut chunk_idx = 2; // after top padding + data
        self.last_perm_areas.clear();
        if let Some(ref perms) = self.permissions {
            let perm_area = chunks[chunk_idx];
            chunk_idx += 1;

            let t = i18n::t();
            let row_labels = [
                t.perm_owner().to_string(),
                t.perm_group().to_string(),
                t.perm_others().to_string(),
            ];
            let col_labels = ["r", "w", "x"];

            // Header line: "  Permissions    r    w    x"
            let perm_label = t.perm_permissions();
            let max_label = row_labels
                .iter()
                .map(|l| l.width())
                .max()
                .unwrap_or(5)
                .max(perm_label.width());
            let label_pad = " ".repeat(max_label - perm_label.width());
            let header = Line::from(vec![
                Span::styled(
                    format!("  {}{}", perm_label, label_pad),
                    Style::default()
                        .fg(theme.accented_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "   {}    {}    {}",
                        col_labels[0], col_labels[1], col_labels[2]
                    ),
                    Style::default()
                        .fg(theme.accented_fg)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            buf.set_line(perm_area.x, perm_area.y + 1, &header, perm_area.width);

            // Separator
            let sep_line = "─".repeat((max_label + 22).min(perm_area.width as usize));
            buf.set_string(
                perm_area.x + 2,
                perm_area.y + 2,
                sep_line,
                Style::default().fg(theme.disabled),
            );

            let is_focused = self.focus == FocusArea::Permissions;

            // 3 rows of checkboxes
            for (row, label) in row_labels.iter().enumerate() {
                let y = perm_area.y + 3 + row as u16;
                let label_padding = " ".repeat(max_label - label.width());

                // Row label
                buf.set_string(
                    perm_area.x + 2,
                    y,
                    format!("{}{}", label, label_padding),
                    Style::default()
                        .fg(theme.accented_fg)
                        .add_modifier(Modifier::BOLD),
                );

                for col in 0..3 {
                    let idx = row * 3 + col;
                    let bit = perms.bits[idx];
                    let is_locked = perms.locked[idx];
                    let checkbox = termide_ui::checkbox(bit);

                    let is_cursor =
                        is_focused && perms.cursor_row == row && perms.cursor_col == col;

                    let style = if is_cursor && is_locked {
                        Style::default().fg(theme.disabled).bg(theme.accented_fg)
                    } else if is_locked {
                        Style::default().fg(theme.disabled)
                    } else if is_cursor {
                        Style::default()
                            .fg(theme.bg)
                            .bg(theme.accented_fg)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.fg)
                    };

                    let x = perm_area.x + (max_label as u16) + 4 + (col as u16) * 5;
                    buf.set_string(x, y, checkbox, style);

                    // Save area for mouse handling
                    self.last_perm_areas.push(Rect {
                        x,
                        y,
                        width: 3,
                        height: 1,
                    });
                }
            }
        }
        chunk_idx += 1; // skip bottom padding

        // Render action buttons
        self.last_button_areas.clear();

        // Build button labels (with spinner for operation in progress)
        // Use same format as git status panel: "Pushing..." / "Pulling..."
        let button_labels: Vec<String> = self
            .buttons
            .iter()
            .map(|button| {
                if self.operation_in_progress.as_ref() == Some(&button.action) {
                    let spinner = self.get_spinner_char();
                    match button.action.as_str() {
                        "push" => format!("{} Pushing...", spinner),
                        "pull" => format!("{} Pulling...", spinner),
                        _ => button.label.clone(),
                    }
                } else {
                    button.label.clone()
                }
            })
            .collect();

        let mut button_spans = Vec::new();
        for (i, (button, label)) in self.buttons.iter().zip(button_labels.iter()).enumerate() {
            let is_selected = i == self.selected_button;
            let is_in_progress = self.operation_in_progress.as_ref() == Some(&button.action);
            let style = if is_in_progress {
                // Animated button style
                Style::default()
                    .fg(theme.accented_fg)
                    .add_modifier(Modifier::BOLD)
            } else if is_selected && self.focus == FocusArea::Buttons {
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };

            if i > 0 {
                button_spans.push(Span::raw("  "));
            }
            button_spans.push(Span::styled(format!("[ {} ]", label), style));
        }

        let buttons_line = Line::from(button_spans);
        let button_paragraph = Paragraph::new(buttons_line).alignment(Alignment::Center);
        let buttons_chunk = chunks[chunk_idx];
        button_paragraph.render(buttons_chunk, buf);

        // Calculate button areas for mouse handling
        let buttons_total_width: usize = button_labels
            .iter()
            .map(|label| label.width() + 4)
            .sum::<usize>()
            + self.buttons.len().saturating_sub(1) * 2;

        let start_x = buttons_chunk.x
            + (buttons_chunk
                .width
                .saturating_sub(buttons_total_width as u16))
                / 2;
        let mut current_x = start_x;

        for label in &button_labels {
            let btn_width = (label.width() + 4) as u16;
            self.last_button_areas.push(Rect {
                x: current_x,
                y: buttons_chunk.y,
                width: btn_width,
                height: 1,
            });
            current_x += btn_width + 2; // button width + spacing
        }
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let key = chord.raw;
        // If operation is in progress, only allow Enter/Space to cancel
        if self.operation_in_progress.is_some() {
            match key.code {
                KeyCode::Enter | KeyCode::Char(' ') => {
                    return Ok(Some(ModalResult::Confirmed(
                        InfoActionResult::CancelOperation,
                    )));
                }
                _ => return Ok(None),
            }
        }

        if key.code == KeyCode::Esc {
            return Ok(Some(ModalResult::Cancelled));
        }

        match self.focus {
            FocusArea::Permissions => {
                if let Some(ref mut perms) = self.permissions {
                    match key.code {
                        KeyCode::Up => {
                            if perms.cursor_row > 0 {
                                perms.cursor_row -= 1;
                            }
                            Ok(None)
                        }
                        KeyCode::Down => {
                            if perms.cursor_row < 2 {
                                perms.cursor_row += 1;
                            } else {
                                self.focus = FocusArea::Buttons;
                            }
                            Ok(None)
                        }
                        KeyCode::Left => {
                            if perms.cursor_col > 0 {
                                perms.cursor_col -= 1;
                            }
                            Ok(None)
                        }
                        KeyCode::Right => {
                            if perms.cursor_col < 2 {
                                perms.cursor_col += 1;
                            }
                            Ok(None)
                        }
                        KeyCode::Char(' ') => {
                            perms.toggle_current();
                            Ok(None)
                        }
                        KeyCode::Tab => {
                            self.focus = FocusArea::Buttons;
                            Ok(None)
                        }
                        KeyCode::Enter => {
                            if let Some(button) = self.buttons.get(self.selected_button) {
                                Ok(Some(ModalResult::Confirmed(InfoActionResult::Action(
                                    button.action.clone(),
                                ))))
                            } else {
                                Ok(Some(ModalResult::Confirmed(InfoActionResult::Closed)))
                            }
                        }
                        _ => Ok(None),
                    }
                } else {
                    Ok(None)
                }
            }
            FocusArea::Buttons => match key.code {
                KeyCode::Enter | KeyCode::Char(' ') => {
                    if let Some(button) = self.buttons.get(self.selected_button) {
                        Ok(Some(ModalResult::Confirmed(InfoActionResult::Action(
                            button.action.clone(),
                        ))))
                    } else {
                        Ok(Some(ModalResult::Confirmed(InfoActionResult::Closed)))
                    }
                }
                KeyCode::Tab | KeyCode::Right => {
                    self.select_next_button();
                    Ok(None)
                }
                KeyCode::BackTab => {
                    if self.permissions.is_some() {
                        self.focus = FocusArea::Permissions;
                    } else {
                        self.select_prev_button();
                    }
                    Ok(None)
                }
                KeyCode::Left => {
                    self.select_prev_button();
                    Ok(None)
                }
                KeyCode::Up => {
                    if self.permissions.is_some() {
                        self.focus = FocusArea::Permissions;
                    }
                    Ok(None)
                }
                _ => Ok(None),
            },
        }
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return Ok(None);
        }

        // Check if click is on a permission checkbox
        for (i, perm_area) in self.last_perm_areas.iter().enumerate() {
            if mouse.row == perm_area.y
                && mouse.column >= perm_area.x
                && mouse.column < perm_area.x + perm_area.width
            {
                if let Some(ref mut perms) = self.permissions {
                    let row = i / 3;
                    let col = i % 3;
                    perms.cursor_row = row;
                    perms.cursor_col = col;
                    perms.toggle_current();
                    self.focus = FocusArea::Permissions;
                }
                return Ok(None);
            }
        }

        // Check if click is on any button
        for (i, button_area) in self.last_button_areas.iter().enumerate() {
            if mouse.row == button_area.y
                && mouse.column >= button_area.x
                && mouse.column < button_area.x + button_area.width
            {
                if let Some(button) = self.buttons.get(i) {
                    // If operation is in progress and clicked on the animated button, cancel
                    if self.operation_in_progress.as_ref() == Some(&button.action) {
                        return Ok(Some(ModalResult::Confirmed(
                            InfoActionResult::CancelOperation,
                        )));
                    }
                    // If operation is in progress, block clicks on other buttons
                    if self.operation_in_progress.is_some() {
                        return Ok(None);
                    }
                    return Ok(Some(ModalResult::Confirmed(InfoActionResult::Action(
                        button.action.clone(),
                    ))));
                }
            }
        }

        Ok(None)
    }
}
