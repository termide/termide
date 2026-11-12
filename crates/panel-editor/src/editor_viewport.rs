//! Viewport and scrolling methods for the Editor.
//!
//! This module contains all viewport management functionality including:
//! - Word-wrap-aware cursor visibility
//! - Visual row scrolling (up/down)
//! - Auto-scroll during mouse selection
//! - Virtual line counting (wrap + deletion markers + diagnostics)

use termide_buffer::Cursor;
use termide_config::Config;

use crate::word_wrap::{self, WrapLayout};

use super::Editor;

impl Editor {
    /// The cursor with its column in display columns (tabs expanded, wide
    /// characters counted twice): the unit `Viewport::left_column` scrolls in
    /// without word wrap.
    pub(crate) fn cursor_in_display_columns(&self) -> Cursor {
        let column = self
            .buffer
            .line_cow(self.cursor.line)
            .map(|line| {
                termide_buffer::display_column(
                    line.trim_end_matches('\n'),
                    self.cursor.column,
                    self.config.tab_size,
                )
            })
            .unwrap_or(self.cursor.column);
        Cursor::at(self.cursor.line, column)
    }

    /// The layout the last render wrapped lines with, for wrap-aware
    /// movement and scrolling.
    pub(crate) fn wrap_layout(&self) -> WrapLayout {
        WrapLayout {
            width: self.render_cache.content_width,
            smart: self.render_cache.use_smart_wrap,
            tab_size: self.config.tab_size,
        }
    }

    /// Check if visual movement should be used (word wrap enabled and width cached).
    pub(crate) fn should_use_visual_movement(&self) -> bool {
        self.config.word_wrap && self.render_cache.content_width > 0
    }

    /// Count virtual rows — git deletion markers and diagnostic rows — rendered
    /// for buffer lines in `[start, end)`. These occupy vertical space on
    /// screen but are not buffer lines, so anything positioned by logical line
    /// (the cursor, the inline blame annotation) must shift down by this many
    /// rows. A line's own deletion marker renders *after* its text, so the
    /// range is exclusive of `end` (the cursor line is not shifted by its own
    /// marker).
    pub(crate) fn count_virtual_rows_between(
        &self,
        start: usize,
        end: usize,
        content_width: usize,
    ) -> usize {
        let mut extra_rows = 0;
        let show_git_diff = self.render_cache.config.editor.show_git_diff;
        // Deduplicated like the drawing, so the rows counted are the rows drawn.
        let diagnostic_rows = word_wrap::count_diagnostic_rows_by_line(
            &self.lsp.diagnostics,
            &self.buffer,
            content_width,
            self.config.tab_size,
        );

        for line in start..end {
            if self.buffer.is_line_hidden(line) {
                continue;
            }
            // Deletion markers (rendered between text and diagnostics).
            if show_git_diff {
                if let Some(ref git_diff) = self.git.diff_cache {
                    if git_diff.has_deletion_marker(line) {
                        extra_rows += 1;
                    }
                }
            }

            // Diagnostic rows.
            extra_rows += diagnostic_rows.get(&line).copied().unwrap_or(0);
        }
        extra_rows
    }

    /// Start a vertical move: settle the screen column it keeps.
    ///
    /// The column the previous vertical move kept holds while the cursor
    /// still stands where that move left it, so passing through a short line
    /// does not lose it. Anything else that moved the cursor (a click, a
    /// jump, a search, an edit) makes it stale, and the cursor's own column
    /// is taken instead.
    pub(crate) fn begin_vertical_move(&mut self) {
        if self.input.preferred_column_cursor != Some((self.cursor.line, self.cursor.column)) {
            self.input.preferred_column = None;
        }
        self.ensure_preferred_column();
    }

    /// Finish a vertical move: record where it left the cursor, for
    /// [`Self::begin_vertical_move`].
    pub(crate) fn end_vertical_move(&mut self) {
        self.input.preferred_column_cursor = Some((self.cursor.line, self.cursor.column));
    }

