use anyhow::{Context, Result};
use ropey::Rope;
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use unicode_segmentation::UnicodeSegmentation;

use super::{Action, Cursor, History};
use crate::line_marks::{shift_marks, LineEdit};
use crate::LineEnding;

/// Beyond this many undrained changes, resending the whole document is
/// cheaper than tracking more — and while LSP is off for a buffer nothing
/// drains them at all, so the list needs a ceiling either way.
const MAX_PENDING_LSP_CHANGES: usize = 512;

/// A replaced range and its replacement, in LSP coordinates.
///
/// Lines are 0-based and columns count UTF-16 code units, the units LSP
/// positions are stated in. Each change describes the document as it was
/// immediately before that change was applied, which is how a sequence of
/// `didChange` content changes is interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspContentChange {
    pub start_line: u32,
    pub start_character: u32,
    pub end_line: u32,
    pub end_character: u32,
    pub text: String,
}

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
    /// Monotonic counter incremented on every mutation (insert/delete/backspace/undo/redo).
    /// Used by outline panel to detect content changes without hashing.
    edit_version: u64,
    /// Ranges replaced since the last drain, for LSP incremental sync.
    lsp_changes: Vec<LspContentChange>,
    /// The recorded ranges no longer describe this document, so the next sync
    /// has to resend the whole text.
    lsp_needs_full_sync: bool,
    /// Lines carrying a mark (a breakpoint), 0-based; they move with edits.
    line_marks: BTreeSet<usize>,
    /// Incremented whenever `line_marks` changes.
    line_marks_version: u64,
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
            edit_version: 0,
            lsp_changes: Vec::new(),
            lsp_needs_full_sync: false,
            line_marks: BTreeSet::new(),
            line_marks_version: 0,
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
            edit_version: 0,
            lsp_changes: Vec::new(),
            lsp_needs_full_sync: false,
            line_marks: BTreeSet::new(),
            line_marks_version: 0,
        }
    }

    /// Create buffer from text string.
    pub fn from_text(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            file_path: None,
            modified: false,
            line_ending: LineEnding::LF,
            history: History::new(),
            edit_version: 0,
            lsp_changes: Vec::new(),
            lsp_needs_full_sync: false,
            line_marks: BTreeSet::new(),
            line_marks_version: 0,
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

        // Normalize CRLF to LF for internal storage.
        // The original line ending type is preserved in `line_ending`
        // and restored when saving to disk.
        let contents = if line_ending == LineEnding::CRLF {
            contents.replace("\r\n", "\n")
        } else {
            contents
        };

        let rope = Rope::from_str(&contents);

        Ok(Self {
            rope,
            file_path: Some(path.to_path_buf()),
            modified: false,
            line_ending,
            history: History::new(),
            edit_version: 0,
            lsp_changes: Vec::new(),
            lsp_needs_full_sync: false,
            line_marks: BTreeSet::new(),
            line_marks_version: 0,
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

    /// Get line as Cow<str> - zero-copy when line is contiguous in memory
    ///
    /// This is more efficient than `line()` for read-only access since it
    /// avoids String allocation when the line is stored contiguously.
    /// Use this in hot paths like rendering.
    #[inline]
    pub fn line_cow(&self, index: usize) -> Option<Cow<'_, str>> {
        if index < self.line_count() {
            let slice = self.rope.line(index);
            // Try to get direct reference (zero-copy)
            // Falls back to String allocation only for non-contiguous chunks
            Some(Cow::from(slice))
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

    /// LSP `character` offset for a grapheme column on `line`.
    ///
    /// The buffer counts columns in graphemes; LSP counts them in UTF-16 code
    /// units. The two agree for ASCII and for the whole BMP — Cyrillic and CJK
    /// included — and diverge for combining sequences (`e` + U+0301 is one
    /// grapheme, two code units), astral characters (an emoji is one grapheme,
    /// two code units) and ZWJ sequences (a family emoji is one grapheme and
    /// seven). Sending a grapheme column as `character` therefore points a
    /// request at the wrong place once such text sits to its left.
    ///
    /// A column past the end of the line clamps to the line's length, matching
    /// how LSP treats an out-of-range position.
    pub fn utf16_column(&self, line: usize, column: usize) -> usize {
        let Some(text) = self.line(line) else {
            return 0;
        };
        text.trim_end_matches('\n')
            .graphemes(true)
            .take(column)
            .map(|g| g.encode_utf16().count())
            .sum()
    }

    /// Grapheme column for an LSP `character` offset on `line`.
    ///
    /// The inverse of [`Self::utf16_column`], for server replies that name a
    /// position inside this buffer. An offset landing inside a grapheme rounds
    /// down to that grapheme's start, and one past the end of the line clamps
    /// to the line's length — both cases the LSP specification calls out.
    pub fn grapheme_column(&self, line: usize, utf16_column: usize) -> usize {
        let Some(text) = self.line(line) else {
            return 0;
        };
        let mut consumed = 0;
        for (column, grapheme) in text.trim_end_matches('\n').graphemes(true).enumerate() {
            if consumed >= utf16_column {
                return column;
            }
            let next = consumed + grapheme.encode_utf16().count();
            if next > utf16_column {
                // The offset lands inside this grapheme — a surrogate half, or
                // a combining mark. Round down to the grapheme's own start so
                // the result stays a column this buffer can address.
                return column;
            }
            consumed = next;
        }
        text.trim_end_matches('\n').graphemes(true).count()
    }

    /// LSP position for a char index in the rope as it stands right now.
    ///
    /// Works off char indices rather than grapheme arithmetic so it stays
    /// exact for every mutation path, including the ones that delete a single
    /// `char` out of a multi-codepoint grapheme.
    fn lsp_position_at(&self, char_idx: usize) -> (u32, u32) {
        let char_idx = char_idx.min(self.rope.len_chars());
        let line = self.rope.char_to_line(char_idx);
        let line_start = self.rope.line_to_char(line);
        let units: usize = self
            .rope
            .slice(line_start..char_idx)
            .chars()
            .map(|c| c.len_utf16())
            .sum();
        (line as u32, units as u32)
    }

    /// Record that `chars` is about to be replaced with `text`: move the line
    /// marks and queue the change for LSP.
    ///
    /// Must be called *before* the rope is modified: the range is stated in
    /// the pre-edit document, which is what a `didChange` content change
    /// means.
    fn record_change(&mut self, chars: std::ops::Range<usize>, text: &str) {
        if !self.line_marks.is_empty() {
            self.shift_line_marks(chars.clone(), text);
        }
        self.record_lsp_change(chars, text);
    }

    fn shift_line_marks(&mut self, chars: std::ops::Range<usize>, text: &str) {
        let at = |idx: usize| {
            let idx = idx.min(self.rope.len_chars());
            let line = self.rope.char_to_line(idx);
            (line, self.rope.line_to_char(line) == idx)
        };
        let (start_line, start_at_line_start) = at(chars.start);
        let (end_line, end_at_line_start) = at(chars.end);
        let edit = LineEdit {
            start_line,
            start_at_line_start,
            end_line,
            end_at_line_start,
            // Count breaks the way the rope does (CR, LS, PS... too).
            inserted_breaks: ropey::str_utils::byte_to_line_idx(text, text.len()),
        };
        if shift_marks(&mut self.line_marks, edit) {
            self.line_marks_version += 1;
        }
    }

    fn record_lsp_change(&mut self, chars: std::ops::Range<usize>, text: &str) {
        if self.lsp_needs_full_sync {
            return;
        }
        if self.lsp_changes.len() >= MAX_PENDING_LSP_CHANGES {
            self.lsp_changes.clear();
            self.lsp_needs_full_sync = true;
            return;
        }
        let (start_line, start_character) = self.lsp_position_at(chars.start);
        let (end_line, end_character) = self.lsp_position_at(chars.end);
        self.lsp_changes.push(LspContentChange {
            start_line,
            start_character,
            end_line,
            end_character,
            text: text.to_string(),
        });
    }

    /// Demand that the next LSP sync resend the whole document.
    ///
    /// For a change this buffer cannot state as ranges — its text being
    /// replaced wholesale by a reload from disk, for instance.
    pub fn request_full_lsp_sync(&mut self) {
        self.lsp_changes.clear();
        self.lsp_needs_full_sync = true;
    }

    /// Take the changes recorded since the last drain.
    ///
    /// `None` means they can no longer describe the document and the whole
    /// text has to be resent.
    pub fn take_lsp_changes(&mut self) -> Option<Vec<LspContentChange>> {
        if std::mem::take(&mut self.lsp_needs_full_sync) {
            self.lsp_changes.clear();
            return None;
        }
        Some(std::mem::take(&mut self.lsp_changes))
    }

    /// Lines carrying a mark (0-based), kept on their text as it is edited.
    pub fn line_marks(&self) -> &BTreeSet<usize> {
        &self.line_marks
    }

    /// Whether `line` (0-based) carries a mark.
    pub fn has_line_mark(&self, line: usize) -> bool {
        self.line_marks.contains(&line)
    }

    /// Replace the marks; lines past the end of the text are dropped.
    pub fn set_line_marks(&mut self, lines: impl IntoIterator<Item = usize>) {
        let count = self.line_count();
        let marks: BTreeSet<usize> = lines.into_iter().filter(|&l| l < count).collect();
        if marks != self.line_marks {
            self.line_marks = marks;
            self.line_marks_version += 1;
        }
    }

    /// Set the mark on `line` when it has none, remove it otherwise. Returns
    /// whether the line carries a mark now.
    pub fn toggle_line_mark(&mut self, line: usize) -> bool {
        if line >= self.line_count() {
            return false;
        }
        let set = if self.line_marks.remove(&line) {
            false
        } else {
            self.line_marks.insert(line);
            true
        };
        self.line_marks_version += 1;
        set
    }

    /// Incremented whenever the marks change, by a toggle or by an edit that
    /// moved them.
    pub fn line_marks_version(&self) -> u64 {
        self.line_marks_version
    }

    /// Get all text
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Total length of the buffer in bytes (O(1) via the rope).
    ///
    /// Cheap enough to call every frame; used to gate whole-document syntax
    /// highlighting so large files fall back to the per-line path.
    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// Insert text at cursor position
    pub fn insert(&mut self, cursor: &Cursor, text: &str) -> Result<Cursor> {
        let char_idx = self.cursor_to_char_idx(cursor)?;
        self.record_change(char_idx..char_idx, text);
        self.rope.insert(char_idx, text);
        self.modified = true;
        self.edit_version += 1;

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
        self.record_change(char_idx..char_idx + 1, "");
        self.rope.remove(char_idx..char_idx + 1);
        self.modified = true;
        self.edit_version += 1;

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
        self.record_change(char_idx - 1..char_idx, "");
        self.rope.remove(char_idx - 1..char_idx);
        self.modified = true;
        self.edit_version += 1;

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
            self.record_change(start_idx..end_idx, "");
            self.rope.remove(start_idx..end_idx);
            self.modified = true;
            self.edit_version += 1;

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
            // Multiple lines - last() is safe because we checked lines.len() > 1 above
            let last_line = lines
                .last()
                .expect("lines has at least 2 elements in else branch");
            let last_line_len = last_line.graphemes(true).count();
            Cursor::at(cursor.line + lines.len() - 1, last_line_len)
        }
    }

    /// Check if buffer is modified
    pub fn is_modified(&self) -> bool {
        self.modified
    }

    /// Get file path
    pub fn file_path(&self) -> Option<&Path> {
        self.file_path.as_deref()
    }

    /// Get buffer contents as string
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        self.rope.to_string()
    }

    /// Append text to the end of buffer (for log viewer, no history tracking)
    pub fn append(&mut self, text: &str) {
        let len = self.rope.len_chars();
        self.rope.insert(len, text);
        self.edit_version += 1;
        // Don't mark as modified - this is for internal use (log viewer)
    }

    /// Line ending type detected on file load.
    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// Monotonic edit version counter. Incremented on every mutation.
    pub fn edit_version(&self) -> u64 {
        self.edit_version
    }

    /// Record the edits that follow, until [`TextBuffer::end_undo_group`],
    /// as one undo step.
    pub fn begin_undo_group(&mut self) {
        self.history.begin_group();
    }

    /// Close the undo step [`TextBuffer::begin_undo_group`] opened.
    pub fn end_undo_group(&mut self) {
        self.history.end_group();
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
        self.edit_version += 1;
        match action {
            Action::Insert { position, text } => {
                let char_idx = self.cursor_to_char_idx(position)?;
                self.record_change(char_idx..char_idx, text);
                self.rope.insert(char_idx, text);
                let new_cursor = self.advance_cursor(position, text);
                Ok(new_cursor)
            }
            Action::Delete { position, text } => {
                let char_idx = self.cursor_to_char_idx(position)?;
                let end_idx = char_idx + text.chars().count();
                self.record_change(char_idx..end_idx, "");
                self.rope.remove(char_idx..end_idx);
                Ok(*position)
            }
            Action::Group(actions) => {
                let mut cursor = Cursor::default();
                for action in actions {
                    cursor = self.apply_action(action)?;
                }
                Ok(cursor)
            }
        }
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
    fn a_group_of_edits_is_one_undo_step() {
        let mut buffer = TextBuffer::from_text("one two three");
        buffer.begin_undo_group();
        buffer
            .delete_range(&Cursor::at(0, 4), &Cursor::at(0, 7))
            .unwrap();
        buffer.insert(&Cursor::at(0, 4), "2").unwrap();
        buffer.insert(&Cursor::at(0, 0), ">> ").unwrap();
        buffer.end_undo_group();
        assert_eq!(buffer.text(), ">> one 2 three");
        buffer.undo().unwrap();
        assert_eq!(buffer.text(), "one two three");
        buffer.redo().unwrap();
        assert_eq!(buffer.text(), ">> one 2 three");
    }

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

#[cfg(test)]
mod position_encoding_tests {
    use super::*;

    fn buffer(text: &str) -> TextBuffer {
        TextBuffer::from_text(text)
    }

    #[test]
    fn ascii_and_bmp_columns_are_unchanged() {
        // The units coincide across the whole BMP, so the common case must
        // stay a straight pass-through.
        let b = buffer("let x = 1;\nlet привет = 2;\nlet 日本 = 3;\n");
        for (line, column) in [(0usize, 10usize), (1, 15), (2, 11)] {
            assert_eq!(b.utf16_column(line, column), column, "line {line}");
            assert_eq!(b.grapheme_column(line, column), column, "line {line}");
        }
    }

    #[test]
    fn a_combining_accent_is_one_grapheme_and_two_code_units() {
        // "e" + U+0301
        let b = buffer("x = \"e\u{301}\";\n");
        // Up to and including the accented cluster: 4 ASCII + quote + cluster.
        assert_eq!(b.utf16_column(0, 6), 7);
        assert_eq!(b.grapheme_column(0, 7), 6);
    }

    #[test]
    fn an_astral_character_is_one_grapheme_and_two_code_units() {
        let b = buffer("s = \"🙂\";\n");
        assert_eq!(b.utf16_column(0, 6), 7);
        assert_eq!(b.grapheme_column(0, 7), 6);
    }

    #[test]
    fn a_zwj_sequence_is_one_grapheme_and_many_code_units() {
        let b = buffer("s = \"👨\u{200d}👩\u{200d}👧\";\n");
        // 5 ASCII columns, then one cluster of three astral chars and two ZWJs.
        assert_eq!(b.utf16_column(0, 5), 5);
        assert_eq!(b.utf16_column(0, 6), 13);
        assert_eq!(b.grapheme_column(0, 13), 6);
    }

    #[test]
    fn the_conversions_round_trip_on_every_column() {
        let b = buffer("a\u{301}🙂b日\u{200d}c\n");
        let len = b.line_len_graphemes(0);
        for column in 0..=len {
            assert_eq!(
                b.grapheme_column(0, b.utf16_column(0, column)),
                column,
                "column {column}"
            );
        }
    }

    #[test]
    fn an_offset_inside_a_grapheme_resolves_to_its_start() {
        let b = buffer("🙂x\n");
        // Offset 1 is the emoji's low surrogate — not a position a buffer
        // column can name, so it must not run past the grapheme.
        assert_eq!(b.grapheme_column(0, 1), 0);
        assert_eq!(b.grapheme_column(0, 2), 1);
    }

    #[test]
    fn out_of_range_input_clamps_instead_of_panicking() {
        let b = buffer("ab\n");
        assert_eq!(b.utf16_column(0, 99), 2);
        assert_eq!(b.grapheme_column(0, 99), 2);
        // A line past the end of the buffer has no columns at all.
        assert_eq!(b.utf16_column(99, 3), 0);
        assert_eq!(b.grapheme_column(99, 3), 0);
    }
}

#[cfg(test)]
mod lsp_change_tests {
    use super::*;

    /// Apply recorded changes to a copy of the text the way a language server
    /// would, so a test can assert the server's document ends up identical to
    /// the buffer's. A wrong range desynchronises the server silently, which
    /// is the whole risk of ranged sync.
    fn replay(original: &str, changes: &[LspContentChange]) -> String {
        let mut text = original.to_string();
        for change in changes {
            let start = utf16_offset(&text, change.start_line, change.start_character);
            let end = utf16_offset(&text, change.end_line, change.end_character);
            text.replace_range(start..end, &change.text);
        }
        text
    }

    /// Byte offset for an LSP position, mirroring a server's own decoding.
    fn utf16_offset(text: &str, line: u32, character: u32) -> usize {
        let mut byte = 0;
        for _ in 0..line {
            byte += text[byte..]
                .find('\n')
                .map(|i| i + 1)
                .expect("line within text");
        }
        let mut units = 0;
        for ch in text[byte..].chars() {
            if units >= character {
                break;
            }
            units += ch.len_utf16() as u32;
            byte += ch.len_utf8();
        }
        byte
    }

    fn drained(buffer: &mut TextBuffer) -> Vec<LspContentChange> {
        buffer
            .take_lsp_changes()
            .expect("changes should be describable as ranges")
    }

    #[test]
    fn typing_records_one_insertion_per_edit() {
        let mut buf = TextBuffer::from_text("");
        let mut cursor = Cursor::at(0, 0);
        for ch in ["l", "e", "t"] {
            cursor = buf.insert(&cursor, ch).unwrap();
        }
        let changes = drained(&mut buf);
        assert_eq!(changes.len(), 3);
        assert_eq!(replay("", &changes), buf.text());
        // Each insertion is zero-width at the point it happened.
        assert_eq!(
            (changes[2].start_character, changes[2].end_character),
            (2, 2)
        );
    }

    #[test]
    fn a_deletion_spans_the_removed_range() {
        let original = "hello world";
        let mut buf = TextBuffer::from_text(original);
        buf.delete_range(&Cursor::at(0, 5), &Cursor::at(0, 11))
            .unwrap();
        let changes = drained(&mut buf);
        assert_eq!(changes.len(), 1);
        assert_eq!(
            (changes[0].start_character, changes[0].end_character),
            (5, 11)
        );
        assert!(changes[0].text.is_empty());
        assert_eq!(replay(original, &changes), buf.text());
    }

    #[test]
    fn a_newline_deletion_spans_two_lines() {
        let original = "ab\ncd";
        let mut buf = TextBuffer::from_text(original);
        // Deleting at the end of line 0 removes the newline itself.
        buf.delete_char(&Cursor::at(0, 2)).unwrap();
        let changes = drained(&mut buf);
        assert_eq!(
            (
                changes[0].start_line,
                changes[0].start_character,
                changes[0].end_line,
                changes[0].end_character
            ),
            (0, 2, 1, 0)
        );
        assert_eq!(replay(original, &changes), buf.text());
    }

    #[test]
    fn backspace_records_the_character_it_removed() {
        let original = "ab";
        let mut buf = TextBuffer::from_text(original);
        buf.backspace(&Cursor::at(0, 2)).unwrap();
        let changes = drained(&mut buf);
        assert_eq!(
            (changes[0].start_character, changes[0].end_character),
            (1, 2)
        );
        assert_eq!(replay(original, &changes), buf.text());
    }

    #[test]
    fn columns_are_utf16_not_graphemes() {
        // An emoji to the left makes the two units disagree; a grapheme column
        // here would point the server one unit short and corrupt its copy.
        let original = "🙂ab";
        let mut buf = TextBuffer::from_text(original);
        buf.delete_range(&Cursor::at(0, 1), &Cursor::at(0, 2))
            .unwrap();
        let changes = drained(&mut buf);
        assert_eq!(
            (changes[0].start_character, changes[0].end_character),
            (2, 3)
        );
        assert_eq!(replay(original, &changes), buf.text());
        assert_eq!(buf.text(), "🙂b");
    }

    #[test]
    fn a_multiline_insertion_replays_exactly() {
        let original = "fn main() {}\n";
        let mut buf = TextBuffer::from_text(original);
        buf.insert(&Cursor::at(0, 11), "\n    let x = 1;\n")
            .unwrap();
        let changes = drained(&mut buf);
        assert_eq!(replay(original, &changes), buf.text());
    }

    #[test]
    fn undo_and_redo_are_recorded_too() {
        // Undo mutates the rope directly rather than going through insert or
        // delete, so it needs its own recording or the server drifts.
        let original = "ab";
        let mut buf = TextBuffer::from_text(original);
        buf.insert(&Cursor::at(0, 2), "c").unwrap();
        let after_insert = buf.text();
        assert_eq!(replay(original, &drained(&mut buf)), after_insert);

        buf.undo().unwrap();
        assert_eq!(replay(&after_insert, &drained(&mut buf)), buf.text());

        let after_undo = buf.text();
        buf.redo().unwrap();
        assert_eq!(replay(&after_undo, &drained(&mut buf)), buf.text());
    }

    #[test]
    fn a_reload_demands_the_whole_document() {
        let mut buf = TextBuffer::from_text("a");
        buf.insert(&Cursor::at(0, 1), "b").unwrap();
        buf.request_full_lsp_sync();
        assert!(
            buf.take_lsp_changes().is_none(),
            "a full resync must discard the ranges it supersedes"
        );
        // And the demand is one-shot.
        assert_eq!(buf.take_lsp_changes(), Some(Vec::new()));
    }

    #[test]
    fn an_undrained_backlog_falls_back_to_a_full_document() {
        // Nothing drains the list while LSP is off for a buffer, so it must
        // not grow without bound.
        let mut buf = TextBuffer::from_text("");
        let mut cursor = Cursor::at(0, 0);
        for _ in 0..MAX_PENDING_LSP_CHANGES + 1 {
            cursor = buf.insert(&cursor, "x").unwrap();
        }
        assert!(buf.take_lsp_changes().is_none());
    }

    #[test]
    fn a_line_mark_follows_its_line_through_edits_and_undo() {
        let mut buf = TextBuffer::from_text("a\nb\nc\n");
        assert!(buf.toggle_line_mark(1));
        let v = buf.line_marks_version();

        // A line inserted above pushes the mark down; undo brings it back.
        buf.insert(&Cursor::at(0, 0), "new\n").unwrap();
        assert!(buf.has_line_mark(2));
        assert!(buf.line_marks_version() > v);
        buf.undo().unwrap();
        assert_eq!(buf.line_marks().iter().copied().collect::<Vec<_>>(), [1]);

        // Deleting the marked line removes the mark.
        buf.delete_range(&Cursor::at(1, 0), &Cursor::at(2, 0))
            .unwrap();
        assert!(buf.line_marks().is_empty());
    }

    #[test]
    fn line_marks_past_the_end_are_dropped() {
        let mut buf = TextBuffer::from_text("a\nb");
        buf.set_line_marks([0, 1, 7]);
        assert_eq!(buf.line_marks().iter().copied().collect::<Vec<_>>(), [0, 1]);
        assert!(!buf.toggle_line_mark(9));
        assert!(!buf.toggle_line_mark(1));
        assert!(!buf.has_line_mark(1));
    }
}
