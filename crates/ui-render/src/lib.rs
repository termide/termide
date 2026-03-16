//! UI rendering components for termide.
//!
//! Provides reusable UI widgets and rendering utilities.

pub mod dropdown;
pub mod inline_selector;
pub mod language_dropdown;
pub mod menu;
pub mod panel_rendering;
pub mod status_bar;
pub mod theme_dropdown;

pub use dropdown::{
    get_bookmarks_group_items, get_bookmarks_item_count, get_bookmarks_items, get_options_items,
    get_scripts_group_items, get_scripts_items, get_sessions_items, get_shell_items,
    get_tools_items, Dropdown, DropdownItem, BOOKMARK_ADD_CURRENT, OPTIONS_SUBMENU_ITEM_COUNT,
    SCRIPT_ADD_NEW, SESSIONS_SUBMENU_ITEM_COUNT, TOOLS_SUBMENU_ITEM_COUNT,
};
pub use inline_selector::InlineSelector;
pub use language_dropdown::{find_current_language_index, LanguageDropdown};
pub use menu::{
    get_menu_item_x_position, get_menu_items, get_resource_indicator_ranges, render_menu,
    resource_color, MenuLayout, MenuRenderParams, BOOKMARKS_MENU_INDEX, MENU_ITEM_COUNT,
    OPTIONS_MENU_INDEX, SCRIPTS_MENU_INDEX, SESSIONS_MENU_INDEX, WINDOWS_MENU_INDEX,
};
pub use panel_rendering::{
    render_collapsed_panel, render_dividers, render_expanded_panel, ExpandedPanelParams,
};
pub use status_bar::{BackgroundOpsSummary, StatusBar, StatusBarParams};
pub use theme_dropdown::ThemeDropdown;
