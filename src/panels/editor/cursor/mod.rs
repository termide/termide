//! Cursor movement utilities for the editor.
//!
//! This module provides cursor movement operations including:
//! - Physical movement (up/down/left/right)
//! - Jump operations (page up/down, document start/end)
//! - Visual movement (accounting for word wrap)

pub mod jump;
pub mod physical;
pub mod visual;

// Re-export common types if needed
