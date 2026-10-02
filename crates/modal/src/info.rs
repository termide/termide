//! Information display modal dialog.

use anyhow::Result;
use crossterm::event::KeyCode;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use crate::base::render_modal_block;
use termide_core::ThemeColors;
use termide_ui::scrollbar::ScrollBar;
use unicode_width::UnicodeWidthStr;

use termide_config::constants::{
    MODAL_MAX_WIDTH_PERCENTAGE_WIDE, MODAL_MIN_VALUE_WIDTH, MODAL_MIN_WIDTH_WIDE, SPINNER_FRAMES,
    SPINNER_FRAMES_COUNT,
};
use termide_i18n as i18n;
use termide_theme::Theme;

use crate::{centered_rect_with_size, is_click_outside, Modal, ModalResult};

/// Style for a colored segment in modal values.
#[derive(Debug, Clone, Copy, Default)]
pub enum SegmentStyle {
    #[default]
    Default,
    Success,
    Error,
    Warning,
    Disabled,
}

/// A text segment with an associated style.
#[derive(Debug, Clone)]
pub struct StyledSegment {
    pub text: String,
    pub style: SegmentStyle,
}

/// A modal value that is either plain text or a sequence of styled segments.
#[derive(Debug, Clone)]
pub enum ModalValue {
    Text(String),
    Segments(Vec<StyledSegment>),
}

/// Information modal window (closes on any key)
#[derive(Debug)]
pub struct InfoModal {
    title: String,
    lines: Vec<(String, ModalValue)>, // (key, value) pairs for table
    spinner_frame: usize,             // Frame counter for spinner animation
    /// Screen rect of the modal from the last render, for clicks beside it.
    last_modal_area: Option<Rect>,
    last_button_area: Option<Rect>,  // For mouse handling
    last_content_area: Option<Rect>, // For mouse-wheel scroll hit-test
    min_width: Option<u16>,          // Optional minimum width to prevent jitter
    anchor: Option<(u16, u16)>,      // Optional anchor position (x, y) instead of centering
    anchor_bottom: bool,             // true = anchor specifies bottom edge, not top
    show_button: bool,               // Whether to show the OK button
    scroll_offset: usize,            // First visible content line (after wrapping)
    cached_total_lines: usize,       // Cached total content lines after last render
    cached_visible: usize,           // Cached visible content-area height after last render
}

impl InfoModal {
    /// Create a new information modal window with tabular data (plain text values).
    pub fn new(title: impl Into<String>, lines: Vec<(String, String)>) -> Self {
        let lines = lines
            .into_iter()
            .map(|(k, v)| (k, ModalValue::Text(v)))
            .collect();
        Self {
            title: title.into(),
            lines,
            spinner_frame: 0,
            last_modal_area: None,
            last_button_area: None,
            last_content_area: None,
            min_width: None,
            anchor: None,
            anchor_bottom: false,
            show_button: true,
            scroll_offset: 0,
            cached_total_lines: 0,
            cached_visible: 0,
        }
    }

    /// Create a new information modal window with rich (styled) values.
    pub fn new_rich(title: impl Into<String>, lines: Vec<(String, ModalValue)>) -> Self {
        Self {
            title: title.into(),
            lines,
            spinner_frame: 0,
            last_modal_area: None,
            last_button_area: None,
            last_content_area: None,
            min_width: None,
            anchor: None,
            anchor_bottom: false,
            show_button: true,
            scroll_offset: 0,
            cached_total_lines: 0,
            cached_visible: 0,
        }
    }

    /// Position modal at anchor point instead of centering.
    pub fn with_anchor(mut self, x: u16, y: u16) -> Self {
        self.anchor = Some((x, y));
        self
    }

    /// Position modal so its bottom edge is at anchor y (grows upward).
    pub fn with_anchor_bottom(mut self, x: u16, y: u16) -> Self {
        self.anchor = Some((x, y));
        self.anchor_bottom = true;
        self
    }

    /// Hide the OK button (for info panels used as dropdown-style displays).
    pub fn without_button(mut self) -> Self {
        self.show_button = false;
        self
    }

    /// Set a minimum width to prevent modal jitter on content refresh.
    pub fn with_min_width(mut self, width: u16) -> Self {
        self.min_width = Some(width);
        self
    }

