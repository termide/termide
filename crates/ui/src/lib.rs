//! UI components for termide.
//!
//! Provides reusable UI widgets and layout utilities.

pub mod config;
pub mod path_utils;
pub mod system_monitor;

// Re-exports for convenience
pub use termide_clipboard as clipboard;
pub use termide_config::constants;

use anyhow::Result;
use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction as LayoutDirection, Layout, Rect},
};

/// Layout direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    Horizontal,
    Vertical,
}

// ===== Modal System =====

/// Modal window result (generic version).
#[derive(Debug, Clone)]
pub enum ModalResult<T> {
    /// User confirmed the action with a result.
    Confirmed(T),
    /// User cancelled the action.
    Cancelled,
}

/// Trait for all modal windows.
///
/// Generic modal trait that doesn't depend on Theme - implementations
/// receive styling information through render method parameters.
pub trait Modal {
    /// Modal window result type.
    type Result;

    /// Render the modal window.
    ///
    /// The modal should render itself using provided styles.
    fn render(&mut self, area: Rect, buf: &mut Buffer);

    /// Handle keyboard event.
    /// Returns Some(result) if the modal window should close.
    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>>;

    /// Handle mouse event.
    /// Returns Some(result) if the modal window should close.
    fn handle_mouse(
        &mut self,
        _mouse: MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        Ok(None) // Default: do nothing
    }
}

// ===== Modal Width Calculation =====

/// Default modal size constants.
pub mod modal_constants {
    /// Minimum modal width (default).
    pub const MIN_WIDTH_DEFAULT: u16 = 30;
    /// Minimum modal width (wide).
    pub const MIN_WIDTH_WIDE: u16 = 40;
    /// Maximum width as percentage of screen (default: 75%).
    pub const MAX_WIDTH_PERCENTAGE_DEFAULT: f32 = 0.75;
    /// Maximum width as percentage of screen (wide: 90%).
    pub const MAX_WIDTH_PERCENTAGE_WIDE: f32 = 0.90;
    /// Padding with single border.
    pub const PADDING_WITH_BORDER: u16 = 6;
    /// Padding with double border.
    pub const PADDING_WITH_DOUBLE_BORDER: u16 = 8;
    /// Button spacing in modal dialogs.
    pub const BUTTON_SPACING: u16 = 4;
}

/// Configuration for modal width calculation.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModalWidthConfig {
    /// Use wide modal constraints (90% max vs 75% default).
    pub wide: bool,
    /// Use double border padding (8px vs 6px).
    pub double_border: bool,
}

impl ModalWidthConfig {
    /// Create config for wide modals with double border.
    pub fn wide() -> Self {
        Self {
            wide: true,
            double_border: true,
        }
    }
}

/// Calculate modal width based on content and screen constraints.
///
/// This consolidates the common modal width calculation pattern:
/// 1. Takes the maximum of all content widths
/// 2. Adds appropriate padding
/// 3. Applies min/max constraints
pub fn calculate_modal_width(
    content_widths: impl Iterator<Item = u16>,
    screen_width: u16,
    config: ModalWidthConfig,
) -> u16 {
    let content_width = content_widths.max().unwrap_or(0);

    let padding = if config.double_border {
        modal_constants::PADDING_WITH_DOUBLE_BORDER
    } else {
        modal_constants::PADDING_WITH_BORDER
    };

    let total_width = content_width + padding;

    let (max_percentage, min_width) = if config.wide {
        (
            modal_constants::MAX_WIDTH_PERCENTAGE_WIDE,
            modal_constants::MIN_WIDTH_WIDE,
        )
    } else {
        (
            modal_constants::MAX_WIDTH_PERCENTAGE_DEFAULT,
            modal_constants::MIN_WIDTH_DEFAULT,
        )
    };

    let max_width = (screen_width as f32 * max_percentage) as u16;

    total_width.max(min_width).min(max_width).min(screen_width)
}

/// Calculate maximum line width from multiline text.
pub fn max_line_width(text: &str) -> u16 {
    text.lines().map(|line| line.len()).max().unwrap_or(0) as u16
}

