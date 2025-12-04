use anyhow::{Context, Result};
use ropey::Rope;
use std::path::{Path, PathBuf};
use unicode_segmentation::UnicodeSegmentation;

use super::{Action, Cursor, History};

/// Text buffer based on Rope for efficient work with large files
#[derive(Debug, Clone)]
pub struct TextBuffer {
    /// Rope structure for storing text
    rope: Rope,
    /// File path (if exists)
    file_path: Option<PathBuf>,
    /// Modified flag
    modified: bool,
    /// Line ending type (for saving)
    line_ending: LineEnding,
    /// Edit history for undo/redo
    history: History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms)]
pub enum LineEnding {
    LF,   // Unix \n
    CRLF, // Windows \r\n
}

impl TextBuffer {
    /// Create a new empty buffer
    pub fn new() -> Self {
        Self {
            rope: Rope::new(),
            file_path: None,
            modified: false,
            line_ending: LineEnding::LF,
            history: History::new(),
        }
    }

    /// Create buffer from Rope (for use from Editor::from_text)
    pub fn from_rope(rope: Rope) -> Self {
        Self {
            rope,
            file_path: None,
            modified: false,
            line_ending: LineEnding::LF,
            history: History::new(),
        }
    }

    /// Load file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read file: {}", path.display()))?;

        // Determine line ending type
        let line_ending = if contents.contains("\r\n") {
            LineEnding::CRLF
        } else {
            LineEnding::LF
        };

        // Rope automatically normalizes line endings to \n
        let rope = Rope::from_str(&contents);