    /// Update a specific field value by key (sets it to plain text).
    pub fn update_value(&mut self, key: &str, new_value: String) {
        if let Some(line) = self.lines.iter_mut().find(|(k, _)| k == key) {
            line.1 = ModalValue::Text(new_value);
        }
    }

    /// Replace all lines (for auto-refreshing modals).
    pub fn set_lines(&mut self, lines: Vec<(String, ModalValue)>) {
        self.lines = lines;
    }

    /// Advance the spinner frame counter (for animation)
    pub fn advance_spinner(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES_COUNT;
    }

    /// Get the current spinner character
    fn get_spinner_char(&self) -> &str {
        SPINNER_FRAMES[self.spinner_frame]
    }

    /// Wrap a single paragraph (no embedded newlines) to fit within max_width.
    fn wrap_paragraph(text: &str, max_width: usize) -> Vec<String> {
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

    /// Wrap text to fit within max_width, respecting embedded newlines.
    fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
        let mut all_lines = Vec::new();
        for paragraph in text.split('\n') {
            if paragraph.is_empty() {
                all_lines.push(String::new());
            } else {
                all_lines.extend(Self::wrap_paragraph(paragraph, max_width));
            }
        }
        if all_lines.is_empty() {
            all_lines.push(String::new());
        }
        all_lines
    }

