//! File-related state for the editor.

use std::path::Path;
use std::time::SystemTime;

use crate::file_io;

/// State related to the file being edited.
#[derive(Default)]
pub(crate) struct FileState {
    /// File modification time at load/save (for detecting external changes).
    pub mtime: Option<SystemTime>,
    /// Flag: file was modified externally.
    pub external_change_detected: bool,
    /// File size in bytes (for determining whether to use smart features).
    pub size: u64,
    /// Cached title (filename).
    pub title: String,
    /// Temporary file name for unsaved buffer (for session restoration).
    pub unsaved_buffer_file: Option<String>,
}

impl FileState {
    /// Create new FileState with default values.
    pub fn new() -> Self {
        Self {
            mtime: None,
            external_change_detected: false,
            size: 0,
            title: "Untitled".to_string(),
            unsaved_buffer_file: None,
        }
    }

    /// Create FileState from file metadata.
    pub fn from_path(path: &Path, mtime: Option<SystemTime>, size: u64) -> Self {
        Self {
            mtime,
            external_change_detected: false,
            size,
            title: file_io::path_to_title(path),
            unsaved_buffer_file: None,
        }
    }

    /// Check if file was modified externally.
    pub fn check_external_modification(&mut self, path: &Path) {
        if file_io::was_modified_externally(path, self.mtime) {
            self.external_change_detected = true;
        }
    }

    /// Update mtime after save.
    pub fn update_mtime(&mut self, path: &Path) {
        self.mtime = file_io::get_file_mtime(path);
        self.external_change_detected = false;
    }

    /// Clear external change flag.
    pub fn clear_external_change(&mut self) {
        self.external_change_detected = false;
    }

    /// Update title from path.
    pub fn update_title(&mut self, path: &Path) {
        self.title = file_io::path_to_title(path);
    }
}
