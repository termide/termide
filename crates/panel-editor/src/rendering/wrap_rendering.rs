//! Word wrap rendering for the editor.
//!
//! This module provides the main rendering logic for word wrap mode,
//! handling line breaking, syntax highlighting, and visual row management.

use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use termide_buffer::{Cursor, TextBuffer, Viewport};
use termide_git::GitDiffCache;
use termide_highlight::LineHighlighter;
use termide_theme::Theme;

use super::{
    context::RenderContext, cursor_renderer, deletion_markers, highlight_renderer, line_rendering,
};
use crate::git;
use crate::word_wrap::get_line_wrap_points;

/// Render editor content in word wrap mode.
///
/// This is the main rendering function that handles all aspects of word wrap mode:
/// - Line breaking at word boundaries (smart wrap) or content width (simple wrap)
/// - Syntax highlighting with search/selection/cursor line styling
/// - Git diff markers and line numbers
/// - Cursor positioning tracking
/// - Diagnostic virtual lines with error/warning messages
#[allow(clippy::too_many_arguments)] // Complex rendering requires many parameters
pub fn render_content_word_wrap<H: LineHighlighter>(
    buf: &mut Buffer,
    area: Rect,
    buffer: &TextBuffer,
    viewport: &Viewport,
    cursor: &Cursor,
    git_diff_cache: &Option<GitDiffCache>,
    show_git_diff: bool,
    syntax_highlighting_enabled: bool,
    highlight_cache: &mut H,
    render_context: &mut RenderContext,
    diagnostics_by_line: &std::collections::HashMap<usize, Vec<git::DiagnosticInfo>>,
    theme: &Theme,
    is_focused: bool,
    content_width: usize,
    content_height: usize,
    line_number_width: u16,
    use_smart_wrap: bool,
    text_style: Style,
    cursor_line_style: Style,
    line_number_style: Style,
    search_match_style: Style,
    current_match_style: Style,
    selection_style: Style,
) {
    let mut visual_row = 0;
    let mut line_idx = viewport.top_line;

    // Track how many visual rows to skip for the first line (for within-line scrolling)
    let mut skip_visual_rows = viewport.top_visual_row_offset;

    while visual_row < content_height && line_idx < buffer.line_count() {
        let is_cursor_line = line_idx == cursor.line;
        let style = if is_cursor_line {
            cursor_line_style
        } else {
            text_style
        };

        if let Some(line_text) = buffer.line_cow(line_idx) {
            let line_text = line_text.trim_end_matches('\n');

            // Compute wrap points once per physical line (O(n)) instead of
            // running `calculate_wrap_point` O(n) for every visual row.
            // `wrap_points` holds the grapheme index where each *next* visual
            // row starts; the final chunk ends at `line_len`.
            let (_, wrap_points) = get_line_wrap_points(line_text, content_width, use_smart_wrap);
            let line_len = line_text.graphemes(true).count();

            let mut grapheme_offset = 0;
            let mut wrap_idx = 0;
            let mut is_first_visual_row = skip_visual_rows == 0;

            // Special handling for empty lines
            if line_len == 0 {
                // Empty lines have only one visual row - skip if needed
                if skip_visual_rows > 0 {
                    skip_visual_rows -= 1;
                } else {
                    render_empty_line(
                        buf,
                        area,
                        visual_row,
                        line_idx,
                        is_cursor_line,
                        git_diff_cache,
                        show_git_diff,
                        theme,
                        line_number_width,
                        content_width,
                        style,
                        cursor,
                        render_context,
                    );
                    visual_row += 1;
                }
            } else {
                // Handle non-empty lines with wrapping using precomputed wrap points
                while grapheme_offset < line_len && visual_row < content_height {
                    let chunk_end = wrap_points.get(wrap_idx).copied().unwrap_or(line_len);
                    wrap_idx += 1;

                    // Skip visual rows if we have an offset (for within-line scrolling)
                    if skip_visual_rows > 0 {
                        skip_visual_rows -= 1;
                        grapheme_offset = chunk_end;
                        is_first_visual_row = false;
                        continue;
                    }

                    render_visual_line(
                        buf,
                        area,
                        visual_row,
                        line_idx,
                        line_text,
                        grapheme_offset,
                        chunk_end,
                        line_len,
                        is_first_visual_row,
                        is_cursor_line,
                        git_diff_cache,
                        show_git_diff,
                        syntax_highlighting_enabled,
                        highlight_cache,
                        render_context,
                        theme,
                        content_width,
                        line_number_width,
                        line_number_style,
                        style,
                        cursor_line_style,
                        search_match_style,
                        current_match_style,
                        selection_style,
                        cursor,
                    );

                    is_first_visual_row = false;
                    grapheme_offset = chunk_end;
                    visual_row += 1;
                }
            }
        }

        // Check for deletion markers after this line
        if show_git_diff && visual_row < content_height {
            if let Some(git_diff) = git_diff_cache {
                if git_diff.has_deletion_marker(line_idx) {
                    let deletion_count = git_diff.get_deletion_count(line_idx);
                    deletion_markers::render_deletion_marker(
                        buf,
                        area,
                        visual_row,
                        deletion_count,
                        theme,
                        content_width,
                        line_number_width,
                    );
                    visual_row += 1;
                }
            }
        }

        // Render diagnostic virtual lines after this line (with multi-row wrapping)
        if let Some(line_diagnostics) = diagnostics_by_line.get(&line_idx) {
            for diag_info in line_diagnostics {
                // Calculate how many rows this diagnostic needs
                let total_rows = git::calculate_diagnostic_rows(
                    diag_info.start_col,
                    diag_info.underline_len,
                    diag_info.code.as_deref(),
                    &diag_info.message,
                    content_width,
                );

                // Render each row of the diagnostic
                for row_index in 0..total_rows {
                    if visual_row >= content_height {
                        break;
                    }
                    line_rendering::render_diagnostic_virtual_line(
                        buf,
                        area,
                        visual_row,
                        diag_info.start_col,
                        diag_info.underline_len,
                        diag_info.severity,
                        diag_info.code.as_deref(),
                        &diag_info.message,
                        theme,
                        line_number_width,
                        content_width,
                        0, // No horizontal scroll in word wrap mode
                        row_index,
                        total_rows,
                    );
                    visual_row += 1;
                }
            }
        }

        line_idx += 1;
    }

    // Render cursor — only when the panel is focused, so inactive editors
    // don't leave a misleading caret behind. Mirrors panel-file-manager's
    // `is_cursor && is_focused` rule.
    if is_focused {
        if let Some((row, col)) = render_context.cursor_viewport_pos {
            let cursor_x = area.x + line_number_width + col as u16;
            let cursor_y = area.y + row as u16;
            cursor_renderer::render_cursor_at(buf, cursor_x, cursor_y, area, theme);
        }
    }
}

