//! Input-related state for the editor.

use crate::click_tracker::ClickTracker;

/// Input-related state for the editor.
#[derive(Default)]
pub(crate) struct InputState {
    /// Mouse click tracking for double-click detection.
    pub click_tracker: ClickTracker,
    /// Preferred column for vertical navigation (maintains column across lines).
    pub preferred_column: Option<usize>,
    /// Left mouse button is currently held down during selection.
    pub selection_drag_active: bool,
    /// Last known mouse position (column, row) in screen coordinates.
    pub last_mouse_position: Option<(u16, u16)>,
    /// Content area bounds for auto-scroll checks: (x, y, width, height).
    pub content_bounds: Option<(u16, u16, u16, u16)>,
}

impl InputState {
    /// Create new InputState.
    pub fn new() -> Self {
        Self::default()
    }
}