    /// Ensure preferred column is set for vertical navigation.
    ///
    /// Sets preferred_column to the visual offset, in screen columns, within
    /// the current visual row if not already set.
    /// Used by visual movement methods to maintain column across wrapped lines.
    pub(crate) fn ensure_preferred_column(&mut self) {
        if self.input.preferred_column.is_none() {
            // Calculate visual offset (position within current visual row)
            let content_width = self.render_cache.content_width;
            let use_smart_wrap = self.render_cache.use_smart_wrap;

            let visual_offset = if content_width > 0 {
                if let Some(line_text) = self.buffer.line(self.cursor.line) {
                    use unicode_segmentation::UnicodeSegmentation;
                    let line_text = line_text.trim_end_matches('\n');
                    let line_len = line_text.graphemes(true).count();
                    let cursor_col = self.cursor.column.min(line_len);
                    // Use cached wrap points to avoid recalculation
                    let (_visual_rows, wrap_points) = word_wrap::get_line_wrap_points_cached(
                        &mut self.render_cache,
                        &self.buffer,
                        self.cursor.line,
                        WrapLayout {
                            width: content_width,
                            smart: use_smart_wrap,
                            tab_size: self.config.tab_size,
                        },
                    );
                    let current_visual_row =
                        wrap_points.iter().filter(|&&wp| wp <= cursor_col).count();
                    let visual_row_start = if current_visual_row == 0 {
                        0
                    } else if current_visual_row - 1 < wrap_points.len() {
                        wrap_points[current_visual_row - 1]
                    } else {
                        0
                    };
                    word_wrap::row_offset_columns(
                        line_text,
                        visual_row_start,
                        cursor_col,
                        self.config.tab_size,
                    )
                } else {
                    self.cursor.column
                }
            } else {
                self.cursor_in_display_columns().column
            };
            self.input.preferred_column = Some(visual_offset);
        }
    }

