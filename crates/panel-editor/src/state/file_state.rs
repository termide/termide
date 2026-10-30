//! File-related state for the editor.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::file_io;
use termide_vfs::VfsPath;

/// State related to the file being edited.
#[derive(Default)]
pub struct FileState {
    /// File modification time at load/save (for detecting external changes).
    pub mtime: Option<SystemTime>,
    /// Flag: file was modified externally.
    pub external_change_detected: bool,
    /// File size in bytes (for determining whether to use smart features).
    pub size: u64,
    /// Cached title (filename).
    pub title: String,
    /// Temporary file name for unsaved buffer (for layout restoration).
    pub unsaved_buffer_file: Option<String>,
    /// Initial directory for new buffers (used in SaveAs dialog).
    pub initial_directory: Option<PathBuf>,
    /// Remote path if editing a remote file (via VFS).
    pub remote_path: Option<VfsPath>,
    /// Temporary local file path for remote editing.
    pub temp_local_path: Option<PathBuf>,
    /// Remote modification time at load (for conflict detection).
    pub remote_mtime: Option<SystemTime>,
    /// Flag: file is currently being uploaded to remote server.
    pub uploading: bool,
    /// Whether the file was read-only on disk when last looked at, for an
    /// editor whose editability follows the file's permissions; `None` for
    /// one opened to view only, which stays read-only whatever they become.
    pub disk_read_only: Option<bool>,
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
            initial_directory: None,
            remote_path: None,
            temp_local_path: None,
            remote_mtime: None,
            uploading: false,
            disk_read_only: None,
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
            initial_directory: None,
            remote_path: None,
            temp_local_path: None,
            remote_mtime: None,
            uploading: false,
            disk_read_only: None,
        }
    }

    /// Create FileState for a remote file.
    pub fn from_remote(
        remote_path: VfsPath,
        temp_path: PathBuf,
        mtime: Option<SystemTime>,
        size: u64,
    ) -> Self {
        let title = remote_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| remote_path.to_url_string());
        Self {
            mtime,
            external_change_detected: false,
            size,
            title,
            unsaved_buffer_file: None,
            initial_directory: None,
            remote_path: Some(remote_path),
            temp_local_path: Some(temp_path),
            remote_mtime: mtime,
            uploading: false,
            disk_read_only: None,
        }
    }

    /// Check if this is a remote file.
    pub fn is_remote(&self) -> bool {
        self.remote_path.is_some()
    }

    /// Get remote path if editing a remote file.
    pub fn remote_path(&self) -> Option<&VfsPath> {
        self.remote_path.as_ref()
    }

    /// Get temporary local path for remote file.
    pub fn temp_local_path(&self) -> Option<&Path> {
        self.temp_local_path.as_deref()
    }

    /// Update remote mtime after upload.
    pub fn update_remote_mtime(&mut self, mtime: Option<SystemTime>) {
        self.remote_mtime = mtime;
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

    /// Set upload state (for remote files).
    pub fn set_uploading(&mut self, uploading: bool) {
        self.uploading = uploading;
    }

    /// Check if file is currently being uploaded.
    pub fn is_uploading(&self) -> bool {
        self.uploading
    }
}
