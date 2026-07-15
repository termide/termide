//! Panel layout management for termide.
//!
//! This crate provides panel layout management with accordion support:
//! - `PanelGroup` - vertical stack of panels with expandable accordion
//! - `LayoutManager` - horizontal arrangement of panel groups

pub mod geometry;
pub mod layout_manager;
pub mod panel_group;

mod group_ops;
mod widths;

pub use geometry::{
    calculate_panel_rects, classify_panel_drag, compute_drop_target, compute_vertical_constraints,
    group_spans_from_rects, PanelDragIntent, PanelDropTarget,
};
pub use layout_manager::LayoutManager;
pub use panel_group::{PanelGroup, MIN_PANEL_HEIGHT};
