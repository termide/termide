//! Rendering utilities for the editor.
//!
//! This module provides the complete rendering system for the text editor,
//! with separate implementations for word wrap and no-wrap modes.

use lsp_types::Diagnostic;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use termide_buffer::{Cursor, SearchState, Selection, TextBuffer, Viewport};
use termide_git::GitDiffCache;
use termide_highlight::LineHighlighter;
use termide_theme::Theme;

pub mod context;
pub mod cursor_renderer;
pub mod deletion_markers;
pub mod highlight_renderer;
pub mod inline_diff;
pub mod line_rendering;
pub mod wrap_rendering;

/// Right-align a number into a fixed-width string without heap allocation.
/// Returns a `&str` of exactly `width` chars (space-padded on the left), or the
/// bare digits when the number needs more room than `width`.
pub(crate) fn itoa_right_align(n: usize, width: usize, buf: &mut [u8; 20]) -> &str {
    let width = width.min(20);
    let mut pos = 20;
    let mut val = n;
    if val == 0 {
        pos -= 1;
        buf[pos] = b'0';
    } else {
        while val > 0 {
            pos -= 1;
            buf[pos] = b'0' + (val % 10) as u8;
            val /= 10;
        }
    }
    let digit_count = 20 - pos;
    if digit_count >= width {
        std::str::from_utf8(&buf[pos..]).unwrap_or("????")
    } else {
        let start = 20 - width;
        buf[start..pos].fill(b' ');
        std::str::from_utf8(&buf[start..]).unwrap_or("????")
    }
}

/// Cells grapheme `g` is drawn into when it takes `width` columns, and the
/// symbol each gets: a TAB fills every column it spans with a blank (so a
/// selection or the cursor-line background covers all of it); any other
/// cluster is drawn in its first column, a wide one's second being ratatui's
/// to skip.
pub(crate) fn drawn_cells(g: &str, width: usize) -> (usize, &str) {
    if g == "\t" {
        (width, " ")
    } else {
        (1, termide_ui::cell_symbol(g))
    }
}

/// Gutter cells that follow the digits: LSP marker + separator space.
pub const LINE_NUMBER_MARKER_CELLS: usize = 2;

/// Number of decimal digits needed to print `n` (at least 1).
fn decimal_digits(n: usize) -> usize {
    let mut digits = 1;
    let mut val = n;
    while val >= 10 {
        digits += 1;
        val /= 10;
    }
    digits
}

/// Digit cells reserved for line numbers in a buffer of `total_lines` lines:
/// exactly what the last line number needs. Nothing is held back for growth,
/// so the content shifts one column when a buffer crosses 9, 99, 999 lines —
/// a rare moment, against a gutter that otherwise wastes width on every
/// short file.
pub fn line_number_digits(total_lines: usize) -> usize {
    decimal_digits(total_lines)
}

/// Width of the line number column (digits + LSP marker + separator).
pub fn line_number_width(total_lines: usize) -> usize {
    line_number_digits(total_lines) + LINE_NUMBER_MARKER_CELLS
}

/// Calculate content area dimensions.
///
/// Returns (content_width, content_height) accounting for line numbers.
pub fn calculate_content_dimensions(
    area_width: u16,
    area_height: u16,
    total_lines: usize,
) -> (usize, usize) {
    let content_width = (area_width as usize).saturating_sub(line_number_width(total_lines));
    let content_height = area_height as usize;
    (content_width, content_height)
}

