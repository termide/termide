//! Text input with dropdown suggestions.
//!
//! Provides a reusable component for text inputs with dropdown suggestions.
//! Used by modals that need combobox-like functionality with Up/Down navigation,
//! Tab to toggle dropdown, Enter to confirm, and Escape to cancel.

use crate::{TextInput, Viewport};
use crossterm::event::{KeyCode, KeyEvent};

/// Dropdown state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DropdownState {
    /// Collapsed - only input field visible.
    #[default]
    Collapsed,
    /// Expanded - input field + suggestions list visible.
    Expanded,
}

/// Result of handling a key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionAction {
    /// Key was handled (navigation, toggle).
    Handled,
    /// Selection confirmed (Enter while expanded).
    Confirmed,
    /// Collapsed with rollback (Escape while expanded).
    Cancelled,
    /// Text was modified.
    TextModified,
    /// Key not handled - caller should process.
    NotHandled,
}

/// A text input with dropdown suggestions.
///
/// This component handles:
/// - Text input with cursor management
/// - Dropdown state (expanded/collapsed)
/// - Up/Down navigation through suggestions
/// - Tab to toggle dropdown
/// - Enter to confirm selection
/// - Escape to cancel and rollback
///
/// The component does NOT handle rendering - callers render using the state accessors.
#[derive(Debug, Clone)]
pub struct SuggestionInput {
    input: TextInput,
    suggestions: Vec<String>,
    selected_index: usize,
    state: DropdownState,
    saved_input: String,
    viewport: Viewport,
    max_visible: usize,
}

impl SuggestionInput {
    /// Default maximum visible suggestions.
    const DEFAULT_MAX_VISIBLE: usize = 6;

    /// Create a new suggestion input with the given suggestions.
    pub fn new(suggestions: Vec<String>) -> Self {
        let total = suggestions.len();
        Self {
            input: TextInput::new(),
            suggestions,
            selected_index: 0,
            state: DropdownState::Collapsed,
            saved_input: String::new(),
            viewport: Viewport::new(Self::DEFAULT_MAX_VISIBLE, total),
            max_visible: Self::DEFAULT_MAX_VISIBLE,
        }
    }

    /// Create a suggestion input with initial text.
    #[must_use]
    pub fn with_text(text: impl Into<String>, suggestions: Vec<String>) -> Self {
        let text = text.into();
        let total = suggestions.len();
        Self {
            input: TextInput::with_text(text.clone()),
            suggestions,
            selected_index: 0,
            state: DropdownState::Collapsed,
            saved_input: text,
            viewport: Viewport::new(Self::DEFAULT_MAX_VISIBLE, total),
            max_visible: Self::DEFAULT_MAX_VISIBLE,
        }
    }

    /// Set maximum visible suggestions in dropdown.
    pub fn with_max_visible(mut self, max: usize) -> Self {
        self.max_visible = max;
        self.viewport.set_visible_height(max);
        self
    }

    // === State accessors ===

    /// Get the current input text.
    pub fn text(&self) -> &str {
        self.input.text()
    }

    /// Get reference to the underlying TextInput.
    pub fn input(&self) -> &TextInput {
        &self.input
    }

    /// Get mutable reference to the underlying TextInput.
    pub fn input_mut(&mut self) -> &mut TextInput {
        &mut self.input
    }

    /// Check if dropdown is expanded.
    pub fn is_expanded(&self) -> bool {
        self.state == DropdownState::Expanded
    }

    /// Get current dropdown state.
    pub fn state(&self) -> DropdownState {
        self.state
    }

    /// Get the currently selected suggestion index.
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// Get reference to suggestions list.
    pub fn suggestions(&self) -> &[String] {
        &self.suggestions
    }

    /// Get reference to viewport for scroll management.
    pub fn viewport(&self) -> &Viewport {
        &self.viewport
    }

    /// Get maximum visible items count.
    pub fn max_visible(&self) -> usize {
        self.max_visible
    }

    // === Dropdown control ===

    /// Expand dropdown and save current input for potential rollback.
    /// If current text matches a suggestion, positions cursor on that item.
    pub fn expand(&mut self) {
        if self.state == DropdownState::Collapsed {
            self.saved_input = self.input.text().to_string();
            self.state = DropdownState::Expanded;

            // Find matching suggestion index, default to 0
            let current_text = self.input.text();
            self.selected_index = self
                .suggestions
                .iter()
                .position(|s| s == current_text)
                .unwrap_or(0);
            self.viewport.ensure_visible(self.selected_index);
        }
    }

    /// Collapse dropdown without changing input.
    pub fn collapse(&mut self) {
        self.state = DropdownState::Collapsed;
    }

    /// Rollback to saved input and collapse dropdown.
    pub fn rollback(&mut self) {
        self.input.set_text(self.saved_input.clone());
        self.state = DropdownState::Collapsed;
    }

    /// Toggle dropdown state.
    pub fn toggle(&mut self) {
        match self.state {
            DropdownState::Collapsed => self.expand(),
            DropdownState::Expanded => self.collapse(),
        }
    }

