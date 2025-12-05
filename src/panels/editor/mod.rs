//! Editor panel module.
//!
//! This module contains the text editor implementation and its supporting modules.

pub mod clipboard;
pub mod config;
mod core;
pub mod cursor;
pub mod git;
pub mod rendering;
pub mod search;
pub mod selection;
pub mod text_editing;
pub mod word_wrap;

// Re-export everything from core and config
pub use config::*;
pub use core::*;
