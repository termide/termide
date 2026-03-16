//! UI component state types.

/// State for a submenu (open/closed + selected item index).
///
/// This struct provides a consistent pattern for submenu state management.
/// Instead of having separate `*_open: bool` and `selected_*_item: usize` fields,
/// use this struct to group related state together.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SubmenuState {
    /// Whether the submenu is open
    pub open: bool,
    /// Selected item index within the submenu
    pub selected: usize,
}

impl SubmenuState {
    /// Create a new closed submenu state
    pub const fn new() -> Self {
        Self {
            open: false,
            selected: 0,
        }
    }

    /// Open the submenu and reset selection to first item
    pub fn open(&mut self) {
        self.open = true;
        self.selected = 0;
    }

    /// Open the submenu with a specific initial selection
    pub fn open_at(&mut self, index: usize) {
        self.open = true;
        self.selected = index;
    }

    /// Close the submenu and reset selection
    pub fn close(&mut self) {
        self.open = false;
        self.selected = 0;
    }

    /// Move selection up (wrapping to last item if at first)
    pub fn select_prev(&mut self, item_count: usize) {
        if item_count == 0 {
            return;
        }
        if self.selected > 0 {
            self.selected -= 1;
        } else {
            self.selected = item_count.saturating_sub(1);
        }
    }

    /// Move selection down (wrapping to first item if at last)
    pub fn select_next(&mut self, item_count: usize) {
        if item_count == 0 {
            return;
        }
        self.selected = (self.selected + 1) % item_count;
    }
}

/// State for divider drag resize operation
#[derive(Debug, Default, Clone)]
pub struct DragState {
    /// Index of divider being dragged (between groups idx and idx+1)
    pub active_divider: Option<usize>,
    /// Initial X position when drag started
    pub start_x: u16,
    /// Initial widths of left and right groups
    pub start_widths: (u16, u16),
    /// Last column applied (skip redraw if unchanged)
    pub last_applied_x: Option<u16>,
}

impl DragState {
    /// Start dragging a divider
    pub fn start(&mut self, divider_idx: usize, x: u16, left_width: u16, right_width: u16) {
        self.active_divider = Some(divider_idx);
        self.start_x = x;
        self.start_widths = (left_width, right_width);
    }

    /// End dragging
    pub fn end(&mut self) {
        self.active_divider = None;
        self.last_applied_x = None;
    }

    /// Check if currently dragging
    pub fn is_dragging(&self) -> bool {
        self.active_divider.is_some()
    }
}

/// UI components state
#[derive(Debug, Default)]
pub struct UiState {
    /// Is menu open
    pub menu_open: bool,
    /// Selected menu item (None if menu closed)
    pub selected_menu_item: Option<usize>,
    /// Selected item in dropdown list
    pub selected_dropdown_item: usize,
    /// Status line message (for displaying errors and notifications)
    pub status_message: Option<(String, bool)>, // (message, is_error)
    /// Options submenu state (e.g., Preferences dropdown)
    pub options_submenu: SubmenuState,
    /// Nested submenu state (e.g., Themes list inside Options)
    pub nested_submenu: SubmenuState,
    /// Original theme name before preview (for restoring on cancel)
    pub theme_preview_original: Option<String>,
    /// Original language code before preview (for restoring on cancel)
    pub language_preview_original: Option<String>,
    /// Divider drag state for panel resize
    pub drag: DragState,
    /// Sessions submenu state
    pub sessions_submenu: SubmenuState,
    /// Tools submenu state
    pub tools_submenu: SubmenuState,
    /// Tools nested submenu state (shell picker inside Terminal)
    pub tools_nested: SubmenuState,
    /// Scripts submenu state
    pub scripts_submenu: SubmenuState,
    /// Scripts nested submenu state (for subdirectory groups)
    pub scripts_nested: SubmenuState,
    /// Current script group name (for nested submenu)
    pub current_scripts_group: Option<String>,
    /// Bookmarks submenu state
    pub bookmarks_submenu: SubmenuState,
    /// Bookmarks nested submenu state (for groups)
    pub bookmarks_nested: SubmenuState,
    /// Current bookmarks group name (for nested submenu)
    pub current_bookmarks_group: Option<String>,
    /// Is git operation (push/pull) in progress
    pub git_operation_in_progress: bool,
    /// Spinner frame for animated loading indicators
    pub spinner_frame: usize,
}

