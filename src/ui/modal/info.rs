use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use super::{Modal, ModalResult};
use crate::constants::{
    MODAL_MAX_WIDTH_PERCENTAGE_WIDE, MODAL_MIN_VALUE_WIDTH, MODAL_MIN_WIDTH_WIDE, SPINNER_FRAMES,
    SPINNER_FRAMES_COUNT,
};
use crate::i18n;
use crate::theme::Theme;
use crate::ui::centered_rect_with_size;

/// Information modal window (closes on any key)
#[derive(Debug)]
pub struct InfoModal {
    title: String,
    lines: Vec<(String, String)>,   // (key, value) pairs for table
    spinner_frame: usize,           // Frame counter for spinner animation
    last_button_area: Option<Rect>, // For mouse handling
}

impl InfoModal {
    /// Create a new information modal window with tabular data
    pub fn new(title: impl Into<String>, lines: Vec<(String, String)>) -> Self {
        Self {
            title: title.into(),
            lines,
            spinner_frame: 0,
            last_button_area: None,
        }
    }

    /// Update a specific field value by key
    pub fn update_value(&mut self, key: &str, new_value: String) {
        if let Some(line) = self.lines.iter_mut().find(|(k, _)| k == key) {
            line.1 = new_value;
        }
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

        // Split by path separators and spaces for better readability
        let parts: Vec<&str> = if text.contains('/') || text.contains('\\') {
            // For paths, split by separators
            text.split_inclusive(&['/', '\\'][..]).collect()
        } else {
            // For regular text, split by words
            text.split_inclusive(' ').collect()
        };

        for part in parts {
            let part_width = part.width();

            // If part alone is too long, do hard break
            if part_width > max_width {
                // Finish current line if any
                if !current_line.is_empty() {
                    lines.push(current_line.clone());
                    current_line.clear();
                    current_width = 0;
                }

                // Break the long part character by character
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
                // Part would overflow, start new line
                if !current_line.is_empty() {
                    lines.push(current_line.clone());
                }
                current_line = part.to_string();
                current_width = part_width;
            } else {
                // Part fits in current line
                current_line.push_str(part);
                current_width += part_width;
            }
        }

        // Add remaining line
        if !current_line.is_empty() {
            lines.push(current_line);
        }

        if lines.is_empty() {
            lines.push(String::new());
        }

        lines
    }

    /// Calculate dynamic modal width based on content size
    /// Fits content without wrapping, but respects screen size limits
    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        // Find maximum key length
        let max_key_len = self
            .lines
            .iter()
            .map(|(key, _)| key.width())
            .max()
            .unwrap_or(0);

        // Find maximum value length (accounting for potential spinner)
        let max_value_len = self
            .lines
            .iter()
            .map(|(_, value)| {
                // Account for spinner characters if value contains "calculating"
                let t = i18n::t();
                if value.contains(t.file_info_calculating()) {
                    value.width() + 2 // spinner char + space
                } else {
                    value.width()
                }
            })
            .max()
            .unwrap_or(0);

        // Calculate required width:
        // padding (4) + borders (2) + key + ": " (2) + value
        let content_width = 6 + max_key_len + 2 + max_value_len;

        // Apply constraints
        let max_width = (screen_width as f32 * MODAL_MAX_WIDTH_PERCENTAGE_WIDE) as u16;
        (content_width as u16)
            .max(MODAL_MIN_WIDTH_WIDE)
            .min(max_width)
            .min(screen_width)
    }
}

impl Modal for InfoModal {
    type Result = ();

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // Calculate dynamic width based on content
        let modal_width = self.calculate_modal_width(area.width);

        // Find maximum key length for alignment
        let max_key_len = self
            .lines
            .iter()
            .map(|(key, _)| key.width())
            .max()
            .unwrap_or(0);

        // Calculate available width for values
        // modal_width - borders (2) - padding (4) - key_width - ": " (2)
        let available_value_width = modal_width
            .saturating_sub(6) // borders + padding
            .saturating_sub(max_key_len as u16)
            .saturating_sub(2) // ": "
            .max(MODAL_MIN_VALUE_WIDTH as u16) as usize;