/// Calculate maximum item width from a list of strings with optional prefix.
pub fn max_item_width(items: &[String], prefix_len: usize) -> u16 {
    items
        .iter()
        .map(|item| prefix_len + item.len())
        .max()
        .unwrap_or(0) as u16
}

/// Text input handler with cursor management
///
/// This utility handles common text input operations for modal windows,
/// including character insertion, deletion, and cursor navigation.
/// It properly handles UTF-8 multi-byte characters by tracking cursor
/// position in characters (not bytes).
#[derive(Debug, Clone)]
pub struct TextInput {
    input: String,
    cursor_pos: usize, // Position in characters, not bytes
}

impl TextInput {
    /// Create a new text input handler with empty input
    pub fn new() -> Self {
        Self {
            input: String::new(),
            cursor_pos: 0,
        }
    }

    /// Create a text input handler with default value
    pub fn with_text(text: impl Into<String>) -> Self {
        let input = text.into();
        let cursor_pos = input.chars().count();
        Self { input, cursor_pos }
    }

    /// Alias for with_text - backward compatibility.
    pub fn with_default(text: impl Into<String>) -> Self {
        Self::with_text(text)
    }

    /// Get the current input text
    pub fn text(&self) -> &str {
        &self.input
    }

    /// Get the cursor position (in characters)
    pub fn cursor_pos(&self) -> usize {
        self.cursor_pos
    }

    /// Set the input text and move cursor to end
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.input = text.into();
        self.cursor_pos = self.input.chars().count();
    }

    /// Clear all input
    pub fn clear(&mut self) {
        self.input.clear();
        self.cursor_pos = 0;
    }

    /// Convert cursor position (in characters) to byte index
    fn byte_index(&self) -> usize {
        self.input
            .char_indices()
            .nth(self.cursor_pos)
            .map(|(idx, _)| idx)
            .unwrap_or(self.input.len())
    }

    /// Insert a character at the cursor position
    pub fn insert(&mut self, c: char) {
        let byte_idx = self.byte_index();
        self.input.insert(byte_idx, c);
        self.cursor_pos += 1;
    }

    /// Alias for insert - backward compatibility.
    #[inline]
    pub fn insert_char(&mut self, c: char) {
        self.insert(c);
    }

    /// Delete character before cursor (backspace)
    pub fn backspace(&mut self) -> bool {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
            let byte_idx = self.byte_index();
            self.input.remove(byte_idx);
            true
        } else {
            false
        }
    }

    /// Delete character at cursor (delete key)
    pub fn delete(&mut self) -> bool {
        let char_count = self.input.chars().count();
        if self.cursor_pos < char_count {
            let byte_idx = self.byte_index();
            self.input.remove(byte_idx);
            true
        } else {
            false
        }
    }

    /// Move cursor left
    pub fn move_left(&mut self) -> bool {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
            true
        } else {
            false
        }
    }

    /// Move cursor right
    pub fn move_right(&mut self) -> bool {
        let char_count = self.input.chars().count();
        if self.cursor_pos < char_count {
            self.cursor_pos += 1;
            true
        } else {
            false
        }
    }

    /// Move cursor to start (Home)
    pub fn move_home(&mut self) {
        self.cursor_pos = 0;
    }

    /// Move cursor to end (End)
    pub fn move_end(&mut self) {
        self.cursor_pos = self.input.chars().count();
    }

    /// Check if input is empty
    pub fn is_empty(&self) -> bool {
        self.input.is_empty()
    }

    /// Get text before cursor (for rendering)
    pub fn text_before_cursor(&self) -> &str {
        let byte_idx = self.byte_index();
        &self.input[..byte_idx]
    }

    /// Get text after cursor (for rendering)
    pub fn text_after_cursor(&self) -> &str {
        let byte_idx = self.byte_index();
        &self.input[byte_idx..]
    }
}

impl Default for TextInput {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a centered rectangle with specified width and height within a container
///
/// This utility function is used by modal dialogs to center themselves on screen.
/// It calculates horizontal and vertical margins and uses ratatui's Layout system
/// to create a properly centered rectangle.
pub fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    centered_rect_with_size(width, height, r)
}

