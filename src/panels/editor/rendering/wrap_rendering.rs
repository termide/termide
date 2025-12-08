//! Word wrap rendering for the editor.
//!
//! This module provides the main rendering logic for word wrap mode,
//! handling line breaking, syntax highlighting, and visual row management.

use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{context::RenderContext, cursor_renderer, deletion_markers, highlight_renderer};
use crate::{
    editor::{LineHighlighter, TextBuffer},
    panels::editor::git,
};

/// Render editor content in word wrap mode.
///
/// This is the main rendering function that handles all aspects of word wrap mode:
/// - Line breaking at word boundaries (smart wrap) or content width (simple wrap)
/// - Syntax highlighting with search/selection/cursor line styling
/// - Git diff markers and line numbers
/// - Cursor positioning tracking
#[allow(clippy::too_many_arguments)] // Complex rendering requires many parameters
pub fn render_content_word_wrap<H: LineHighlighter>(
    buf: &mut Buffer,
    area: Rect,
    buffer: &TextBuffer,
    viewport: &crate::editor::Viewport,
    cursor: &crate::editor::Cursor,
    git_diff_cache: &Option<crate::git::GitDiffCache>,
    show_git_diff: bool,
    syntax_highlighting_enabled: bool,
    highlight_cache: &mut H,
    render_context: &mut RenderContext,
    theme: &crate::theme::Theme,
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

    while visual_row < content_height && line_idx < buffer.line_count() {
        let is_cursor_line = line_idx == cursor.line;
        let style = if is_cursor_line {
            cursor_line_style
        } else {
            text_style
        };

        if let Some(line_text) = buffer.line(line_idx) {
            let line_text = line_text.trim_end_matches('\n');
            let graphemes: Vec<&str> = line_text.graphemes(true).collect();
            let line_len = graphemes.len();

            let mut grapheme_offset = 0;
            let mut is_first_visual_row = true;

            // Special handling for empty lines
            if line_len == 0 {
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
            } else {
                // Handle non-empty lines with wrapping
                while grapheme_offset < line_len && visual_row < content_height {
                    let chunk_end = if use_smart_wrap {
                        crate::editor::calculate_wrap_point(
                            &graphemes,
                            grapheme_offset,
                            content_width,
                            line_len,
                        )
                    } else {
                        // Simple wrap: calculate based on display width
                        calculate_simple_wrap_point(&graphemes, grapheme_offset, content_width)
                    };

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

        line_idx += 1;
    }

    // Render cursor
    if let Some((row, col)) = render_context.cursor_viewport_pos {
        let cursor_x = area.x + line_number_width + col as u16;
        let cursor_y = area.y + row as u16;
        cursor_renderer::render_cursor_at(buf, cursor_x, cursor_y, area, theme);
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
    git_diff_cache: &Option<crate::git::GitDiffCache>,
    show_git_diff: bool,
    theme: &crate::theme::Theme,
    line_number_width: u16,
    content_width: usize,
    style: Style,
    cursor: &crate::editor::Cursor,
    render_context: &mut RenderContext,
) {
    let git_info = git::get_git_line_info(line_idx, git_diff_cache, show_git_diff, theme);

    // Render line number
    let line_num_style = Style::default().fg(git_info.status_color);
    let line_num_part = format!("{:>4}{}", line_idx + 1, git_info.status_marker);

    for (i, ch) in line_num_part.chars().enumerate() {
        let x = area.x + i as u16;
        let y = area.y + visual_row as u16;
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char(ch);
            cell.set_style(line_num_style);
        }
    }

    // Space after marker
    let x = area.x + 5;
    let y = area.y + visual_row as u16;
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(' ');
        cell.set_style(line_num_style);
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
    git_diff_cache: &Option<crate::git::GitDiffCache>,
    show_git_diff: bool,
    syntax_highlighting_enabled: bool,
    highlight_cache: &mut H,
    render_context: &mut RenderContext,
    theme: &crate::theme::Theme,
    content_width: usize,
    line_number_width: u16,
    line_number_style: Style,
    style: Style,
    cursor_line_style: Style,
    search_match_style: Style,
    current_match_style: Style,
    selection_style: Style,
    cursor: &crate::editor::Cursor,
) {
    // Render line number gutter
    if is_first_visual_row {
        let git_info = git::get_git_line_info(line_idx, git_diff_cache, show_git_diff, theme);
        let line_num_style = Style::default().fg(git_info.status_color);
        let line_num_part = format!("{:>4}{}", line_idx + 1, git_info.status_marker);

        for (i, ch) in line_num_part.chars().enumerate() {
            let x = area.x + i as u16;
            let y = area.y + visual_row as u16;
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(ch);
                cell.set_style(line_num_style);
            }
        }

        let x = area.x + 5;
        let y = area.y + visual_row as u16;
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char(' ');
            cell.set_style(line_num_style);
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
    let segments = if syntax_highlighting_enabled && highlight_cache.has_syntax() {
        highlight_cache.get_line_segments(line_idx, line_text)
    } else {
        &[(line_text.to_string(), style)][..]
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

    // Check cursor at end of line
    if is_cursor_line
        && cursor.column >= char_offset
        && cursor.column <= chunk_end
        && (cursor.column == chunk_end || (chunk_end == line_len && cursor.column >= line_len))
    {
        render_context.cursor_viewport_pos = Some((visual_row, cursor.column - char_offset));
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

/// Calculate simple wrap point based on display width (no word boundary detection)
fn calculate_simple_wrap_point(graphemes: &[&str], start: usize, max_width: usize) -> usize {
    let mut display_width = 0;

    for (i, grapheme) in graphemes.iter().enumerate().skip(start) {
        let grapheme_width = grapheme.width();

        if display_width + grapheme_width > max_width {
            return i;
        }

        display_width += grapheme_width;
    }

    graphemes.len()
}
