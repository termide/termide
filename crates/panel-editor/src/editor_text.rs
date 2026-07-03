//! Text editing methods for the Editor.
//!
//! This module contains text editing functionality including:
//! - Clipboard operations (copy, cut, paste)
//! - Character insertion and deletion
//! - Line operations (duplicate, indent, unindent)
//! - Newline insertion with auto-indentation

use anyhow::Result;
use termide_buffer::{Cursor, Selection};

use crate::{auto_pairs, clipboard, cursor, selection, text_editing};

use super::Editor;

impl Editor {
    // =========================================================================
    // Selection Helpers (Private)
    // =========================================================================

    /// Get selected text
    pub(crate) fn get_selected_text(&self) -> Option<String> {
        selection::get_selected_text(&self.buffer, self.selection.as_ref())
    }

    /// Delete selected text
    pub(crate) fn delete_selection(&mut self) -> Result<()> {
        let is_multiline = self
            .selection
            .as_ref()
            .is_some_and(|s| s.start().line != s.end().line);

        if let Some(new_cursor) =
            selection::delete_selection(&mut self.buffer, self.selection.as_ref())?
        {
            self.cursor = new_cursor;
            self.selection = None;
            self.input.preferred_column = None; // Reset preferred column on text edit

            // Invalidate highlighting + wrap caches and schedule git diff update
            self.invalidate_cache_after_edit(new_cursor.line, is_multiline);
        }
        Ok(())
    }

    // =========================================================================
    // Clipboard Operations
    // =========================================================================

    /// Copy selected text to clipboard
    pub(crate) fn copy_to_clipboard(&mut self) -> Result<()> {
        let selected_text = self.get_selected_text();
        let result = clipboard::copy_to_clipboard(selected_text);
        self.status_message = Some(result.status_message);
        Ok(())
    }

    /// Cut selected text to clipboard
    pub(crate) fn cut_to_clipboard(&mut self) -> Result<()> {
        let selected_text = self.get_selected_text();
        let (result, should_delete) = clipboard::cut_to_clipboard(selected_text);
        self.status_message = Some(result.status_message);

        if should_delete {
            self.delete_selection()?;
        }
        Ok(())
    }

    /// Paste from clipboard
    pub fn paste_from_clipboard(&mut self) -> Result<()> {
        // Close search mode when editing begins
        self.close_search();

        // Delete selected text before pasting
        self.delete_selection()?;

        // Paste from clipboard using clipboard module
        if let Some((new_cursor, start_line, is_multiline)) =
            clipboard::paste_from_clipboard(&mut self.buffer, &self.cursor)?
        {
            self.cursor = new_cursor;
            self.input.preferred_column = None; // Reset preferred column on text edit
            self.clamp_cursor();

            // Invalidate highlighting cache and schedule git update
            self.invalidate_cache_after_edit(start_line, is_multiline);
        }
        Ok(())
    }

    /// Paste text directly (from bracketed paste event)
    pub fn paste_text(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }

        // Normalize line endings: some terminals (VTE/gnome-terminal) send \r
        // for newlines in bracketed paste instead of \n
        let text = text.replace("\r\n", "\n").replace('\r', "\n");

        // Close search mode when editing begins
        self.close_search();

        // Delete selected text before pasting
        self.delete_selection()?;

        // Insert text at cursor position
        let start_line = self.cursor.line;
        let new_cursor = self.buffer.insert(&self.cursor, &text)?;
        let is_multiline = text.contains('\n');

        self.cursor = new_cursor;
        self.input.preferred_column = None;
        self.clamp_cursor();

        // Invalidate highlighting cache and schedule git update
        self.invalidate_cache_after_edit(start_line, is_multiline);

