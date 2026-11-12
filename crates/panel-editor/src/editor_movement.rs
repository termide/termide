//! Cursor movement methods for the Editor.
//!
//! This module contains all cursor movement functionality including:
//! - Physical line movement (up, down, left, right)
//! - Visual line movement (accounting for word wrap)
//! - Page up/down navigation
//! - Document start/end navigation
//! - Selection operations

use termide_buffer::Cursor;

use crate::word_wrap::WrapLayout;
use crate::{cursor, selection, word_wrap};

use super::Editor;

impl Editor {
    // =========================================================================
    // Physical Cursor Movement
    // =========================================================================

    /// Put the cursor, just moved to another line, in the screen column it
    /// had (`display_col`) instead of at the same grapheme index: a tab
    /// before it on either line would otherwise shift it sideways.
    fn land_in_display_column(&mut self, display_col: usize) {
        use unicode_segmentation::UnicodeSegmentation;
        let line = self
            .buffer
            .line_cow(self.cursor.line)
            .map(|l| l.trim_end_matches('\n').to_string())
            .unwrap_or_default();
        let len = line.graphemes(true).count();
        self.cursor.column =
            word_wrap::column_at_row_offset(&line, 0, len, display_col, self.config.tab_size);
    }

    /// Move cursor up
    pub(crate) fn move_cursor_up(&mut self) {
        self.begin_vertical_move();
        let display_col = self.input.preferred_column.unwrap_or(0);
        cursor::physical::move_up(&mut self.cursor, &self.buffer);
        self.land_in_display_column(display_col);
        self.clamp_cursor();
        self.end_vertical_move();
    }

    /// Move cursor down
    pub(crate) fn move_cursor_down(&mut self) {
        self.begin_vertical_move();
        let display_col = self.input.preferred_column.unwrap_or(0);
        cursor::physical::move_down(&mut self.cursor, &self.buffer);
        self.land_in_display_column(display_col);
        self.clamp_cursor();
        self.end_vertical_move();
    }

    /// Move cursor left
    pub(crate) fn move_cursor_left(&mut self) {
        let maintain_preferred = cursor::physical::move_left(&mut self.cursor, &self.buffer);
        if !maintain_preferred {
            self.input.preferred_column = None;
        }
    }

    /// Move cursor right
    pub(crate) fn move_cursor_right(&mut self) {
        let maintain_preferred = cursor::physical::move_right(&mut self.cursor, &self.buffer);
        if !maintain_preferred {
            self.input.preferred_column = None;
        }
        self.clamp_cursor();
    }

    /// Move cursor to start of line
    pub(crate) fn move_to_line_start(&mut self) {
        let maintain_preferred = cursor::physical::move_to_line_start(&mut self.cursor);
        if !maintain_preferred {
            self.input.preferred_column = None;
        }
    }

    /// Move cursor to end of line
    pub(crate) fn move_to_line_end(&mut self) {
        let maintain_preferred = cursor::physical::move_to_line_end(&mut self.cursor, &self.buffer);
        if !maintain_preferred {
            self.input.preferred_column = None;
        }
    }

    /// Move cursor to start of document
    pub(crate) fn move_to_document_start(&mut self) {
        let (new_cursor, should_scroll) = cursor::physical::move_to_document_start();
        self.cursor = new_cursor;
        if should_scroll {
            self.viewport.scroll_to_top();
        }
    }

    /// Move cursor forward by one word
    pub(crate) fn move_word_forward(&mut self) {
        let maintain_preferred =
            cursor::physical::move_word_forward(&mut self.cursor, &self.buffer);
        if !maintain_preferred {
            self.input.preferred_column = None;
        }
        self.clamp_cursor();
    }

    /// Move cursor backward by one word
    pub(crate) fn move_word_backward(&mut self) {
        let maintain_preferred =
            cursor::physical::move_word_backward(&mut self.cursor, &self.buffer);
        if !maintain_preferred {
            self.input.preferred_column = None;
        }
        self.clamp_cursor();
    }

    /// Move cursor to previous paragraph/symbol boundary (Ctrl+Up).
    pub(crate) fn move_paragraph_up(&mut self) {
        if !self.symbol_lines.is_empty() {
            let target = self
                .symbol_lines
                .iter()
                .rev()
                .find(|&&line| line < self.cursor.line);
            self.cursor.line = target.copied().unwrap_or(0);
        } else {
            let mut line = self.cursor.line;
            while line > 0 && self.is_line_blank(line) {
                line -= 1;
            }
            while line > 0 && !self.is_line_blank(line) {
                line -= 1;
            }
            self.cursor.line = line;
        }
        self.cursor.column = 0;
        self.input.preferred_column = None;
        self.clamp_cursor();
    }

