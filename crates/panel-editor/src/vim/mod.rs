//! Vim mode implementation for the editor.
//!
//! This module provides configurable Vim-style keybindings including:
//! - Normal mode for navigation and commands
//! - Insert mode for text input
//! - Visual mode for text selection
//! - Visual Line mode for line-wise selection
//!
//! Vim mode is disabled by default and can be enabled via config.

pub mod key_handler;
mod mode;
pub mod motions;
mod normal_mode;
pub mod operators;
mod state;
mod visual_mode;

pub use key_handler::{handle_vim_key, InsertPosition, VimKeyResult};
pub use mode::VimMode;
pub use motions::VimMotion;
pub use operators::VimOperator;
pub use state::VimState;

/// Direction for panel navigation (Ctrl+w h/j/k/l)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelDirection {
    Left,
    Down,
    Up,
    Right,
}
