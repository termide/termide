//! Mouse click tracking for multi-click detection (double = word, triple =
//! line), based on timing and position.

use std::time::Instant;

/// Consecutive-click time threshold in milliseconds.
const MULTI_CLICK_THRESHOLD_MS: u128 = 500;

/// Mouse click tracking for double/triple-click detection.
#[derive(Default)]
pub(crate) struct ClickTracker {
    /// Last click time.
    time: Option<Instant>,
    /// Last click position (line, column).
    position: Option<(usize, usize)>,
    /// Consecutive-click count at `position` (1, 2, or 3; cycles).
    count: u8,
    /// Skip next MouseUp event (after a word/line selection).
    pub(crate) skip_next_up: bool,
}

impl ClickTracker {
    /// Register a click and return the consecutive-click count at this
    /// position: 1 (single), 2 (double), 3 (triple), cycling back to 1 on the
    /// fourth. A click at a different position or after the timeout resets to 1.
    pub(crate) fn click(&mut self, line: usize, col: usize) -> u8 {
        let now = Instant::now();
        let consecutive = match (self.time, self.position) {
            (Some(t), Some(pos))
                if pos == (line, col)
                    && now.duration_since(t).as_millis() < MULTI_CLICK_THRESHOLD_MS =>
            {
                (self.count % 3) + 1
            }
            _ => 1,
        };
        self.time = Some(now);
        self.position = Some((line, col));
        self.count = consecutive;
        consecutive
    }

    /// Reset click tracking.
    pub(crate) fn reset(&mut self) {
        self.time = None;
        self.position = None;
        self.count = 0;
    }
}
