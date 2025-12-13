//! Mouse click tracking for double-click detection.
//!
//! Provides functionality to detect double-clicks based on timing and position.

use std::time::Instant;

/// Double-click detection time threshold in milliseconds.
const DOUBLE_CLICK_THRESHOLD_MS: u128 = 500;

/// Mouse click tracking for double-click detection.
#[derive(Default)]
pub(crate) struct ClickTracker {
    /// Last click time.
    time: Option<Instant>,
    /// Last click position (line, column).
    position: Option<(usize, usize)>,
    /// Skip next MouseUp event (after double-click word selection).
    pub(crate) skip_next_up: bool,
}

impl ClickTracker {
    /// Check if this click is a double-click (same position within threshold).
    pub(crate) fn is_double_click(&self, line: usize, col: usize) -> bool {
        if let (Some(last_time), Some((last_line, last_col))) = (self.time, self.position) {
            let elapsed = Instant::now().duration_since(last_time);
            elapsed.as_millis() < DOUBLE_CLICK_THRESHOLD_MS && last_line == line && last_col == col
        } else {
            false
        }
    }

    /// Record a click at the given position.
    pub(crate) fn record_click(&mut self, line: usize, col: usize) {
        self.time = Some(Instant::now());
        self.position = Some((line, col));
    }

    /// Reset click tracking (e.g., after double-click).
    pub(crate) fn reset(&mut self) {
        self.time = None;
        self.position = None;
    }
}