impl UiState {
    /// Close all main-level submenus (sessions, tools, options, scripts, bookmarks)
    /// and their nested submenus. Use before opening a specific submenu.
    pub fn close_all_submenus(&mut self) {
        self.sessions_submenu.close();
        self.tools_submenu.close();
        self.tools_nested.close();
        self.options_submenu.close();
        self.nested_submenu.close();
        self.scripts_submenu.close();
        self.scripts_nested.close();
        self.current_scripts_group = None;
        self.bookmarks_submenu.close();
        self.bookmarks_nested.close();
        self.current_bookmarks_group = None;
    }
}

/// Terminal state (dimensions)
#[derive(Debug, Clone, Copy)]
pub struct TerminalState {
    /// Terminal width
    pub width: u16,
    /// Terminal height
    pub height: u16,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            width: 80,
            height: 24,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // SubmenuState tests
    // =========================================================================

    #[test]
    fn test_submenu_state_new() {
        let state = SubmenuState::new();
        assert!(!state.open);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_submenu_state_open() {
        let mut state = SubmenuState::new();
        state.selected = 5; // Set some value
        state.open();
        assert!(state.open);
        assert_eq!(state.selected, 0); // Reset to 0
    }

    #[test]
    fn test_submenu_state_open_at() {
        let mut state = SubmenuState::new();
        state.open_at(3);
        assert!(state.open);
        assert_eq!(state.selected, 3);
    }

    #[test]
    fn test_submenu_state_close() {
        let mut state = SubmenuState::new();
        state.open = true;
        state.selected = 5;
        state.close();
        assert!(!state.open);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_submenu_state_select_prev() {
        let mut state = SubmenuState::new();
        state.selected = 2;

        state.select_prev(5);
        assert_eq!(state.selected, 1);

        state.select_prev(5);
        assert_eq!(state.selected, 0);

        // Wrap to last
        state.select_prev(5);
        assert_eq!(state.selected, 4);
    }

    #[test]
    fn test_submenu_state_select_next() {
        let mut state = SubmenuState::new();
        state.selected = 3;

        state.select_next(5);
        assert_eq!(state.selected, 4);

        // Wrap to first
        state.select_next(5);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_submenu_state_empty_list() {
        let mut state = SubmenuState::new();
        state.selected = 0;

        // Should not panic with empty list
        state.select_prev(0);
        assert_eq!(state.selected, 0);

        state.select_next(0);
        assert_eq!(state.selected, 0);
    }

    // =========================================================================
    // DragState tests
    // =========================================================================

    #[test]
    fn test_drag_state_lifecycle() {
        let mut drag = DragState::default();
        assert!(!drag.is_dragging());

        drag.start(1, 100, 50, 50);
        assert!(drag.is_dragging());
        assert_eq!(drag.active_divider, Some(1));
        assert_eq!(drag.start_x, 100);
        assert_eq!(drag.start_widths, (50, 50));

        drag.end();
        assert!(!drag.is_dragging());
    }

    // =========================================================================
    // UiState tests
    // =========================================================================

    #[test]
    fn test_ui_state_close_all_submenus() {
        let mut ui = UiState::default();
        ui.sessions_submenu.open();
        ui.tools_submenu.open();
        ui.tools_nested.open();
        ui.options_submenu.open();
        ui.scripts_submenu.open();
        ui.bookmarks_submenu.open();
        ui.current_scripts_group = Some("test".to_string());
        ui.current_bookmarks_group = Some("test".to_string());

        ui.close_all_submenus();

        assert!(!ui.sessions_submenu.open);
        assert!(!ui.tools_submenu.open);
        assert!(!ui.tools_nested.open);
        assert!(!ui.options_submenu.open);
        assert!(!ui.scripts_submenu.open);
        assert!(!ui.bookmarks_submenu.open);
        assert!(ui.current_scripts_group.is_none());
        assert!(ui.current_bookmarks_group.is_none());
    }

    // =========================================================================
    // TerminalState tests
    // =========================================================================

    #[test]
    fn test_terminal_state_default() {
        let ts = TerminalState::default();
        assert_eq!(ts.width, 80);
        assert_eq!(ts.height, 24);
    }
}
