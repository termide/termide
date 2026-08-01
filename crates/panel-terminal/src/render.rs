//! Terminal display-line rendering: builds the visible screen — scrollback,
//! selection highlight and link overlay — into styled lines behind a
//! dirty-flag cache. Driven by `Panel::render` in the parent module.

use std::collections::HashMap;
use std::sync::Arc;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use termide_theme::Theme;

use crate::terminal::Cell;
use crate::Terminal;

impl Terminal {
    /// Get lines for display with zero-copy rendering under lock.
    ///
    /// Optimization: Renders directly from screen buffer under lock,
    /// eliminating Vec<Vec<Cell>> cloning (~77KB per dirty frame).
    /// Uses dirty flag to skip re-rendering when content hasn't changed.
    ///
    /// Returns: (lines_arc, cursor_position, cursor_shown)
    pub(super) fn get_display_lines(
        &mut self,
        show_cursor: bool,
        theme: &Theme,
    ) -> (Arc<Vec<Line<'static>>>, (usize, usize), bool) {
        // === PHASE 0: Check if we can return cached result ===
        let (
            is_dirty,
            has_selection,
            sync_output,
            sync_output_ended,
            use_alt_screen,
            force_invalidation,
            current_cursor,
        ) = {
            let screen = self.read_screen();
            (
                screen.dirty,
                screen.selection_start.is_some(),
                screen.sync_output,
                screen.sync_output_ended,
                screen.use_alt_screen,
                screen.force_cache_invalidation,
                screen.cursor,
            )
        };

        // During sync_output, return cached content to prevent partial frame rendering
        // Only invalidate cache when sync_output is NOT active
        // IMPORTANT: Only use cache if it's from the same buffer (main vs alt)
        if sync_output && self.cached_use_alt_screen == use_alt_screen {
            // Clear force_invalidation flag but DON'T invalidate cache during batch
            // This defers invalidation until the batch ends
            if force_invalidation {
                if let Ok(mut screen) = self.screen.write() {
                    screen.force_cache_invalidation = false;
                }
            }
            // Return cached content (previous complete frame)
            if let Some(ref cached) = self.cached_lines {
                return (
                    Arc::clone(cached),
                    self.cached_cursor,
                    self.cached_cursor_shown,
                );
            }
            // If no cache exists during sync_output, we must render
            // This happens on first frame - fall through to regenerate
        }

        // Invalidate cache if active buffer changed (main <-> alt screen switch)
        // This prevents showing stale main buffer content over alt screen apps (e.g., Claude Code, htop)
        if self.cached_use_alt_screen != use_alt_screen {
            self.cached_lines = None;
            self.cached_use_alt_screen = use_alt_screen;
        }

        // Force invalidation when NOT in sync_output
        // This handles ED (clear screen) commands that need immediate visual update
        if force_invalidation {
            self.cached_lines = None;
            if let Ok(mut screen) = self.screen.write() {
                screen.force_cache_invalidation = false;
            }
        }

        // Handle sync_output batch end (transition from true to false)
        // IMPORTANT: Don't invalidate cache immediately when sync ends!
        // Between sync blocks, the terminal may be in an intermediate state
        // (e.g., after scroll but before new content is drawn).
        // Instead, return cached content until new dirty content arrives.
        // This prevents rendering artifacts like duplicate prompts.
        if sync_output_ended && !sync_output {
            // Clear the flag but DON'T invalidate cache yet
            if let Ok(mut screen) = self.screen.write() {
                screen.sync_output_ended = false;
            }
            // If we have cached content and screen is not dirty, return cache
            // This prevents showing intermediate state between sync blocks
            if !is_dirty {
                if let Some(ref cached) = self.cached_lines {
                    return (
                        Arc::clone(cached),
                        self.cached_cursor,
                        self.cached_cursor_shown,
                    );
                }
            }
            // Only invalidate cache when there's actual new content (dirty=true)
            self.cached_lines = None;
        }

        // Return cached if:
        // - Screen is not dirty (no new PTY output)
        // - Focus state hasn't changed (cursor visibility depends on focus)
        // - Cursor position hasn't changed (BS/CR move cursor without dirty flag)
        // - No active selection (selection changes without dirty flag)
        // - We have cached lines
        let has_search = self.search_state.is_some();
        if !is_dirty
            && self.cached_focus == show_cursor
            && !has_selection
            && !has_search
            && current_cursor == self.cached_cursor
        {
            if let Some(ref cached) = self.cached_lines {
                // O(1) Arc clone - no data copying!
                return (
                    Arc::clone(cached),
                    self.cached_cursor,
                    self.cached_cursor_shown,
                );
            }
        }

        // === PHASE 1: Render directly under lock (zero-copy) ===
        let mut screen = self.write_screen();
        // Clear dirty flag since we're about to render
        screen.dirty = false;
        // Ensure buffer has correct size before rendering (guards against IL/DL edge cases)
        screen.ensure_buffer_size();

        let visible_rows = screen.rows;
        let cols = screen.cols;
        let cursor_pos = screen.cursor;
        let cursor_visible = screen.cursor_visible;
        let scroll_offset = screen.scroll_offset;
        let use_alt_screen = screen.use_alt_screen;
        let has_selection = screen.selection_start.is_some() && screen.selection_end.is_some();
        let selection_start = screen.selection_start;
        let selection_end = screen.selection_end;

        // Determine view bounds based on scroll state
        let (view_start, total_scrollback, scrollback_slice) =
            if scroll_offset > 0 && !use_alt_screen {
                let total_scrollback = screen.scrollback.len();
                let total_lines = total_scrollback + visible_rows;
                let view_end = total_lines.saturating_sub(scroll_offset);
                let view_start = view_end.saturating_sub(visible_rows);
                (view_start, total_scrollback, true)
            } else {
                (0, 0, false)
            };

        // Pre-allocate output structures
        let mut lines = Vec::with_capacity(visible_rows);
        let mut current_text = String::with_capacity(cols);

        // Don't show cursor when viewing history
        let show_cursor_now = if scrollback_slice {
            false
        } else {
            show_cursor && cursor_visible
        };

        // Pre-compute selection bounds if selection exists (now in absolute coordinates)
        let selection_bounds = match (selection_start, selection_end) {
            (Some(start), Some(end)) if has_selection => {
                if start <= end {
                    Some((start, end))
                } else {
                    Some((end, start))
                }
            }
            _ => None,
        };

        // Calculate base for converting visual row to absolute
        // When scrolled: view_start is already the absolute index
        // When not scrolled: visual row 0 = scrollback.len() (start of active buffer)
        let scrollback_len = screen.scrollback.len();

        // Helper to check selection using absolute coordinates
        let is_in_selection = |visual_row: usize, col: usize| -> bool {
            if let Some((start, end)) = selection_bounds {
                // Convert visual row to absolute
                let abs_row = if scrollback_slice {
                    view_start + visual_row
                } else {
                    scrollback_len + visual_row
                };

                // Compare with absolute selection bounds
                if abs_row < start.0 || abs_row > end.0 {
                    return false;
                }
                if abs_row == start.0 && abs_row == end.0 {
                    col >= start.1 && col <= end.1
                } else if abs_row == start.0 {
                    col >= start.1
                } else if abs_row == end.0 {
                    col <= end.1
                } else {
                    true
                }
            } else {
                false
            }
        };

        // Pre-index URL segments by row for O(1) lookup per row
        // Instead of iterating all segments for each cell, we build a HashMap<row, Vec<(start, end)>>
        let url_segments_by_row: HashMap<usize, Vec<(usize, usize)>> = if self.ctrl_pressed {
            if let Some((_, segments)) = &self.hovered_link {
                let mut map: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
                for &(row, start, end) in segments {
                    map.entry(row).or_default().push((start, end));
                }
                map
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };

        // Helper to check if cell is in hovered URL (O(1) row lookup, then check ranges)
        let is_in_url = |visual_row: usize, col: usize| -> bool {
            if url_segments_by_row.is_empty() {
                return false;
            }
            // Convert visual row to absolute
            let abs_row = if scrollback_slice {
                view_start + visual_row
            } else {
                scrollback_len + visual_row
            };

            // O(1) lookup for the row, then check ranges (typically 1-2 per row)
            if let Some(ranges) = url_segments_by_row.get(&abs_row) {
                ranges.iter().any(|&(start, end)| col >= start && col < end)
            } else {
                false
            }
        };

        // Pre-index search matches by row for O(1) lookup per cell
        // Maps abs_row -> Vec<(col_start, col_end, is_current_match)>
        let search_matches_by_row: HashMap<usize, Vec<(usize, usize, bool)>> =
            if let Some(ref search) = self.search_state {
                let mut map: HashMap<usize, Vec<(usize, usize, bool)>> = HashMap::new();
                for (idx, &(abs_row, col_start, match_len)) in search.matches.iter().enumerate() {
                    let is_current = search.current_match == Some(idx);
                    map.entry(abs_row).or_default().push((
                        col_start,
                        col_start + match_len,
                        is_current,
                    ));
                }
                map
            } else {
                HashMap::new()
            };

        // Helper to check if cell is in a search match
        // Returns: None = not in match, Some(true) = current match, Some(false) = other match
        let is_in_search_match = |visual_row: usize, col: usize| -> Option<bool> {
            if search_matches_by_row.is_empty() {
                return None;
            }
            let abs_row = if scrollback_slice {
                view_start + visual_row
            } else {
                scrollback_len + visual_row
            };
            if let Some(ranges) = search_matches_by_row.get(&abs_row) {
                for &(start, end, is_current) in ranges {
                    if col >= start && col < end {
                        return Some(is_current);
                    }
                }
            }
            None
        };

        // Render directly from screen buffer (zero-copy)
        for row_idx in 0..visible_rows {
            // Get row reference without cloning
            let row: &[Cell] = if scrollback_slice {
                let source_idx = view_start + row_idx;
                if source_idx < total_scrollback {
                    &screen.scrollback[source_idx]
                } else {
                    let buf_idx = source_idx - total_scrollback;
                    if buf_idx < screen.active_buffer().len() {
                        &screen.active_buffer()[buf_idx]
                    } else {
                        lines.push(Line::default());
                        continue;
                    }
                }
            } else if row_idx < screen.active_buffer().len() {
                &screen.active_buffer()[row_idx]
            } else {
                lines.push(Line::default());
                continue;
            };

            let mut spans = Vec::with_capacity(8); // Pre-allocate for typical line
            current_text.clear();
            // Use direct style value instead of Option for faster comparison
            let mut current_style = Style::default();

            for (col_idx, cell) in row.iter().enumerate() {
                // Apply reverse if set
                let (mut fg, mut bg) = if cell.style.reverse {
                    (cell.style.bg, cell.style.fg)
                } else {
                    (cell.style.fg, cell.style.bg)
                };

                // Apply theme colors during rendering (not post-processing)
                if fg == Color::White || fg == Color::Reset {
                    fg = theme.fg;
                }
                if bg == Color::Reset {
                    bg = theme.bg;
                }

                let mut style = Style::default().fg(fg).bg(bg);

                if cell.style.bold {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if cell.style.italic {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if cell.style.underline {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if cell.style.reverse {
                    style = style.add_modifier(Modifier::REVERSED);
                }

                // Check if cell is in selection (optimized - skips if no selection)
                if is_in_selection(row_idx, col_idx) {
                    style = Style::default().fg(theme.bg).bg(theme.accented_fg);
                }

                // Check if cell is in hovered URL (Ctrl+hover) - use warning color
                if is_in_url(row_idx, col_idx) {
                    style = Style::default().fg(theme.bg).bg(theme.warning);
                }

                // Check if cell is in a search match
                if let Some(is_current) = is_in_search_match(row_idx, col_idx) {
                    if is_current {
                        // Current match: accented foreground background
                        style = Style::default().fg(theme.bg).bg(theme.accented_fg);
                    } else {
                        // Other matches: warning background
                        style = Style::default().fg(theme.bg).bg(theme.warning);
                    }
                }

                // If this is cursor position and needs showing, use inverse colors
                if show_cursor_now && row_idx == cursor_pos.0 && col_idx == cursor_pos.1 {
                    // Flush accumulated text
                    if !current_text.is_empty() {
                        spans.push(Span::styled(
                            std::mem::take(&mut current_text),
                            current_style,
                        ));
                    }

                    // Cursor with inverted colors (use original fg/bg for inversion)
                    let cursor_style = Style::default()
                        .bg(
                            if cell.style.fg == Color::White || cell.style.fg == Color::Reset {
                                theme.fg
                            } else {
                                cell.style.fg
                            },
                        )
                        .fg(if cell.style.bg == Color::Reset {
                            theme.bg
                        } else {
                            cell.style.bg
                        })
                        .add_modifier(Modifier::BOLD);

                    let cursor_char = if cell.ch == ' ' || cell.ch == '\0' {
                        ' '
                    } else {
                        cell.ch
                    };
                    let mut cursor_buf = [0u8; 4];
                    let cursor_str = cursor_char.encode_utf8(&mut cursor_buf);
                    spans.push(Span::styled(cursor_str.to_owned(), cursor_style));
                    continue;
                }

                // Group characters with same style (no Option overhead)
                if current_text.is_empty() || current_style == style {
                    current_text.push(cell.ch);
                    current_style = style;
                } else {
                    // Flush accumulated text with previous style
                    spans.push(Span::styled(
                        std::mem::take(&mut current_text),
                        current_style,
                    ));
                    current_text.push(cell.ch);
                    current_style = style;
                }
            }

            // Add last span
            if !current_text.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut current_text),
                    current_style,
                ));
            }

            // If line is empty and cursor is on it, add cursor
            if show_cursor_now && spans.is_empty() && row_idx == cursor_pos.0 {
                let cursor_style = Style::default()
                    .bg(theme.fg)
                    .fg(theme.bg)
                    .add_modifier(Modifier::BOLD);
                spans.push(Span::styled(" ", cursor_style));
            }

            lines.push(Line::from(spans));
        }

        // Release write lock before modifying other self fields
        drop(screen);

        // === PHASE 3: Cache the result (no clone - just wrap in Arc) ===
        let arc_lines = Arc::new(lines);
        self.cached_lines = Some(Arc::clone(&arc_lines));
        self.cached_cursor = cursor_pos;
        self.cached_cursor_shown = show_cursor_now;
        self.cached_focus = show_cursor;
        // Sync cached_use_alt_screen with actual rendered buffer (from write lock)
        self.cached_use_alt_screen = use_alt_screen;

        (arc_lines, cursor_pos, show_cursor_now)
    }
}
