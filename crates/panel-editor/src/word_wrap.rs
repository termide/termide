//! Word wrap calculations for editor content.
//!
//! This module provides utilities for calculating line wrapping in the editor,
//! including smart wrapping (breaking at word boundaries) and hard wrapping
//! (breaking at fixed column width).

use std::collections::HashMap;

use lsp_types::Diagnostic;
use termide_buffer::{calculate_wrap_point, TextBuffer};
use termide_git::GitDiffCache;
use unicode_segmentation::UnicodeSegmentation;

/// How buffer lines are laid out into visual rows: everything a wrap point,
/// a visual row count or a vertical move depends on besides the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrapLayout {
    /// Content width the lines wrap at; 0 means they do not wrap.
    pub width: usize,
    /// Break at word boundaries rather than at the last column.
    pub smart: bool,
    /// Tab stop interval.
    pub tab_size: usize,
}

/// Calculate wrap points for a single line of text.
///
/// Returns (visual_row_count, wrap_points) where wrap_points contains
/// the grapheme indices where each new visual line starts.
///
/// Uses display width and grapheme clusters for proper Unicode handling.
/// This function iterates exactly like rendering does to ensure consistency.
pub fn get_line_wrap_points(line_text: &str, layout: WrapLayout) -> (usize, Vec<usize>) {
    let WrapLayout {
        width: content_width,
        smart: use_smart_wrap,
        tab_size,
    } = layout;
    if content_width == 0 {
        return (1, Vec::new());
    }

    // Check display width, not char/grapheme count
    let display_width = termide_buffer::display_width(line_text, tab_size);
    if display_width == 0 {
        return (1, Vec::new());
    }

    if display_width <= content_width {
        return (1, Vec::new()); // No wrapping needed
    }

    // Iterate exactly like rendering does to ensure wrap points match
    let graphemes: Vec<&str> = line_text.graphemes(true).collect();
    let line_len = graphemes.len();
    let mut wrap_points = Vec::new();
    let mut grapheme_offset = 0;

    while grapheme_offset < line_len {
        let chunk_end = if use_smart_wrap {
            calculate_wrap_point(
                &graphemes,
                grapheme_offset,
                content_width,
                line_len,
                tab_size,
            )
        } else {
            calculate_simple_wrap_point(&graphemes, grapheme_offset, content_width, tab_size)
        };

        // Push chunk_end as start of NEXT visual line (not grapheme_offset!)
        if chunk_end > grapheme_offset && chunk_end < line_len {
            wrap_points.push(chunk_end);
        }

        // Prevent infinite loop
        if chunk_end == grapheme_offset {
            grapheme_offset += 1;
        } else {
            grapheme_offset = chunk_end;
        }
    }

    (wrap_points.len() + 1, wrap_points)
}

/// Calculate simple wrap point for a single visual line (iterative version).
///
/// Returns the grapheme index where the visual line ends.
/// This mirrors the logic in wrap_rendering.rs for consistency: tab stops
/// restart at `start`, the first column of the visual row.
fn calculate_simple_wrap_point(
    graphemes: &[&str],
    start: usize,
    max_width: usize,
    tab_size: usize,
) -> usize {
    let mut display_width = 0;

    for (i, grapheme) in graphemes.iter().enumerate().skip(start) {
        let grapheme_width = termide_buffer::grapheme_columns(grapheme, display_width, tab_size);

        if display_width + grapheme_width > max_width {
            return i;
        }

        display_width += grapheme_width;
    }

    graphemes.len()
}

/// Get which visual row within a single line the cursor is on (cached version).
///
/// Uses the wrap cache to avoid recalculating wrap points.
pub(crate) fn get_cursor_visual_row_in_line_cached(
    cache: &mut RenderingCache,
    buffer: &TextBuffer,
    line: usize,
    column: usize,
    layout: WrapLayout,
) -> usize {
    let WrapLayout {
        width: content_width,
        ..
    } = layout;
    if content_width == 0 {
        return 0;
    }

    let (_, wrap_points) = get_line_wrap_points_cached(cache, buffer, line, layout);

    // Clamp column to line length (using cached grapheme count)
    let line_len = get_line_grapheme_count_cached(cache, buffer, line, layout);
    let column_clamped = column.min(line_len);

    // Find which visual row contains the cursor column
    wrap_points.partition_point(|&wp| wp <= column_clamped)
}

