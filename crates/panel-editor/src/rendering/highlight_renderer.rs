//! Syntax highlighting and text styling.
//!
//! This module provides functions for determining the final visual style of each
//! character based on syntax highlighting, selection, search matches, and cursor position.

use ratatui::style::{Color, Style};

use termide_buffer::Cursor;

use super::context::RenderContext;

/// Determine the final style for a cell at the given position.
///
/// Applies styling priority:
/// 1. Current search match (highest priority)
/// 2. Regular search match
/// 3. Text selection
/// 4. Cursor line (base style with accented background)
/// 5. Base syntax highlighting style
#[allow(clippy::too_many_arguments)] // Logical grouping of styling parameters
pub fn determine_cell_style(
    line: usize,
    column: usize,
    base_style: Style,
    is_cursor_line: bool,
    render_context: &RenderContext,
    search_match_style: Style,
    current_match_style: Style,
    selection_style: Style,
    cursor_line_bg: Color,
) -> Style {
    // Check if this is a search match (O(1) HashMap lookup)
    let match_idx = render_context
        .search_match_map
        .get(&(line, column))
        .copied();

    // Check if this character is in selection
    let is_selected = if let Some((sel_start, sel_end)) = &render_context.selection_range {
        let pos = Cursor::at(line, column);
        (pos.line > sel_start.line
            || (pos.line == sel_start.line && pos.column >= sel_start.column))
            && (pos.line < sel_end.line
                || (pos.line == sel_end.line && pos.column < sel_end.column))
    } else {
        false
    };

    // Determine final style based on priority
    if let Some(idx) = match_idx {
        // Search match - highest priority
        if Some(idx) == render_context.current_match_idx {
            current_match_style
        } else {
            search_match_style
        }
    } else if is_selected {
        // Selected text
        selection_style
    } else if is_cursor_line {
        // Cursor line (but not search match or selection)
        base_style.bg(cursor_line_bg)
    } else {
        // Regular syntax highlighting
        base_style
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Style};
    use std::collections::HashMap;

    fn create_test_context(
        search_matches: Vec<(usize, usize)>,
        current_match_idx: Option<usize>,
        selection_range: Option<(Cursor, Cursor)>,
    ) -> RenderContext {
        let mut search_match_map = HashMap::new();
        for (idx, (line, col)) in search_matches.iter().enumerate() {
            search_match_map.insert((*line, *col), idx);
        }

        RenderContext {
            search_match_map,
            search_matches: vec![],
            current_match_idx,
            selection_range,
            cursor_viewport_pos: None,
        }
    }

    #[test]
    fn test_current_search_match_priority() {
        let context = create_test_context(vec![(0, 5)], Some(0), None);

        let base_style = Style::default().fg(Color::White);
        let search_match_style = Style::default().bg(Color::Yellow);
        let current_match_style = Style::default().bg(Color::Green);
        let selection_style = Style::default().bg(Color::Blue);

        let result = determine_cell_style(
            0,
            5,
            base_style,
            false,
            &context,
            search_match_style,
            current_match_style,
            selection_style,
            Color::DarkGray,
        );

        assert_eq!(result.bg, Some(Color::Green)); // Current match has highest priority
    }

    #[test]
    fn test_regular_search_match_priority() {
        let context = create_test_context(vec![(0, 5)], Some(999), None);

        let base_style = Style::default().fg(Color::White);
        let search_match_style = Style::default().bg(Color::Yellow);
        let current_match_style = Style::default().bg(Color::Green);
        let selection_style = Style::default().bg(Color::Blue);

        let result = determine_cell_style(
            0,
            5,
            base_style,
            false,
            &context,
            search_match_style,
            current_match_style,
            selection_style,
            Color::DarkGray,
        );

        assert_eq!(result.bg, Some(Color::Yellow)); // Regular search match
    }

    #[test]
    fn test_selection_priority() {
        let sel_start = Cursor::at(0, 3);
        let sel_end = Cursor::at(0, 8);
        let context = create_test_context(vec![], None, Some((sel_start, sel_end)));

        let base_style = Style::default().fg(Color::White);
        let search_match_style = Style::default().bg(Color::Yellow);
        let current_match_style = Style::default().bg(Color::Green);
        let selection_style = Style::default().bg(Color::Blue);

        let result = determine_cell_style(
            0,
            5,
            base_style,
            false,
            &context,
            search_match_style,
            current_match_style,
            selection_style,
            Color::DarkGray,
        );

        assert_eq!(result.bg, Some(Color::Blue)); // Selection style
    }

    #[test]
    fn test_cursor_line_priority() {
        let context = create_test_context(vec![], None, None);

        let base_style = Style::default().fg(Color::White).bg(Color::Black);
        let search_match_style = Style::default().bg(Color::Yellow);
        let current_match_style = Style::default().bg(Color::Green);
        let selection_style = Style::default().bg(Color::Blue);

        let result = determine_cell_style(
            0,
            5,
            base_style,
            true, // is_cursor_line = true
            &context,
            search_match_style,
            current_match_style,
            selection_style,
            Color::DarkGray,
        );

        assert_eq!(result.bg, Some(Color::DarkGray)); // Cursor line bg
        assert_eq!(result.fg, Some(Color::White)); // Preserves base fg
    }

    #[test]
    fn test_base_style_fallback() {
        let context = create_test_context(vec![], None, None);

        let base_style = Style::default().fg(Color::Cyan).bg(Color::Black);
        let search_match_style = Style::default().bg(Color::Yellow);
        let current_match_style = Style::default().bg(Color::Green);
        let selection_style = Style::default().bg(Color::Blue);

        let result = determine_cell_style(
            0,
            5,
            base_style,
            false,
            &context,
            search_match_style,
            current_match_style,
            selection_style,
            Color::DarkGray,
        );

        assert_eq!(result, base_style); // Returns base style unchanged
    }
}