/// Render an empty line in word wrap mode.
#[allow(clippy::too_many_arguments)]
fn render_empty_line(
    buf: &mut Buffer,
    area: Rect,
    visual_row: usize,
    line_idx: usize,
    is_cursor_line: bool,
    git_diff_cache: &Option<GitDiffCache>,
    show_git_diff: bool,
    theme: &Theme,
    line_number_width: u16,
    content_width: usize,
    style: Style,
    cursor: &Cursor,
    render_context: &mut RenderContext,
) {
    let diagnostic_severity = render_context.diagnostic_severity_at_line(line_idx);
    let (line_num_fg, line_num_bg) =
        git::get_line_number_git_style(line_idx, git_diff_cache, show_git_diff, theme);
    let lsp_marker = git::get_lsp_marker(diagnostic_severity, theme);

    // Render the line number with git bg color
    let mut line_num_style = Style::default().fg(line_num_fg);
    if let Some(bg) = line_num_bg {
        line_num_style = line_num_style.bg(bg);
    }
    let digit_cells = line_number_width.saturating_sub(super::LINE_NUMBER_MARKER_CELLS as u16);
    let mut num_buf = [0u8; 20];
    let line_num_str = super::itoa_right_align(line_idx + 1, digit_cells as usize, &mut num_buf);

    for (i, ch) in line_num_str.chars().enumerate() {
        let x = area.x + i as u16;
        let y = area.y + visual_row as u16;
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char(ch);
            cell.set_style(line_num_style);
        }
    }

    // Render LSP marker (penultimate gutter cell) with its own color
    let marker_style = Style::default().fg(lsp_marker.color);
    let x = area.x + digit_cells;
    let y = area.y + visual_row as u16;
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(lsp_marker.marker);
        cell.set_style(marker_style);
    }

    // Space separator (last gutter cell)
    let x = area.x + digit_cells + 1;
    let y = area.y + visual_row as u16;
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(' ');
        cell.set_style(Style::default());
    }

    // Fill line with background
    for col in 0..content_width {
        let x = area.x + line_number_width + col as u16;
        let y = area.y + visual_row as u16;

        if x < area.x + area.width && y < area.y + area.height {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(' ');
                cell.set_style(style);
            }
        }
    }

    // Track cursor position
    if is_cursor_line && cursor.column == 0 {
        render_context.cursor_viewport_pos = Some((visual_row, 0));
    }
}

