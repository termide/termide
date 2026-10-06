//! Multi-line text area handler with cursor management and selection support.
//!
//! Extends the concept of TextInput for multi-line text editing with:
//! - Multiple lines storage
//! - 2D cursor navigation (row, col)
//! - Multi-line selection (keyboard and mouse drag)
//! - Clipboard support for multi-line text
//! - Undo/Redo history
//! - The soft-wrap geometry of the last render, so a host that draws the text
//!   wrapped can map a click on a wrapped row back to the character under it

/// Position in the text area (row, column in characters)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CursorPos {
    pub row: usize,
    pub col: usize,
}

impl CursorPos {
    pub fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

impl PartialOrd for CursorPos {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CursorPos {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.row.cmp(&other.row) {
            std::cmp::Ordering::Equal => self.col.cmp(&other.col),
            ord => ord,
        }
    }
}

/// Multi-line text area handler with selection and undo support.
#[derive(Debug, Clone)]
pub struct TextArea {
    /// Lines of text
    lines: Vec<String>,
    /// Current cursor position (active point)
    cursor: CursorPos,
    /// Selection anchor (None = no selection)
    selection_anchor: Option<CursorPos>,
    /// Undo history: (lines, cursor)
    undo_stack: Vec<(Vec<String>, CursorPos)>,
    /// Redo history: (lines, cursor)
    redo_stack: Vec<(Vec<String>, CursorPos)>,
    /// Scroll offset for vertical scrolling
    scroll_offset: usize,
    /// Soft-wrap geometry of the last render: one `(logical row, first char,
    /// end char)` per visual row. Empty until the host records it through
    /// [`TextArea::set_wrap`], which also means "rendered unwrapped".
    wrap_rows: Vec<(usize, usize, usize)>,
    /// First visual row drawn by that render, i.e. the vertical scroll.
    visual_scroll: usize,
}

impl Default for TextArea {
    fn default() -> Self {
        Self::new()
    }
}

impl TextArea {
    /// Create a new empty text area.
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor: CursorPos::default(),
            selection_anchor: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            scroll_offset: 0,
            wrap_rows: Vec::new(),
            visual_scroll: 0,
        }
    }

    /// Create a text area with initial text.
    pub fn with_text(text: &str) -> Self {
        let lines: Vec<String> = if text.is_empty() {
            vec![String::new()]
        } else {
            text.lines().map(String::from).collect()
        };
        let row = lines.len().saturating_sub(1);
        let col = lines.last().map(|l| l.chars().count()).unwrap_or(0);

        Self {
            lines,
            cursor: CursorPos::new(row, col),
            selection_anchor: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            scroll_offset: 0,
            wrap_rows: Vec::new(),
            visual_scroll: 0,
        }
    }

    // === Getters ===

    /// Get all lines.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Get the full text content.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Get cursor position.
    pub fn cursor(&self) -> CursorPos {
        self.cursor
    }

    /// Get scroll offset.
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Set scroll offset.
    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.scroll_offset = offset.min(self.lines.len().saturating_sub(1));
    }

    /// Get number of lines.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Check if text is empty.
    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    /// Get current line.
    fn current_line(&self) -> &str {
        self.lines
            .get(self.cursor.row)
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    /// Get current line length in characters.
    fn current_line_len(&self) -> usize {
        self.current_line().chars().count()
    }

    // === Undo/Redo ===

    fn save_undo_state(&mut self) {
        // Don't save if state hasn't changed
        if let Some((last_lines, _)) = self.undo_stack.last() {
            if last_lines == &self.lines {
                return;
            }
        }
        self.undo_stack.push((self.lines.clone(), self.cursor));
        self.redo_stack.clear();

        const MAX_UNDO_HISTORY: usize = 100;
        if self.undo_stack.len() > MAX_UNDO_HISTORY {
            self.undo_stack.remove(0);
        }
    }

    /// Undo last change.
    pub fn undo(&mut self) -> bool {
        if let Some((lines, cursor)) = self.undo_stack.pop() {
            self.redo_stack.push((self.lines.clone(), self.cursor));
            self.lines = lines;
            self.cursor = cursor;
            self.selection_anchor = None;
            true
        } else {
            false
        }
    }

    /// Redo last undone change.
    pub fn redo(&mut self) -> bool {
        if let Some((lines, cursor)) = self.redo_stack.pop() {
            self.undo_stack.push((self.lines.clone(), self.cursor));
            self.lines = lines;
            self.cursor = cursor;
            self.selection_anchor = None;
            true
        } else {
            false
        }
    }

    // === Selection ===

    /// Check if has selection.
    pub fn has_selection(&self) -> bool {
        self.selection_anchor
            .is_some_and(|anchor| anchor != self.cursor)
    }

    /// Get selection range (start, end) positions.
    pub fn selection_range(&self) -> Option<(CursorPos, CursorPos)> {
        self.selection_anchor.map(|anchor| {
            if anchor <= self.cursor {
                (anchor, self.cursor)
            } else {
                (self.cursor, anchor)
            }
        })
    }

    /// Get selected text.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        if start == end {
            return None;
        }

        if start.row == end.row {
            // Single line selection
            let line = &self.lines[start.row];
            let start_byte = char_to_byte_index(line, start.col);
            let end_byte = char_to_byte_index(line, end.col);
            Some(line[start_byte..end_byte].to_string())
        } else {
            // Multi-line selection
            let mut result = String::new();

            // First line (from start.col to end)
            let first_line = &self.lines[start.row];
            let start_byte = char_to_byte_index(first_line, start.col);
            result.push_str(&first_line[start_byte..]);

            // Middle lines (complete)
            for row in (start.row + 1)..end.row {
                result.push('\n');
                result.push_str(&self.lines[row]);
            }

            // Last line (from start to end.col)
            result.push('\n');
            let last_line = &self.lines[end.row];
            let end_byte = char_to_byte_index(last_line, end.col);
            result.push_str(&last_line[..end_byte]);

            Some(result)
        }
    }

    /// Select all text.
    pub fn select_all(&mut self) {
        if !self.is_empty() {
            self.selection_anchor = Some(CursorPos::new(0, 0));
            let last_row = self.lines.len() - 1;
            let last_col = self.lines[last_row].chars().count();
            self.cursor = CursorPos::new(last_row, last_col);
        }
    }

    /// Start selection at current position.
    pub fn start_selection(&mut self) {
        if self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.cursor);
        }
    }

    /// Clear selection.
    pub fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    /// Delete selected text.
    pub fn delete_selection(&mut self) -> bool {
        if let Some((start, end)) = self.selection_range() {
            if start != end {
                self.save_undo_state();
                self.delete_selection_internal();
                return true;
            }
        }
        self.selection_anchor = None;
        false
    }

    fn delete_selection_internal(&mut self) {
        let Some((start, end)) = self.selection_range() else {
            return;
        };
        if start == end {
            self.selection_anchor = None;
            return;
        }

        if start.row == end.row {
            // Single line deletion
            let line = &mut self.lines[start.row];
            let start_byte = char_to_byte_index(line, start.col);
            let end_byte = char_to_byte_index(line, end.col);
            line.replace_range(start_byte..end_byte, "");
        } else {
            // Multi-line deletion
            let first_line = &self.lines[start.row];
            let start_byte = char_to_byte_index(first_line, start.col);
            let first_part = first_line[..start_byte].to_string();

            let last_line = &self.lines[end.row];
            let end_byte = char_to_byte_index(last_line, end.col);
            let last_part = last_line[end_byte..].to_string();

            // Combine first and last parts
            self.lines[start.row] = first_part + &last_part;

            // Remove middle and last lines
            self.lines.drain((start.row + 1)..=end.row);
        }

        self.cursor = start;
        self.selection_anchor = None;
    }

    // === Text modification ===

    /// Insert a character at cursor.
    pub fn insert(&mut self, c: char) {
        if c == '\n' {
            self.insert_newline();
            return;
        }

        self.save_undo_state();
        self.delete_selection_internal();

        let line = &mut self.lines[self.cursor.row];
        let byte_idx = char_to_byte_index(line, self.cursor.col);
        line.insert(byte_idx, c);
        self.cursor.col += 1;
    }

    /// Insert a string at cursor (handles multi-line).
    pub fn insert_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }

        self.save_undo_state();
        self.delete_selection_internal();

        let mut lines_to_insert: Vec<&str> = s.split('\n').collect();

        if lines_to_insert.len() == 1 {
            // Single line insert
            let line = &mut self.lines[self.cursor.row];
            let byte_idx = char_to_byte_index(line, self.cursor.col);
            line.insert_str(byte_idx, s);
            self.cursor.col += s.chars().count();
        } else {
            // Multi-line insert
            let current_line = &self.lines[self.cursor.row];
            let byte_idx = char_to_byte_index(current_line, self.cursor.col);
            let before = current_line[..byte_idx].to_string();
            let after = current_line[byte_idx..].to_string();

            // First line: before + first part of inserted text
            let first_insert = lines_to_insert.remove(0);
            self.lines[self.cursor.row] = before + first_insert;

            // Last line: last part of inserted text + after
            let last_insert = lines_to_insert.pop().unwrap_or("");
            let last_line = last_insert.to_string() + &after;

            // Insert middle lines and last line
            let insert_row = self.cursor.row + 1;
            for (i, line) in lines_to_insert.iter().enumerate() {
                self.lines.insert(insert_row + i, line.to_string());
            }
            self.lines
                .insert(insert_row + lines_to_insert.len(), last_line);

            // Update cursor
            self.cursor.row += lines_to_insert.len() + 1;
            self.cursor.col = last_insert.chars().count();
        }
    }

    /// Insert a newline (Enter key).
    pub fn insert_newline(&mut self) {
        self.save_undo_state();
        self.delete_selection_internal();

        let line = &self.lines[self.cursor.row];
        let byte_idx = char_to_byte_index(line, self.cursor.col);
        let after = line[byte_idx..].to_string();
        self.lines[self.cursor.row].truncate(byte_idx);

        self.cursor.row += 1;
        self.cursor.col = 0;
        self.lines.insert(self.cursor.row, after);
    }

    /// Delete character before cursor (Backspace).
    pub fn backspace(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }

        if self.cursor.col > 0 {
            self.save_undo_state();
            self.cursor.col -= 1;
            let line = &mut self.lines[self.cursor.row];
            let byte_idx = char_to_byte_index(line, self.cursor.col);
            line.remove(byte_idx);
            true
        } else if self.cursor.row > 0 {
            // Join with previous line
            self.save_undo_state();
            let current_line = self.lines.remove(self.cursor.row);
            self.cursor.row -= 1;
            self.cursor.col = self.lines[self.cursor.row].chars().count();
            self.lines[self.cursor.row].push_str(&current_line);
            true
        } else {
            false
        }
    }

    /// Delete character at cursor (Delete key).
    pub fn delete(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }

        let line_len = self.current_line_len();
        if self.cursor.col < line_len {
            self.save_undo_state();
            let line = &mut self.lines[self.cursor.row];
            let byte_idx = char_to_byte_index(line, self.cursor.col);
            line.remove(byte_idx);
            true
        } else if self.cursor.row + 1 < self.lines.len() {
            // Join with next line
            self.save_undo_state();
            let next_line = self.lines.remove(self.cursor.row + 1);
            self.lines[self.cursor.row].push_str(&next_line);
            true
        } else {
            false
        }
    }

    // === Navigation ===

    /// Move cursor left.
    pub fn move_left(&mut self) -> bool {
        self.clear_selection();
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
            true
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
            self.cursor.col = self.current_line_len();
            true
        } else {
            false
        }
    }

    /// Move cursor right.
    pub fn move_right(&mut self) -> bool {
        self.clear_selection();
        let line_len = self.current_line_len();
        if self.cursor.col < line_len {
            self.cursor.col += 1;
            true
        } else if self.cursor.row + 1 < self.lines.len() {
            self.cursor.row += 1;
            self.cursor.col = 0;
            true
        } else {
            false
        }
    }

    /// Move cursor up.
    pub fn move_up(&mut self) -> bool {
        self.clear_selection();
        self.step_vertical(false)
    }

    /// Move cursor down.
    pub fn move_down(&mut self) -> bool {
        self.clear_selection();
        self.step_vertical(true)
    }

    /// Move the cursor one row up or down, leaving the selection to the
    /// caller. The row is a visual one when the last render recorded a
    /// soft-wrap that still matches the text, a logical one otherwise.
    /// Returns `false` at the first or last row, so a host can give the arrow
    /// another meaning there (prompt history).
    fn step_vertical(&mut self, down: bool) -> bool {
        if self.wrap_matches_text() {
            return self.step_visual(down);
        }
        let target = if down {
            self.cursor.row + 1
        } else {
            match self.cursor.row.checked_sub(1) {
                Some(row) => row,
                None => return false,
            }
        };
        if target >= self.lines.len() {
            return false;
        }
        self.cursor.row = target;
        self.cursor.col = self.cursor.col.min(self.current_line_len());
        true
    }

    /// Whether the recorded soft-wrap describes the current text: every
    /// logical row, in order, split into contiguous ranges covering it whole.
    /// An edit since the last render leaves it stale, and stepping by it would
    /// land on the wrong character.
    fn wrap_matches_text(&self) -> bool {
        if self.wrap_rows.is_empty() {
            return false;
        }
        let mut rows = self.wrap_rows.iter().peekable();
        for (r, line) in self.lines.iter().enumerate() {
            let len = line.chars().count();
            let mut next = 0;
            let mut seen = false;
            while let Some(&&(row, start, end)) = rows.peek() {
                if row != r {
                    break;
                }
                if start != next || end < start {
                    return false;
                }
                next = end;
                seen = true;
                rows.next();
            }
            if !seen || next != len {
                return false;
            }
        }
        rows.next().is_none()
    }

    /// [`TextArea::step_vertical`] over the recorded soft-wrap: keep the
    /// cursor's display column on the neighbouring visual row.
    fn step_visual(&mut self, down: bool) -> bool {
        let rows = &self.wrap_rows;
        // The cursor sits on the last visual row of its line that starts at or
        // before it: at a wrap boundary that is the row below, as rendered.
        let Some(current) = rows
            .iter()
            .rposition(|&(row, start, _)| row == self.cursor.row && start <= self.cursor.col)
        else {
            return false;
        };
        let target = if down {
            current + 1
        } else {
            match current.checked_sub(1) {
                Some(i) => i,
                None => return false,
            }
        };
        let Some(&(row, start, end)) = rows.get(target) else {
            return false;
        };
        let (_, cur_start, _) = rows[current];
        let line = &self.lines[self.cursor.row];
        let column = crate::grapheme_utils::str_display_width(
            &line
                .chars()
                .skip(cur_start)
                .take(self.cursor.col - cur_start)
                .collect::<String>(),
        );
        // Every row but a line's last ends where the next begins, so its
        // `end` itself is drawn on the row below: stop one short of it.
        let last_of_line = rows.get(target + 1).is_none_or(|next| next.0 != row);
        let limit = if last_of_line || end == start {
            end
        } else {
            end - 1
        };
        let mut col = start;
        let mut width = 0usize;
        for c in self.lines[row].chars().skip(start).take(limit - start) {
            let cw = crate::grapheme_utils::str_display_width(&c.to_string()).max(1);
            if width + cw > column {
                break;
            }
            width += cw;
            col += 1;
        }
        self.cursor = CursorPos::new(row, col);
        true
    }

    /// Move to start of line (Home).
    pub fn move_home(&mut self) {
        self.clear_selection();
        self.cursor.col = 0;
    }

    /// Move to end of line (End).
    pub fn move_end(&mut self) {
        self.clear_selection();
        self.cursor.col = self.current_line_len();
    }

    /// Move to start of text (Ctrl+Home).
    pub fn move_to_start(&mut self) {
        self.clear_selection();
        self.cursor = CursorPos::default();
    }

    /// Move to end of text (Ctrl+End).
    pub fn move_to_end(&mut self) {
        self.clear_selection();
        self.cursor.row = self.lines.len().saturating_sub(1);
        self.cursor.col = self.current_line_len();
    }

    // === Word navigation ===

    /// Char index of the word boundary before `col` on the cursor's line:
    /// back over trailing whitespace, punctuation, then the word itself.
    fn word_boundary_before(&self, col: usize) -> usize {
        let chars: Vec<char> = self.current_line().chars().collect();
        let mut pos = col.min(chars.len());
        while pos > 0 && chars[pos - 1].is_whitespace() {
            pos -= 1;
        }
        while pos > 0 && !chars[pos - 1].is_whitespace() && !chars[pos - 1].is_alphanumeric() {
            pos -= 1;
        }
        while pos > 0 && chars[pos - 1].is_alphanumeric() {
            pos -= 1;
        }
        pos
    }

    /// Char index of the word boundary after `col` on the cursor's line.
    fn word_boundary_after(&self, col: usize) -> usize {
        let chars: Vec<char> = self.current_line().chars().collect();
        let len = chars.len();
        let mut pos = col.min(len);
        while pos < len && chars[pos].is_alphanumeric() {
            pos += 1;
        }
        while pos < len && !chars[pos].is_whitespace() && !chars[pos].is_alphanumeric() {
            pos += 1;
        }
        while pos < len && chars[pos].is_whitespace() {
            pos += 1;
        }
        pos
    }

    /// Move one word left (Ctrl+Left); at the start of a line, to the end of
    /// the line above, like [`TextArea::move_left`].
    pub fn move_word_left(&mut self) -> bool {
        self.clear_selection();
        let target = self.word_boundary_before(self.cursor.col);
        if target < self.cursor.col {
            self.cursor.col = target;
            true
        } else {
            self.move_left()
        }
    }

    /// Move one word right (Ctrl+Right); at the end of a line, to the start of
    /// the line below, like [`TextArea::move_right`].
    pub fn move_word_right(&mut self) -> bool {
        self.clear_selection();
        let target = self.word_boundary_after(self.cursor.col);
        if target > self.cursor.col {
            self.cursor.col = target;
            true
        } else {
            self.move_right()
        }
    }

    // === Selection movement ===

    /// Move left with selection.
    pub fn move_left_with_selection(&mut self) -> bool {
        self.start_selection();
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
            true
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
            self.cursor.col = self.current_line_len();
            true
        } else {
            false
        }
    }

    /// Move right with selection.
    pub fn move_right_with_selection(&mut self) -> bool {
        self.start_selection();
        let line_len = self.current_line_len();
        if self.cursor.col < line_len {
            self.cursor.col += 1;
            true
        } else if self.cursor.row + 1 < self.lines.len() {
            self.cursor.row += 1;
            self.cursor.col = 0;
            true
        } else {
            false
        }
    }

    /// Move up with selection.
    pub fn move_up_with_selection(&mut self) -> bool {
        self.start_selection();
        self.step_vertical(false)
    }

    /// Move down with selection.
    pub fn move_down_with_selection(&mut self) -> bool {
        self.start_selection();
        self.step_vertical(true)
    }

    /// Move to home with selection.
    pub fn move_home_with_selection(&mut self) {
        self.start_selection();
        self.cursor.col = 0;
    }

    /// Move to end with selection.
    pub fn move_end_with_selection(&mut self) {
        self.start_selection();
        self.cursor.col = self.current_line_len();
    }

    /// Move one word left with selection (Ctrl+Shift+Left).
    pub fn move_word_left_with_selection(&mut self) -> bool {
        self.start_selection();
        let target = self.word_boundary_before(self.cursor.col);
        if target < self.cursor.col {
            self.cursor.col = target;
            true
        } else {
            self.move_left_with_selection()
        }
    }

    /// Move one word right with selection (Ctrl+Shift+Right).
    pub fn move_word_right_with_selection(&mut self) -> bool {
        self.start_selection();
        let target = self.word_boundary_after(self.cursor.col);
        if target > self.cursor.col {
            self.cursor.col = target;
            true
        } else {
            self.move_right_with_selection()
        }
    }

    // === Scrolling ===

    /// Ensure cursor is visible within given height.
    pub fn ensure_cursor_visible(&mut self, visible_height: usize) {
        if visible_height == 0 {
            return;
        }
        if self.cursor.row < self.scroll_offset {
            self.scroll_offset = self.cursor.row;
        } else if self.cursor.row >= self.scroll_offset + visible_height {
            self.scroll_offset = self.cursor.row - visible_height + 1;
        }
    }

    /// Set cursor position directly (for mouse clicks).
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.clear_selection();
        self.cursor.row = row.min(self.lines.len().saturating_sub(1));
        self.cursor.col = col.min(self.current_line_len());
    }

    /// Place the cursor for a mouse press, dropping any selection: the anchor
    /// lands on the same spot, so a drag from here selects from where the
    /// button went down.
    pub fn place_cursor(&mut self, row: usize, col: usize) {
        self.set_cursor(row, col);
        self.selection_anchor = Some(self.cursor);
    }

    /// Extend the selection to `(row, col)` for a mouse drag, starting one at
    /// the current cursor when there is no selection in flight.
    pub fn extend_selection_to(&mut self, row: usize, col: usize) {
        if self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.cursor);
        }
        self.cursor.row = row.min(self.lines.len().saturating_sub(1));
        let line_len = self.lines[self.cursor.row].chars().count();
        self.cursor.col = col.min(line_len);
    }

    // === Soft-wrap geometry ===

    /// Record how the text was laid out by the last render: `rows` holds one
    /// `(logical row, first char, end char)` per visual row, `scroll` the
    /// first visual row drawn. [`TextArea::position_at_drawn_row`] then maps a
    /// click on a wrapped row back to the character under it.
    pub fn set_wrap(&mut self, rows: Vec<(usize, usize, usize)>, scroll: usize) {
        self.wrap_rows = rows;
        self.visual_scroll = scroll;
    }

    /// The text position under a row of the drawn field area: `drawn_row`
    /// counts rows from the top of the area the last render used (so it is
    /// already offset-free, and can be the mouse row minus the area's `y`) and
    /// `column` counts display columns from the start of the text. Without a
    /// recorded wrap it treats the row as a scrolled logical row, and without
    /// any geometry it keeps the cursor where it is.
    #[must_use]
    pub fn position_at_drawn_row(&self, drawn_row: usize, column: usize) -> CursorPos {
        let (row, start) = match self
            .wrap_rows
            .get(self.visual_scroll + drawn_row)
            .copied()
            .map(|(row, start, _)| (row, start))
            .or_else(|| {
                // Unwrapped: the rows on screen are `scroll_offset` onward.
                let row = self.scroll_offset + drawn_row;
                (row < self.lines.len()).then_some((row, 0))
            }) {
            Some(pair) => pair,
            None => return self.cursor,
        };
        let line = self.lines.get(row).map(|s| s.as_str()).unwrap_or_default();
        let mut col = start;
        let mut width = 0usize;
        for c in line.chars().skip(start) {
            let cw = crate::grapheme_utils::str_display_width(&c.to_string()).max(1);
            if width + cw > column {
                break;
            }
            width += cw;
            col += 1;
        }
        CursorPos::new(row, col)
    }
}

