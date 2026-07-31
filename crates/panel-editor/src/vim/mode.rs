//! Vim mode enum and related types.

use std::fmt;

/// Vim editing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VimMode {
    /// Normal mode for navigation and commands.
    #[default]
    Normal,
    /// Insert mode for text input (pass-through to standard editor).
    Insert,
    /// Visual mode for character-wise selection.
    Visual,
    /// Visual Line mode for line-wise selection.
    VisualLine,
}

impl VimMode {
    /// Get display string for status bar.
    pub fn display(&self) -> &'static str {
        match self {
            VimMode::Normal => "NORMAL",
            VimMode::Insert => "INSERT",
            VimMode::Visual => "VISUAL",
            VimMode::VisualLine => "V-LINE",
        }
    }

    /// Check if mode allows text insertion.
    pub fn is_insert(&self) -> bool {
        matches!(self, VimMode::Insert)
    }
}

impl fmt::Display for VimMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display())
    }
}
