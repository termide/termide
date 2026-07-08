//! UI rendering components for termide.
//!
//! Provides reusable UI widgets and rendering utilities.

pub mod dropdown;
pub mod inline_selector;
pub mod language_dropdown;
pub mod menu;
pub mod panel_rendering;
pub mod simple_dropdown;
pub mod status_bar;
pub mod theme_dropdown;

pub use dropdown::{
    get_bookmarks_group_items, get_bookmarks_item_count, get_bookmarks_items,
    get_commands_group_items, get_commands_items, get_operation_action_menu_items,
    get_options_items, get_panel_action_menu_items, get_sessions_items, get_shell_items,
    get_stash_items, get_tools_items, operation_action_dropdown_position,
    panel_action_dropdown_position, Dropdown, DropdownItem, BOOKMARK_ADD_CURRENT, COMMAND_ADD_NEW,
    COMMAND_MANAGE, OPERATION_ACTION_CANCEL, OPERATION_ACTION_PAUSE, OPERATION_ACTION_RESUME,
    OPTIONS_SUBMENU_HELP, OPTIONS_SUBMENU_ITEM_COUNT, OPTIONS_SUBMENU_LANGUAGE,
    OPTIONS_SUBMENU_PREFERENCES, OPTIONS_SUBMENU_QUIT, OPTIONS_SUBMENU_THEMES, PANEL_ACTION_CLOSE,
    PANEL_ACTION_MOVE_DOWN, PANEL_ACTION_MOVE_LEFT, PANEL_ACTION_MOVE_RIGHT, PANEL_ACTION_MOVE_UP,
    PANEL_ACTION_SPLIT, SESSIONS_SUBMENU_CHANGE_ROOT, SESSIONS_SUBMENU_ITEM_COUNT,
    SESSIONS_SUBMENU_NEW, SESSIONS_SUBMENU_SWITCH, STASH_NEW, TOOLS_SUBMENU_DIAGNOSTICS,
    TOOLS_SUBMENU_EDITOR, TOOLS_SUBMENU_FILES, TOOLS_SUBMENU_GIT_LOG, TOOLS_SUBMENU_GIT_STATUS,
    TOOLS_SUBMENU_ITEM_COUNT, TOOLS_SUBMENU_JOURNAL, TOOLS_SUBMENU_OPEN, TOOLS_SUBMENU_OPERATIONS,
    TOOLS_SUBMENU_OUTLINE, TOOLS_SUBMENU_SEPARATOR, TOOLS_SUBMENU_TERMINAL,
};
pub use inline_selector::InlineSelector;
pub use language_dropdown::{find_current_language_index, LanguageDropdown};
pub use menu::{
    get_menu_item_x_position, get_menu_items, get_resource_indicator_ranges, render_menu,
    resource_color, MenuLayout, MenuRenderParams, BOOKMARKS_MENU_INDEX, COMMANDS_MENU_INDEX,
    INDICATOR_CLOCK_INDEX, INDICATOR_CPU_INDEX, INDICATOR_DISK_INDEX, INDICATOR_NET_INDEX,
    INDICATOR_RAM_INDEX, MENU_INDICATOR_COUNT, MENU_ITEM_COUNT, MENU_TOTAL_COUNT,
    OPTIONS_MENU_INDEX, SESSIONS_MENU_INDEX, WINDOWS_MENU_INDEX,
};
pub use panel_rendering::{
    panel_icon, render_collapsed_panel, render_dividers, render_expanded_panel,
    render_v_divider_ghost, ExpandedPanelParams,
};
pub use simple_dropdown::render_simple_dropdown;
pub use status_bar::{
    segment_hit_areas, BackgroundOpsSummary, SegmentHit, StatusBar, StatusBarParams,
};
pub use theme_dropdown::ThemeDropdown;