/// Alias for centered_rect - backward compatibility.
#[inline]
pub fn centered_rect_with_size(width: u16, height: u16, r: Rect) -> Rect {
    // Calculate margins
    let horizontal_margin = r.width.saturating_sub(width) / 2;
    let vertical_margin = r.height.saturating_sub(height) / 2;

    let vertical_layout = Layout::default()
        .direction(LayoutDirection::Vertical)
        .constraints([
            Constraint::Length(vertical_margin),
            Constraint::Length(height),
            Constraint::Length(vertical_margin),
        ])
        .split(r);

    Layout::default()
        .direction(LayoutDirection::Horizontal)
        .constraints([
            Constraint::Length(horizontal_margin),
            Constraint::Length(width),
            Constraint::Length(horizontal_margin),
        ])
        .split(vertical_layout[1])[1]
}

/// Center a smaller rect inside a larger rect (simple version).
pub fn center_rect(outer: Rect, width: u16, height: u16) -> Rect {
    let x = outer.x + outer.width.saturating_sub(width) / 2;
    let y = outer.y + outer.height.saturating_sub(height) / 2;
    Rect::new(x, y, width.min(outer.width), height.min(outer.height))
}

/// Create a rect with margin.
pub fn with_margin(rect: Rect, margin: u16) -> Rect {
    Rect::new(
        rect.x + margin,
        rect.y + margin,
        rect.width.saturating_sub(margin * 2),
        rect.height.saturating_sub(margin * 2),
    )
}

/// Calculate percentage of a value.
pub fn percentage(value: u16, percent: u16) -> u16 {
    (value as u32 * percent as u32 / 100) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_input_new() {
        let input = TextInput::new();
        assert_eq!(input.text(), "");
        assert_eq!(input.cursor_pos(), 0);
    }

    #[test]
    fn test_text_input_with_text() {
        let input = TextInput::with_text("hello");
        assert_eq!(input.text(), "hello");
        assert_eq!(input.cursor_pos(), 5);
    }

    #[test]
    fn test_text_input_insert() {
        let mut input = TextInput::new();
        input.insert('a');
        input.insert('b');
        assert_eq!(input.text(), "ab");
        assert_eq!(input.cursor_pos(), 2);
    }

    #[test]
    fn test_text_input_unicode() {
        let mut input = TextInput::new();
        input.insert('п');
        input.insert('р');
        input.insert('и');
        assert_eq!(input.text(), "при");
        assert_eq!(input.cursor_pos(), 3);
    }

    #[test]
    fn test_text_input_backspace() {
        let mut input = TextInput::with_text("abc");
        assert!(input.backspace());
        assert_eq!(input.text(), "ab");
        assert_eq!(input.cursor_pos(), 2);
    }

    #[test]
    fn test_text_input_delete() {
        let mut input = TextInput::with_text("abc");
        input.move_home();
        assert!(input.delete());
        assert_eq!(input.text(), "bc");
        assert_eq!(input.cursor_pos(), 0);
    }

    #[test]
    fn test_text_input_navigation() {
        let mut input = TextInput::with_text("abc");
        input.move_home();
        assert_eq!(input.cursor_pos(), 0);

        assert!(input.move_right());
        assert_eq!(input.cursor_pos(), 1);

        assert!(input.move_left());
        assert_eq!(input.cursor_pos(), 0);

        input.move_end();
        assert_eq!(input.cursor_pos(), 3);
    }

    #[test]
    fn test_center_rect() {
        let outer = Rect::new(0, 0, 100, 50);
        let inner = center_rect(outer, 20, 10);
        assert_eq!(inner.x, 40);
        assert_eq!(inner.y, 20);
        assert_eq!(inner.width, 20);
        assert_eq!(inner.height, 10);
    }

    #[test]
    fn test_with_margin() {
        let rect = Rect::new(10, 10, 100, 50);
        let margined = with_margin(rect, 5);
        assert_eq!(margined.x, 15);
        assert_eq!(margined.y, 15);
        assert_eq!(margined.width, 90);
        assert_eq!(margined.height, 40);
    }
}