/// Convert visual row to buffer position accounting for both word wrap and diagnostic virtual lines.
///
/// Returns (buffer_line, column_offset, chunk_end, is_virtual_line).
/// - `buffer_line`: The buffer line index
/// - `column_offset`: Grapheme index where this visual line starts
/// - `chunk_end`: Grapheme index where this visual line ends (exclusive)
/// - `is_virtual_line`: True if this row is a diagnostic virtual line
///
/// Diagnostic virtual lines appear after all wrapped segments of their associated buffer line.
/// Multi-row diagnostics are handled correctly.
pub fn visual_row_to_buffer_position_with_diagnostics(
    buffer: &TextBuffer,
    visual_row: usize,
    viewport_top: usize,
    layout: WrapLayout,
    diagnostics: &[Diagnostic],
) -> (usize, usize, usize, bool) {
    let WrapLayout {
        width: content_width,
        smart: use_smart_wrap,
        tab_size,
    } = layout;
    // Group diagnostics by line with total row count (accounting for multi-row diagnostics)
    let diagnostics_by_line =
        count_diagnostic_rows_by_line(diagnostics, buffer, content_width, tab_size);

    if content_width == 0 {
        // No wrap, but still need to account for diagnostic lines
        let mut current_visual_row = 0;
        let mut line_idx = viewport_top;

        while line_idx < buffer.line_count() {
            // Real line
            if current_visual_row == visual_row {
                let line_len = buffer
                    .line(line_idx)
                    .map(|s| s.trim_end_matches('\n').graphemes(true).count())
                    .unwrap_or(0);
                return (line_idx, 0, line_len, false);
            }
            current_visual_row += 1;

            // Diagnostic virtual rows for this line (may be more than number of diagnostics)
            let diag_row_count = diagnostics_by_line.get(&line_idx).copied().unwrap_or(0);
            for _ in 0..diag_row_count {
                if current_visual_row == visual_row {
                    let line_len = buffer
                        .line(line_idx)
                        .map(|s| s.trim_end_matches('\n').graphemes(true).count())
                        .unwrap_or(0);
                    return (line_idx, 0, line_len, true);
                }
                current_visual_row += 1;
            }

            line_idx = buffer.visible_at_or_after(line_idx + 1);
        }

        let last_line = buffer.line_count().saturating_sub(1);
        let line_len = buffer
            .line(last_line)
            .map(|s| s.trim_end_matches('\n').graphemes(true).count())
            .unwrap_or(0);
        return (last_line, 0, line_len, false);
    }

    let mut current_visual_row = 0;
    let mut line_idx = viewport_top;

    while line_idx < buffer.line_count() {
        if let Some(line_text) = buffer.line(line_idx) {
            let line_text = line_text.trim_end_matches('\n');
            let graphemes: Vec<&str> = line_text.graphemes(true).collect();
            let line_len = graphemes.len();

            // Handle empty lines (1 visual row)
            if line_len == 0 {
                if current_visual_row == visual_row {
                    return (line_idx, 0, 0, false);
                }
                current_visual_row += 1;
            } else {
                // Iterate through wrapped segments
                let mut grapheme_offset = 0;

                while grapheme_offset < line_len {
                    let chunk_end = if use_smart_wrap {
                        calculate_wrap_point(
                            &graphemes,
                            grapheme_offset,
                            content_width,
                            line_len,
                            tab_size,
                        )
                    } else {
                        calculate_simple_wrap_point(
                            &graphemes,
                            grapheme_offset,
                            content_width,
                            tab_size,
                        )
                    };

                    if current_visual_row == visual_row {
                        // Found the target visual row - it's a real line segment
                        return (line_idx, grapheme_offset, chunk_end, false);
                    }

                    current_visual_row += 1;

                    // Safety: prevent infinite loop
                    if chunk_end == grapheme_offset {
                        grapheme_offset += 1;
                    } else {
                        grapheme_offset = chunk_end;
                    }
                }
            }

            // After all wrapped segments, check for diagnostic virtual rows
            let diag_row_count = diagnostics_by_line.get(&line_idx).copied().unwrap_or(0);
            for _ in 0..diag_row_count {
                if current_visual_row == visual_row {
                    // This visual row is a diagnostic virtual line
                    return (line_idx, 0, line_len, true);
                }
                current_visual_row += 1;
            }
        } else {
            // If line doesn't exist, treat as empty (1 visual row)
            if current_visual_row == visual_row {
                return (line_idx, 0, 0, false);
            }
            current_visual_row += 1;
        }

        line_idx = buffer.visible_at_or_after(line_idx + 1);
    }

    // If we've exhausted all lines, return the last line
    let last_line = buffer.line_count().saturating_sub(1);
    let last_line_len = buffer
        .line(last_line)
        .map(|s| s.trim_end_matches('\n').graphemes(true).count())
        .unwrap_or(0);
    (last_line, 0, last_line_len, false)
}

