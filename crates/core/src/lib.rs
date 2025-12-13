//! Core types and traits for termide panels.
//!
//! This crate provides the foundational abstractions for building panels
//! in termide without coupling them to the application state.

pub mod command;
pub mod event;
pub mod panel;

pub use command::{CommandResult, PanelCommand};
pub use event::{
    ConfirmAction, ConflictResolution, Event, EventHandler, InputAction, PanelEvent, SelectAction,
    SplitDirection,
};
pub use panel::{Panel, PanelConfig, RenderContext, SessionPanel, ThemeColors};

// Re-export theme and config for convenience
pub use termide_config::Config;
pub use termide_theme::Theme;
