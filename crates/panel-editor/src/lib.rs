//! Text editor panel for termide.
//!
//! Provides a full-featured text editor with syntax highlighting,
//! search/replace, git integration, and more.

mod click_tracker;
pub mod clipboard;
pub mod config;
pub mod constants;
mod core;
pub mod cursor;
mod file_io;
pub mod git;
pub mod keyboard;
pub mod rendering;
pub mod search;
pub mod selection;
mod state;
pub mod text_editing;
pub mod word_wrap;

// Re-export main types
pub use config::{EditorConfig, EditorInfo};
pub use core::Editor;