/// Count total diagnostic visual rows per buffer line.
///
/// This accounts for multi-row diagnostic messages that wrap based on content_width.
pub(crate) fn count_diagnostic_rows_by_line(
    diagnostics: &[Diagnostic],
    buffer: &TextBuffer,
    content_width: usize,
    tab_size: usize,
) -> HashMap<usize, usize> {
    use crate::git;
    use std::collections::HashSet;

    let mut result: HashMap<usize, usize> = HashMap::with_capacity(diagnostics.len());
    let mut seen: HashSet<(usize, u64)> = HashSet::with_capacity(diagnostics.len());

    for diag in diagnostics {
        let line = diag.range.start.line as usize;
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        diag.message.hash(&mut hasher);
        let key = (line, hasher.finish());
        if !seen.insert(key) {
            continue;
        }

        // The same span git::group_diagnostics_by_line draws.
        let (start_col, underline_len) = git::diagnostic_span(diag, buffer, tab_size);

        // Extract code
        let code = diag.code.as_ref().map(|c| match c {
            lsp_types::NumberOrString::Number(n) => n.to_string(),
            lsp_types::NumberOrString::String(s) => s.clone(),
        });

        // Calculate how many rows this diagnostic needs
        let rows = git::calculate_diagnostic_rows(
            start_col,
            underline_len,
            code.as_deref(),
            &diag.message,
            content_width,
        );

        *result.entry(line).or_insert(0) += rows;
    }

    result
}

// =============================================================================
// Cached Versions of Word Wrap Functions
// =============================================================================
//
// These functions use the RenderingCache to avoid redundant calculations.
// They check the cache first and only compute if needed.

use crate::state::rendering_cache::RenderingCache;

/// Get wrap points for a line, using cache if available.
///
/// Returns (visual_rows, wrap_points) for the given line.
/// Uses cache lookup first, computes and caches if miss.
/// Cache validation ensures data was computed with matching content_width and use_smart_wrap.
pub(crate) fn get_line_wrap_points_cached(
    cache: &mut RenderingCache,
    buffer: &TextBuffer,
    line: usize,
    layout: WrapLayout,
) -> (usize, Vec<usize>) {
    let WrapLayout {
        width: content_width,
        smart: use_smart_wrap,
        tab_size,
    } = layout;
    // Check if cache has valid data for this line with matching width settings
    if let Some(cached) = cache.get_wrap_data(line, layout) {
        return (cached.visual_rows, cached.wrap_points.clone());
    }

    // Cache miss - compute wrap points
    let line_cow = buffer.line_cow(line);
    let line_text = line_cow
        .as_deref()
        .map(|s| s.trim_end_matches('\n'))
        .unwrap_or("");

    let grapheme_count = line_text.graphemes(true).count();
    let (visual_rows, wrap_points) = get_line_wrap_points(line_text, layout);

    // Store in cache with width settings. The cache holds one tab size for
    // all its entries, so a lookup made with another one is not stored.
    if cache.tab_size == tab_size {
        cache.set_wrap_data(
            line,
            visual_rows,
            wrap_points.clone(),
            grapheme_count,
            content_width,
            use_smart_wrap,
        );
    }

    (visual_rows, wrap_points)
}