    /// Ensure cursor is visible when word wrap is enabled.
    /// This is more complex than the standard ensure_cursor_visible because we need
    /// to work with visual rows, not physical lines.
    ///
    /// Optimized to avoid O(n) iteration through lines by using direct calculations.
    pub(crate) fn ensure_cursor_visible_word_wrap(&mut self, content_height: usize) {
        if content_height == 0 || self.render_cache.content_width == 0 {
            return;
        }

        let content_width = self.render_cache.content_width;
        let use_smart_wrap = self.render_cache.use_smart_wrap;

        // Get cursor's visual row within its own line (uses cache for O(1) after first call)
        let cursor_visual_row_in_line = word_wrap::get_cursor_visual_row_in_line_cached(
            &mut self.render_cache,
            &self.buffer,
            self.cursor.line,
            self.cursor.column,
            WrapLayout {
                width: content_width,
                smart: use_smart_wrap,
                tab_size: self.config.tab_size,
            },
        );

        // Handle cursor above viewport (physical line check)
        if self.cursor.line < self.viewport.top_line {
            self.viewport.top_line = self.cursor.line;
            self.viewport.top_visual_row_offset = cursor_visual_row_in_line;
            return;
        }

        // If cursor is on the same line as top_line, check if cursor is above visible area
        if self.cursor.line == self.viewport.top_line {
            if cursor_visual_row_in_line < self.viewport.top_visual_row_offset {
                // Cursor is above visible area - scroll up within line
                self.viewport.top_visual_row_offset = cursor_visual_row_in_line;
                return;
            }

            // Check if cursor is below visible area (within same line)
            let visible_row = cursor_visual_row_in_line - self.viewport.top_visual_row_offset;
            if visible_row >= content_height {
                // Position cursor at bottom of viewport
                self.viewport.top_visual_row_offset =
                    cursor_visual_row_in_line.saturating_sub(content_height - 1);
            }
            return;
        }

        // Cursor is on a different line than top_line (cursor.line > top_line)
        // Calculate visual distance from top of viewport to cursor

        // Visual rows from top_line (after offset) to end of top_line
        let top_line_visual_rows = word_wrap::get_visual_rows_cached(
            &mut self.render_cache,
            &self.buffer,
            self.viewport.top_line,
            WrapLayout {
                width: content_width,
                smart: use_smart_wrap,
                tab_size: self.config.tab_size,
            },
        );
        let rows_remaining_in_top_line =
            top_line_visual_rows.saturating_sub(self.viewport.top_visual_row_offset);

        // Count virtual rows between viewport top and cursor line
        // These take up visual space but aren't accounted for by wrap row counts
        let virtual_rows_between = self.count_virtual_rows_between(
            self.viewport.top_line,
            self.cursor.line,
            content_width,
        );

        // Visual rows for lines between top_line and cursor_line (exclusive)
        // Use cumulative cache for O(1) lookup when available
        let visual_rows_between = if self.render_cache.is_cumulative_valid()
            && self.cursor.line > 0
            && self.viewport.top_line + 1 < self.cursor.line
        {
            // cumulative[cursor_line - 1] - cumulative[top_line] gives us
            // sum of visual rows for lines (top_line + 1)..(cursor_line)
            let start_cumulative = self
                .render_cache
                .get_cumulative_visual_rows(self.viewport.top_line)
                .unwrap_or(0);
            let end_cumulative = self
                .render_cache
                .get_cumulative_visual_rows(self.cursor.line - 1)
                .unwrap_or(0);
            end_cumulative.saturating_sub(start_cumulative)
        } else if self.viewport.top_line + 1 < self.cursor.line {
            // Fallback to O(n) loop if cumulative cache is not valid
            let mut rows = 0;
            for line in (self.viewport.top_line + 1)..self.cursor.line {
                if self.buffer.is_line_hidden(line) {
                    continue;
                }
                let line_visual_rows = word_wrap::get_visual_rows_cached(
                    &mut self.render_cache,
                    &self.buffer,
                    line,
                    WrapLayout {
                        width: content_width,
                        smart: use_smart_wrap,
                        tab_size: self.config.tab_size,
                    },
                );
                rows += line_visual_rows;
            }
            rows
        } else {
            0
        };

        // Total visual rows from viewport top to cursor position
        // Include virtual rows (deletion markers + diagnostics) between viewport top and cursor
        let cursor_visual_pos = rows_remaining_in_top_line
            + visual_rows_between
            + virtual_rows_between
            + cursor_visual_row_in_line;

        // If cursor is visible, no scrolling needed
        if cursor_visual_pos < content_height {
            return;
        }

        // Cursor is below visible area - need to scroll down
        // Calculate target: place cursor at bottom of viewport
        let scroll_needed = cursor_visual_pos - (content_height - 1);

        // Apply scroll directly by computing final position
        self.apply_visual_scroll_down(
            scroll_needed,
            WrapLayout {
                width: content_width,
                smart: use_smart_wrap,
                tab_size: self.config.tab_size,
            },
        );
    }

