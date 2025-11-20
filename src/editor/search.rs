use crate::editor::cursor::Cursor;

/// Search direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchDirection {
    Forward,
    Backward,
}

/// Search state in editor
#[derive(Debug, Clone)]
pub struct SearchState {
    /// Search query
    pub query: String,
    /// Replace string (if replace mode is enabled)
    pub replace_with: Option<String>,
    /// Current position in results
    pub current_match: Option<usize>,
    /// All found matches (line, column)
    pub matches: Vec<Cursor>,
    /// Case sensitivity
    pub case_sensitive: bool,
    /// Search direction
    pub direction: SearchDirection,
}

impl SearchState {
    /// Create a new search state
    pub fn new(query: String, case_sensitive: bool) -> Self {
        Self {
            query,
            replace_with: None,
            current_match: None,
            matches: Vec::new(),
            case_sensitive,
            direction: SearchDirection::Forward,
        }
    }

    /// Create search state with replace
    pub fn new_with_replace(query: String, replace_with: String, case_sensitive: bool) -> Self {
        Self {
            query,
            replace_with: Some(replace_with),
            current_match: None,
            matches: Vec::new(),
            case_sensitive,
            direction: SearchDirection::Forward,
        }
    }

    /// Check if replace mode is enabled
    pub fn is_replace_mode(&self) -> bool {
        self.replace_with.is_some()
    }

    /// Check if search is active
    pub fn is_active(&self) -> bool {
        !self.query.is_empty()
    }

    /// Get match count
    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// Get current match
    pub fn current_match_cursor(&self) -> Option<&Cursor> {
        self.current_match
            .and_then(|idx| self.matches.get(idx))
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

    /// Clear search state
    pub fn clear(&mut self) {
        self.query.clear();
        self.matches.clear();
        self.current_match = None;
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
            Cursor { line: 2, column: 10 },
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
            Cursor { line: 5, column: 10 },
            Cursor { line: 10, column: 5 },
        ];

        // Cursor before first match
        state.find_closest_match(&Cursor { line: 0, column: 0 });
        assert_eq!(state.current_match, Some(0));

        // Cursor between matches
        state.find_closest_match(&Cursor { line: 3, column: 0 });
        assert_eq!(state.current_match, Some(1));

        // Cursor after all matches - return to beginning
        state.find_closest_match(&Cursor { line: 20, column: 0 });
        assert_eq!(state.current_match, Some(0));
    }
}
