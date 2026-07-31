use crate::Cursor;

/// Search state in editor
#[derive(Debug, Clone)]
pub struct SearchState {
    /// Search query
    pub query: String,
    /// Replace string (if replace mode is enabled)
    pub replace_with: Option<String>,
    /// Current position in results
    pub current_match: Option<usize>,
    /// All found matches (line, column) — the start of each match
    pub matches: Vec<Cursor>,
    /// Grapheme length of each match, parallel to `matches`. For literal
    /// search every entry equals the query length; for regex search the
    /// lengths vary per match, so they must be tracked explicitly.
    pub match_lengths: Vec<usize>,
    /// Case sensitivity
    pub case_sensitive: bool,
    /// Treat `query` as a regular expression (capture groups usable in
    /// `replace_with` as `$1` / `${name}`). When false, `query` is literal.
    pub use_regex: bool,
}

impl SearchState {
    /// Create a new search state
    pub fn new(query: String, case_sensitive: bool) -> Self {
        Self {
            query,
            replace_with: None,
            current_match: None,
            matches: Vec::new(),
            match_lengths: Vec::new(),
            case_sensitive,
            use_regex: false,
        }
    }

    /// Create search state with replace
    pub fn new_with_replace(query: String, replace_with: String, case_sensitive: bool) -> Self {
        Self {
            query,
            replace_with: Some(replace_with),
            current_match: None,
            matches: Vec::new(),
            match_lengths: Vec::new(),
            case_sensitive,
            use_regex: false,
        }
    }

    /// Builder: enable/disable regex mode.
    pub fn with_regex(mut self, use_regex: bool) -> Self {
        self.use_regex = use_regex;
        self
    }

    /// Grapheme length of the match at `idx`, falling back to the query's
    /// grapheme length (literal search) when not explicitly recorded.
    pub fn match_len_at(&self, idx: usize) -> usize {
        self.match_lengths
            .get(idx)
            .copied()
            .unwrap_or_else(|| self.query.chars().count())
    }

    /// Check if search is active
    pub fn is_active(&self) -> bool {
        !self.query.is_empty()
    }

    /// Get match count
    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// Go to next match
    pub fn next_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }

        self.current_match = Some(match self.current_match {
            Some(idx) => (idx + 1) % self.matches.len(),
            None => 0,
        });
    }

    /// Go to previous match
    pub fn prev_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }

        self.current_match = Some(match self.current_match {
            Some(0) => self.matches.len() - 1,
            Some(idx) => idx - 1,
            None => self.matches.len() - 1,
        });
    }

    /// Find closest match to cursor
    pub fn find_closest_match(&mut self, cursor: &Cursor) {
        if self.matches.is_empty() {
            self.current_match = None;
            return;
        }

        // Find first match after cursor
        for (idx, match_cursor) in self.matches.iter().enumerate() {
            if match_cursor.line > cursor.line
                || (match_cursor.line == cursor.line && match_cursor.column >= cursor.column)
            {
                self.current_match = Some(idx);
                return;
            }
        }

        // If nothing found, return to beginning
        self.current_match = Some(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_state_navigation() {
        let mut state = SearchState::new("test".to_string(), false);
        state.matches = vec![
            Cursor { line: 0, column: 0 },
            Cursor { line: 1, column: 5 },
            Cursor {
                line: 2,
                column: 10,
            },
        ];

        // Initial state
        assert_eq!(state.current_match, None);

        // Go to next
        state.next_match();
        assert_eq!(state.current_match, Some(0));

        state.next_match();
        assert_eq!(state.current_match, Some(1));

        state.next_match();
        assert_eq!(state.current_match, Some(2));

        // Cycle
        state.next_match();
        assert_eq!(state.current_match, Some(0));

        // Previous
        state.prev_match();
        assert_eq!(state.current_match, Some(2));
    }

    #[test]
    fn test_find_closest_match() {
        let mut state = SearchState::new("test".to_string(), false);
        state.matches = vec![
            Cursor { line: 0, column: 0 },
            Cursor {
                line: 5,
                column: 10,
            },
            Cursor {
                line: 10,
                column: 5,
            },
        ];

        // Cursor before first match
        state.find_closest_match(&Cursor { line: 0, column: 0 });
        assert_eq!(state.current_match, Some(0));

        // Cursor between matches
        state.find_closest_match(&Cursor { line: 3, column: 0 });
        assert_eq!(state.current_match, Some(1));

        // Cursor after all matches - return to beginning
        state.find_closest_match(&Cursor {
            line: 20,
            column: 0,
        });
        assert_eq!(state.current_match, Some(0));
    }
}
