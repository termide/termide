//! Search state management for the editor.

use termide_buffer::SearchState;

/// Search-related state for the editor.
#[derive(Default)]
pub(crate) struct SearchController {
    /// Current search state (active search session).
    pub state: Option<SearchState>,
    /// Last search query (preserved when search is closed).
    pub last_query: Option<String>,
    /// Last replace find query (preserved when replace is closed).
    pub last_replace_find: Option<String>,
    /// Last replace with text (preserved when replace is closed).
    pub last_replace_with: Option<String>,
}

impl SearchController {
    /// Create new empty SearchController.
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if search is active.
    pub fn is_active(&self) -> bool {
        self.state.is_some()
    }

    /// Get current search query.
    pub fn current_query(&self) -> Option<&str> {
        self.state.as_ref().map(|s| s.query.as_str())
    }

    /// Save current search query as last query.
    pub fn save_last_query(&mut self) {
        if let Some(ref state) = self.state {
            if !state.query.is_empty() {
                self.last_query = Some(state.query.clone());
            }
        }
    }

    /// Clear active search state.
    pub fn clear(&mut self) {
        self.save_last_query();
        self.state = None;
    }

    /// Save replace queries.
    pub fn save_replace_queries(&mut self, find: &str, replace: &str) {
        if !find.is_empty() {
            self.last_replace_find = Some(find.to_string());
        }
        if !replace.is_empty() {
            self.last_replace_with = Some(replace.to_string());
        }
    }
}