    /// Apply scroll down by a given number of visual rows.
    /// Updates top_line and top_visual_row_offset directly.
    /// Accounts for deletion markers and diagnostic virtual rows between lines.
    fn apply_visual_scroll_down(&mut self, mut remaining: usize, layout: WrapLayout) {
        let WrapLayout {
            width: content_width,
            tab_size,
            ..
        } = layout;
        let show_git_diff = self.render_cache.config.editor.show_git_diff;
        // Deduplicated like the drawing, so the rows skipped are the rows drawn.
        let diagnostic_rows = word_wrap::count_diagnostic_rows_by_line(
            &self.lsp.diagnostics,
            &self.buffer,
            content_width,
            tab_size,
        );

        while remaining > 0 && self.viewport.top_line < self.buffer.line_count() {
            let line_visual_rows = word_wrap::get_visual_rows_cached(
                &mut self.render_cache,
                &self.buffer,
                self.viewport.top_line,
                layout,
            );

            let rows_available =
                line_visual_rows.saturating_sub(self.viewport.top_visual_row_offset);

            if remaining < rows_available {
                // Scroll within current line
                self.viewport.top_visual_row_offset += remaining;
                return;
            }

            // Consume remaining text rows for this line
            remaining -= rows_available;

            // Count virtual rows after this line (deletion markers + diagnostics)
            let mut virtual_after_line = 0;
            if show_git_diff {
                if let Some(ref git_diff) = self.git.diff_cache {
                    if git_diff.has_deletion_marker(self.viewport.top_line) {
                        virtual_after_line += 1;
                    }
                }
            }
            virtual_after_line += diagnostic_rows
                .get(&self.viewport.top_line)
                .copied()
                .unwrap_or(0);

            // Consume virtual rows
            if remaining <= virtual_after_line {
                // We stop within the virtual rows — advance to next line anyway
                // since virtual rows aren't addressable for viewport positioning
                remaining = 0;
            } else {
                remaining -= virtual_after_line;
            }

            // Move to the next line that shows
            self.viewport.top_line = self.buffer.visible_at_or_after(self.viewport.top_line + 1);
            self.viewport.top_visual_row_offset = 0;
        }

        // Clamp to valid range
        if self.viewport.top_line >= self.buffer.line_count() {
            let last = self.buffer.line_count().saturating_sub(1);
            self.viewport.top_line = self.buffer.visible_at_or_before(last);
            self.viewport.top_visual_row_offset = 0;
        }
    }

    /// Scroll up by visual rows (accounting for word wrap).
    /// Used for mouse scroll and other visual-based navigation.
    pub(crate) fn scroll_visual_rows_up(&mut self, count: usize) {
        // Use viewport.width as fallback when render_cache.content_width is not yet set
        // This handles the case when mouse events are processed before first render
        let content_width = if self.render_cache.content_width > 0 {
            self.render_cache.content_width
        } else {
            self.viewport.width
        };

        if !self.config.word_wrap || content_width == 0 {
            // Fallback to buffer line scrolling
            self.in_visible_lines(|viewport| viewport.scroll_up(count));
            return;
        }

        let use_smart_wrap = self.render_cache.use_smart_wrap;

        // Ensure cache is valid for current width settings
        self.render_cache.update_wrap_settings(WrapLayout {
            width: content_width,
            smart: use_smart_wrap,
            tab_size: self.config.tab_size,
        });

        for _ in 0..count {
            if self.viewport.top_visual_row_offset > 0 {
                // Scroll within current line
                self.viewport.top_visual_row_offset -= 1;
            } else if self.viewport.top_line > 0 {
                // Move to the last visual row of the previous line that shows
                self.viewport.top_line =
                    self.buffer.visible_at_or_before(self.viewport.top_line - 1);
                let visual_rows = word_wrap::get_visual_rows_cached(
                    &mut self.render_cache,
                    &self.buffer,
                    self.viewport.top_line,
                    WrapLayout {
                        width: content_width,
                        smart: use_smart_wrap,
                        tab_size: self.config.tab_size,
                    },
                );
                self.viewport.top_visual_row_offset = visual_rows.saturating_sub(1);
            } else {
                // Already at top
                break;
            }
        }
    }

