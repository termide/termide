//! Rendering utilities for the editor.
//!
//! This module provides rendering-related functions and constants.
//! The main render logic remains in core.rs for now (Phase 4 extraction in progress).

pub mod context;
pub mod cursor_renderer;
pub mod deletion_markers;
pub mod highlight_renderer;
pub mod line_rendering;

/// Width of the line number column (including git markers).
///
/// Format: "  123  " (2 spaces + 3 digits + 2 git markers)
pub const LINE_NUMBER_WIDTH: usize = 6;

/// Calculate content area dimensions.
///
/// Returns (content_width, content_height) accounting for line numbers.
pub fn calculate_content_dimensions(area_width: u16, area_height: u16) -> (usize, usize) {
    let content_width = (area_width as usize).saturating_sub(LINE_NUMBER_WIDTH);
    let content_height = area_height as usize;
    (content_width, content_height)
}

// Note: should_use_smart_wrap logic remains in Editor for now
// as it requires access to editor state (syntax highlighting, file size, etc.)
