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

/// State for panel action context menu (opened from [≡] button)
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PanelActionMenuState {
    /// Whether the menu is open
    pub open: bool,
    /// Selected item index
    pub selected: usize,
    /// Group index of the panel
    pub group_idx: usize,
    /// Panel index within the group
    pub panel_idx: usize,
    /// Screen X of the [≡] button
    pub anchor_x: u16,
    /// Screen Y of the panel header row
    pub anchor_y: u16,
}

impl PanelActionMenuState {
    /// Open the menu at the given button position
    pub fn open(&mut self, group_idx: usize, panel_idx: usize, anchor_x: u16, anchor_y: u16) {
        self.open = true;
        self.selected = 0;
        self.group_idx = group_idx;
        self.panel_idx = panel_idx;
        self.anchor_x = anchor_x;
        self.anchor_y = anchor_y;
    }

    /// Close the menu
    pub fn close(&mut self) {
        self.open = false;
    }
}

/// State for the per-operation popup menu opened from the type icon
/// on an operation card in the Operations panel (Pause/Resume/Cancel).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OperationActionMenuState {
    pub open: bool,
    pub selected: usize,
    /// Operation id (uuid u128) so we can target the right operation
    /// even if the panel reorders.
    /// Raw operation id (OperationId.0); kept as u64 here so this
    /// crate doesn't have to depend on termide-file-ops.
    pub op_id: u64,
    /// Screen X of the clicked icon
    pub anchor_x: u16,
    /// Screen Y of the clicked icon
    pub anchor_y: u16,
    /// Cached "is paused" snapshot — controls the label of the first
    /// item (Pause vs Resume).
    pub is_paused: bool,
    /// Cached "is a command/script run" snapshot. Such operations can't be
    /// paused (no way to suspend an external process), so the menu omits
    /// Pause/Resume and offers only Cancel for them.
    pub is_command: bool,
}

impl OperationActionMenuState {
    pub fn open(
        &mut self,
        op_id: u64,
        anchor_x: u16,
        anchor_y: u16,
        is_paused: bool,
        is_command: bool,
    ) {
        self.open = true;
        self.selected = 0;
        self.op_id = op_id;
        self.anchor_x = anchor_x;
        self.anchor_y = anchor_y;
        self.is_paused = is_paused;
        self.is_command = is_command;
    }