/// Get visual row count for a line, using cache if available.
///
/// This avoids the Vec<usize> clone that `get_line_wrap_points_cached` requires,
/// making it more efficient when only the row count is needed (e.g., scrolling calculations).
pub(crate) fn get_visual_rows_cached(
    cache: &mut RenderingCache,
    buffer: &TextBuffer,
    line: usize,
    layout: WrapLayout,
) -> usize {
    if let Some(cached) = cache.get_wrap_data(line, layout) {
        return cached.visual_rows;
    }

    // Cache miss — compute and cache, return only visual_rows
    let (visual_rows, _) = get_line_wrap_points_cached(cache, buffer, line, layout);
    visual_rows
}

/// Get the grapheme count for a line, using cache if available.
///
/// This avoids repeated `graphemes(true).count()` calls by returning
/// the count stored in the wrap cache.
pub(crate) fn get_line_grapheme_count_cached(
    cache: &mut RenderingCache,
    buffer: &TextBuffer,
    line: usize,
    layout: WrapLayout,
) -> usize {
    // Ensure wrap data is cached (populates grapheme_count)
    if cache.get_wrap_data(line, layout).is_none() {
        // Populate cache
        get_visual_rows_cached(cache, buffer, line, layout);
    }

    cache
        .get_wrap_data(line, layout)
        .map(|c| c.grapheme_count)
        .unwrap_or(0)
}

/// Calculate total visual rows in buffer using cumulative cache.
///
/// Uses O(1) lookup if cumulative cache is valid, otherwise builds cache first.
pub(crate) fn calculate_total_visual_rows_cached(
    cache: &mut RenderingCache,
    buffer: &TextBuffer,
    layout: WrapLayout,
    word_wrap_enabled: bool,
) -> usize {
    if layout.width == 0 || !word_wrap_enabled {
        return buffer.line_count();
    }

    // Update wrap settings (invalidates cache if changed)
    cache.update_wrap_settings(layout);

    // Try to use cumulative cache (verify it covers all buffer lines)
    if cache.cumulative_covers_line_count(buffer.line_count()) {
        if let Some(total) = cache.get_total_visual_rows() {
            return total;
        }
    }

    // Build cumulative cache
    cache.build_cumulative_cache(buffer);

    cache.get_total_visual_rows().unwrap_or(buffer.line_count())
}

/// Text of buffer line `line` without its line break (empty past the end).
fn line_text_at(buffer: &TextBuffer, line: usize) -> String {
    buffer
        .line(line)
        .map(|s| s.trim_end_matches('\n').to_string())
        .unwrap_or_default()
}

/// Screen columns between grapheme `row_start` and grapheme `col` of `line`,
/// with tab stops restarting at `row_start` as on a drawn row: how far right
/// the cursor sits on its visual row.
pub(crate) fn row_offset_columns(
    line: &str,
    row_start: usize,
    col: usize,
    tab_size: usize,
) -> usize {
    line.graphemes(true)
        .take(col)
        .skip(row_start)
        .fold(0, |width, g| {
            width + termide_buffer::grapheme_columns(g, width, tab_size)
        })
}

/// The grapheme of `line` drawn `offset` screen columns right of grapheme
/// `row_start` (tab stops restarting there), no further than `max_col`:
/// where vertical movement lands to keep the cursor in the same screen
/// column. An offset inside a tab lands on the tab.
pub(crate) fn column_at_row_offset(
    line: &str,
    row_start: usize,
    max_col: usize,
    offset: usize,
    tab_size: usize,
) -> usize {
    let mut width = 0;
    for (idx, g) in line.graphemes(true).enumerate().skip(row_start) {
        if idx >= max_col {
            return max_col;
        }
        let cols = termide_buffer::grapheme_columns(g, width, tab_size);
        if width + cols > offset {
            return idx;
        }
        width += cols;
    }
    max_col.min(line.graphemes(true).count()).max(row_start)
}