        Ok(())
    }

    // =========================================================================
    // Character Operations
    // =========================================================================

    /// Insert character at cursor position
    pub(crate) fn insert_char(&mut self, ch: char) -> Result<()> {
        // Close search mode when editing begins
        self.close_search();

        // Delete selected text before insertion
        self.delete_selection()?;

        // Auto-close brackets: if typing a closing char that already exists at cursor, skip over it
        if self.config.auto_close_brackets && auto_pairs::is_closing(ch) {
            let char_at_cursor = self.char_at_cursor();
            if char_at_cursor == Some(ch) {
                // Skip over the existing closing character
                self.cursor.column += 1;
                self.input.preferred_column = None;
                self.clamp_cursor();
                self.lsp.last_inserted_char = Some(ch);
                return Ok(());
            }
        }

        // Capture character before cursor before insertion (for auto-close context)
        let char_before_insert = if self.config.auto_close_brackets {
            self.char_before_cursor()
        } else {
            None
        };

        let result = text_editing::insert_char(&mut self.buffer, &self.cursor, ch)?;
        self.cursor = result.new_cursor;
        self.input.preferred_column = None;
        self.clamp_cursor();

        // Auto-close brackets: insert matching closing character
        if self.config.auto_close_brackets {
            if let Some(close) = auto_pairs::closing_char(ch) {
                if auto_pairs::should_auto_close(ch, char_before_insert) {
                    // Insert closing char at the current cursor position (after the opening char)
                    let close_str = close.to_string();
                    self.buffer.insert(&self.cursor, &close_str)?;
                    // Don't advance cursor — it stays between the pair
                }
            }
        }

        // Track inserted character for auto-completion
        self.lsp.last_inserted_char = Some(ch);

        // Invalidate highlighting cache and schedule git update
        self.invalidate_cache_after_edit(result.start_line, result.is_multiline);

        Ok(())
    }

    /// Insert tab (spaces based on tab_size config)
    pub(crate) fn insert_tab(&mut self) -> Result<()> {
        // Close search mode when editing begins
        self.close_search();

        // Delete selected text before insertion
        self.delete_selection()?;

        // Insert tab_size spaces
        let spaces = " ".repeat(self.config.tab_size);
        for ch in spaces.chars() {
            let result = text_editing::insert_char(&mut self.buffer, &self.cursor, ch)?;
            self.cursor = result.new_cursor;
        }

        self.input.preferred_column = None;
        self.clamp_cursor();

        // Invalidate highlighting cache and schedule git update
        self.invalidate_cache_after_edit(self.cursor.line, false);

        Ok(())
    }

    /// Insert newline
    pub(crate) fn insert_newline(&mut self) -> Result<()> {
        // Close search mode when editing begins
        self.close_search();

        // Delete selected text before insertion
        self.delete_selection()?;

        let result = if self.config.auto_indent {
            // Only apply the `{` / `(` / `[` / `:` smart-indent triggers when
            // the file has a recognized syntax — in plain-text buffers a
            // trailing colon in prose shouldn't push the next line in.
            let smart_indent = self.render_cache.highlight.has_syntax();
            text_editing::insert_newline_with_indent(
                &mut self.buffer,
                &self.cursor,
                self.config.tab_size,
                smart_indent,
            )?
        } else {
            text_editing::insert_newline(&mut self.buffer, &self.cursor)?
        };
        self.cursor = result.new_cursor;
        self.input.preferred_column = None; // Reset preferred column on text edit
        self.clamp_cursor();

        // Invalidate highlighting cache and schedule git update
        self.invalidate_cache_after_edit(result.start_line, result.is_multiline);

        Ok(())
    }

    /// Delete character (backspace)
    pub(crate) fn backspace(&mut self) -> Result<()> {
        // Auto-close brackets: if cursor is between a matching pair, delete both
        if self.config.auto_close_brackets && self.cursor.column > 0 {
            let char_before = self.char_before_cursor();
            let char_after = self.char_at_cursor();
            if let (Some(before), Some(after)) = (char_before, char_after) {
                if auto_pairs::is_matching_pair(before, after) {
                    // Delete the closing character first (at cursor position)
                    self.buffer.delete_char(&self.cursor)?;
                    // Then backspace the opening character
                    if let Some(result) = text_editing::backspace(&mut self.buffer, &self.cursor)? {
                        self.cursor = result.new_cursor;
                        self.input.preferred_column = None;
                        self.clamp_cursor();
                        self.invalidate_cache_after_edit(result.start_line, result.is_multiline);
                    }
                    return Ok(());
                }
            }
        }

        if let Some(result) = text_editing::backspace(&mut self.buffer, &self.cursor)? {
            self.cursor = result.new_cursor;
            self.input.preferred_column = None; // Reset preferred column on text edit
            self.clamp_cursor();

            // Invalidate highlighting cache and schedule git update
            self.invalidate_cache_after_edit(result.start_line, result.is_multiline);
        }
        Ok(())
    }

    /// Delete character (delete)
    pub(crate) fn delete(&mut self) -> Result<()> {
        if let Some(result) = text_editing::delete_char(&mut self.buffer, &self.cursor)? {
            self.input.preferred_column = None; // Reset preferred column on text edit
                                                // Invalidate highlighting cache and schedule git update
            self.invalidate_cache_after_edit(result.start_line, result.is_multiline);
        }
        Ok(())
    }

    /// Delete from the cursor back to the start of the previous word
    /// (Ctrl+Backspace). No-op at the buffer start.
    pub(crate) fn backspace_word(&mut self) -> Result<()> {
        let mut target = self.cursor;
        cursor::physical::move_word_backward(&mut target, &self.buffer);
        if target == self.cursor {
            return Ok(());
        }
        self.selection = Some(Selection::new(target, self.cursor));
        self.delete_selection()
    }

    /// Delete from the cursor forward to the end of the next word
    /// (Ctrl+Delete). No-op at the buffer end.
    pub(crate) fn delete_word(&mut self) -> Result<()> {
        let mut target = self.cursor;
        cursor::physical::move_word_forward(&mut target, &self.buffer);
        if target == self.cursor {
            return Ok(());
        }
        self.selection = Some(Selection::new(self.cursor, target));
        self.delete_selection()
    }

    // =========================================================================
    // Line Operations
    // =========================================================================

    /// Duplicate current line or selected lines
    pub(crate) fn duplicate_line(&mut self) -> Result<()> {
        let result =
            text_editing::duplicate_line(&mut self.buffer, &self.cursor, self.selection.as_ref())?;

        self.cursor = result.new_cursor;
        self.input.preferred_column = None; // Reset preferred column on text edit
        self.clamp_cursor();

        // Clear selection
        self.selection = None;

        // Invalidate highlighting cache and schedule git update
        self.invalidate_cache_after_edit(result.start_line, result.is_multiline);

        Ok(())
    }

    /// Delete the line under the cursor, or every line touched by the
    /// active selection. Pure deletion — clipboard is untouched.
    pub(crate) fn delete_line(&mut self) -> Result<()> {
        let result =
            text_editing::delete_line(&mut self.buffer, &self.cursor, self.selection.as_ref())?;

        self.cursor = result.new_cursor;
        self.input.preferred_column = None;
        self.clamp_cursor();
        self.selection = None;
        self.invalidate_cache_after_edit(result.start_line, result.is_multiline);

        Ok(())
    }

    /// Indent selected lines (or current line if no selection)
    pub(crate) fn indent_lines(&mut self) -> Result<()> {
        // Close search mode when editing begins
        self.close_search();

        let tab_size = self.config.tab_size;
        let indent = " ".repeat(tab_size);

        // Get line range from selection or current cursor
        let (start_line, end_line) = if let Some(ref sel) = self.selection {
            (sel.start().line, sel.end().line)
        } else {
            (self.cursor.line, self.cursor.line)
        };

        // Insert indent at the beginning of each line (iterate in reverse to avoid index shifts)
        for line_idx in (start_line..=end_line).rev() {
            let cursor_at_start = Cursor::at(line_idx, 0);
            self.buffer.insert(&cursor_at_start, &indent)?;
        }

        // Update cursor position
        self.cursor.column += tab_size;

        // Update selection positions if present
        if let Some(ref mut sel) = self.selection {
            sel.anchor.column += tab_size;
            sel.active.column += tab_size;
        }

        self.input.preferred_column = None;
        self.clamp_cursor();

        // Invalidate highlighting cache and schedule git update
        self.invalidate_cache_after_edit(start_line, true);
        self.schedule_git_diff_update();

        Ok(())
    }

    /// Unindent selected lines (or current line if no selection)
    pub(crate) fn unindent_lines(&mut self) -> Result<()> {
        // Close search mode when editing begins
        self.close_search();

        let tab_size = self.config.tab_size;

        // Get line range from selection or current cursor
        let (start_line, end_line) = if let Some(ref sel) = self.selection {
            (sel.start().line, sel.end().line)
        } else {
            (self.cursor.line, self.cursor.line)
        };

        // Track how many spaces were removed from each line for cursor adjustment
        let mut cursor_line_spaces_removed = 0;
        let mut anchor_line_spaces_removed = 0;
        let mut active_line_spaces_removed = 0;

        // Remove up to tab_size spaces from the beginning of each line
        for line_idx in (start_line..=end_line).rev() {
            if let Some(line) = self.buffer.line(line_idx) {
                // Count leading spaces (up to tab_size)
                let spaces_to_remove = line
                    .chars()
                    .take(tab_size)
                    .take_while(|c| *c == ' ')
                    .count();

                if spaces_to_remove > 0 {
                    let start = Cursor::at(line_idx, 0);
                    let end = Cursor::at(line_idx, spaces_to_remove);
                    self.buffer.delete_range(&start, &end)?;

                    // Track spaces removed for cursor/selection adjustment
                    if line_idx == self.cursor.line {
                        cursor_line_spaces_removed = spaces_to_remove;
                    }
                    if let Some(ref sel) = self.selection {
                        if line_idx == sel.anchor.line {
                            anchor_line_spaces_removed = spaces_to_remove;
                        }
                        if line_idx == sel.active.line {
                            active_line_spaces_removed = spaces_to_remove;
                        }
                    }
                }
            }
        }

        // Update cursor position (subtract removed spaces, but don't go below 0)
        self.cursor.column = self
            .cursor
            .column
            .saturating_sub(cursor_line_spaces_removed);

        // Update selection positions if present
        if let Some(ref mut sel) = self.selection {
            sel.anchor.column = sel.anchor.column.saturating_sub(anchor_line_spaces_removed);
            sel.active.column = sel.active.column.saturating_sub(active_line_spaces_removed);
        }

        self.input.preferred_column = None;
        self.clamp_cursor();

        // Invalidate highlighting cache and schedule git update
        self.invalidate_cache_after_edit(start_line, true);
        self.schedule_git_diff_update();

        Ok(())
    }

    /// Toggle line comment for current line or selected lines
    pub(crate) fn toggle_comment(&mut self) -> Result<()> {
        self.close_search();

        // Determine comment prefix from syntax
        let comment_prefix = match self
            .render_cache
            .highlight
            .current_syntax()
            .map(|s| s.to_lowercase())
            .as_deref()
        {
            Some(
                "rust" | "go" | "c" | "cpp" | "c++" | "java" | "javascript" | "typescript" | "tsx"
                | "jsx" | "php" | "swift" | "kotlin" | "scala" | "c#" | "dart" | "zig",
            ) => "// ",
            Some(
                "python" | "ruby" | "bash" | "shell" | "sh" | "toml" | "yaml" | "yml" | "nix"
                | "perl" | "r" | "makefile" | "dockerfile" | "fish",
            ) => "# ",
            Some("haskell" | "lua" | "sql") => "-- ",
            Some("lisp" | "clojure" | "scheme") => ";; ",
            Some("html" | "xml") => "<!-- ", // simplified, no closing tag
            _ => "// ",
        };
        let prefix_trimmed = comment_prefix.trim_end();

        // Get line range from selection or current cursor
        let (start_line, end_line) = if let Some(ref sel) = self.selection {
            (sel.start().line, sel.end().line)
        } else {
            (self.cursor.line, self.cursor.line)
        };

        // Determine action: if ALL lines start with comment prefix → uncomment, else comment
        let all_commented = (start_line..=end_line).all(|line_idx| {
            self.buffer
                .line(line_idx)
                .map(|line| line.trim_start().starts_with(prefix_trimmed))
                .unwrap_or(true) // empty/missing lines count as commented
        });

        if all_commented {
            // Uncomment: remove comment prefix from each line
            let mut cursor_line_removed = 0;
            let mut anchor_line_removed = 0;
            let mut active_line_removed = 0;

            for line_idx in (start_line..=end_line).rev() {
                if let Some(line) = self.buffer.line(line_idx) {
                    let leading_spaces = line.chars().take_while(|c| *c == ' ').count();
                    let after_spaces = &line[leading_spaces..];

                    // Check for prefix with space first, then without
                    let remove_len = if after_spaces.starts_with(comment_prefix) {
                        comment_prefix.len()
                    } else if after_spaces.starts_with(prefix_trimmed) {
                        prefix_trimmed.len()
                    } else {
                        continue;
                    };

                    let start = Cursor::at(line_idx, leading_spaces);
                    let end = Cursor::at(line_idx, leading_spaces + remove_len);
                    self.buffer.delete_range(&start, &end)?;

                    if line_idx == self.cursor.line {
                        cursor_line_removed = remove_len;
                    }
                    if let Some(ref sel) = self.selection {
                        if line_idx == sel.anchor.line {
                            anchor_line_removed = remove_len;
                        }
                        if line_idx == sel.active.line {
                            active_line_removed = remove_len;
                        }
                    }
                }
            }

            self.cursor.column = self.cursor.column.saturating_sub(cursor_line_removed);
            if let Some(ref mut sel) = self.selection {
                sel.anchor.column = sel.anchor.column.saturating_sub(anchor_line_removed);
                sel.active.column = sel.active.column.saturating_sub(active_line_removed);
            }
        } else {
            // Comment: insert comment prefix at column 0 of each line
            let prefix_len = comment_prefix.len();

            for line_idx in (start_line..=end_line).rev() {
                let cursor_at_start = Cursor::at(line_idx, 0);
                self.buffer.insert(&cursor_at_start, comment_prefix)?;
            }

            self.cursor.column += prefix_len;
            if let Some(ref mut sel) = self.selection {
                sel.anchor.column += prefix_len;
                sel.active.column += prefix_len;
            }
        }

        self.input.preferred_column = None;
        self.clamp_cursor();

        self.invalidate_cache_after_edit(start_line, true);
        self.schedule_git_diff_update();

        Ok(())
    }

    // =========================================================================
    // Cursor Helpers (Private)
    // =========================================================================

    /// Get the character at the cursor position (the character the cursor is on).
    fn char_at_cursor(&self) -> Option<char> {
        let line = self.buffer.line(self.cursor.line)?;
        let line = line.trim_end_matches('\n');
        line.chars().nth(self.cursor.column)
    }

    /// Get the character immediately before the cursor position.
    fn char_before_cursor(&self) -> Option<char> {
        if self.cursor.column == 0 {
            return None;
        }
        let line = self.buffer.line(self.cursor.line)?;
        let line = line.trim_end_matches('\n');
        line.chars().nth(self.cursor.column - 1)
    }

    /// Clamp cursor position to valid values
    pub(crate) fn clamp_cursor(&mut self) {
        cursor::physical::clamp_cursor(&mut self.cursor, &self.buffer);
    }
}