    /// Scroll down by visual rows (accounting for word wrap).
    /// Used for mouse scroll and other visual-based navigation.
    pub(crate) fn scroll_visual_rows_down(&mut self, count: usize) {
        // Use viewport.width as fallback when render_cache.content_width is not yet set
        // This handles the case when mouse events are processed before first render
        let content_width = if self.render_cache.content_width > 0 {
            self.render_cache.content_width
        } else {
            self.viewport.width
        };

        if !self.config.word_wrap || content_width == 0 {
            // Fallback to buffer line scrolling
            let total = self.render_cache.virtual_line_count;
            self.in_visible_lines(|viewport| viewport.scroll_down(count, total));
            return;
        }

        let use_smart_wrap = self.render_cache.use_smart_wrap;
        let line_count = self.buffer.line_count();

        // Ensure cache is valid for current width settings
        self.render_cache.update_wrap_settings(WrapLayout {
            width: content_width,
            smart: use_smart_wrap,
            tab_size: self.config.tab_size,
        });

        for _ in 0..count {
            let visual_rows = word_wrap::get_visual_rows_cached(
                &mut self.render_cache,
                &self.buffer,
                self.viewport.top_line,
                WrapLayout {
                    width: content_width,
                    smart: use_smart_wrap,
                    tab_size: self.config.tab_size,
                },
            );

            if self.viewport.top_visual_row_offset + 1 < visual_rows {
                // Scroll within current line
                self.viewport.top_visual_row_offset += 1;
            } else if self.buffer.visible_at_or_after(self.viewport.top_line + 1) < line_count {
                // Move to the next line that shows
                self.viewport.top_line =
                    self.buffer.visible_at_or_after(self.viewport.top_line + 1);
                self.viewport.top_visual_row_offset = 0;
            } else {
                // Already at bottom
                break;
            }
        }
    }

    /// Auto-scroll during mouse selection drag when mouse is outside the panel.
    /// Called from tick() to provide continuous scrolling without requiring mouse move events.
    /// Returns true if scrolled (needs redraw).
    pub(crate) fn tick_auto_scroll(&mut self) -> bool {
        // Check if selection drag is active
        if !self.input.selection_drag_active || self.selection.is_none() {
            return false;
        }

        let Some((_mouse_col, mouse_row)) = self.input.last_mouse_position else {
            return false;
        };

        let Some((_content_x, content_y, content_width, content_height)) =
            self.input.content_bounds
        else {
            return false;
        };

        // Check if mouse is outside content area (vertically)
        let is_above = mouse_row < content_y;
        let is_below = mouse_row >= content_y + content_height;

        if !is_above && !is_below {
            return false;
        }

        let mut scrolled = false;

        if is_above {
            // Auto-scroll up
            if self.viewport.top_line > 0 || self.viewport.top_visual_row_offset > 0 {
                self.scroll_visual_rows_up(1);
                // Extend selection to first visible line
                self.extend_selection_to_viewport_edge(true, content_width as usize);
                scrolled = true;
            }
        } else if is_below {
            // Auto-scroll down
            self.scroll_visual_rows_down(1);
            // Extend selection to last visible line
            self.extend_selection_to_viewport_edge(false, content_width as usize);
            scrolled = true;
        }

        // Ensure viewport follows cursor for visual updates
        if scrolled {
            self.scroll_follows_cursor = true;
        }

        scrolled
    }

    /// Extend selection to the edge of the viewport during auto-scroll.
    /// When scrolling up, extends to the first visible line.
    /// When scrolling down, extends to the last visible line.
    fn extend_selection_to_viewport_edge(&mut self, to_top: bool, content_width: usize) {
        if self.selection.is_none() {
            return;
        }

        if to_top {
            // Extend to first visible line
            let first_visible_line = self.viewport.top_line;
            let first_col = 0;
            self.cursor = Cursor::at(first_visible_line, first_col);
        } else {
            // Extend to last visible line
            // Calculate last visible line accounting for word wrap
            let content_height = self.render_cache.content_height.max(1);
            let last_visible_line = if self.config.word_wrap && content_width > 0 {
                // In word wrap mode, calculate the buffer line at the bottom of viewport
                self.calculate_last_visible_buffer_line(content_height)
            } else {
                // Without word wrap, it's straightforward
                let index = self.visible_index(self.viewport.top_line) + content_height - 1;
                self.buffer
                    .line_at_visible_index(index)
                    .min(self.buffer.line_count().saturating_sub(1))
            };

            let line_len = self.buffer.line_len_graphemes(last_visible_line);
            self.cursor = Cursor::at(last_visible_line, line_len);
        }

        // Update selection active end
        if let Some(ref mut selection) = self.selection {
            selection.active = self.cursor;
        }
    }