/// Render a single visual line (wrapped segment) in word wrap mode.
#[allow(clippy::too_many_arguments)]
fn render_visual_line<H: LineHighlighter>(
    buf: &mut Buffer,
    area: Rect,
    visual_row: usize,
    line_idx: usize,
    line_text: &str,
    char_offset: usize,
    chunk_end: usize,
    line_len: usize,
    is_first_visual_row: bool,
    is_cursor_line: bool,
    git_diff_cache: &Option<GitDiffCache>,
    show_git_diff: bool,
    syntax_highlighting_enabled: bool,
    highlight_cache: &mut H,
    render_context: &mut RenderContext,
    theme: &Theme,
    content_width: usize,
    line_number_width: u16,
    line_number_style: Style,
    style: Style,
    cursor_line_style: Style,
    search_match_style: Style,
    current_match_style: Style,
    selection_style: Style,
    cursor: &Cursor,
) {
    // Render line number gutter
    if is_first_visual_row {
        let diagnostic_severity = render_context.diagnostic_severity_at_line(line_idx);
        let (line_num_fg, line_num_bg) =
            git::get_line_number_git_style(line_idx, git_diff_cache, show_git_diff, theme);
        let lsp_marker = git::get_lsp_marker(diagnostic_severity, theme);

        // Render the line number with git bg color
        let mut line_num_style = Style::default().fg(line_num_fg);
        if let Some(bg) = line_num_bg {
            line_num_style = line_num_style.bg(bg);
        }
        let digit_cells = line_number_width.saturating_sub(super::LINE_NUMBER_MARKER_CELLS as u16);
        let mut num_buf = [0u8; 20];
        let line_num_str =
            super::itoa_right_align(line_idx + 1, digit_cells as usize, &mut num_buf);

        for (i, ch) in line_num_str.chars().enumerate() {
            let x = area.x + i as u16;
            let y = area.y + visual_row as u16;
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(ch);
                cell.set_style(line_num_style);
            }
        }

        // Render LSP marker (penultimate gutter cell) with its own color
        let marker_style = Style::default().fg(lsp_marker.color);
        let x = area.x + digit_cells;
        let y = area.y + visual_row as u16;
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char(lsp_marker.marker);
            cell.set_style(marker_style);
        }

        // Space separator (last gutter cell)
        let x = area.x + digit_cells + 1;
        let y = area.y + visual_row as u16;
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char(' ');
            cell.set_style(Style::default());
        }
    } else {
        // Empty gutter for continuation lines
        for i in 0..line_number_width as usize {
            let x = area.x + i as u16;
            let y = area.y + visual_row as u16;
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(' ');
                cell.set_style(line_number_style);
            }
        }
    }

    // Get syntax highlighting segments
    let no_syntax_segment;
    let segments = if syntax_highlighting_enabled && highlight_cache.has_syntax() {
        highlight_cache.get_line_segments(line_idx, line_text)
    } else {
        // No syntax highlighting - borrow line_text directly, no allocation per frame
        no_syntax_segment = [(std::borrow::Cow::Borrowed(line_text), style)];
        &no_syntax_segment[..]
    };

    // Render graphemes for this visual line
    // Using graphemes instead of chars to properly handle combining characters (Hindi, etc.)
    let mut grapheme_idx = 0;
    let mut visual_col = 0;

    for (segment_text, segment_style) in segments {
        for grapheme in segment_text.graphemes(true) {
            if grapheme_idx >= char_offset && grapheme_idx < chunk_end {
                // Get display width of grapheme cluster
                let grapheme_width = grapheme.width();

                // Skip zero-width graphemes (shouldn't happen with proper grapheme iteration)
                if grapheme_width == 0 {
                    grapheme_idx += 1;
                    continue;
                }

                let x = area.x + line_number_width + visual_col as u16;
                let y = area.y + visual_row as u16;

                if x < area.x + area.width && y < area.y + area.height {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        // Use set_symbol for proper grapheme cluster handling
                        cell.set_symbol(grapheme);

                        let final_style = highlight_renderer::determine_cell_style(
                            line_idx,
                            grapheme_idx,
                            *segment_style,
                            is_cursor_line,
                            render_context,
                            search_match_style,
                            current_match_style,
                            selection_style,
                            theme.accented_bg,
                            theme.error,
                            theme.warning,
                        );
                        cell.set_style(final_style);
                    }
                }

                // Track cursor position
                if is_cursor_line && cursor.column == grapheme_idx {
                    render_context.cursor_viewport_pos = Some((visual_row, visual_col));
                }

                visual_col += grapheme_width;
            }
            grapheme_idx += 1;
        }
    }

    // Check cursor at end of PHYSICAL line only
    // Wrap points are handled by the next visual line's main loop
    if is_cursor_line && chunk_end == line_len && cursor.column >= line_len {
        render_context.cursor_viewport_pos = Some((visual_row, visual_col));
    }

    // Fill remainder with cursor line background
    if is_cursor_line {
        for col in visual_col..content_width {
            let x = area.x + line_number_width + col as u16;
            let y = area.y + visual_row as u16;

            if x < area.x + area.width && y < area.y + area.height {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ');
                    cell.set_style(cursor_line_style);
                }
            }
        }
    }
}