    /// Move cursor to next paragraph/symbol boundary (Ctrl+Down).
    pub(crate) fn move_paragraph_down(&mut self) {
        let max_line = self.buffer.line_count().saturating_sub(1);
        if !self.symbol_lines.is_empty() {
            let target = self
                .symbol_lines
                .iter()
                .find(|&&line| line > self.cursor.line);
            self.cursor.line = target.copied().unwrap_or(max_line);
        } else {
            let mut line = self.cursor.line;
            while line < max_line && self.is_line_blank(line) {
                line += 1;
            }
            while line < max_line && !self.is_line_blank(line) {
                line += 1;
            }
            self.cursor.line = line;
        }
        self.cursor.column = 0;
        self.input.preferred_column = None;
        self.clamp_cursor();
    }

    fn is_line_blank(&self, line: usize) -> bool {
        self.buffer
            .line(line)
            .map(|l| l.trim_end_matches('\n').trim().is_empty())
            .unwrap_or(true)
    }

    /// Move cursor to end of document
    pub(crate) fn move_to_document_end(&mut self) {
        let (new_cursor, should_scroll) = cursor::physical::move_to_document_end(&self.buffer);
        self.cursor = new_cursor;
        if should_scroll {
            // Use cached virtual line count for viewport scroll
            let total = self.render_cache.virtual_line_count;
            self.in_visible_lines(|viewport| viewport.scroll_to_bottom(total));
        }
    }

    // =========================================================================
    // Visual Cursor Movement (Word Wrap Aware)
    // =========================================================================

    /// Move cursor up by one visual line (accounting for word wrap)
    pub(crate) fn move_cursor_up_visual(&mut self) {
        let content_width = self.render_cache.content_width;
        if content_width == 0 {
            self.move_cursor_up();
            return;
        }

        self.begin_vertical_move();

        let use_smart_wrap = self.render_cache.use_smart_wrap;
        let cursor_pos = (self.cursor.line, self.cursor.column);
        let preferred_column = self.input.preferred_column;

        // Use cached version for better performance
        if let Some((line, col)) = word_wrap::move_up_cached(
            &mut self.render_cache,
            &self.buffer,
            cursor_pos,
            preferred_column,
            WrapLayout {
                width: content_width,
                smart: use_smart_wrap,
                tab_size: self.config.tab_size,
            },
        ) {
            self.cursor = Cursor::at(line, col);
        }

        self.clamp_cursor();
        self.end_vertical_move();
    }

    /// Move cursor down by one visual line (accounting for word wrap)
    pub(crate) fn move_cursor_down_visual(&mut self) {
        let content_width = self.render_cache.content_width;
        if content_width == 0 {
            self.move_cursor_down();
            return;
        }

        self.begin_vertical_move();

        let use_smart_wrap = self.render_cache.use_smart_wrap;
        let cursor_pos = (self.cursor.line, self.cursor.column);
        let preferred_column = self.input.preferred_column;

        // Use cached version for better performance
        if let Some((line, col)) = word_wrap::move_down_cached(
            &mut self.render_cache,
            &self.buffer,
            cursor_pos,
            preferred_column,
            WrapLayout {
                width: content_width,
                smart: use_smart_wrap,
                tab_size: self.config.tab_size,
            },
        ) {
            self.cursor = Cursor::at(line, col);
        }

        self.clamp_cursor();
        self.end_vertical_move();
    }

    /// Move cursor to start of visual line (for wrapped lines)
    pub(crate) fn move_to_visual_line_start(&mut self) {
        // Reset preferred column on horizontal movement
        self.input.preferred_column = None;

        if self.render_cache.content_width == 0 {
            // No word wrap - fall back to physical line start
            self.move_to_line_start();
            return;
        }

        self.cursor.column = cursor::visual::move_to_visual_line_start(
            &self.cursor,
            &self.buffer,
            self.wrap_layout(),
        );
    }

    /// Move cursor to end of visual line (for wrapped lines)
    pub(crate) fn move_to_visual_line_end(&mut self) {
        // Reset preferred column on horizontal movement
        self.input.preferred_column = None;

        if self.render_cache.content_width == 0 {
            // No word wrap - fall back to physical line end
            self.move_to_line_end();
            return;
        }

        self.cursor.column =
            cursor::visual::move_to_visual_line_end(&self.cursor, &self.buffer, self.wrap_layout());
    }

    // =========================================================================
    // Page Navigation
    // =========================================================================

