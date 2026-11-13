//! Editor state sub-modules.
//!
//! Groups related Editor fields into focused structs for better organization
//! and cache locality.

mod file_state;
mod git_integration;
mod input_state;
mod lsp_state;
pub mod rendering_cache;
mod search_controller;

pub use file_state::FileState;
pub(crate) use git_integration::GitIntegration;
pub(crate) use input_state::InputState;
pub(crate) use lsp_state::LspState;
pub(crate) use rendering_cache::RenderingCache;
pub(crate) use search_controller::SearchController;
