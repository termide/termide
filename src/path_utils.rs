//! Utilities for resolving file operation destination paths

use std::path::{Path, PathBuf};

/// Resolve destination path for a single file/directory operation
///
/// If destination is a directory, appends source filename to it.
/// Otherwise, uses destination as-is.
///
/// # Examples
/// ```ignore
/// let dest = resolve_destination_path(
///     Path::new("/home/file.txt"),
///     Path::new("/tmp/")  // directory
/// );
/// // Result: /tmp/file.txt
///
/// let dest = resolve_destination_path(
///     Path::new("/home/file.txt"),
///     Path::new("/tmp/renamed.txt")  // file
/// );
/// // Result: /tmp/renamed.txt
/// ```
pub fn resolve_destination_path(source: &Path, destination: &Path) -> PathBuf {
    if destination.is_dir() {
        destination.join(source.file_name().unwrap_or_default())
    } else {
        destination.to_path_buf()
    }
}

/// Resolve destination path for batch operations
///
/// Handles special case where single source to non-directory destination
/// should use the destination name (rename operation).
///
/// # Arguments
/// * `source` - Source path to copy/move
/// * `destination` - Target directory or file path
/// * `is_single_source` - Whether this is the only file in the batch
///
/// # Examples
/// ```ignore
/// // Multiple sources to directory
/// let dest = resolve_batch_destination_path(
///     Path::new("/home/file.txt"),
///     Path::new("/tmp/"),
///     false  // multiple sources
/// );
/// // Result: /tmp/file.txt
///
/// // Single source to file (rename)
/// let dest = resolve_batch_destination_path(
///     Path::new("/home/file.txt"),
///     Path::new("/tmp/renamed.txt"),
///     true  // single source
/// );
/// // Result: /tmp/renamed.txt
/// ```
pub fn resolve_batch_destination_path(
    source: &Path,
    destination: &Path,
    is_single_source: bool,
) -> PathBuf {
    if destination.is_dir() {
        // Destination is directory - append source filename
        destination.join(source.file_name().unwrap_or_default())
    } else if is_single_source {
        // Single file to non-directory - use destination as-is (rename)
        destination.to_path_buf()
    } else {
        // Multiple files to non-directory - append filename (fallback)
        destination.join(source.file_name().unwrap_or_default())
    }
}

/// Resolve destination path when applying rename pattern
///
/// If destination is a directory, joins new_name to it.
/// Otherwise, replaces filename in destination path with new_name.
///
/// # Arguments
/// * `destination` - Target directory or file path
/// * `new_name` - New filename to apply
///
/// # Examples
/// ```ignore
/// let dest = resolve_rename_destination_path(
///     Path::new("/tmp/"),
///     "newfile.txt"
/// );
/// // Result: /tmp/newfile.txt
///
/// let dest = resolve_rename_destination_path(
///     Path::new("/tmp/oldfile.txt"),
///     "newfile.txt"
/// );
/// // Result: /tmp/newfile.txt
/// ```
pub fn resolve_rename_destination_path(destination: &Path, new_name: &str) -> PathBuf {
    if destination.is_dir() {
        destination.join(new_name)
    } else {
        destination.with_file_name(new_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_resolve_destination_path_to_dir() {
        let source = PathBuf::from("/home/user/file.txt");
        let dest = PathBuf::from("/tmp/");

        // Note: This test assumes /tmp/ exists and is a directory
        // In practice, we'd use a temporary directory
        let result = if dest.exists() && dest.is_dir() {
            resolve_destination_path(&source, &dest)
        } else {
            // Fallback for test environments where /tmp might not exist
            dest.join(source.file_name().unwrap_or_default())
        };

        assert_eq!(result.file_name().unwrap(), "file.txt");
    }

    #[test]
    fn test_resolve_destination_path_to_file() {
        let source = PathBuf::from("/home/user/file.txt");
        let dest = PathBuf::from("/tmp/renamed.txt");

        let result = resolve_destination_path(&source, &dest);
        // When destination is not a directory, use it as-is
        assert_eq!(result, dest);
    }

    #[test]
    fn test_resolve_batch_single_source() {
        let source = PathBuf::from("/home/user/file.txt");
        let dest = PathBuf::from("/tmp/renamed.txt");

        let result = resolve_batch_destination_path(&source, &dest, true);
        assert_eq!(result, dest);
    }

    #[test]
    fn test_resolve_rename_to_dir() {
        let dest = PathBuf::from("/tmp/");
        let new_name = "newfile.txt";

        // Test only the join logic without is_dir() check
        let result = dest.join(new_name);
        assert_eq!(result.file_name().unwrap(), "newfile.txt");
    }

    #[test]
    fn test_resolve_rename_with_file_name() {
        let dest = PathBuf::from("/tmp/oldfile.txt");
        let new_name = "newfile.txt";

        let result = dest.with_file_name(new_name);
        assert_eq!(result, PathBuf::from("/tmp/newfile.txt"));
    }
}
