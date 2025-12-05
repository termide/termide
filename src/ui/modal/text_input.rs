/// Text input handler with cursor management
///
/// This utility handles common text input operations for modal windows,
/// including character insertion, deletion, and cursor navigation.
/// It properly handles UTF-8 multi-byte characters by tracking cursor
/// position in characters (not bytes).
#[derive(Debug, Clone)]
pub struct TextInputHandler {
    input: String,
    cursor_pos: usize, // Position in characters, not bytes
}

impl TextInputHandler {
    /// Create a new text input handler with empty input
    pub fn new() -> Self {
        Self {
            input: String::new(),
            cursor_pos: 0,
        }
    }

    /// Create a text input handler with default value
    pub fn with_default(default: impl Into<String>) -> Self {
        let input = default.into();
        let cursor_pos = input.chars().count();
        Self { input, cursor_pos }
    }

    /// Get the current input text
    pub fn text(&self) -> &str {
        &self.input
    }

    /// Get the cursor position (in characters)
    #[allow(dead_code)] // Used by other modals
    pub fn cursor_pos(&self) -> usize {
        self.cursor_pos
    }

    /// Set the input text and move cursor to end
    #[allow(dead_code)] // Used by other modals
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.input = text.into();
        self.cursor_pos = self.input.chars().count();
    }

    /// Clear all input
    #[allow(dead_code)] // Used by other modals
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
    pub fn insert_char(&mut self, c: char) {
        let byte_idx = self.byte_index();
        self.input.insert(byte_idx, c);
        self.cursor_pos += 1;
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

impl Default for TextInputHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let handler = TextInputHandler::new();
        assert_eq!(handler.text(), "");
        assert_eq!(handler.cursor_pos(), 0);
    }

    #[test]
    fn test_with_default() {
        let handler = TextInputHandler::with_default("hello");
        assert_eq!(handler.text(), "hello");
        assert_eq!(handler.cursor_pos(), 5);
    }

    #[test]
    fn test_insert_char() {
        let mut handler = TextInputHandler::new();
        handler.insert_char('a');
        handler.insert_char('b');
        assert_eq!(handler.text(), "ab");
        assert_eq!(handler.cursor_pos(), 2);
    }

    #[test]
    fn test_insert_unicode() {
        let mut handler = TextInputHandler::new();
        handler.insert_char('п'); // Cyrillic
        handler.insert_char('р');
        handler.insert_char('и');
        assert_eq!(handler.text(), "при");
        assert_eq!(handler.cursor_pos(), 3);
    }

    #[test]
    fn test_backspace() {
        let mut handler = TextInputHandler::with_default("abc");
        assert!(handler.backspace());
        assert_eq!(handler.text(), "ab");
        assert_eq!(handler.cursor_pos(), 2);
    }

    #[test]
    fn test_backspace_empty() {
        let mut handler = TextInputHandler::new();
        assert!(!handler.backspace());
    }

    #[test]
    fn test_delete() {
        let mut handler = TextInputHandler::with_default("abc");
        handler.move_home();
        assert!(handler.delete());
        assert_eq!(handler.text(), "bc");
        assert_eq!(handler.cursor_pos(), 0);
    }

    #[test]
    fn test_cursor_movement() {
        let mut handler = TextInputHandler::with_default("abc");
        handler.move_home();
        assert_eq!(handler.cursor_pos(), 0);

        assert!(handler.move_right());
        assert_eq!(handler.cursor_pos(), 1);

        assert!(handler.move_left());
        assert_eq!(handler.cursor_pos(), 0);

        assert!(!handler.move_left()); // Can't go further left

        handler.move_end();
        assert_eq!(handler.cursor_pos(), 3);

        assert!(!handler.move_right()); // Can't go further right
    }

    #[test]
    fn test_text_before_after_cursor() {
        let mut handler = TextInputHandler::with_default("hello");
        handler.cursor_pos = 2;

        assert_eq!(handler.text_before_cursor(), "he");
        assert_eq!(handler.text_after_cursor(), "llo");
    }

    #[test]
    fn test_clear() {
        let mut handler = TextInputHandler::with_default("test");
        handler.clear();
        assert_eq!(handler.text(), "");
        assert_eq!(handler.cursor_pos(), 0);
    }
}