/// Move the cursor one visual row up, keeping `preferred_column` (screen
/// columns from the start of the visual row; the cursor's own when `None`).
///
/// `rows` gives a buffer line's wrap points and grapheme count — from the
/// wrap cache, or computed — so the cached arrow-key movement and the vim
/// motions share this one walk. Returns `None` at the top of the buffer.
pub(crate) fn step_up(
    buffer: &TextBuffer,
    cursor_pos: (usize, usize),
    preferred_column: Option<usize>,
    tab_size: usize,
    rows: &mut impl FnMut(usize) -> (Vec<usize>, usize),
) -> Option<(usize, usize)> {
    let (line, col) = cursor_pos;
    let (wrap_points, line_len) = rows(line);
    let col = col.min(line_len);
    let row = wrap_points.partition_point(|&wp| wp <= col);
    let (row_start, _) = get_visual_row_bounds(row, &wrap_points, line_len);
    let text = line_text_at(buffer, line);
    let offset =
        preferred_column.unwrap_or_else(|| row_offset_columns(&text, row_start, col, tab_size));

    if row > 0 {
        // Up within the same line: an intermediate row ends before its wrap point.
        let (start, end) = get_visual_row_bounds(row - 1, &wrap_points, line_len);
        let max_col = end.saturating_sub(1).max(start);
        return Some((
            line,
            column_at_row_offset(&text, start, max_col, offset, tab_size),
        ));
    }
    if line == 0 {
        return None;
    }
    // The last visual row of the previous line that shows, where the cursor
    // may sit at the end of the line.
    let prev = buffer.visible_at_or_before(line - 1);
    let (prev_wraps, prev_len) = rows(prev);
    let (start, end) = get_visual_row_bounds(prev_wraps.len(), &prev_wraps, prev_len);
    let col = column_at_row_offset(
        &line_text_at(buffer, prev),
        start,
        end.max(start),
        offset,
        tab_size,
    );
    Some((prev, col))
}

/// Move the cursor one visual row down; the counterpart of [`step_up`].
/// Returns `None` at the bottom of the buffer.
pub(crate) fn step_down(
    buffer: &TextBuffer,
    cursor_pos: (usize, usize),
    preferred_column: Option<usize>,
    tab_size: usize,
    rows: &mut impl FnMut(usize) -> (Vec<usize>, usize),
) -> Option<(usize, usize)> {
    let (line, col) = cursor_pos;
    let (wrap_points, line_len) = rows(line);
    let col = col.min(line_len);
    let row = wrap_points.partition_point(|&wp| wp <= col);
    let (row_start, _) = get_visual_row_bounds(row, &wrap_points, line_len);
    let text = line_text_at(buffer, line);
    let offset =
        preferred_column.unwrap_or_else(|| row_offset_columns(&text, row_start, col, tab_size));

    // On the last visual row of a line (end == line_len) the cursor can sit
    // after the last grapheme; on an intermediate row the grapheme at the
    // wrap point belongs to the next row.
    let max_col = |start: usize, end: usize, len: usize| {
        let max = if end == len {
            end
        } else {
            end.saturating_sub(1)
        };
        max.max(start)
    };

    if row < wrap_points.len() {
        let (start, end) = get_visual_row_bounds(row + 1, &wrap_points, line_len);
        let col = column_at_row_offset(
            &text,
            start,
            max_col(start, end, line_len),
            offset,
            tab_size,
        );
        return Some((line, col));
    }
    let next = buffer.visible_at_or_after(line + 1);
    if next >= buffer.line_count() {
        return None;
    }
    let (next_wraps, next_len) = rows(next);
    let (start, end) = get_visual_row_bounds(0, &next_wraps, next_len);
    let col = column_at_row_offset(
        &line_text_at(buffer, next),
        start,
        max_col(start, end, next_len),
        offset,
        tab_size,
    );
    Some((next, col))
}