    /// Calculate dynamic modal width based on content size.
    /// Fits content without wrapping, but respects screen size limits.
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
                match value {
                    ModalValue::Text(text) => {
                        // Account for spinner characters if value contains "calculating"
                        let t = i18n::t();
                        let extra = if text.contains(t.file_info_calculating()) {
                            2 // spinner char + space
                        } else {
                            0
                        };
                        // Max width of any single line (split by \n)
                        text.split('\n').map(|line| line.width()).max().unwrap_or(0) + extra
                    }
                    ModalValue::Segments(segments) => {
                        segments.iter().map(|s| s.text.width()).sum::<usize>()
                    }
                }
            })
            .max()
            .unwrap_or(0);

        // Calculate required width:
        // padding (4) + borders (2) + key + separator (2) + value
        let content_width = 6 + max_key_len + 2 + max_value_len;

        // Apply constraints
        let max_width = (screen_width as f32 * MODAL_MAX_WIDTH_PERCENTAGE_WIDE) as u16;
        let base = (content_width as u16).max(MODAL_MIN_WIDTH_WIDE);
        let base = if let Some(mw) = self.min_width {
            base.max(mw)
        } else {
            base
        };
        base.min(max_width).min(screen_width)
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
        // modal_width - borders (2) - padding (4) - key_width - separator (2)
        let available_value_width = modal_width
            .saturating_sub(6) // borders + padding
            .saturating_sub(max_key_len as u16)
            .saturating_sub(2) // separator
            .max(MODAL_MIN_VALUE_WIDTH as u16) as usize;

        let t = i18n::t();
        let key_style = Style::default()
            .fg(theme.accented_fg)
            .add_modifier(Modifier::BOLD);

        // Build a flat list of all rendered lines first (with wrapping applied).
        // Scrolling operates on this list; no truncation is performed.
        let mut all_lines: Vec<Line<'_>> = Vec::new();
        for (key, value) in &self.lines {
            let padding = " ".repeat(max_key_len - key.width());
            match value {
                ModalValue::Text(text) => {
                    let display_value = if text.contains(t.file_info_calculating()) {
                        format!("{} {}", self.get_spinner_char(), text)
                    } else {
                        text.clone()
                    };
                    let wrapped_values = Self::wrap_text(&display_value, available_value_width);
                    if wrapped_values.is_empty() {
                        continue;
                    }
                    all_lines.push(Line::from(vec![
                        Span::styled(format!("  {}{}", key, padding), key_style),
                        Span::raw("  "),
                        Span::styled(wrapped_values[0].clone(), Style::default().fg(theme.fg)),
                    ]));
                    let indent = " ".repeat(max_key_len + 4);
                    for wrapped_line in wrapped_values.iter().skip(1) {
                        all_lines.push(Line::from(vec![Span::styled(
                            format!("{}{}", indent, wrapped_line),
                            Style::default().fg(theme.fg),
                        )]));
                    }
                }
                ModalValue::Segments(segments) => {
                    let separator = "  ";
                    let mut spans = vec![
                        Span::styled(format!("  {}{}", key, padding), key_style),
                        Span::raw(separator),
                    ];
                    for segment in segments {
                        let color = match segment.style {
                            SegmentStyle::Default => theme.fg,
                            SegmentStyle::Success => theme.success,
                            SegmentStyle::Error => theme.error,
                            SegmentStyle::Warning => theme.warning,
                            SegmentStyle::Disabled => theme.disabled,
                        };
                        spans.push(Span::styled(
                            segment.text.clone(),
                            Style::default().fg(color),
                        ));
                    }
                    all_lines.push(Line::from(spans));
                }
            }
        }

        let total_lines = all_lines.len();

        // Cap the content area height at the available screen height.
        // reserve for borders, spacing, button: same 7 rows as before.
        let max_data_lines = area.height.saturating_sub(7) as usize;
        let visible = total_lines.min(max_data_lines.max(1));

        // Clamp scroll offset so it can never exceed the amount of content hidden.
        let max_scroll = total_lines.saturating_sub(visible);
        if self.scroll_offset > max_scroll {
            self.scroll_offset = max_scroll;
        }

        // Cache for keyboard / mouse handlers (they don't know the render area).
        self.cached_total_lines = total_lines;
        self.cached_visible = visible;

        // Calculate required height based on visible window:
        // 1 (top border) + 1 (empty line) + N (visible data lines) +
        // 1 (empty line) + optional 1 (button) + 1 (bottom border)
        let button_height = if self.show_button { 1u16 } else { 0 };
        let modal_height = (visible as u16) + 4 + button_height;

        // Position: anchored or centered
        let modal_area = if let Some((ax, ay)) = self.anchor {
            let x = ax.min(area.width.saturating_sub(modal_width));
            let y = if self.anchor_bottom {
                // Bottom anchor: modal grows upward from ay
                ay.saturating_sub(modal_height)
            } else {
                ay
            }
            .min(area.height.saturating_sub(modal_height));
            Rect {
                x,
                y,
                width: modal_width,
                height: modal_height,
            }
        } else {
            centered_rect_with_size(modal_width, modal_height, area)
        };
        self.last_modal_area = Some(modal_area);

        let inner = render_modal_block(modal_area, buf, &self.title, theme);

        // Split into: empty line, data, empty line, optional button
        let mut constraints = vec![
            Constraint::Length(1),              // Empty line at top
            Constraint::Length(visible as u16), // Visible data window
            Constraint::Length(1),              // Empty line
        ];
        if self.show_button {
            constraints.push(Constraint::Length(1)); // Button
        }
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        // Slice the visible window from the full content.
        let visible_slice: Vec<Line<'_>> = all_lines
            .into_iter()
            .skip(self.scroll_offset)
            .take(visible)
            .collect();

        let data = Paragraph::new(visible_slice).alignment(Alignment::Left);
        data.render(chunks[1], buf);

        // Scrollbar on the right border (same pattern as panels & dropdown).
        if ScrollBar::needs_scrollbar(visible, total_lines) {
            let theme_colors = ThemeColors::from(theme);
            ScrollBar::render(
                buf,
                modal_area.x + modal_area.width - 1,
                chunks[1].y,
                chunks[1].height,
                self.scroll_offset,
                visible,
                total_lines,
                &theme_colors,
                true,
            );
        }

        self.last_content_area = Some(chunks[1]);

        // Render Close button (conditionally)
        if self.show_button {
            let close_button = Line::from(vec![Span::styled(
                format!("[ {} ]", t.ui_close()),
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.fg)
                    .add_modifier(Modifier::BOLD),
            )]);

            let button_paragraph = Paragraph::new(close_button).alignment(Alignment::Center);
            button_paragraph.render(chunks[3], buf);

            // Save button area for mouse handling
            self.last_button_area = Some(chunks[3]);
        } else {
            self.last_button_area = None;
        }
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        // No text input here: letter keys are shortcuts, matched on the
        // layout-normalized form so they work on a Cyrillic layout too.
        let key = chord.canonical;
        let max_scroll = self.cached_total_lines.saturating_sub(self.cached_visible);
        let page = self.cached_visible.max(1);

        match key.code {
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
            KeyCode::Enter => Ok(Some(ModalResult::Confirmed(()))),
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                Ok(None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll_offset = (self.scroll_offset + 1).min(max_scroll);
                Ok(None)
            }
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(page);
                Ok(None)
            }
            KeyCode::PageDown => {
                self.scroll_offset = (self.scroll_offset + page).min(max_scroll);
                Ok(None)
            }
            KeyCode::Home => {
                self.scroll_offset = 0;
                Ok(None)
            }
            KeyCode::End => {
                self.scroll_offset = max_scroll;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        use crossterm::event::MouseEventKind;

        let max_scroll = self.cached_total_lines.saturating_sub(self.cached_visible);

        // Mouse wheel scrolling over the content area (or anywhere inside the modal).
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(3);
                return Ok(None);
            }
            MouseEventKind::ScrollDown => {
                self.scroll_offset = (self.scroll_offset + 3).min(max_scroll);
                return Ok(None);
            }
            _ => {}
        }

        // A click beside the modal closes it, as Esc does.
        if is_click_outside(&mouse, self.last_modal_area) {
            return Ok(Some(ModalResult::Cancelled));
        }

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
        let button_width = button_text.width() as u16;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseEvent};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use std::sync::Once;

    static INIT_I18N: Once = Once::new();

    fn setup_i18n() {
        INIT_I18N.call_once(|| {
            let _ = i18n::init();
        });
    }

    fn many_lines(n: usize) -> Vec<(String, String)> {
        (0..n)
            .map(|i| (String::new(), format!("line {i}")))
            .collect()
    }

    fn wheel(kind: crossterm::event::MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::empty(),
        }
    }

    fn press(code: KeyCode) -> termide_core::KeyChord {
        let ev = KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        termide_core::KeyChord::identity(ev)
    }

    #[test]
    fn keys_are_divided_from_values_by_spaces() {
        setup_i18n();
        let mut modal = InfoModal::new("Info", vec![("Model".into(), "gpt".into())]);
        let theme = Theme::default();
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        modal.render(area, &mut buf, &theme);
        let text = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("Model  gpt") && !text.contains("Model:"),
            "{text}"
        );
    }

    #[test]
    fn mouse_wheel_scrolls_when_content_overflows() {
        // Construct a modal with more lines than the render window can show,
        // render once so cached_total_lines / cached_visible are populated,
        // then dispatch ScrollDown and verify scroll_offset moved.
        setup_i18n();
        let mut modal = InfoModal::new("Report", many_lines(200));
        let theme = Theme::default();
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        modal.render(area, &mut buf, &theme);

        assert!(
            modal.cached_total_lines > modal.cached_visible,
            "test scaffold: content must exceed visible window \
             (total={}, visible={})",
            modal.cached_total_lines,
            modal.cached_visible
        );

        let before = modal.scroll_offset;
        let _ = modal
            .handle_mouse(wheel(crossterm::event::MouseEventKind::ScrollDown), area)
            .expect("handle_mouse returns Ok");
        assert!(
            modal.scroll_offset > before,
            "ScrollDown should advance scroll_offset: before={}, after={}",
            before,
            modal.scroll_offset
        );

        let mid = modal.scroll_offset;
        let _ = modal
            .handle_mouse(wheel(crossterm::event::MouseEventKind::ScrollUp), area)
            .expect("handle_mouse returns Ok");
        assert!(
            modal.scroll_offset < mid,
            "ScrollUp should rewind scroll_offset: mid={}, after={}",
            mid,
            modal.scroll_offset
        );
    }

    #[test]
    fn keyboard_and_mouse_wheel_agree() {
        setup_i18n();
        let mut modal = InfoModal::new("Report", many_lines(200));
        let theme = Theme::default();
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        modal.render(area, &mut buf, &theme);

        let _ = modal.handle_key(press(KeyCode::Down)).unwrap();
        let after_key = modal.scroll_offset;
        assert!(after_key > 0, "keyboard Down should advance scroll_offset");

        let _ = modal
            .handle_mouse(wheel(crossterm::event::MouseEventKind::ScrollDown), area)
            .unwrap();
        assert!(
            modal.scroll_offset > after_key,
            "ScrollDown after keyboard should advance further: key={}, mouse={}",
            after_key,
            modal.scroll_offset
        );
    }
}