        // Calculate total lines needed (with wrapping)
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

        // Calculate required height based on wrapped content:
        // 1 (top border) + 1 (empty line) + N (wrapped data lines) +
        // 1 (empty line) + 1 (button) + 1 (bottom border) = N + 5
        let modal_height = (total_data_lines + 5) as u16;

        // Create centered area with calculated dimensions
        let modal_area = centered_rect_with_size(modal_width, modal_height, area);

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

        // Split into: empty line, data, empty line, button
        use ratatui::layout::{Constraint, Direction, Layout};
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),                       // Empty line at top
                Constraint::Length(total_data_lines as u16), // Data (wrapped)
                Constraint::Length(1),                       // Empty line
                Constraint::Length(1),                       // Button
            ])
            .split(inner);

        // Render tabular data with left alignment and text wrapping
        let mut text_lines = Vec::new();
        for (key, value) in &self.lines {
            let padding = " ".repeat(max_key_len - key.width());

            // If value contains calculating text, show spinner
            let display_value = if value.contains(t.file_info_calculating()) {
                format!("{} {}", self.get_spinner_char(), value)
            } else {
                value.clone()
            };

            // Wrap the value to fit available width
            let wrapped_values = Self::wrap_text(&display_value, available_value_width);

            // First line with key
            if !wrapped_values.is_empty() {
                // For empty keys, add 2 spaces instead of ": " to maintain alignment
                let spans = if key.is_empty() {
                    vec![
                        Span::styled(
                            format!("  {}{}", key, padding),
                            Style::default()
                                .fg(theme.accented_fg)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "), // 2 spaces to align with ": "
                        Span::styled(wrapped_values[0].clone(), Style::default().fg(theme.bg)),
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
                        Span::styled(wrapped_values[0].clone(), Style::default().fg(theme.bg)),
                    ]
                };
                text_lines.push(Line::from(spans));

                // Additional lines with indent (continuation of value)
                // Both empty and normal keys now have same indent since we add 2 spaces for empty keys
                let indent = " ".repeat(max_key_len + 4); // "  " + key_len + "  " or ": "
                for wrapped_line in wrapped_values.iter().skip(1) {
                    text_lines.push(Line::from(vec![Span::styled(
                        format!("{}{}", indent, wrapped_line),
                        Style::default().fg(theme.bg),
                    )]));
                }
            }
        }

        let data = Paragraph::new(text_lines).alignment(Alignment::Left);
        data.render(chunks[1], buf);

        // Render Close button (always highlighted)
        let close_button = Line::from(vec![Span::styled(
            format!("[ {} ]", t.ui_close()),
            Style::default()
                .fg(theme.fg)
                .bg(theme.accented_fg)
                .add_modifier(Modifier::BOLD),
        )]);

        let button_paragraph = Paragraph::new(close_button).alignment(Alignment::Center);
        button_paragraph.render(chunks[3], buf);

        // Save button area for mouse handling
        self.last_button_area = Some(chunks[3]);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>> {
        // Close on any key
        match key.code {
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
            _ => Ok(Some(ModalResult::Confirmed(()))),
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

        // Check if we have stored button area
        let Some(button_area) = self.last_button_area else {
            return Ok(None);
        };

        // Check if click is within button area
        if mouse.row < button_area.y
            || mouse.row >= button_area.y + button_area.height
            || mouse.column < button_area.x
            || mouse.column >= button_area.x + button_area.width
        {
            return Ok(None);
        }

        // Calculate button position
        // Button is centered: "[ Close ]"
        let t = i18n::t();
        let button_text = format!("[ {} ]", t.ui_close());
        let button_width = button_text.len() as u16;

        let start_col = button_area.x + (button_area.width.saturating_sub(button_width)) / 2;
        let end_col = start_col + button_width;

        // Check if click is within button bounds
        if mouse.column >= start_col && mouse.column < end_col {
            // Close button clicked
            Ok(Some(ModalResult::Confirmed(())))
        } else {
            Ok(None)
        }
    }
}