        Ok(Self {
            rope,
            file_path: Some(path.to_path_buf()),
            modified: false,
            line_ending,
            history: History::new(),
        })
    }

    /// Save file
    pub fn save(&mut self) -> Result<()> {
        if let Some(path) = self.file_path.clone() {
            self.save_to(&path)?;
            self.modified = false;
            Ok(())
        } else {
            anyhow::bail!("No file path set")
        }
    }

    /// Save to specified file
    pub fn save_to<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let path = path.as_ref();
        let mut contents = String::new();

        // Collect text with appropriate line endings
        // rope.lines() returns lines with '\n' at the end (except possibly the last line)
        // We need to replace '\n' with the appropriate line ending
        for line in self.rope.lines() {
            let line_str = line.to_string();

            // If line ends with '\n', replace it with the appropriate line ending
            if line_str.ends_with('\n') {
                let line_without_newline = &line_str[..line_str.len() - 1];
                contents.push_str(line_without_newline);
                match self.line_ending {
                    LineEnding::LF => contents.push('\n'),
                    LineEnding::CRLF => contents.push_str("\r\n"),
                }
            } else {
                // Last line without '\n' - add as is
                contents.push_str(&line_str);
            }
        }

        std::fs::write(path, contents)
            .with_context(|| format!("Failed to write file: {}", path.display()))?;

        self.file_path = Some(path.to_path_buf());
        self.modified = false;
        Ok(())
    }

    /// Check if buffer content differs from file on disk
    fn is_content_modified(&self) -> Result<bool> {
        // If no file path, use current modified flag
        let Some(path) = &self.file_path else {
            return Ok(self.modified);
        };

        // Try to read file content
        match std::fs::read_to_string(path) {
            Ok(file_content) => {
                // Compare buffer content with file content
                let buffer_content = self.rope.to_string();
                Ok(buffer_content != file_content)
            }
            Err(_) => {
                // If can't read file (deleted, permissions, etc.), keep current flag
                Ok(self.modified)
            }
        }
    }

    /// Get line count
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Get line by index
    pub fn line(&self, index: usize) -> Option<String> {
        if index < self.line_count() {
            Some(self.rope.line(index).to_string())
        } else {
            None
        }
    }

    /// Get line length in graphemes (without newline character)
    pub fn line_len_graphemes(&self, line_idx: usize) -> usize {
        if let Some(line) = self.line(line_idx) {
            // Remove newline character before counting
            line.trim_end_matches('\n').graphemes(true).count()
        } else {
            0
        }
    }

    /// Get all text
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Insert text at cursor position
    pub fn insert(&mut self, cursor: &Cursor, text: &str) -> Result<Cursor> {
        let char_idx = self.cursor_to_char_idx(cursor)?;
        self.rope.insert(char_idx, text);
        self.modified = true;

        // Record to history
        self.history.push(Action::Insert {
            position: *cursor,
            text: text.to_string(),
        });

        // Calculate new cursor position after insertion
        let new_cursor = self.advance_cursor(cursor, text);
        Ok(new_cursor)
    }

    /// Delete character at cursor position (delete)
    pub fn delete_char(&mut self, cursor: &Cursor) -> Result<bool> {
        let char_idx = self.cursor_to_char_idx(cursor)?;

        // Check if there is something to delete
        if char_idx >= self.rope.len_chars() {
            return Ok(false);
        }

        // Get deleted character for history
        let deleted_char = self.rope.char(char_idx).to_string();

        // Delete one character
        self.rope.remove(char_idx..char_idx + 1);
        self.modified = true;

        // Record to history
        self.history.push(Action::Delete {
            position: *cursor,
            text: deleted_char,
        });

        Ok(true)
    }

    /// Delete character before cursor (backspace)
    pub fn backspace(&mut self, cursor: &Cursor) -> Result<Option<Cursor>> {
        if cursor.line == 0 && cursor.column == 0 {
            return Ok(None);
        }

        let char_idx = self.cursor_to_char_idx(cursor)?;

        if char_idx == 0 {
            return Ok(None);
        }

        // Get deleted character for history
        let deleted_char = self.rope.char(char_idx - 1).to_string();

        // Calculate new cursor position
        let new_cursor = if cursor.column > 0 {
            Cursor::at(cursor.line, cursor.column - 1)
        } else {
            // Move to previous line
            let prev_line_len = self.line_len_graphemes(cursor.line - 1);
            Cursor::at(cursor.line - 1, prev_line_len)
        };

        // Delete character before cursor
        self.rope.remove(char_idx - 1..char_idx);
        self.modified = true;

        // Record to history (position is the new cursor position after deletion)
        self.history.push(Action::Delete {
            position: new_cursor,
            text: deleted_char,
        });

        Ok(Some(new_cursor))
    }

    /// Delete text range
    pub fn delete_range(&mut self, start: &Cursor, end: &Cursor) -> Result<()> {
        let start_idx = self.cursor_to_char_idx(start)?;
        let end_idx = self.cursor_to_char_idx(end)?;

        if start_idx < end_idx {
            // Get deleted text for history
            let deleted_text: String = self.rope.slice(start_idx..end_idx).to_string();

            // Delete text
            self.rope.remove(start_idx..end_idx);
            self.modified = true;

            // Record to history
            self.history.push(Action::Delete {
                position: *start,
                text: deleted_text,
            });
        }

        Ok(())
    }

    /// Convert cursor position to character index in Rope
    fn cursor_to_char_idx(&self, cursor: &Cursor) -> Result<usize> {
        if cursor.line >= self.line_count() {
            anyhow::bail!("Line {} out of range", cursor.line);
        }

        let line_start = self.rope.line_to_char(cursor.line);
        let line = self.rope.line(cursor.line);
        let line_str = line.to_string();

        // Calculate position in bytes for column graphemes
        let mut grapheme_count = 0;
        let mut byte_pos = 0;

        #[allow(clippy::explicit_counter_loop)]
        for grapheme in line_str.graphemes(true) {
            if grapheme_count >= cursor.column {
                break;
            }
            byte_pos += grapheme.len();
            grapheme_count += 1;
        }

        // Convert byte position to character position
        let char_offset = line_str[..byte_pos].chars().count();
        Ok(line_start + char_offset)
    }

    /// Advance cursor after text insertion
    fn advance_cursor(&self, cursor: &Cursor, text: &str) -> Cursor {
        let lines: Vec<&str> = text.lines().collect();

        if lines.is_empty() || (lines.len() == 1 && text.ends_with('\n')) {
            // Only newline
            Cursor::at(cursor.line + 1, 0)
        } else if lines.len() == 1 {
            // Single line without newline
            let graphemes = text.graphemes(true).count();
            Cursor::at(cursor.line, cursor.column + graphemes)
        } else {
            // Multiple lines
            let last_line = lines.last().unwrap();
            let last_line_len = last_line.graphemes(true).count();
            Cursor::at(cursor.line + lines.len() - 1, last_line_len)
        }
    }

    /// Check if buffer is modified
    pub fn is_modified(&self) -> bool {
        self.modified
    }

    /// Set modified flag
    #[allow(dead_code)]
    pub fn set_modified(&mut self, modified: bool) {
        self.modified = modified;
    }

    /// Get file path
    pub fn file_path(&self) -> Option<&Path> {
        self.file_path.as_deref()
    }

    /// Set file path
    #[allow(dead_code)]
    pub fn set_file_path<P: AsRef<Path>>(&mut self, path: P) {
        self.file_path = Some(path.as_ref().to_path_buf());
    }

    /// Get file name
    #[allow(dead_code)]
    pub fn file_name(&self) -> Option<&str> {
        self.file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
    }

    /// Get buffer contents as string
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        self.rope.to_string()
    }

    /// Undo last action
    pub fn undo(&mut self) -> Result<Option<Cursor>> {
        if let Some(action) = self.history.undo() {
            let cursor = self.apply_action(&action)?;
            // Check if buffer content actually differs from file
            self.modified = self.is_content_modified()?;
            Ok(Some(cursor))
        } else {
            Ok(None)
        }
    }

    /// Redo undone action
    pub fn redo(&mut self) -> Result<Option<Cursor>> {
        if let Some(action) = self.history.redo() {
            let cursor = self.apply_action(&action)?;
            // Check if buffer content actually differs from file
            self.modified = self.is_content_modified()?;
            Ok(Some(cursor))
        } else {
            Ok(None)
        }
    }

    /// Apply action to buffer (for undo/redo)
    fn apply_action(&mut self, action: &Action) -> Result<Cursor> {
        match action {
            Action::Insert { position, text } => {
                let char_idx = self.cursor_to_char_idx(position)?;
                self.rope.insert(char_idx, text);
                let new_cursor = self.advance_cursor(position, text);
                Ok(new_cursor)
            }
            Action::Delete { position, text } => {
                let char_idx = self.cursor_to_char_idx(position)?;
                let end_idx = char_idx + text.chars().count();
                self.rope.remove(char_idx..end_idx);
                Ok(*position)
            }
            Action::Group { actions } => {
                let mut cursor = Cursor::new();
                for action in actions {
                    cursor = self.apply_action(action)?;
                }
                Ok(cursor)
            }
        }
    }

    /// Check if undo is possible
    #[allow(dead_code)]
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// Check if redo is possible
    #[allow(dead_code)]
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }
}

