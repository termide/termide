//! Application orchestrator for termide.
//!
//! This crate ties the panels, modals, and layout together and provides:
//! - `App` struct — the main application and event loop
//! - `AppState` — global application state
//!
//! The concrete orchestration logic lives in the `app` module; shared
//! contracts (`StateManager`, `ModalManager`, `PanelProvider`, …) come from
//! the `app-core` crate, re-exported below.

// Internal modules
pub mod app;
pub mod layout_session;
pub mod panel_ext;
pub mod state;
mod state_operations;
mod state_types;

// Re-export main types for convenience
pub use app::App;
pub use layout_session::LayoutManagerSession;
#[allow(deprecated)]
pub use panel_ext::PanelExt;
pub use state::AppState;

// Note: anyhow::Result is available through re-exports if needed

// Re-export the app-core crate and its commonly used types.
pub use termide_app_core;
pub use termide_app_core::{
    AppCommand, Direction, LayoutController, Message, ModalManager, PanelProvider, PanelType,
    StateManager,
};