    /// Confirm current selection (set input to selected suggestion) and collapse.
    pub fn confirm(&mut self) {
        if !self.suggestions.is_empty() {
            if let Some(selected) = self.suggestions.get(self.selected_index) {
                self.input.set_text(selected.clone());
            }
        }
        self.state = DropdownState::Collapsed;
    }

    /// Select item at index and confirm (used for mouse clicks).
    /// Returns true if selection was valid, false otherwise.
    pub fn select_and_confirm(&mut self, index: usize) -> bool {
        if index < self.suggestions.len() {
            self.selected_index = index;
            self.confirm();
            true
        } else {
            false
        }
    }

    // === Navigation ===

    /// Move selection up in the suggestions list.
    /// Updates input text to show selected value immediately.
    pub fn select_up(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }

        if self.selected_index > 0 {
            self.selected_index -= 1;
            self.viewport.ensure_visible(self.selected_index);
            // Update input with selected value
            if let Some(selected) = self.suggestions.get(self.selected_index) {
                self.input.set_text(selected.clone());
            }
        }
    }

    /// Move selection down in the suggestions list.
    /// Updates input text to show selected value immediately.
    pub fn select_down(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }

        if self.selected_index < self.suggestions.len().saturating_sub(1) {
            self.selected_index += 1;
            self.viewport.ensure_visible(self.selected_index);
            // Update input with selected value
            if let Some(selected) = self.suggestions.get(self.selected_index) {
                self.input.set_text(selected.clone());
            }
        }
    }

    // === Keyboard handling ===

    /// Handle a key event.
    ///
    /// Returns `SuggestionAction` indicating what happened:
    /// - `Handled`: Key was consumed (navigation, toggle)
    /// - `Confirmed`: Enter was pressed while expanded
    /// - `Cancelled`: Escape was pressed while expanded (rolled back)
    /// - `TextModified`: Text input was modified
    /// - `NotHandled`: Key should be processed by caller
    pub fn handle_key(&mut self, key: KeyEvent) -> SuggestionAction {
        match key.code {
            KeyCode::Tab => {
                self.toggle();
                SuggestionAction::Handled
            }
            KeyCode::Esc if self.state == DropdownState::Expanded => {
                self.rollback();
                SuggestionAction::Cancelled
            }
            KeyCode::Up if self.state == DropdownState::Expanded => {
                self.select_up();
                SuggestionAction::Handled
            }
            KeyCode::Down if self.state == DropdownState::Expanded => {
                self.select_down();
                SuggestionAction::Handled
            }
            KeyCode::Enter if self.state == DropdownState::Expanded => {
                // Just collapse, keeping whatever text is in the input.
                // Up/Down navigation already updates input text to the selected value,
                // so if the user edited the text manually, their edits are preserved.
                self.collapse();
                SuggestionAction::Confirmed
            }
            _ => SuggestionAction::NotHandled,
        }
    }

    // === Suggestions management ===

    /// Update the suggestions list.
    pub fn set_suggestions(&mut self, suggestions: Vec<String>) {
        let total = suggestions.len();
        self.suggestions = suggestions;
        self.selected_index = 0;
        self.viewport.set_total_items(total);
        self.viewport.ensure_visible(0);
    }

    /// Set input text directly (used when restoring state).
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.input.set_text(text);
    }
}

