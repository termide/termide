//! Editor state sub-modules.
//!
//! Groups related Editor fields into focused structs for better organization
//! and cache locality.

// Allow unused methods - they are helpers for future refactoring phases
#![allow(dead_code)]

mod file_state;
mod git_integration;
mod input_state;
mod rendering_cache;
mod search_controller;

pub(crate) use file_state::FileState;
pub(crate) use git_integration::GitIntegration;
pub(crate) use input_state::InputState;
pub(crate) use rendering_cache::RenderingCache;
pub(crate) use search_controller::SearchController;
