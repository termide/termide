//! Miscellaneous panels for termide.
//!
//! This crate contains simple utility panels: welcome screen, log viewer, and debug panel.

pub mod debug;
pub mod log_viewer;
pub mod welcome;

pub use debug::DebugPanel;
pub use log_viewer::LogViewerPanel;
pub use welcome::WelcomePanel;