/// Wrap points and grapheme count of `line` from the wrap cache, for
/// [`step_up`] / [`step_down`].
fn cached_rows<'a>(
    cache: &'a mut RenderingCache,
    buffer: &'a TextBuffer,
    layout: WrapLayout,
) -> impl FnMut(usize) -> (Vec<usize>, usize) + 'a {
    move |line| {
        let (_, wrap_points) = get_line_wrap_points_cached(cache, buffer, line, layout);
        let len = get_line_grapheme_count_cached(cache, buffer, line, layout);
        (wrap_points, len)
    }
}

/// Move cursor up by one visual line, using cached wrap data.
///
/// Returns Some((line, col)) if movement was possible, None if at top.
pub(crate) fn move_up_cached(
    cache: &mut RenderingCache,
    buffer: &TextBuffer,
    cursor_pos: (usize, usize),
    preferred_column: Option<usize>,
    layout: WrapLayout,
) -> Option<(usize, usize)> {
    let WrapLayout { tab_size, .. } = layout;
    let mut rows = cached_rows(cache, buffer, layout);
    step_up(buffer, cursor_pos, preferred_column, tab_size, &mut rows)
}

/// Move cursor down by one visual line, using cached wrap data.
///
/// Returns Some((line, col)) if movement was possible, None if at bottom.
pub(crate) fn move_down_cached(
    cache: &mut RenderingCache,
    buffer: &TextBuffer,
    cursor_pos: (usize, usize),
    preferred_column: Option<usize>,
    layout: WrapLayout,
) -> Option<(usize, usize)> {
    let WrapLayout { tab_size, .. } = layout;
    let mut rows = cached_rows(cache, buffer, layout);
    step_down(buffer, cursor_pos, preferred_column, tab_size, &mut rows)
}

/// Page up by visual lines, using cached wrap data.
///
/// Returns (line, col) for the new cursor position.
pub(crate) fn page_up_cached(
    cache: &mut RenderingCache,
    buffer: &TextBuffer,
    cursor_pos: (usize, usize),
    preferred_column: Option<usize>,
    layout: WrapLayout,
    page_size: usize,
) -> (usize, usize) {
    let (mut line, mut col) = cursor_pos;

    for _ in 0..page_size {
        if let Some((new_line, new_col)) =
            move_up_cached(cache, buffer, (line, col), preferred_column, layout)
        {
            line = new_line;
            col = new_col;
        } else {
            break; // At top
        }
    }

    (line, col)
}

/// Page down by visual lines, using cached wrap data.
///
/// Returns (line, col) for the new cursor position.
pub(crate) fn page_down_cached(
    cache: &mut RenderingCache,
    buffer: &TextBuffer,
    cursor_pos: (usize, usize),
    preferred_column: Option<usize>,
    layout: WrapLayout,
    page_size: usize,
) -> (usize, usize) {
    let (mut line, mut col) = cursor_pos;

    for _ in 0..page_size {
        if let Some((new_line, new_col)) =
            move_down_cached(cache, buffer, (line, col), preferred_column, layout)
        {
            line = new_line;
            col = new_col;
        } else {
            break; // At bottom
        }
    }

    (line, col)
}

/// Helper: Get the start and end grapheme indices for a visual row.
pub(crate) fn get_visual_row_bounds(
    visual_row: usize,
    wrap_points: &[usize],
    line_len: usize,
) -> (usize, usize) {
    let start = if visual_row == 0 {
        0
    } else if visual_row - 1 < wrap_points.len() {
        wrap_points[visual_row - 1]
    } else {
        line_len
    };

    let end = if visual_row < wrap_points.len() {
        wrap_points[visual_row]
    } else {
        line_len
    };

    (start, end)
}

