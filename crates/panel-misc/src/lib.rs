//! Miscellaneous panels for termide.
//!
//! This crate contains simple utility panels: help screen, journal, and references.

pub mod help;
pub mod journal;
pub mod references;

pub use help::HelpPanel;
pub use journal::JournalPanel;
pub use references::ReferencesPanel;
