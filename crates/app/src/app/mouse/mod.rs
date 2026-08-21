//! Submodules for mouse-event helpers.
//!
//! The main dispatcher stays in `mouse_handler.rs`; this directory contains
//! specialised helpers (resource-indicator builders, submenu click handlers,
//! divider drag, scrollbar thumb drag) that each keep their own `impl App`
//! block.

mod drag;
mod indicators;
mod layout;
mod scrollbar;
pub(in crate::app) mod submenu;