    /// Calculate the buffer line index at the bottom of the visible viewport.
    /// Accounts for word wrap when enabled.
    fn calculate_last_visible_buffer_line(&mut self, content_height: usize) -> usize {
        let content_width = self.render_cache.content_width;
        let use_smart_wrap = self.render_cache.use_smart_wrap;

        if content_width == 0 || content_height == 0 {
            return self.viewport.top_line;
        }

        let mut visual_rows_remaining = content_height;
        let mut current_line = self.viewport.top_line;
        let line_count = self.buffer.line_count();

        // Start with remaining rows in the first visible line
        let first_line_visual_rows = word_wrap::get_visual_rows_cached(
            &mut self.render_cache,
            &self.buffer,
            current_line,
            WrapLayout {
                width: content_width,
                smart: use_smart_wrap,
                tab_size: self.config.tab_size,
            },
        );
        let rows_in_first_line =
            first_line_visual_rows.saturating_sub(self.viewport.top_visual_row_offset);

        if rows_in_first_line >= visual_rows_remaining {
            return current_line;
        }

        visual_rows_remaining -= rows_in_first_line;
        current_line = self.buffer.visible_at_or_after(current_line + 1);

        // Continue through subsequent lines
        while current_line < line_count && visual_rows_remaining > 0 {
            let line_visual_rows = word_wrap::get_visual_rows_cached(
                &mut self.render_cache,
                &self.buffer,
                current_line,
                WrapLayout {
                    width: content_width,
                    smart: use_smart_wrap,
                    tab_size: self.config.tab_size,
                },
            );

            if line_visual_rows >= visual_rows_remaining {
                return current_line;
            }

            visual_rows_remaining -= line_visual_rows;
            current_line = self.buffer.visible_at_or_after(current_line + 1);
        }

        // Return last line if we've gone past the end
        self.buffer
            .visible_at_or_before(line_count.saturating_sub(1))
    }

    /// Get the total count of virtual lines (real buffer lines + deletion marker lines + diagnostics + word wrap)
    /// This is used for viewport calculations to account for deletion markers, diagnostics, and word wrapping
    pub(crate) fn virtual_line_count(&mut self, config: &Config) -> usize {
        self.render_cache.sync_folds(self.buffer.folds_version());
        // Count diagnostic virtual lines
        let diagnostic_line_count = self.lsp.diagnostics.len();

        // If word wrap is enabled, count visual rows instead of buffer lines
        if self.should_use_visual_movement() {
            // Use cached version for O(1) lookup when cache is valid
            let layout = self.wrap_layout();
            let word_wrap = self.config.word_wrap;

            let total_visual_rows = word_wrap::calculate_total_visual_rows_cached(
                &mut self.render_cache,
                &self.buffer,
                layout,
                word_wrap,
            );

            // Add deletion markers if git diff is shown (O(1) lookup)
            let deletion_markers = if config.editor.show_git_diff {
                self.git
                    .diff_cache
                    .as_ref()
                    .map(|cache| cache.deletion_marker_count())
                    .unwrap_or(0)
            } else {
                0
            };

            return total_visual_rows + deletion_markers;
        }

        // No word wrap - use the lines that show + deletion markers + diagnostics
        let buffer_line_count = self.buffer.visible_line_count();
        let deletion_marker_count = if config.editor.show_git_diff {
            self.git
                .diff_cache
                .as_ref()
                .map(|cache| cache.deletion_marker_count())
                .unwrap_or(0)
        } else {
            0
        };

        buffer_line_count + deletion_marker_count + diagnostic_line_count
    }
}