    pub fn close(&mut self) {
        self.open = false;
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

/// State for vertical-divider drag (resize between two panels of the
/// same group).
///
/// `active = (group_idx, upper_panel_idx)` identifies the divider
/// between `panels[upper_panel_idx]` and `panels[upper_panel_idx + 1]`.
/// `start_y` is the row where mouse-down landed (= upper panel's
/// bottom-border row at drag start). `last_applied_y` tracks the
/// current cursor row for the ghost preview; the actual panel-height
/// delta is applied on drag-end.
#[derive(Debug, Default, Clone, Copy)]
pub struct VerticalDividerDragState {
    pub active: Option<(usize, usize)>,
    pub start_y: u16,
    pub last_applied_y: Option<u16>,
}

impl VerticalDividerDragState {
    pub fn start(&mut self, group_idx: usize, upper_panel_idx: usize, y: u16) {
        self.active = Some((group_idx, upper_panel_idx));
        self.start_y = y;
        self.last_applied_y = Some(y);
    }

    pub fn end(&mut self) {
        self.active = None;
        self.last_applied_y = None;
    }

    pub fn is_dragging(&self) -> bool {
        self.active.is_some()
    }
}

/// Source panel being dragged by its top border.
#[derive(Debug, Clone, Copy)]
pub struct PanelDragSource {
    pub group_idx: usize,
    pub panel_idx: usize,
    pub start_x: u16,
    pub start_y: u16,
}

/// State for panel drag-and-drop (grabbing a panel by its top border).
///
/// A drag is first tracked in a "pending" state (source is set, `active`
/// is false): this is the grace period between Down and the first Drag
/// event where we don't yet know whether the user intends to click or
/// drag. After the cursor moves past a threshold, `active` becomes true
/// and the overlay is rendered.
#[derive(Debug, Default, Clone, Copy)]
pub struct PanelDragState {
    pub source: Option<PanelDragSource>,
    pub active: bool,
    pub cursor_x: u16,
    pub cursor_y: u16,
}

impl PanelDragState {
    /// Movement in cells required to promote a pending drag to an active one.
    pub const THRESHOLD: u16 = 3;

    /// Record a potential drag start. Until the cursor moves past the
    /// threshold, this is inert — clicks still fire normally.
    pub fn begin_pending(&mut self, group_idx: usize, panel_idx: usize, x: u16, y: u16) {
        self.source = Some(PanelDragSource {
            group_idx,
            panel_idx,
            start_x: x,
            start_y: y,
        });
        self.active = false;
        self.cursor_x = x;
        self.cursor_y = y;
    }

    /// Update cursor on Drag event. Returns true if the drag just became
    /// active (threshold crossed on this call), so the caller can trigger
    /// an initial redraw.
    pub fn update_cursor(&mut self, x: u16, y: u16) -> bool {
        self.cursor_x = x;
        self.cursor_y = y;
        if self.active {
            return false;
        }
        let Some(src) = self.source else {
            return false;
        };
        let dx = x.abs_diff(src.start_x);
        let dy = y.abs_diff(src.start_y);
        if dx + dy >= Self::THRESHOLD {
            self.active = true;
            true
        } else {
            false
        }
    }

    /// Cancel any in-progress drag.
    pub fn cancel(&mut self) {
        self.source = None;
        self.active = false;
    }

    /// Whether there is a pending-or-active drag.
    pub fn is_pending_or_active(&self) -> bool {
        self.source.is_some()
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
    /// Divider drag state for panel-group resize (horizontal).
    pub drag: DragState,
    /// Divider drag state for in-group panel resize (vertical).
    pub vdrag: VerticalDividerDragState,
    /// Projects submenu state
    pub projects_submenu: SubmenuState,
    /// Tools submenu state
    pub tools_submenu: SubmenuState,
    /// Tools nested submenu state (shell picker inside Terminal)
    pub tools_nested: SubmenuState,
    /// Commands submenu state
    pub commands_submenu: SubmenuState,
    /// Commands nested submenu state (for groups)
    pub commands_nested: SubmenuState,
    /// Current command group name (for nested submenu)
    pub current_commands_group: Option<String>,
    /// AI submenu state (Agents / Sessions / Skills / Prompts sections)
    pub ai_submenu: SubmenuState,
    /// AI nested submenu state (the selected section's item list)
    pub ai_nested: SubmenuState,
    /// The AI section whose item list is open
    pub current_ai_section: Option<crate::AiSection>,
    /// Bookmarks submenu state
    pub bookmarks_submenu: SubmenuState,
    /// Bookmarks nested submenu state (for groups)
    pub bookmarks_nested: SubmenuState,
    /// Current bookmarks group name (for nested submenu)
    pub current_bookmarks_group: Option<String>,
    /// Whether the current bookmarks group is from project-local .termide/
    pub current_bookmarks_group_is_project: bool,
    /// Stash dropdown state (opened from git status panel button)
    pub stash_submenu: SubmenuState,
    /// Screen position of the stash button (for dropdown anchoring)
    pub stash_button_area: Option<ratatui::layout::Rect>,
    /// Is git operation (push/pull) in progress
    pub git_operation_in_progress: bool,
    /// Spinner frame for animated loading indicators
    pub spinner_frame: usize,
    /// Panel action context menu state
    pub panel_action_menu: PanelActionMenuState,
    /// Per-operation popup menu (Pause/Resume/Cancel) on Operations panel
    pub operation_action_menu: OperationActionMenuState,
    /// Panel drag-and-drop state (grab a panel by its top border)
    pub panel_drag: PanelDragState,
}

impl UiState {
    /// Close all main-level submenus (projects, tools, options, commands, bookmarks)
    /// and their nested submenus. Use before opening a specific submenu.
    pub fn close_all_submenus(&mut self) {
        self.projects_submenu.close();
        self.tools_submenu.close();
        self.tools_nested.close();
        self.options_submenu.close();
        self.nested_submenu.close();
        self.commands_submenu.close();
        self.commands_nested.close();
        self.current_commands_group = None;
        self.ai_submenu.close();
        self.ai_nested.close();
        self.current_ai_section = None;
        self.bookmarks_submenu.close();
        self.bookmarks_nested.close();
        self.current_bookmarks_group = None;
        self.current_bookmarks_group_is_project = false;
        self.stash_submenu.close();
        self.panel_action_menu.close();
        self.operation_action_menu.close();
        self.panel_drag.cancel();
    }
}

/// Terminal state (dimensions)
#[derive(Debug, Clone, Copy)]
pub struct TerminalState {
    /// Terminal width
    pub width: u16,
    /// Terminal height
    pub height: u16,
    /// The terminal window has keyboard focus, as its focus reports say.
    /// Starts true: a terminal that sends no reports is taken as focused.
    pub focused: bool,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            width: 80,
            height: 24,
            focused: true,
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
        ui.projects_submenu.open();
        ui.tools_submenu.open();
        ui.tools_nested.open();
        ui.options_submenu.open();
        ui.commands_submenu.open();
        ui.bookmarks_submenu.open();
        ui.current_commands_group = Some("test".to_string());
        ui.current_bookmarks_group = Some("test".to_string());

        ui.close_all_submenus();

        assert!(!ui.projects_submenu.open);
        assert!(!ui.tools_submenu.open);
        assert!(!ui.tools_nested.open);
        assert!(!ui.options_submenu.open);
        assert!(!ui.commands_submenu.open);
        assert!(!ui.bookmarks_submenu.open);
        assert!(ui.current_commands_group.is_none());
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