/// Render editor content with word wrap or no-wrap mode.
///
/// This is the main orchestrator function that:
/// - Creates rendering styles based on theme
/// - Prepares rendering context (search matches, selection, diagnostics)
/// - Selects appropriate rendering mode (word wrap vs no wrap)
/// - Delegates to specialized rendering functions
///
/// Returns the screen cell of the cursor when it is on screen.
#[allow(clippy::too_many_arguments)]
pub fn render_editor_content<H: LineHighlighter>(
    buf: &mut Buffer,
    area: Rect,
    buffer: &TextBuffer,
    viewport: &Viewport,
    cursor: &Cursor,
    git_diff_cache: &Option<GitDiffCache>,
    syntax_highlighting_enabled: bool,
    highlight_cache: &mut H,
    search_state: &Option<SearchState>,
    selection: &Option<Selection>,
    diagnostics: &[Diagnostic],
    theme: &Theme,
    is_focused: bool,
    show_git_diff: bool,
    word_wrap_enabled: bool,
    use_smart_wrap: bool,
    tab_size: usize,
    content_width: usize,
    content_height: usize,
) -> Option<(u16, u16)> {
    let line_number_width = line_number_width(buffer.line_count()) as u16;

    // Create rendering styles from theme
    let text_style = Style::default().fg(theme.fg);
    let line_number_style = Style::default().fg(theme.disabled);
    // When the panel is unfocused, drop the current-line highlight so the
    // inactive editor looks fully passive — mirrors how panel-file-manager
    // hides the cursor on blur.
    let cursor_line_style = if is_focused {
        Style::default().bg(theme.accented_bg).fg(theme.accented_fg)
    } else {
        text_style
    };

    let search_match_style = Style::default().bg(theme.warning).fg(theme.bg);

    let current_match_style = Style::default()
        .bg(theme.accented_fg)
        .fg(theme.bg)
        .add_modifier(Modifier::BOLD);

    let selection_style = Style::default().bg(theme.selected_bg).fg(theme.selected_fg);

    // Prepare rendering context. Bound the search-highlight map to the
    // physical lines that can be on screen; in word-wrap mode fewer physical
    // lines fit than `content_height`, so this range is a safe superset.
    let visible_lines = viewport.top_line
        ..viewport
            .top_line
            .saturating_add(content_height)
            .saturating_add(1);
    let mut render_context = context::RenderContext::prepare(
        search_state,
        selection,
        diagnostics,
        visible_lines,
        tab_size,
    );
    render_context.breakpoint_lines = buffer.line_marks().clone();
    render_context.execution_line = buffer
        .file_path()
        .and_then(termide_debug::execution::line_in);

    // Group diagnostics by line once per render — hot paths read this
    // instead of rebuilding the HashMap for every visible row.
    let diagnostics_by_line = crate::git::group_diagnostics_by_line(diagnostics, buffer, tab_size);

    // Rebuild the whole-document highlight when it is stale (after an edit or a
    // syntax change) and the buffer is small enough to re-parse per edit. This
    // resolves cross-line context — PHP's HTML/PHP mode, multi-line strings and
    // comments — that the per-line path cannot. Large buffers skip it and fall
    // back to per-line highlighting to keep editing responsive.
    if syntax_highlighting_enabled
        && highlight_cache.needs_document()
        && buffer.len_bytes() <= termide_highlight::WHOLE_DOCUMENT_BYTE_LIMIT
    {
        highlight_cache.set_document(&buffer.text());
    }

    // Select rendering mode
    if word_wrap_enabled && content_width > 0 {
        // Word wrap mode
        wrap_rendering::render_content_word_wrap(
            buf,
            area,
            buffer,
            viewport,
            cursor,
            git_diff_cache,
            show_git_diff,
            syntax_highlighting_enabled,
            highlight_cache,
            &mut render_context,
            &diagnostics_by_line,
            theme,
            is_focused,
            content_width,
            content_height,
            line_number_width,
            use_smart_wrap,
            tab_size,
            text_style,
            cursor_line_style,
            line_number_style,
            search_match_style,
            current_match_style,
            selection_style,
        );
    } else {
        // No-wrap mode
        line_rendering::render_content_no_wrap(
            buf,
            area,
            buffer,
            viewport,
            cursor,
            git_diff_cache,
            show_git_diff,
            syntax_highlighting_enabled,
            highlight_cache,
            &mut render_context,
            &diagnostics_by_line,
            theme,
            is_focused,
            content_width,
            content_height,
            line_number_width,
            text_style,
            cursor_line_style,
            search_match_style,
            current_match_style,
            selection_style,
        );
    }

    // Where the cursor was drawn (or would be, unfocused): the anchor for
    // popups that open at the cursor.
    render_context.cursor_viewport_pos.and_then(|(row, col)| {
        (row < content_height && col < content_width)
            .then(|| (area.x + line_number_width + col as u16, area.y + row as u16))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_number_width_is_exactly_what_the_last_line_needs() {
        // digits + marker + separator
        assert_eq!(line_number_width(1), 3);
        assert_eq!(line_number_width(3), 3);
        assert_eq!(line_number_width(9), 3);
        assert_eq!(line_number_width(10), 4);
        assert_eq!(line_number_width(99), 4);
        assert_eq!(line_number_width(100), 5);
        assert_eq!(line_number_width(9_999), 6);
    }

    #[test]
    fn line_number_width_grows_with_line_count() {
        assert_eq!(line_number_width(10_000), 7);
        assert_eq!(line_number_width(123_456), 8);
        assert_eq!(line_number_width(1_000_000), 9);
    }

    #[test]
    fn content_width_shrinks_as_gutter_grows() {
        let (small, _) = calculate_content_dimensions(80, 24, 500);
        let (large, _) = calculate_content_dimensions(80, 24, 250_000);
        assert_eq!(small, 80 - 5, "3 digits + marker + separator");
        assert_eq!(large, 80 - 8, "6 digits + marker + separator");
    }

    #[test]
    fn itoa_right_align_pads_and_overflows() {
        let mut buf = [0u8; 20];
        assert_eq!(itoa_right_align(42, 4, &mut buf), "  42");
        let mut buf = [0u8; 20];
        assert_eq!(itoa_right_align(123_456, 6, &mut buf), "123456");
        // Narrower than the number: digits win over padding, never truncation.
        let mut buf = [0u8; 20];
        assert_eq!(itoa_right_align(123_456, 4, &mut buf), "123456");
    }
}
