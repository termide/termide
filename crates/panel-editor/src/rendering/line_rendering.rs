//! Line rendering for no-wrap mode.
//!
//! This module provides functions for rendering individual lines in the editor
//! when word wrap is disabled. Handles horizontal scrolling and syntax highlighting.

use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use termide_buffer::{Cursor, TextBuffer, Viewport};
use termide_git::{truncate_right, GitDiffCache, InlineChangeType};
use termide_highlight::LineHighlighter;
use termide_theme::Theme;

use super::{context::RenderContext, highlight_renderer, inline_diff};
use crate::git;

/// Render a single line in no-wrap mode.
///
/// Handles:
/// - Line number gutter with git status
/// - Syntax-highlighted content with horizontal scrolling
/// - Search matches, selection, and cursor line styling
/// - Background fill for cursor line
#[allow(clippy::too_many_arguments)] // Complex rendering requires many parameters
pub fn render_line_no_wrap<H: LineHighlighter>(
    buf: &mut Buffer,
    area: Rect,
    row: usize,
    line_idx: usize,
    line_text: &str,
    is_cursor_line: bool,
    text_style: Style,
    cursor_line_style: Style,
    git_diff_cache: &Option<GitDiffCache>,
    show_git_diff: bool,
    theme: &Theme,
    line_number_width: u16,
    content_width: usize,
    left_column: usize,
    syntax_highlighting_enabled: bool,
    highlight_cache: &mut H,
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

    // Render line number gutter with git status (or diagnostic marker if present)
    let diagnostic_severity = render_context.diagnostic_severity_at_line(line_idx);
    render_line_gutter(
        buf,
        area,
        row,
        line_idx,
        git_diff_cache,
        show_git_diff,
        diagnostic_severity,
        theme,
        line_number_width,
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
        git_diff_cache,
        show_git_diff,
    );

    // Fill remainder of line with cursor line background
    if is_cursor_line {
        // Calculate visual line width (including deleted text from inline diff)
        let visual_line_width = if show_git_diff {
            git_diff_cache
                .as_ref()
                .and_then(|cache| cache.get_inline_diff(line_idx, line_text))
                .map(|changes| line_text.width() + inline_diff::calculate_deleted_width(&changes))
                .unwrap_or_else(|| line_text.width())
        } else {
            line_text.width()
        };

        fill_line_remainder(
            buf,
            area,
            row,
            visual_line_width,
            line_number_width,
            content_width,
            left_column,
            cursor_line_style,
        );
    }
}

/// Render line number gutter with git color and LSP marker.
///
/// Format: `1234▶ ` (`line_number_width` cells total)
/// - Leading digit cells: line number (color = git status)
/// - Penultimate cell: LSP marker (▶ for error/warning, space otherwise)
/// - Last cell: space separator
#[allow(clippy::too_many_arguments)]
fn render_line_gutter(
    buf: &mut Buffer,
    area: Rect,
    row: usize,
    line_idx: usize,
    git_diff_cache: &Option<GitDiffCache>,
    show_git_diff: bool,
    diagnostic_severity: Option<lsp_types::DiagnosticSeverity>,
    theme: &Theme,
    line_number_width: u16,
) {
    // Get separate colors for line number and LSP marker
    let (line_num_fg, line_num_bg) =
        git::get_line_number_git_style(line_idx, git_diff_cache, show_git_diff, theme);
    let lsp_marker = git::get_lsp_marker(diagnostic_severity, theme);

    // Render the line number with git bg color — write digits directly to avoid format!()
    let mut line_num_style = Style::default().fg(line_num_fg);
    if let Some(bg) = line_num_bg {
        line_num_style = line_num_style.bg(bg);
    }
    let digit_cells = line_number_width.saturating_sub(super::LINE_NUMBER_MARKER_CELLS as u16);
    let mut num_buf = [0u8; 20];
    let num_str = super::itoa_right_align(line_idx + 1, digit_cells as usize, &mut num_buf);

    for (i, ch) in num_str.chars().enumerate() {
        let x = area.x + i as u16;
        let y = area.y + row as u16;
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char(ch);
            cell.set_style(line_num_style);
        }
    }

    // Render LSP marker (penultimate gutter cell) with its own color
    let marker_style = Style::default().fg(lsp_marker.color);
    let x = area.x + digit_cells;
    let y = area.y + row as u16;
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(lsp_marker.marker);
        cell.set_style(marker_style);
    }

    // Render space separator (last gutter cell)
    let x = area.x + digit_cells + 1;
    let y = area.y + row as u16;
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(' ');
        cell.set_style(Style::default());
    }
}

