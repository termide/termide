//! Editor panel module.
//!
//! This module contains the text editor implementation and its supporting modules.

pub mod config;
mod core;
pub mod git;
pub mod word_wrap;

// Re-export everything from core and config
pub use config::*;
pub use core::*;
