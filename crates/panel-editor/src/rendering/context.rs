//! Rendering context for editor display.
//!
//! This module provides the RenderContext struct that encapsulates all state
//! needed for rendering the editor content area.

use std::collections::HashMap;

use lsp_types::{Diagnostic, DiagnosticSeverity};
use termide_buffer::{Cursor, SearchState, Selection};

/// Pre-computed rendering context.
///
/// Contains all derived state needed for efficient rendering without
/// repeatedly querying the editor during the render loop.
#[derive(Debug)]
pub struct RenderContext {
    /// Map of (line, column) -> match_index for O(1) search highlight lookups.
    pub search_match_map: HashMap<(usize, usize), usize>,

    /// Index of the current (active) search match, if any.
    pub current_match_idx: Option<usize>,

    /// Selection range as (start, end) cursors, if selection exists.
    pub selection_range: Option<(Cursor, Cursor)>,

    /// Cursor position in viewport coordinates (row, col), set during rendering.
    pub cursor_viewport_pos: Option<(usize, usize)>,

    /// Map of line -> most severe diagnostic severity for gutter markers.
    pub diagnostic_line_severity: HashMap<usize, DiagnosticSeverity>,

    /// Map of (line, column) -> diagnostic severity for inline underlines.
    pub diagnostic_ranges: HashMap<(usize, usize), DiagnosticSeverity>,
}

impl RenderContext {
    /// Prepare rendering context from editor state.
    ///
    /// Extracts and pre-computes all derived state needed for rendering.
    /// `visible_lines` bounds the search-highlight map to the physical lines
    /// that can appear on screen: the per-character map is only ever queried
    /// for visible rows, so building entries for the whole document (which can
    /// be tens of thousands of inserts per keystroke when a common substring
    /// matches) is wasted work.
    pub fn prepare(
        search_state: &Option<SearchState>,
        selection: &Option<Selection>,
        diagnostics: &[Diagnostic],
        visible_lines: std::ops::Range<usize>,
    ) -> Self {
        // Pre-extract match information
        let search_matches: Vec<(usize, usize, usize)> = if let Some(ref search) = search_state {
            search
                .matches
                .iter()
                .map(|c| (c.line, c.column, search.query.chars().count()))
                .collect()
        } else {
            Vec::new()
        };

        let current_match_idx = search_state.as_ref().and_then(|s| s.current_match);

        // Build search match map for O(1) lookups during rendering
        let search_match_map = build_search_match_map(&search_matches, &visible_lines);

        // Pre-extract selection information
        let selection_range = selection.as_ref().map(|s| (s.start(), s.end()));

        // Build diagnostic maps for gutter markers and inline underlines
        let (diagnostic_line_severity, diagnostic_ranges) = build_diagnostic_maps(diagnostics);

        Self {
            search_match_map,
            current_match_idx,
            selection_range,
            cursor_viewport_pos: None,
            diagnostic_line_severity,
            diagnostic_ranges,
        }
    }

    /// Get diagnostic severity for a line (for gutter marker).
    pub fn diagnostic_severity_at_line(&self, line: usize) -> Option<DiagnosticSeverity> {
        self.diagnostic_line_severity.get(&line).copied()
    }

    /// Get diagnostic severity at a specific position (for inline underline).
    pub fn diagnostic_severity_at(&self, line: usize, column: usize) -> Option<DiagnosticSeverity> {
        self.diagnostic_ranges.get(&(line, column)).copied()
    }
}

/// Build HashMap for O(1) search match lookups during rendering.
///
/// Maps each (line, column) coordinate within a match to its match index.
/// This allows fast character-by-character highlighting during rendering.
///
/// Only matches on lines within `visible_lines` are inserted — off-screen
/// entries are never queried. Match indices remain the position in the full
/// `search_matches` list so they stay comparable with `current_match`.
fn build_search_match_map(
    search_matches: &[(usize, usize, usize)],
    visible_lines: &std::ops::Range<usize>,
) -> HashMap<(usize, usize), usize> {
    let capacity: usize = search_matches
        .iter()
        .filter(|(m_line, _, _)| visible_lines.contains(m_line))
        .map(|(_, _, m_len)| *m_len)
        .sum();
    let mut map = HashMap::with_capacity(capacity);

    for (idx, &(m_line, m_col, m_len)) in search_matches.iter().enumerate() {
        if !visible_lines.contains(&m_line) {
            continue;
        }
        for col in m_col..(m_col + m_len) {
            map.insert((m_line, col), idx);
        }
    }

    map
}

/// Build diagnostic maps for O(1) lookups during rendering.
///
/// Returns two maps:
/// 1. Line -> most severe diagnostic severity (for gutter markers)
/// 2. Empty map (inline underlines disabled - virtual diagnostic lines are used instead)
fn build_diagnostic_maps(
    diagnostics: &[Diagnostic],
) -> (
    HashMap<usize, DiagnosticSeverity>,
    HashMap<(usize, usize), DiagnosticSeverity>,
) {
    let mut line_severity: HashMap<usize, DiagnosticSeverity> = HashMap::new();

    for diag in diagnostics {
        let severity = diag.severity.unwrap_or(DiagnosticSeverity::ERROR);
        let start_line = diag.range.start.line as usize;
        let end_line = diag.range.end.line as usize;

        // Update line severity (keep most severe) for gutter markers
        for line in start_line..=end_line {
            line_severity
                .entry(line)
                .and_modify(|existing| {
                    if severity_priority(severity) < severity_priority(*existing) {
                        *existing = severity;
                    }
                })
                .or_insert(severity);
        }
    }

    // Return empty map for inline underlines - virtual diagnostic lines are used instead
    (line_severity, HashMap::new())
}

/// Get priority for diagnostic severity (lower is more severe).
fn severity_priority(severity: DiagnosticSeverity) -> u8 {
    match severity {
        DiagnosticSeverity::ERROR => 0,
        DiagnosticSeverity::WARNING => 1,
        DiagnosticSeverity::INFORMATION => 2,
        DiagnosticSeverity::HINT => 3,
        _ => 4,
    }
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

        let map = build_search_match_map(&matches, &(0..1000));

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
        let map = build_search_match_map(&matches, &(0..1000));
        assert!(map.is_empty());
    }
}
