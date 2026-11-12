//! Vim state management.

use termide_buffer::Cursor;

use super::mode::VimMode;
use super::operators::VimOperator;

/// Vim state tracking for the editor.
#[derive(Debug, Clone, Default)]
pub struct VimState {
    /// Current Vim mode.
    pub mode: VimMode,
    /// Pending operator waiting for motion (d, y, c).
    pub pending_operator: Option<VimOperator>,
    /// Numeric prefix for repeated commands (5j, 3dd).
    pub count: Option<usize>,
    /// Partial key sequence for multi-key commands (gg, Ctrl+w h).
    pub partial_keys: Vec<char>,
    /// Visual mode selection anchor point.
    pub visual_anchor: Option<Cursor>,
    /// Vim yank register content.
    pub register: Option<String>,
    /// Whether the last yank/delete was line-wise.
    pub register_linewise: bool,
}

impl VimState {
    /// Create new Vim state in normal mode.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset to normal mode, clearing all pending state.
    pub fn reset_to_normal(&mut self) {
        self.mode = VimMode::Normal;
        self.pending_operator = None;
        self.count = None;
        self.partial_keys.clear();
        self.visual_anchor = None;
    }

    /// Enter insert mode.
    pub fn enter_insert(&mut self) {
        self.mode = VimMode::Insert;
        self.clear_pending();
    }

    /// Enter visual mode at current cursor position.
    pub fn enter_visual(&mut self, cursor: Cursor) {
        self.mode = VimMode::Visual;
        self.visual_anchor = Some(cursor);
        self.clear_pending();
    }

    /// Enter visual line mode at current cursor position.
    pub fn enter_visual_line(&mut self, cursor: Cursor) {
        self.mode = VimMode::VisualLine;
        self.visual_anchor = Some(cursor);
        self.clear_pending();
    }

    /// Exit to normal mode.
    pub fn exit_to_normal(&mut self) {
        self.reset_to_normal();
    }

    /// Clear pending operator, count, and partial keys.
    pub fn clear_pending(&mut self) {
        self.pending_operator = None;
        self.count = None;
        self.partial_keys.clear();
    }

    /// Accumulate a digit into the count prefix.
    ///
    /// Returns true if the digit was accumulated, false if it should be treated as a command.
    pub fn accumulate_count(&mut self, digit: char) -> bool {
        if let Some(d) = digit.to_digit(10) {
            // '0' at the start of count is line start command, not count
            if d == 0 && self.count.is_none() {
                return false;
            }
            let current = self.count.unwrap_or(0);
            // Prevent overflow with reasonable limit
            if current > 10000 {
                return true;
            }
            self.count = Some(current * 10 + d as usize);
            true
        } else {
            false
        }
    }

    /// Get the effective count (defaults to 1 if not specified).
    pub fn effective_count(&self) -> usize {
        self.count.unwrap_or(1)
    }

    /// Set pending operator for next motion.
    pub fn set_pending_operator(&mut self, op: VimOperator) {
        self.pending_operator = Some(op);
    }

    /// Take pending operator (consumes it).
    pub fn take_pending_operator(&mut self) -> Option<VimOperator> {
        self.pending_operator.take()
    }

    /// Whether a command is half typed: a count, an operator or a prefix
    /// waits on the keys that complete it.
    pub fn has_pending(&self) -> bool {
        self.count.is_some() || self.pending_operator.is_some() || !self.partial_keys.is_empty()
    }

    /// Add a character to partial key sequence.
    pub fn push_partial_key(&mut self, ch: char) {
        self.partial_keys.push(ch);
    }

    /// Check if partial keys match a sequence.
    pub fn partial_keys_match(&self, expected: &[char]) -> bool {
        self.partial_keys == expected
    }

    /// Store text in the register.
    pub fn yank(&mut self, text: String, linewise: bool) {
        self.register = Some(text);
        self.register_linewise = linewise;
    }

    /// Get register content.
    pub fn get_register(&self) -> Option<&str> {
        self.register.as_deref()
    }

    /// Check if register contains linewise content.
    pub fn is_register_linewise(&self) -> bool {
        self.register_linewise
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_state_is_normal() {
        let state = VimState::new();
        assert_eq!(state.mode, VimMode::Normal);
        assert!(state.pending_operator.is_none());
        assert!(state.count.is_none());
        assert!(state.partial_keys.is_empty());
    }

    #[test]
    fn test_accumulate_count() {
        let mut state = VimState::new();

        // '0' at start is not a count
        assert!(!state.accumulate_count('0'));
        assert!(state.count.is_none());

        // '5' starts a count
        assert!(state.accumulate_count('5'));
        assert_eq!(state.count, Some(5));

        // '0' after digits is added to count
        assert!(state.accumulate_count('0'));
        assert_eq!(state.count, Some(50));

        assert_eq!(state.effective_count(), 50);
    }

    #[test]
    fn test_mode_transitions() {
        let mut state = VimState::new();
        let cursor = Cursor::new();

        // Normal -> Insert
        state.enter_insert();
        assert_eq!(state.mode, VimMode::Insert);

        // Insert -> Normal
        state.exit_to_normal();
        assert_eq!(state.mode, VimMode::Normal);

        // Normal -> Visual
        state.enter_visual(cursor);
        assert_eq!(state.mode, VimMode::Visual);
        assert!(state.visual_anchor.is_some());

        // Visual -> Normal
        state.exit_to_normal();
        assert_eq!(state.mode, VimMode::Normal);
        assert!(state.visual_anchor.is_none());
    }

    #[test]
    fn test_yank_register() {
        let mut state = VimState::new();

        state.yank("hello".to_string(), false);
        assert_eq!(state.get_register(), Some("hello"));
        assert!(!state.is_register_linewise());

        state.yank("line\n".to_string(), true);
        assert_eq!(state.get_register(), Some("line\n"));
        assert!(state.is_register_linewise());
    }
}