impl Default for SuggestionInput {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let suggestions = vec!["foo".to_string(), "bar".to_string()];
        let input = SuggestionInput::new(suggestions.clone());
        assert_eq!(input.text(), "");
        assert_eq!(input.suggestions(), &suggestions);
        assert_eq!(input.state(), DropdownState::Collapsed);
        assert_eq!(input.selected_index(), 0);
    }

    #[test]
    fn test_with_text() {
        let suggestions = vec!["foo".to_string(), "bar".to_string()];
        let input = SuggestionInput::with_text("hello", suggestions);
        assert_eq!(input.text(), "hello");
        assert!(!input.is_expanded());
    }

    #[test]
    fn test_expand_collapse() {
        let suggestions = vec!["foo".to_string(), "bar".to_string()];
        let mut input = SuggestionInput::with_text("hello", suggestions);

        input.expand();
        assert!(input.is_expanded());

        input.collapse();
        assert!(!input.is_expanded());
    }

    #[test]
    fn test_toggle() {
        let suggestions = vec!["foo".to_string()];
        let mut input = SuggestionInput::new(suggestions);

        input.toggle();
        assert!(input.is_expanded());

        input.toggle();
        assert!(!input.is_expanded());
    }

    #[test]
    fn test_rollback() {
        let suggestions = vec!["foo".to_string(), "bar".to_string()];
        let mut input = SuggestionInput::with_text("original", suggestions);

        input.expand();
        input.input_mut().set_text("modified");
        assert_eq!(input.text(), "modified");

        input.rollback();
        assert_eq!(input.text(), "original");
        assert!(!input.is_expanded());
    }

    #[test]
    fn test_navigation() {
        let suggestions = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut input = SuggestionInput::new(suggestions);

        input.expand();
        assert_eq!(input.selected_index(), 0);
        assert_eq!(input.text(), ""); // Initially empty

        input.select_down();
        assert_eq!(input.selected_index(), 1);
        assert_eq!(input.text(), "b");

        input.select_down();
        assert_eq!(input.selected_index(), 2);
        assert_eq!(input.text(), "c");

        // Should not go past end
        input.select_down();
        assert_eq!(input.selected_index(), 2);

        input.select_up();
        assert_eq!(input.selected_index(), 1);
        assert_eq!(input.text(), "b");
    }

    #[test]
    fn test_confirm() {
        let suggestions = vec!["foo".to_string(), "bar".to_string()];
        let mut input = SuggestionInput::new(suggestions);

        input.expand();
        input.select_down();
        assert_eq!(input.text(), "bar");

        input.confirm();
        assert_eq!(input.text(), "bar");
        assert!(!input.is_expanded());
    }

    #[test]
    fn test_handle_key_tab() {
        let suggestions = vec!["foo".to_string()];
        let mut input = SuggestionInput::new(suggestions);

        let action = input.handle_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(action, SuggestionAction::Handled);
        assert!(input.is_expanded());

        let action = input.handle_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(action, SuggestionAction::Handled);
        assert!(!input.is_expanded());
    }

    #[test]
    fn test_handle_key_esc_expanded() {
        let suggestions = vec!["foo".to_string()];
        let mut input = SuggestionInput::with_text("original", suggestions);

        input.expand();
        input.input_mut().set_text("modified");

        let action = input.handle_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(action, SuggestionAction::Cancelled);
        assert_eq!(input.text(), "original");
        assert!(!input.is_expanded());
    }

    #[test]
    fn test_handle_key_esc_collapsed() {
        let suggestions = vec!["foo".to_string()];
        let mut input = SuggestionInput::new(suggestions);

        let action = input.handle_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(action, SuggestionAction::NotHandled);
    }

    #[test]
    fn test_handle_key_enter_expanded() {
        let suggestions = vec!["foo".to_string(), "bar".to_string()];
        let mut input = SuggestionInput::new(suggestions);

        input.expand();
        input.select_down();

        let action = input.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(action, SuggestionAction::Confirmed);
        assert_eq!(input.text(), "bar");
        assert!(!input.is_expanded());
    }

    #[test]
    fn test_handle_key_enter_expanded_after_edit() {
        let suggestions = vec!["foo".to_string(), "bar".to_string()];
        let mut input = SuggestionInput::new(suggestions);

        input.expand();
        input.select_down(); // text becomes "bar"
        input.input_mut().set_text("bar_edited".to_string()); // user edits manually

        let action = input.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(action, SuggestionAction::Confirmed);
        // Should preserve the manually edited text, not replace with suggestion
        assert_eq!(input.text(), "bar_edited");
        assert!(!input.is_expanded());
    }

    #[test]
    fn test_handle_key_enter_collapsed() {
        let suggestions = vec!["foo".to_string()];
        let mut input = SuggestionInput::new(suggestions);

        let action = input.handle_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(action, SuggestionAction::NotHandled);
    }

    #[test]
    fn test_set_suggestions() {
        let mut input = SuggestionInput::new(vec!["a".to_string()]);
        input.expand();
        input.select_down(); // would go to index 0 (no effect since only 1 item)

        input.set_suggestions(vec!["x".to_string(), "y".to_string(), "z".to_string()]);
        assert_eq!(input.suggestions().len(), 3);
        assert_eq!(input.selected_index(), 0);
    }

    #[test]
    fn test_expand_with_matching_text() {
        let suggestions = vec!["foo".to_string(), "bar".to_string(), "baz".to_string()];
        let mut input = SuggestionInput::with_text("bar", suggestions);

        input.expand();
        // Should position cursor on "bar" which is at index 1
        assert_eq!(input.selected_index(), 1);
        assert!(input.is_expanded());
    }

    #[test]
    fn test_expand_with_non_matching_text() {
        let suggestions = vec!["foo".to_string(), "bar".to_string()];
        let mut input = SuggestionInput::with_text("unknown", suggestions);

        input.expand();
        // Should default to index 0 when no match
        assert_eq!(input.selected_index(), 0);
    }

    #[test]
    fn test_select_and_confirm() {
        let suggestions = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut input = SuggestionInput::new(suggestions);

        input.expand();
        assert!(input.select_and_confirm(2));
        assert_eq!(input.text(), "c");
        assert!(!input.is_expanded());
    }

    #[test]
    fn test_select_and_confirm_invalid_index() {
        let suggestions = vec!["a".to_string(), "b".to_string()];
        let mut input = SuggestionInput::new(suggestions);

        input.expand();
        assert!(!input.select_and_confirm(10)); // Invalid index
        assert!(input.is_expanded()); // Should remain expanded
    }
}
