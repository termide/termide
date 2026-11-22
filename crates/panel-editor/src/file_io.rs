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
                return Err(anyhow::anyhow!(termide_i18n::t()
                    .editor_file_too_large_fmt(
                        &format!("{:.1}", metadata.len() as f64 / constants::MEGABYTE as f64),
                        &(constants::MAX_EDITOR_FILE_SIZE / constants::MEGABYTE).to_string(),
                    )));
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
    file_readonly(path).unwrap_or(false)
}

/// Whether the file is read-only, or `None` when it cannot be looked at.
pub(crate) fn file_readonly(path: &Path) -> Option<bool> {
    std::fs::metadata(path)
        .ok()
        .map(|m| m.permissions().readonly())
}

/// Get current modification time and size of a file.
pub(crate) fn get_file_stamp(path: &Path) -> (Option<SystemTime>, Option<u64>) {
    match std::fs::metadata(path) {
        Ok(m) => (m.modified().ok(), Some(m.len())),
        Err(_) => (None, None),
    }
}

/// Check if file was modified externally since its modification time and
/// size were recorded.
///
/// Any other modification time counts, older as well as newer: a restore
/// from a backup or a `git checkout` of an old blob can move it back. A
/// different size counts even when the time did not move, as on file
/// systems with a coarse clock. Nothing recorded, or a file that cannot be
/// looked at, is not a change.
pub(crate) fn was_modified_externally(
    path: &Path,
    saved_mtime: Option<SystemTime>,
    saved_size: Option<u64>,
) -> bool {
    let Some(saved) = saved_mtime else {
        return false;
    };
    match std::fs::metadata(path) {
        Ok(metadata) => {
            metadata.modified().is_ok_and(|current| current != saved)
                || saved_size.is_some_and(|size| size != metadata.len())
        }
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

    fn set_mtime(path: &Path, mtime: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }

    #[test]
    fn an_older_mtime_is_an_external_change() {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "abc").unwrap();
        let (mtime, size) = get_file_stamp(file.path());
        assert!(!was_modified_externally(file.path(), mtime, size));

        set_mtime(
            file.path(),
            mtime.unwrap() - std::time::Duration::from_secs(3600),
        );
        assert!(was_modified_externally(file.path(), mtime, size));
    }

    #[test]
    fn a_new_size_under_the_same_mtime_is_an_external_change() {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "abc").unwrap();
        let (mtime, size) = get_file_stamp(file.path());

        std::fs::write(file.path(), "abcdef").unwrap();
        set_mtime(file.path(), mtime.unwrap());
        assert!(was_modified_externally(file.path(), mtime, size));
    }
}