/// Convert visual row to buffer position using cached wrap data.
///
/// This is the cached version of `visual_row_to_buffer_position_with_diagnostics`.
/// Uses the wrap cache to avoid redundant calculations.
///
/// Parameters:
/// - `cache`: The rendering cache for wrap data
/// - `buffer`: The text buffer
/// - `visual_row`: Visual row index (includes top_visual_row_offset if scrolled within a line)
/// - `viewport_top`: First visible buffer line
/// - `content_width`: Width for wrapping
/// - `use_smart_wrap`: Whether to use smart wrapping
/// - `diagnostics`: Diagnostic list for virtual lines
/// - `git_diff_cache`: Git diff cache for deletion markers
/// - `show_git_diff`: Whether git diff display is enabled
///
/// Returns (buffer_line, column_offset, chunk_end, is_virtual_line).
#[allow(clippy::too_many_arguments)]
pub(crate) fn visual_row_to_buffer_position_cached(
    cache: &mut RenderingCache,
    buffer: &TextBuffer,
    visual_row: usize,
    viewport_top: usize,
    layout: WrapLayout,
    diagnostics: &[Diagnostic],
    git_diff_cache: &Option<GitDiffCache>,
    show_git_diff: bool,
) -> (usize, usize, usize, bool) {
    let WrapLayout {
        width: content_width,
        tab_size,
        ..
    } = layout;
    if content_width == 0 {
        // No wrap - delegate to non-cached version
        return visual_row_to_buffer_position_with_diagnostics(
            buffer,
            visual_row,
            viewport_top,
            layout,
            diagnostics,
        );
    }

    // Ensure diagnostic rows cache is populated
    if !cache.is_diagnostic_cache_valid(content_width) {
        let map = count_diagnostic_rows_by_line(diagnostics, buffer, content_width, tab_size);
        cache.set_diagnostic_rows_cache(map, content_width);
    }

    let mut current_visual_row = 0;
    let mut line_idx = viewport_top;

    while line_idx < buffer.line_count() {
        // Use cached wrap data
        let (visual_rows, wrap_points) =
            get_line_wrap_points_cached(cache, buffer, line_idx, layout);

        let line_len = get_line_grapheme_count_cached(cache, buffer, line_idx, layout);

        // Check if target is within this line's visual rows
        if visual_row < current_visual_row + visual_rows {
            // Found the target line - determine exact position
            let row_within_line = visual_row - current_visual_row;
            let (start, end) = get_visual_row_bounds(row_within_line, &wrap_points, line_len);
            return (line_idx, start, end, false);
        }
        current_visual_row += visual_rows;

        // Check deletion marker after this line (rendered between text and diagnostics)
        if show_git_diff {
            if let Some(git_diff) = git_diff_cache.as_ref() {
                if git_diff.has_deletion_marker(line_idx) {
                    if visual_row == current_visual_row {
                        // Target is a deletion marker virtual line
                        return (line_idx, 0, line_len, true);
                    }
                    current_visual_row += 1;
                }
            }
        }

        // Check diagnostic virtual rows after this line (from cache)
        let diag_row_count = cache.diagnostic_rows_for_line(line_idx);
        if diag_row_count > 0 && visual_row < current_visual_row + diag_row_count {
            // Target is a diagnostic virtual line
            return (line_idx, 0, line_len, true);
        }
        current_visual_row += diag_row_count;

        line_idx = buffer.visible_at_or_after(line_idx + 1);
    }

    // If we've exhausted all lines, return the last line
    let last_line = buffer.line_count().saturating_sub(1);
    let last_line_len = get_line_grapheme_count_cached(cache, buffer, last_line, layout);
    (last_line, 0, last_line_len, false)
}

#[cfg(test)]
mod tests {
    use super::{column_at_row_offset, row_offset_columns};

    #[test]
    fn row_offsets_count_screen_columns_from_the_row_start() {
        assert_eq!(row_offset_columns("\t\txy", 0, 3, 4), 9);
        // Tab stops restart at the row start: grapheme 1 is the second tab.
        assert_eq!(row_offset_columns("\t\txy", 1, 3, 4), 5);
        assert_eq!(row_offset_columns("abc", 0, 9, 4), 3);
    }

    #[test]
    fn a_row_offset_lands_on_the_grapheme_drawn_there() {
        assert_eq!(column_at_row_offset("\t\txy", 0, 4, 9, 4), 3);
        // Inside a tab: the tab itself.
        assert_eq!(column_at_row_offset("\t\txy", 0, 4, 6, 4), 1);
        // Past the end: the end, or the row's last column.
        assert_eq!(column_at_row_offset("ab", 0, 2, 7, 4), 2);
        assert_eq!(column_at_row_offset("abcdef", 2, 4, 9, 4), 4);
    }
}