/// Convert character position to byte index in a string.
fn char_to_byte_index(s: &str, char_pos: usize) -> usize {
    s.char_indices()
        .nth(char_pos)
        .map(|(idx, _)| idx)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_textarea() {
        let ta = TextArea::new();
        assert_eq!(ta.lines().len(), 1);
        assert_eq!(ta.lines()[0], "");
        assert!(ta.is_empty());
    }

    #[test]
    fn test_with_text() {
        let ta = TextArea::with_text("hello\nworld");
        assert_eq!(ta.lines().len(), 2);
        assert_eq!(ta.lines()[0], "hello");
        assert_eq!(ta.lines()[1], "world");
        assert_eq!(ta.cursor(), CursorPos::new(1, 5));
    }

    #[test]
    fn test_insert_char() {
        let mut ta = TextArea::new();
        ta.insert('a');
        ta.insert('b');
        assert_eq!(ta.text(), "ab");
        assert_eq!(ta.cursor(), CursorPos::new(0, 2));
    }

    #[test]
    fn test_insert_newline() {
        let mut ta = TextArea::with_text("hello");
        ta.set_cursor(0, 2);
        ta.insert_newline();
        assert_eq!(ta.lines().len(), 2);
        assert_eq!(ta.lines()[0], "he");
        assert_eq!(ta.lines()[1], "llo");
    }

    #[test]
    fn test_backspace_join_lines() {
        let mut ta = TextArea::with_text("hello\nworld");
        ta.set_cursor(1, 0);
        ta.backspace();
        assert_eq!(ta.lines().len(), 1);
        assert_eq!(ta.text(), "helloworld");
    }

    #[test]
    fn test_navigation() {
        let mut ta = TextArea::with_text("ab\ncd");
        ta.set_cursor(0, 0);

        ta.move_right();
        assert_eq!(ta.cursor(), CursorPos::new(0, 1));

        ta.move_down();
        assert_eq!(ta.cursor(), CursorPos::new(1, 1));

        ta.move_left();
        assert_eq!(ta.cursor(), CursorPos::new(1, 0));

        ta.move_up();
        assert_eq!(ta.cursor(), CursorPos::new(0, 0));
    }

    #[test]
    fn test_selection() {
        let mut ta = TextArea::with_text("hello");
        ta.set_cursor(0, 0);
        ta.select_all();
        assert!(ta.has_selection());
        assert_eq!(ta.selected_text(), Some("hello".to_string()));
    }

    #[test]
    fn word_navigation_walks_words_then_lines() {
        // The boundary walk matches `TextInput`: back over whitespace, then
        // punctuation, then the word.
        let mut ta = TextArea::with_text("let x = 1;\nnext");
        ta.set_cursor(0, 10);
        assert!(ta.move_word_left());
        assert_eq!(ta.cursor(), CursorPos::new(0, 8));
        assert!(ta.move_word_left());
        assert_eq!(ta.cursor(), CursorPos::new(0, 6));
        assert!(ta.move_word_left());
        assert_eq!(ta.cursor(), CursorPos::new(0, 4));
        assert!(ta.move_word_left());
        assert_eq!(ta.cursor(), CursorPos::new(0, 0));
        // At the start of a line the word jump wraps to the line above's end.
        ta.set_cursor(1, 0);
        assert!(ta.move_word_left());
        assert_eq!(ta.cursor(), CursorPos::new(0, 10));

        ta.set_cursor(0, 0);
        assert!(ta.move_word_right());
        assert_eq!(ta.cursor(), CursorPos::new(0, 4));
        assert!(ta.move_word_right());
        assert_eq!(ta.cursor(), CursorPos::new(0, 6));
    }

    #[test]
    fn word_selection_extends_from_the_anchor() {
        let mut ta = TextArea::with_text("hello world");
        ta.set_cursor(0, 5);
        // The anchor stays put while the cursor walks the word boundaries.
        assert!(ta.move_word_right_with_selection());
        assert_eq!(ta.selected_text(), Some(" ".to_string()));
        assert!(ta.move_word_right_with_selection());
        assert_eq!(ta.selected_text(), Some(" world".to_string()));
        assert!(ta.move_word_left_with_selection());
        assert_eq!(ta.selected_text(), Some(" ".to_string()));
        assert!(ta.move_word_left_with_selection());
        assert_eq!(ta.selected_text(), Some("hello".to_string()));
    }

    #[test]
    fn a_press_then_drag_selects_across_lines() {
        let mut ta = TextArea::with_text("one\ntwo\nthree");
        ta.place_cursor(0, 2);
        assert!(!ta.has_selection(), "a press alone selects nothing");
        ta.extend_selection_to(2, 2);
        assert_eq!(ta.selected_text(), Some("e\ntwo\nth".to_string()));
        // A plain move drops the selection, as after a keyboard edit.
        ta.move_right();
        assert!(!ta.has_selection());
    }

    #[test]
    fn vertical_moves_follow_the_recorded_wrap_while_it_matches() {
        // "abcdef" drawn as "abc" / "def", then "wxyz" on its own line.
        let mut ta = TextArea::with_text("abcdef\nwxyz");
        ta.set_wrap(vec![(0, 0, 3), (0, 3, 6), (1, 0, 4)], 0);
        ta.set_cursor(1, 1);
        assert!(ta.move_up());
        assert_eq!(ta.cursor(), CursorPos::new(0, 4));
        assert!(ta.move_up());
        assert_eq!(ta.cursor(), CursorPos::new(0, 1));
        assert!(!ta.move_up(), "the first visual row is the edge");
        assert!(ta.move_down());
        assert!(ta.move_down());
        assert_eq!(ta.cursor(), CursorPos::new(1, 1));
        assert!(!ta.move_down(), "the last visual row is the edge");
        // Past the end of a row that wraps, the column stops short of the
        // boundary, which is drawn on the row below.
        ta.set_cursor(1, 4);
        assert!(ta.move_up());
        assert_eq!(ta.cursor(), CursorPos::new(0, 6));
        assert!(ta.move_up());
        assert_eq!(ta.cursor(), CursorPos::new(0, 2));
        // An edit since the render makes the wrap stale: logical rows again.
        ta.insert('x');
        ta.set_cursor(1, 0);
        assert!(ta.move_up());
        assert_eq!(ta.cursor(), CursorPos::new(0, 0));
        assert!(!ta.move_up());
    }

    #[test]
    fn drawn_row_maps_a_wrapped_click_to_its_character() {
        let mut ta = TextArea::with_text("abcdef");
        // "abcdef" drawn as "abc" / "def".
        ta.set_wrap(vec![(0, 0, 3), (0, 3, 6)], 0);
        assert_eq!(ta.position_at_drawn_row(0, 1), CursorPos::new(0, 1));
        assert_eq!(ta.position_at_drawn_row(1, 1), CursorPos::new(0, 4));
        // With the view scrolled to the second row, row 0 is that row.
        ta.set_wrap(vec![(0, 0, 3), (0, 3, 6)], 1);
        assert_eq!(ta.position_at_drawn_row(0, 1), CursorPos::new(0, 4));
        // No geometry recorded: the row is read as a scrolled logical row.
        let mut plain = TextArea::with_text("one\ntwo");
        plain.set_scroll_offset(1);
        assert_eq!(plain.position_at_drawn_row(0, 1), CursorPos::new(1, 1));
    }
}
