//! Text editor panel for termide.
//!
//! Provides a full-featured text editor with syntax highlighting,
//! search/replace, git integration, and more.

mod auto_pairs;
mod click_tracker;
pub mod clipboard;
mod code_action_popup;
mod completion_popup;
pub mod config;
pub mod constants;
mod core;
pub mod cursor;
mod diagram;
mod file_io;
pub mod git;
mod hover_popup;
pub mod keyboard;
pub mod remote;
pub mod rendering;
pub mod search;
pub mod selection;
pub mod state;
mod syntax_picker;
pub mod text_editing;
pub mod vim;
pub mod word_boundary;
pub mod word_wrap;

// Re-export main types
pub use config::{EditorConfig, EditorInfo};
pub use core::{Editor, LspManager};
pub use remote::PendingRemoteOpen;
pub use state::FileState;
