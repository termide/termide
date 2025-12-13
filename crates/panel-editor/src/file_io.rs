//! File I/O operations for the editor.
//!
//! Provides helper functions for file loading, saving, and modification tracking.

use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;

use crate::constants;

/// Result of file metadata check.
pub(crate) struct FileMetadata {
    /// File size in bytes.
    pub size: u64,
    /// File modification time.
    pub mtime: Option<SystemTime>,
}

/// Check file size and modification time.
///
/// Returns `Err` if file is too large, otherwise returns metadata.
pub(crate) fn check_file_metadata(path: &Path) -> Result<FileMetadata> {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            if metadata.is_file() && metadata.len() > constants::MAX_EDITOR_FILE_SIZE {
                return Err(anyhow::anyhow!(
                    "File is too large to open ({:.1} MB). Maximum allowed size is {} MB.",
                    metadata.len() as f64 / constants::MEGABYTE as f64,
                    constants::MAX_EDITOR_FILE_SIZE / constants::MEGABYTE
                ));
            }
            Ok(FileMetadata {
                size: metadata.len(),
                mtime: metadata.modified().ok(),
            })
        }
        Err(e) => {
            log::warn!("File metadata check failed for {}: {}", path.display(), e);
            Ok(FileMetadata {
                size: 0,
                mtime: None,
            })
        }
    }
}

/// Check if file is read-only.
pub(crate) fn is_file_readonly(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.permissions().readonly())
        .unwrap_or(false)
}

/// Get current modification time of a file.
pub(crate) fn get_file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// Check if file was modified externally by comparing modification times.
///
/// Returns `true` if `current_mtime > saved_mtime`.
pub(crate) fn was_modified_externally(path: &Path, saved_mtime: Option<SystemTime>) -> bool {
    let saved = match saved_mtime {
        Some(t) => t,
        None => return false,
    };

    match std::fs::metadata(path) {
        Ok(metadata) => match metadata.modified() {
            Ok(current) => current > saved,
            Err(_) => false,
        },
        Err(_) => false,
    }
}

/// Extract filename from path for title display.
pub(crate) fn path_to_title(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Untitled".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_path_to_title() {
        assert_eq!(path_to_title(Path::new("/foo/bar/test.rs")), "test.rs");
        assert_eq!(path_to_title(Path::new("simple.txt")), "simple.txt");
    }

    #[test]
    fn test_check_file_metadata_normal() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "test content").unwrap();

        let result = check_file_metadata(file.path());
        assert!(result.is_ok());

        let metadata = result.unwrap();
        assert!(metadata.size > 0);
        assert!(metadata.mtime.is_some());
    }
}
