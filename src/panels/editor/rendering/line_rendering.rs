//! Line rendering for no-wrap mode.
//!
//! This module provides functions for rendering individual lines in the editor
//! when word wrap is disabled. Handles horizontal scrolling and syntax highlighting.

use ratatui::{buffer::Buffer, layout::Rect, style::Style};

use super::{context::RenderContext, highlight_renderer};
use crate::{editor::HighlightCache, panels::editor::git};

/// Render a single line in no-wrap mode.
///
/// Handles:
/// - Line number gutter with git status
/// - Syntax-highlighted content with horizontal scrolling
/// - Search matches, selection, and cursor line styling
/// - Background fill for cursor line
#[allow(clippy::too_many_arguments)] // Complex rendering requires many parameters
pub fn render_line_no_wrap(
    buf: &mut Buffer,
    area: Rect,
    row: usize,
    line_idx: usize,
    line_text: &str,
    is_cursor_line: bool,
    text_style: Style,
    cursor_line_style: Style,
    git_diff_cache: &Option<crate::git::GitDiffCache>,
    show_git_diff: bool,
    theme: &crate::theme::Theme,
    line_number_width: u16,
    content_width: usize,
    left_column: usize,
    syntax_highlighting_enabled: bool,
    highlight_cache: &mut HighlightCache,
    render_context: &RenderContext,
    search_match_style: Style,
    current_match_style: Style,
    selection_style: Style,
) {
    let style = if is_cursor_line {
        cursor_line_style
    } else {
        text_style
    };

    // Render line number gutter with git status
    render_line_gutter(
        buf,
        area,
        row,
        line_idx,
        git_diff_cache,
        show_git_diff,
        theme,
    );

    // Render line content with horizontal scrolling
    render_line_content_horizontal_scroll(
        buf,
        area,
        row,
        line_idx,
        line_text,
        is_cursor_line,
        style,
        line_number_width,
        content_width,
        left_column,
        syntax_highlighting_enabled,
        highlight_cache,
        render_context,
        search_match_style,
        current_match_style,
        selection_style,
        theme,
    );

    // Fill remainder of line with cursor line background
    if is_cursor_line {
        fill_line_remainder(
            buf,
            area,
            row,
            line_text,
            line_number_width,
            content_width,
            left_column,
            cursor_line_style,
        );
    }
}

/// Render line number gutter with git status markers.
fn render_line_gutter(
    buf: &mut Buffer,
    area: Rect,
    row: usize,
    line_idx: usize,
    git_diff_cache: &Option<crate::git::GitDiffCache>,
    show_git_diff: bool,
    theme: &crate::theme::Theme,
) {
    let git_info = git::get_git_line_info(line_idx, git_diff_cache, show_git_diff, theme);

    // Render line number (4 chars) + status marker (1 char)
    let line_num_style = Style::default().fg(git_info.status_color);
    let line_num_part = format!("{:>4}{}", line_idx + 1, git_info.status_marker);

    for (i, ch) in line_num_part.chars().enumerate() {
        let x = area.x + i as u16;
        let y = area.y + row as u16;
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char(ch);
            cell.set_style(line_num_style);
        }
    }

    // Render space after marker (deletion markers are now virtual lines)
    let x = area.x + 5;
    let y = area.y + row as u16;
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(' ');
        cell.set_style(line_num_style);
    }
}

/// Render line content with horizontal scrolling.
#[allow(clippy::too_many_arguments)]
fn render_line_content_horizontal_scroll(
    buf: &mut Buffer,
    area: Rect,
    row: usize,
    line_idx: usize,
    line_text: &str,
    is_cursor_line: bool,
    style: Style,
    line_number_width: u16,
    content_width: usize,
    left_column: usize,
    syntax_highlighting_enabled: bool,
    highlight_cache: &mut HighlightCache,
    render_context: &RenderContext,
    search_match_style: Style,
    current_match_style: Style,
    selection_style: Style,
    theme: &crate::theme::Theme,
) {
    // Get syntax highlighting segments
    let segments = if syntax_highlighting_enabled && highlight_cache.has_syntax() {
        highlight_cache.get_line_segments(line_idx, line_text)
    } else {
        // No syntax highlighting - use single segment
        &[(line_text.to_string(), style)][..]
    };

    // Render segments with horizontal scrolling
    let mut col_offset = 0;
    for (segment_text, segment_style) in segments {
        for ch in segment_text.chars() {
            if col_offset >= left_column && col_offset < left_column + content_width {
                let x = area.x + line_number_width + (col_offset - left_column) as u16;
                let y = area.y + row as u16;

                if x < area.x + area.width && y < area.y + area.height {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_char(ch);

                        // Determine final style using highlight renderer
                        let final_style = highlight_renderer::determine_cell_style(
                            line_idx,
                            col_offset,
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
            }
            col_offset += 1;
        }
    }
}

/// Fill remainder of line with cursor line background.
#[allow(clippy::too_many_arguments)] // Helper for render_line_no_wrap
fn fill_line_remainder(
    buf: &mut Buffer,
    area: Rect,
    row: usize,
    line_text: &str,
    line_number_width: u16,
    content_width: usize,
    left_column: usize,
    cursor_line_style: Style,
) {
    let line_len = line_text.chars().count();

    for col in line_len..content_width {
        if col >= left_column {
            let x = area.x + line_number_width + (col - left_column) as u16;
            let y = area.y + row as u16;

            if x < area.x + area.width && y < area.y + area.height {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ');
                    cell.set_style(cursor_line_style);
                }
            }
        }
    }
}
