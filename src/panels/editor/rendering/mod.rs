//! Rendering utilities for the editor.
//!
//! This module provides the complete rendering system for the text editor,
//! with separate implementations for word wrap and no-wrap modes.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

pub mod context;
pub mod cursor_renderer;
pub mod deletion_markers;
pub mod highlight_renderer;
pub mod line_rendering;
pub mod wrap_rendering;

/// Width of the line number column (including git markers).
///
/// Format: "  123  " (2 spaces + 3 digits + 2 git markers)
pub const LINE_NUMBER_WIDTH: usize = 6;

/// Calculate content area dimensions.
///
/// Returns (content_width, content_height) accounting for line numbers.
pub fn calculate_content_dimensions(area_width: u16, area_height: u16) -> (usize, usize) {
    let content_width = (area_width as usize).saturating_sub(LINE_NUMBER_WIDTH);
    let content_height = area_height as usize;
    (content_width, content_height)
}

/// Render editor content with word wrap or no-wrap mode.
///
/// This is the main orchestrator function that:
/// - Creates rendering styles based on theme
/// - Prepares rendering context (search matches, selection)
/// - Selects appropriate rendering mode (word wrap vs no wrap)
/// - Delegates to specialized rendering functions
#[allow(clippy::too_many_arguments)]
pub fn render_editor_content<H: crate::editor::LineHighlighter>(
    buf: &mut Buffer,
    area: Rect,
    buffer: &crate::editor::TextBuffer,
    viewport: &crate::editor::Viewport,
    cursor: &crate::editor::Cursor,
    git_diff_cache: &Option<crate::git::GitDiffCache>,
    syntax_highlighting_enabled: bool,
    highlight_cache: &mut H,
    search_state: &Option<crate::editor::SearchState>,
    selection: &Option<crate::editor::Selection>,
    theme: &crate::theme::Theme,
    show_git_diff: bool,
    word_wrap_enabled: bool,
    use_smart_wrap: bool,
    content_width: usize,
    content_height: usize,
) {
    let line_number_width = LINE_NUMBER_WIDTH as u16;

    // Create rendering styles from theme
    let text_style = Style::default().fg(theme.fg);
    let line_number_style = Style::default().fg(theme.disabled);
    let cursor_line_style = Style::default().bg(theme.accented_bg).fg(theme.fg);

    let search_match_style = Style::default().bg(theme.warning).fg(theme.bg);

    let current_match_style = Style::default()
        .bg(theme.accented_fg)
        .fg(theme.bg)
        .add_modifier(Modifier::BOLD);

    let selection_style = Style::default().bg(theme.selected_bg).fg(theme.selected_fg);

    // Prepare rendering context
    let mut render_context = context::RenderContext::prepare(search_state, selection);

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
            theme,
            content_width,
            content_height,
            line_number_width,
            use_smart_wrap,
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
            &render_context,
            theme,
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
}