    /// Move cursor page up
    pub(crate) fn page_up(&mut self) {
        let page_size = self.viewport.height;
        // A full page keeps the screen column; the jump to the document
        // start sets its own.
        let keeps_column = self.visible_index(self.cursor.line) >= page_size;
        self.begin_vertical_move();
        let display_col = self.input.preferred_column.unwrap_or(0);
        let (should_scroll, scroll_amount) =
            cursor::jump::page_up(&mut self.cursor, &self.buffer, page_size);
        if keeps_column {
            self.land_in_display_column(display_col);
        } else {
            self.input.preferred_column = None;
        }
        self.clamp_cursor();
        self.end_vertical_move();
        if should_scroll {
            self.in_visible_lines(|viewport| viewport.scroll_up(scroll_amount));
        }
    }

    /// Move cursor page down
    pub(crate) fn page_down(&mut self) {
        let page_size = self.viewport.height;
        // A full page keeps the screen column; the jump to the document end
        // sets its own.
        let max_line = self
            .buffer
            .visible_at_or_before(self.buffer.line_count().saturating_sub(1));
        let keeps_column = self
            .visible_index(max_line)
            .saturating_sub(self.visible_index(self.cursor.line))
            >= page_size;
        self.begin_vertical_move();
        let display_col = self.input.preferred_column.unwrap_or(0);
        let (should_scroll, scroll_amount) =
            cursor::jump::page_down(&mut self.cursor, &self.buffer, page_size);
        if keeps_column {
            self.land_in_display_column(display_col);
        } else {
            self.input.preferred_column = None;
        }
        self.clamp_cursor();
        self.end_vertical_move();
        if should_scroll {
            // Use cached virtual line count for viewport scroll (accounts for deletion markers)
            let total = self.render_cache.virtual_line_count;
            self.in_visible_lines(|viewport| viewport.scroll_down(scroll_amount, total));
        }
    }

    /// Move cursor page up by visual lines (accounting for word wrap)
    pub(crate) fn page_up_visual(&mut self) {
        let content_width = self.render_cache.content_width;
        if content_width == 0 {
            // No word wrap - fall back to physical line movement
            self.page_up();
            return;
        }

        self.begin_vertical_move();

        let use_smart_wrap = self.render_cache.use_smart_wrap;
        let cursor_pos = (self.cursor.line, self.cursor.column);
        let preferred_column = self.input.preferred_column;
        let page_size = self.viewport.height;

        // Use cached version for better performance
        let (line, col) = word_wrap::page_up_cached(
            &mut self.render_cache,
            &self.buffer,
            cursor_pos,
            preferred_column,
            WrapLayout {
                width: content_width,
                smart: use_smart_wrap,
                tab_size: self.config.tab_size,
            },
            page_size,
        );
        self.cursor = Cursor::at(line, col);
        self.end_vertical_move();

        // Don't manually scroll viewport - let ensure_cursor_visible() handle it during rendering
        // This is correct because the viewport needs to track visual rows, not buffer lines
    }

    /// Move cursor page down by visual lines (accounting for word wrap)
    pub(crate) fn page_down_visual(&mut self) {
        let content_width = self.render_cache.content_width;
        if content_width == 0 {
            // No word wrap - fall back to physical line movement
            self.page_down();
            return;
        }

        self.begin_vertical_move();

        let use_smart_wrap = self.render_cache.use_smart_wrap;
        let cursor_pos = (self.cursor.line, self.cursor.column);
        let preferred_column = self.input.preferred_column;
        let page_size = self.viewport.height;

        // Use cached version for better performance
        let (line, col) = word_wrap::page_down_cached(
            &mut self.render_cache,
            &self.buffer,
            cursor_pos,
            preferred_column,
            WrapLayout {
                width: content_width,
                smart: use_smart_wrap,
                tab_size: self.config.tab_size,
            },
            page_size,
        );
        self.cursor = Cursor::at(line, col);
        self.end_vertical_move();

        // Don't manually scroll viewport - let ensure_cursor_visible() handle it during rendering
        // This is correct because the viewport needs to track visual rows, not buffer lines
    }

    // =========================================================================
    // Selection Operations
    // =========================================================================

    /// Select all
    pub(crate) fn select_all(&mut self) {
        let (new_selection, new_cursor) = selection::select_all(&self.buffer);
        self.selection = Some(new_selection);
        self.cursor = new_cursor;
    }

    /// Start new selection or continue existing
    pub(crate) fn start_or_extend_selection(&mut self) {
        if let Some(new_selection) =
            selection::start_or_extend_selection(self.selection.as_ref(), self.cursor)
        {
            self.selection = Some(new_selection);
        }
    }

    /// Update active point of selection (after cursor movement)
    pub(crate) fn update_selection_active(&mut self) {
        selection::update_selection_active(&mut self.selection, self.cursor);
    }
}