/// Render line content with horizontal scrolling.
#[allow(clippy::too_many_arguments)]
fn render_line_content_horizontal_scroll<H: LineHighlighter>(
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
    highlight_cache: &mut H,
    render_context: &RenderContext,
    search_match_style: Style,
    current_match_style: Style,
    selection_style: Style,
    theme: &Theme,
    git_diff_cache: &Option<GitDiffCache>,
    show_git_diff: bool,
) {
    // Check if this line has inline diff (modified line with original content)
    let inline_changes = if show_git_diff {
        git_diff_cache
            .as_ref()
            .and_then(|cache| cache.get_inline_diff(line_idx, line_text))
    } else {
        None
    };

    if let Some(ref changes) = inline_changes {
        // Render with inline diff highlighting
        render_line_with_inline_diff(
            buf,
            area,
            row,
            line_idx,
            changes,
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
    } else {
        // Regular rendering without inline diff
        render_line_regular(
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
    }
}

/// Render line content without inline diff (regular mode).
#[allow(clippy::too_many_arguments)]
fn render_line_regular<H: LineHighlighter>(
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
    highlight_cache: &mut H,
    render_context: &RenderContext,
    search_match_style: Style,
    current_match_style: Style,
    selection_style: Style,
    theme: &Theme,
) {
    // Get syntax highlighting segments
    let no_syntax_segment;
    let segments = if syntax_highlighting_enabled && highlight_cache.has_syntax() {
        highlight_cache.get_line_segments(line_idx, line_text)
    } else {
        // No syntax highlighting - borrow line_text directly, no allocation per frame
        no_syntax_segment = [(std::borrow::Cow::Borrowed(line_text), style)];
        &no_syntax_segment[..]
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
                            theme.error,
                            theme.warning,
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

/// Render line content with inline diff highlighting.
///
/// Shows deleted text with red background and inserted text with green background.
/// Visual line is longer than buffer line due to deleted text being shown.
#[allow(clippy::too_many_arguments)]
fn render_line_with_inline_diff<H: LineHighlighter>(
    buf: &mut Buffer,
    area: Rect,
    row: usize,
    line_idx: usize,
    inline_changes: &[termide_git::InlineChange],
    is_cursor_line: bool,
    style: Style,
    line_number_width: u16,
    content_width: usize,
    left_column: usize,
    syntax_highlighting_enabled: bool,
    highlight_cache: &mut H,
    render_context: &RenderContext,
    search_match_style: Style,
    current_match_style: Style,
    selection_style: Style,
    theme: &Theme,
) {
    // Build visual segments from inline changes
    let visual_segments = inline_diff::build_visual_line(inline_changes);

    // Build the current line text (excluding deleted text) for syntax highlighting
    let current_text: String = inline_changes
        .iter()
        .filter(|c| c.change_type != InlineChangeType::Deleted)
        .map(|c| c.text.as_str())
        .collect();

    // Pre-calculate capacity before potential move (bytes >= graphemes count)
    let syntax_styles_capacity = current_text.len();

    // Get syntax highlighting for the current text
    let no_syntax_seg;
    let syntax_segments = if syntax_highlighting_enabled && highlight_cache.has_syntax() {
        highlight_cache.get_line_segments(line_idx, &current_text)
    } else {
        no_syntax_seg = [(std::borrow::Cow::Owned(current_text), style)];
        &no_syntax_seg[..]
    };

    // Build a map of buffer position -> syntax style
    let mut syntax_styles: Vec<(usize, Style)> = Vec::with_capacity(syntax_styles_capacity);
    let mut buf_pos = 0;
    for (seg_text, seg_style) in syntax_segments {
        let seg_len = seg_text.graphemes(true).count();
        for _ in 0..seg_len {
            syntax_styles.push((buf_pos, *seg_style));
            buf_pos += 1;
        }
    }

    // Render visual segments
    let mut visual_col = 0;
    let mut buffer_grapheme_idx = 0; // For selection/search matching (buffer coordinates)

    for segment in &visual_segments {
        let change_type = segment.change_type;

        for grapheme in segment.text.graphemes(true) {
            let grapheme_width = grapheme.width();

            if grapheme_width == 0 {
                if change_type != InlineChangeType::Deleted {
                    buffer_grapheme_idx += 1;
                }
                continue;
            }

            // Check if visible in viewport
            if visual_col >= left_column && visual_col < left_column + content_width {
                let x = area.x + line_number_width + (visual_col - left_column) as u16;
                let y = area.y + row as u16;

                if x < area.x + area.width && y < area.y + area.height {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_symbol(grapheme);

                        // Get base style from syntax highlighting (for non-deleted text)
                        let base_style = if change_type == InlineChangeType::Deleted {
                            style // Use default style for deleted text
                        } else {
                            syntax_styles
                                .get(buffer_grapheme_idx)
                                .map(|(_, s)| *s)
                                .unwrap_or(style)
                        };

                        // Apply diff styling
                        let diff_style = inline_diff::apply_diff_style(
                            change_type,
                            base_style,
                            theme.error,   // deleted bg
                            theme.success, // inserted bg
                        );

                        // For non-deleted text, also check selection/search highlights
                        let final_style = if change_type == InlineChangeType::Deleted {
                            // Deleted text: use diff style directly (no search/selection)
                            diff_style
                        } else {
                            // Check if this position has search/selection override
                            let highlight_style = highlight_renderer::determine_cell_style(
                                line_idx,
                                buffer_grapheme_idx,
                                diff_style,
                                is_cursor_line,
                                render_context,
                                search_match_style,
                                current_match_style,
                                selection_style,
                                theme.accented_bg,
                                theme.error,
                                theme.warning,
                            );
                            // If unchanged and no highlight, apply diff style if inserted
                            if change_type == InlineChangeType::Inserted
                                && highlight_style == diff_style
                            {
                                diff_style
                            } else if change_type == InlineChangeType::Unchanged {
                                // For unchanged text in a modified line, check cursor line bg
                                if is_cursor_line && highlight_style == base_style {
                                    base_style.bg(theme.accented_bg)
                                } else {
                                    highlight_style
                                }
                            } else {
                                highlight_style
                            }
                        };

                        cell.set_style(final_style);
                    }
                }
            }

            visual_col += grapheme_width;
            if change_type != InlineChangeType::Deleted {
                buffer_grapheme_idx += 1;
            }
        }
    }
}

/// Fill remainder of line with cursor line background.
#[allow(clippy::too_many_arguments)] // Helper for render_line_no_wrap
fn fill_line_remainder(
    buf: &mut Buffer,
    area: Rect,
    row: usize,
    visual_line_width: usize,
    line_number_width: u16,
    content_width: usize,
    left_column: usize,
    cursor_line_style: Style,
) {
    for col in visual_line_width..content_width {
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

/// Render a diagnostic virtual line.
///
/// Format for row 0: `     │~~~  [E0425] error message start`
/// Format for row 1+: `     │      message continuation`
///
/// - Empty gutter (no line number)
/// - Row 0: Wavy underline + diagnostic code + message start
/// - Row 1+: Indented message continuation
///
/// Used by both no-wrap and word-wrap rendering modes.
#[allow(clippy::too_many_arguments)]
pub fn render_diagnostic_virtual_line(
    buf: &mut Buffer,
    area: Rect,
    row: usize,
    start_col: usize,
    underline_len: usize,
    severity: lsp_types::DiagnosticSeverity,
    code: Option<&str>,
    message: &str,
    theme: &Theme,
    line_number_width: u16,
    content_width: usize,
    left_column: usize,
    row_index: usize,
    _total_rows: usize,
) {
    use lsp_types::DiagnosticSeverity;

    if row >= area.height as usize {
        return;
    }

    let y = area.y + row as u16;

    // Get severity color
    let severity_color = match severity {
        DiagnosticSeverity::ERROR => theme.error,
        DiagnosticSeverity::WARNING => theme.warning,
        DiagnosticSeverity::INFORMATION => theme.accented_fg,
        DiagnosticSeverity::HINT => theme.disabled,
        _ => theme.disabled,
    };

    let style = Style::default().fg(severity_color);

    // Render empty gutter (spaces for line number area)
    let gutter_x = area.x;
    for i in 0..line_number_width {
        if let Some(cell) = buf.cell_mut((gutter_x + i, y)) {
            cell.set_char(' ');
            cell.set_style(Style::default().fg(theme.disabled));
        }
    }

    // Content starts after gutter
    let content_x = area.x + line_number_width;

    let full_text = if row_index == 0 {
        // First row: underline + code + message start
        render_diagnostic_first_row(start_col, underline_len, code, message, content_width)
    } else {
        // Continuation row: indented message part
        render_diagnostic_continuation_row(
            start_col,
            underline_len,
            code,
            message,
            content_width,
            row_index,
        )
    };

    // Render visible portion (accounting for horizontal scroll)
    let visible_start = left_column;

    for (i, ch) in full_text
        .chars()
        .skip(visible_start)
        .take(content_width)
        .enumerate()
    {
        let x = content_x + i as u16;
        if x < area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(ch);
                cell.set_style(style);
            }
        }
    }
}

/// Render the first row of a diagnostic (underline + code + message start).
fn render_diagnostic_first_row(
    start_col: usize,
    underline_len: usize,
    code: Option<&str>,
    message: &str,
    content_width: usize,
) -> String {
    const WAVY: &str = "~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~";
    const SPACES: &str = "                                                                                                                                ";
    let wavy = &WAVY[..underline_len.min(WAVY.len())];
    let code_part = code.map(|c| format!(" [{}]", c)).unwrap_or_default();

    // Calculate prefix length
    let prefix_len = start_col + underline_len + code_part.len() + 1; // +1 for space before message

    if content_width > prefix_len {
        let indent = &SPACES[..start_col.min(SPACES.len())];
        let available_for_msg = content_width - prefix_len;
        let msg_part = get_message_part(message, 0, available_for_msg);
        format!("{}{}{} {}", indent, wavy, code_part, msg_part)
    } else if content_width > start_col + underline_len {
        // Only show underline and partial code
        let available = content_width.saturating_sub(start_col + underline_len);
        let truncated_code = truncate_right(&code_part, available);
        let indent = &SPACES[..start_col.min(SPACES.len())];
        format!("{}{}{}", indent, wavy, truncated_code)
    } else if content_width > start_col {
        // Only show partial underline
        let available = content_width.saturating_sub(start_col);
        let indent = &SPACES[..start_col.min(SPACES.len())];
        let partial_wavy = &WAVY[..available.min(underline_len).min(WAVY.len())];
        format!("{}{}", indent, partial_wavy)
    } else {
        // Show what we can from the start
        WAVY[..content_width.min(underline_len).min(WAVY.len())].to_string()
    }
}

/// Render a continuation row of a diagnostic (indented message part).
fn render_diagnostic_continuation_row(
    start_col: usize,
    underline_len: usize,
    code: Option<&str>,
    message: &str,
    content_width: usize,
    row_index: usize,
) -> String {
    // Calculate where message continuation should start (aligned with first row message)
    let continuation_indent = start_col + 2; // Align nicely after underline start
    let continuation_space = content_width.saturating_sub(continuation_indent);

    if continuation_space == 0 {
        return String::new();
    }

    // Calculate how much of the message was shown on previous rows
    let code_part_len = code.map(|c| c.len() + 3).unwrap_or(0);
    let first_row_prefix_len = start_col + underline_len + code_part_len + 1;
    let first_row_msg_space = content_width.saturating_sub(first_row_prefix_len);

    // Calculate char offset for this row
    let chars_before_this_row = if row_index == 1 {
        first_row_msg_space
    } else {
        first_row_msg_space + (row_index - 1) * continuation_space
    };

    let msg_part = get_message_part(message, chars_before_this_row, continuation_space);

    if msg_part.is_empty() {
        return String::new();
    }

    const SPACES: &str = "                                                                                                                                ";
    let indent = &SPACES[..continuation_indent.min(SPACES.len())];
    format!("{}{}", indent, msg_part)
}

/// Get a part of message starting at char_offset, fitting within max_width.
fn get_message_part(message: &str, char_offset: usize, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    // Skip to char_offset using byte index (avoids allocating a String)
    let byte_start = message
        .char_indices()
        .nth(char_offset)
        .map(|(i, _)| i)
        .unwrap_or(message.len());
    let remaining = &message[byte_start..];

    if remaining.is_empty() {
        return String::new();
    }

    // Take up to max_width display width
    let mut result = String::new();
    let mut current_width = 0;

    for ch in remaining.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
        if current_width + ch_width > max_width {
            break;
        }
        result.push(ch);
        current_width += ch_width;
    }

    result
}

/// Render editor content in no-wrap mode with virtual lines.
///
/// This is the main rendering function for no-wrap mode that handles:
/// - Virtual lines (real lines + deletion markers + diagnostics)
/// - Horizontal scrolling
/// - Cursor positioning accounting for virtual lines
#[allow(clippy::too_many_arguments)]
pub fn render_content_no_wrap<H: LineHighlighter>(
    buf: &mut Buffer,
    area: Rect,
    buffer: &TextBuffer,
    viewport: &Viewport,
    cursor: &Cursor,
    git_diff_cache: &Option<GitDiffCache>,
    show_git_diff: bool,
    syntax_highlighting_enabled: bool,
    highlight_cache: &mut H,
    render_context: &RenderContext,
    diagnostics_by_line: &std::collections::HashMap<usize, Vec<git::DiagnosticInfo>>,
    theme: &Theme,
    is_focused: bool,
    content_width: usize,
    content_height: usize,
    line_number_width: u16,
    text_style: Style,
    cursor_line_style: Style,
    search_match_style: Style,
    current_match_style: Style,
    selection_style: Style,
) {
    // Build only the virtual lines visible in the viewport
    let virtual_lines = git::build_virtual_lines_for_viewport(
        buffer,
        git_diff_cache,
        show_git_diff,
        diagnostics_by_line,
        viewport.top_line,
        content_height,
        content_width,
    );

    // Render visible virtual lines
    for (row, virtual_line) in virtual_lines.iter().enumerate().take(content_height) {
        // Handle different types of virtual lines
        match virtual_line {
            git::VirtualLine::Real(line_idx) => {
                // Render real line - use line_cow for zero-copy when possible
                if let Some(line_text) = buffer.line_cow(*line_idx) {
                    let line_text = line_text.trim_end_matches('\n');
                    let is_cursor_line = *line_idx == cursor.line;

                    render_line_no_wrap(
                        buf,
                        area,
                        row,
                        *line_idx,
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
            git::VirtualLine::DeletionMarker(_after_line_idx, deletion_count) => {
                // Render deletion marker virtual line
                super::deletion_markers::render_deletion_marker(
                    buf,
                    area,
                    row,
                    *deletion_count,
                    theme,
                    content_width,
                    line_number_width,
                );
            }
            git::VirtualLine::Diagnostic {
                start_col,
                underline_len,
                severity,
                code,
                message,
                row_index,
                total_rows,
                ..
            } => {
                // Render diagnostic virtual line
                render_diagnostic_virtual_line(
                    buf,
                    area,
                    row,
                    *start_col,
                    *underline_len,
                    *severity,
                    code.as_deref(),
                    message,
                    theme,
                    line_number_width,
                    content_width,
                    viewport.left_column,
                    *row_index,
                    *total_rows,
                );
            }
        }
    }

    // Render cursor accounting for virtual lines
    let cursor_viewport_row = virtual_lines
        .iter()
        .position(|vline| matches!(vline, git::VirtualLine::Real(idx) if *idx == cursor.line));

    if let Some(viewport_row) = cursor_viewport_row {
        // cursor.column is a grapheme index — convert to display width for correct
        // positioning with wide characters (CJK, emoji).
        let cursor_display_col = if let Some(line_text) = buffer.line(cursor.line) {
            use unicode_segmentation::UnicodeSegmentation;
            use unicode_width::UnicodeWidthStr;
            let trimmed = line_text.trim_end_matches('\n');
            trimmed
                .graphemes(true)
                .take(cursor.column)
                .map(|g| g.width())
                .sum::<usize>()
        } else {
            cursor.column
        };

        let left_display_col = if let Some(line_text) = buffer.line(cursor.line) {
            use unicode_segmentation::UnicodeSegmentation;
            use unicode_width::UnicodeWidthStr;
            let trimmed = line_text.trim_end_matches('\n');
            trimmed
                .graphemes(true)
                .take(viewport.left_column)
                .map(|g| g.width())
                .sum::<usize>()
        } else {
            viewport.left_column
        };

        if cursor_display_col >= left_display_col {
            let viewport_col = cursor_display_col - left_display_col;
            let cursor_x = area.x + line_number_width + viewport_col as u16;
            let cursor_y = area.y + viewport_row as u16;
            if viewport_col < content_width && is_focused {
                super::cursor_renderer::render_cursor_at(buf, cursor_x, cursor_y, area, theme);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn test_theme() -> Theme {
        Theme {
            name: "test",
            bg: Color::Black,
            fg: Color::White,
            selected_bg: Color::Blue,
            selected_fg: Color::White,
            accented_bg: Color::DarkGray,
            accented_fg: Color::Cyan,
            disabled: Color::Gray,
            error: Color::Red,
            success: Color::Green,
            warning: Color::Yellow,
            is_light: Some(false),
        }
    }

    fn gutter_text(line_idx: usize, line_number_width: u16) -> String {
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        render_line_gutter(
            &mut buf,
            area,
            0,
            line_idx,
            &None,
            false,
            None,
            &test_theme(),
            line_number_width,
        );
        (0..line_number_width)
            .map(|x| buf.cell((x, 0)).map(|c| c.symbol()).unwrap_or(""))
            .collect()
    }

    #[test]
    fn gutter_right_aligns_four_digit_numbers() {
        assert_eq!(gutter_text(41, 6), "  42  ");
    }

    #[test]
    fn gutter_shows_full_number_in_wide_gutter() {
        // 5-digit line in a widened gutter: the number is not clipped and the
        // marker/separator cells stay to its right.
        assert_eq!(gutter_text(12_344, 7), "12345  ");
        assert_eq!(gutter_text(41, 7), "   42  ");
    }
}
