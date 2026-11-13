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
}