impl Default for TextBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_buffer() {
        let buf = TextBuffer::new();
        assert_eq!(buf.line_count(), 1); // Rope always has at least 1 line
        assert!(!buf.is_modified());
    }

    #[test]
    fn test_insert_single_char() {
        let mut buf = TextBuffer::new();
        let cursor = Cursor::at(0, 0);

        let new_cursor = buf.insert(&cursor, "a").unwrap();
        assert_eq!(new_cursor, Cursor::at(0, 1));
        assert_eq!(buf.text(), "a");
        assert!(buf.is_modified());
    }

    #[test]
    fn test_insert_newline() {
        let mut buf = TextBuffer::new();
        let cursor = Cursor::at(0, 0);

        let new_cursor = buf.insert(&cursor, "hello\nworld").unwrap();
        assert_eq!(new_cursor, Cursor::at(1, 5));
        assert_eq!(buf.line_count(), 2);
        assert_eq!(buf.line(0).unwrap(), "hello\n");
        assert_eq!(buf.line(1).unwrap(), "world");
    }

    #[test]
    fn test_backspace() {
        let mut buf = TextBuffer::new();
        buf.insert(&Cursor::at(0, 0), "hello").unwrap();

        let cursor = Cursor::at(0, 5);
        let new_cursor = buf.backspace(&cursor).unwrap().unwrap();

        assert_eq!(new_cursor, Cursor::at(0, 4));
        assert_eq!(buf.text(), "hell");
    }

    #[test]
    fn test_delete_char() {
        let mut buf = TextBuffer::new();
        buf.insert(&Cursor::at(0, 0), "hello").unwrap();

        let cursor = Cursor::at(0, 0);
        let deleted = buf.delete_char(&cursor).unwrap();

        assert!(deleted);
        assert_eq!(buf.text(), "ello");
    }

    #[test]
    fn test_unicode_handling() {
        let mut buf = TextBuffer::new();
        buf.insert(&Cursor::at(0, 0), "hello").unwrap();

        assert_eq!(buf.line_len_graphemes(0), 5);

        let cursor = Cursor::at(0, 3);
        let char_idx = buf.cursor_to_char_idx(&cursor).unwrap();
        assert_eq!(char_idx, 3);
    }

    #[test]
    fn test_save_load_cycle() {
        use std::fs;
        use tempfile::NamedTempFile;

        // Create a temporary file
        let temp_file = NamedTempFile::new().unwrap();
        let temp_path = temp_file.path();

        // Create a buffer with some content
        let mut buf = TextBuffer::new();
        buf.insert(&Cursor::at(0, 0), "line 1\nline 2\nline 3")
            .unwrap();

        // Save the buffer
        buf.save_to(temp_path).unwrap();

        // Read the saved content
        let saved_content = fs::read_to_string(temp_path).unwrap();
        assert_eq!(saved_content, "line 1\nline 2\nline 3");

        // Load the file back
        let mut buf2 = TextBuffer::from_file(temp_path).unwrap();

        // Save it again to a different temp file
        let temp_file2 = NamedTempFile::new().unwrap();
        let temp_path2 = temp_file2.path();
        buf2.save_to(temp_path2).unwrap();

        // Read the re-saved content
        let resaved_content = fs::read_to_string(temp_path2).unwrap();

        // They should be identical
        assert_eq!(
            saved_content, resaved_content,
            "Content changed after save-load-save cycle"
        );
    }

    #[test]
    fn test_save_preserves_line_count() {
        use std::fs;
        use tempfile::NamedTempFile;

        let temp_file = NamedTempFile::new().unwrap();
        let temp_path = temp_file.path();

        // Create buffer with 5 lines
        let mut buf = TextBuffer::new();
        buf.insert(&Cursor::at(0, 0), "1\n2\n3\n4\n5").unwrap();

        // Save
        buf.save_to(temp_path).unwrap();
        let content1 = fs::read_to_string(temp_path).unwrap();
        let lines1: Vec<&str> = content1.lines().collect();
        assert_eq!(lines1.len(), 5, "First save should have 5 lines");

        // Load and save again
        let mut buf2 = TextBuffer::from_file(temp_path).unwrap();
        buf2.save_to(temp_path).unwrap();
        let content2 = fs::read_to_string(temp_path).unwrap();
        let lines2: Vec<&str> = content2.lines().collect();
        assert_eq!(lines2.len(), 5, "Second save should still have 5 lines");

        // Verify content is identical
        assert_eq!(content1, content2, "Content should not change across saves");
    }
}
