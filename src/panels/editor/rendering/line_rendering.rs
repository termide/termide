//! Line rendering for no-wrap mode.
//!
//! This module provides functions for rendering individual lines in the editor
//! when word wrap is disabled. Handles horizontal scrolling and syntax highlighting.

use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

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
    // Using graphemes instead of chars to properly handle combining characters (Hindi, etc.)
    let mut col_offset = 0;
    let mut grapheme_idx = 0; // Grapheme index for selection/search matching
    for (segment_text, segment_style) in segments {
        for grapheme in segment_text.graphemes(true) {
            // Get display width of grapheme cluster
            let grapheme_width = grapheme.width();

            // Skip zero-width graphemes
            if grapheme_width == 0 {
                grapheme_idx += 1;
                continue;
            }

            if col_offset >= left_column && col_offset < left_column + content_width {
                let x = area.x + line_number_width + (col_offset - left_column) as u16;
                let y = area.y + row as u16;

                if x < area.x + area.width && y < area.y + area.height {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        // Use set_symbol for proper grapheme cluster handling
                        cell.set_symbol(grapheme);

                        // Determine final style using highlight renderer
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
            }
            col_offset += grapheme_width;
            grapheme_idx += 1;
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
    // Use display width for CJK characters
    let line_display_width = line_text.width();

    for col in line_display_width..content_width {
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

/// Render editor content in no-wrap mode with virtual lines.
///
/// This is the main rendering function for no-wrap mode that handles:
/// - Virtual lines (real lines + deletion markers)
/// - Horizontal scrolling
/// - Cursor positioning accounting for virtual lines
#[allow(clippy::too_many_arguments)]
pub fn render_content_no_wrap(
    buf: &mut Buffer,
    area: Rect,
    buffer: &crate::editor::TextBuffer,
    viewport: &crate::editor::Viewport,
    cursor: &crate::editor::Cursor,
    git_diff_cache: &Option<crate::git::GitDiffCache>,
    show_git_diff: bool,
    syntax_highlighting_enabled: bool,
    highlight_cache: &mut HighlightCache,
    render_context: &RenderContext,
    theme: &crate::theme::Theme,
    content_width: usize,
    content_height: usize,
    line_number_width: u16,
    text_style: Style,
    cursor_line_style: Style,
    search_match_style: Style,
    current_match_style: Style,
    selection_style: Style,
) {
    // Build list of virtual lines (real buffer lines + deletion markers)
    let virtual_lines =
        crate::panels::editor::git::build_virtual_lines(buffer, git_diff_cache, show_git_diff);

    // Find index of first virtual line for viewport.top_line
    let start_virtual_idx = virtual_lines
        .iter()
        .position(|vline| {
            matches!(vline, crate::panels::editor::git::VirtualLine::Real(idx) if *idx >= viewport.top_line)
        })
        .unwrap_or(virtual_lines.len());

    // Render visible virtual lines
    for row in 0..content_height {
        let virtual_idx = start_virtual_idx + row;

        if virtual_idx >= virtual_lines.len() {
            break;
        }

        let virtual_line = virtual_lines[virtual_idx];

        // Handle different types of virtual lines
        match virtual_line {
            crate::panels::editor::git::VirtualLine::Real(line_idx) => {
                // Render real line
                if let Some(line_text) = buffer.line(line_idx) {
                    let line_text = line_text.trim_end_matches('\n');
                    let is_cursor_line = line_idx == cursor.line;

                    render_line_no_wrap(
                        buf,
                        area,
                        row,
                        line_idx,
                        line_text,
                        is_cursor_line,
                        text_style,
                        cursor_line_style,
                        git_diff_cache,
                        show_git_diff,
                        theme,
                        line_number_width,
                        content_width,
                        viewport.left_column,
                        syntax_highlighting_enabled,
                        highlight_cache,
                        render_context,
                        search_match_style,
                        current_match_style,
                        selection_style,
                    );
                }
            }
            crate::panels::editor::git::VirtualLine::DeletionMarker(
                _after_line_idx,
                deletion_count,
            ) => {
                // Render deletion marker virtual line
                super::deletion_markers::render_deletion_marker(
                    buf,
                    area,
                    row,
                    deletion_count,
                    theme,
                    content_width,
                    line_number_width,
                );
            }
        }
    }

    // Render cursor accounting for virtual lines
    let cursor_virtual_idx = virtual_lines.iter().position(|vline| {
        matches!(vline, crate::panels::editor::git::VirtualLine::Real(idx) if *idx == cursor.line)
    });

    if let Some(cursor_virtual_idx) = cursor_virtual_idx {
        if cursor_virtual_idx >= start_virtual_idx {
            let viewport_row = cursor_virtual_idx - start_virtual_idx;

            if cursor.column >= viewport.left_column {
                let viewport_col = cursor.column - viewport.left_column;

                let cursor_x = area.x + line_number_width + viewport_col as u16;
                let cursor_y = area.y + viewport_row as u16;

                if viewport_col < content_width {
                    super::cursor_renderer::render_cursor_at(buf, cursor_x, cursor_y, area, theme);
                }
            }
        }
    }
}
