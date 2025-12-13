//! UI rendering components for termide.
//!
//! Provides reusable UI widgets and rendering utilities.

pub mod dropdown;
pub mod menu;
pub mod panel_rendering;
pub mod status_bar;

pub use dropdown::{Dropdown, DropdownItem};
pub use menu::{get_menu_items, render_menu, resource_color, MenuRenderParams, MENU_ITEM_COUNT};
pub use panel_rendering::{render_collapsed_panel, render_expanded_panel, ExpandedPanelParams};
pub use status_bar::{StatusBar, StatusBarParams};
