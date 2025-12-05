//! Rendering context for editor display.
//!
//! This module provides the RenderContext struct that encapsulates all state
//! needed for rendering the editor content area.

use std::collections::HashMap;

use crate::editor::Cursor;

/// Pre-computed rendering context.
///
/// Contains all derived state needed for efficient rendering without
/// repeatedly querying the editor during the render loop.
#[derive(Debug)]
#[allow(dead_code)] // Used in Phase 4.2+
pub struct RenderContext {
    /// Map of (line, column) -> match_index for O(1) search highlight lookups.
    pub search_match_map: HashMap<(usize, usize), usize>,

    /// List of search matches as (line, column, length) tuples.
    pub search_matches: Vec<(usize, usize, usize)>,

    /// Index of the current (active) search match, if any.
    pub current_match_idx: Option<usize>,

    /// Selection range as (start, end) cursors, if selection exists.
    pub selection_range: Option<(Cursor, Cursor)>,

    /// Cursor position in viewport coordinates (row, col), set during rendering.
    pub cursor_viewport_pos: Option<(usize, usize)>,
}

impl RenderContext {
    /// Prepare rendering context from editor state.
    ///
    /// Extracts and pre-computes all derived state needed for rendering.
    #[allow(dead_code)] // Used in Phase 4.2+
    pub fn prepare(
        search_state: &Option<crate::editor::SearchState>,
        selection: &Option<crate::editor::Selection>,
    ) -> Self {
        // Pre-extract match information
        let search_matches: Vec<(usize, usize, usize)> = if let Some(ref search) = search_state {
            search
                .matches
                .iter()
                .map(|c| (c.line, c.column, search.query.len()))
                .collect()
        } else {
            Vec::new()
        };

        let current_match_idx = search_state.as_ref().and_then(|s| s.current_match);

        // Build search match map for O(1) lookups during rendering
        let search_match_map = build_search_match_map(&search_matches);

        // Pre-extract selection information
        let selection_range = selection.as_ref().map(|s| (s.start(), s.end()));

        Self {
            search_match_map,
            search_matches,
            current_match_idx,
            selection_range,
            cursor_viewport_pos: None,
        }
    }
}

/// Build HashMap for O(1) search match lookups during rendering.
///
/// Maps each (line, column) coordinate within a match to its match index.
/// This allows fast character-by-character highlighting during rendering.
fn build_search_match_map(
    search_matches: &[(usize, usize, usize)],
) -> HashMap<(usize, usize), usize> {
    let mut map = HashMap::with_capacity(search_matches.len() * 10);

    for (idx, &(m_line, m_col, m_len)) in search_matches.iter().enumerate() {
        for col in m_col..(m_col + m_len) {
            map.insert((m_line, col), idx);
        }
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_search_match_map() {
        let matches = vec![
            (0, 5, 3),  // Line 0, col 5-7
            (1, 10, 4), // Line 1, col 10-13
        ];

        let map = build_search_match_map(&matches);

        // First match
        assert_eq!(map.get(&(0, 5)), Some(&0));
        assert_eq!(map.get(&(0, 6)), Some(&0));
        assert_eq!(map.get(&(0, 7)), Some(&0));
        assert_eq!(map.get(&(0, 8)), None);

        // Second match
        assert_eq!(map.get(&(1, 10)), Some(&1));
        assert_eq!(map.get(&(1, 11)), Some(&1));
        assert_eq!(map.get(&(1, 12)), Some(&1));
        assert_eq!(map.get(&(1, 13)), Some(&1));
        assert_eq!(map.get(&(1, 14)), None);
    }

    #[test]
    fn test_build_search_match_map_empty() {
        let matches = vec![];
        let map = build_search_match_map(&matches);
        assert!(map.is_empty());
    }
}
