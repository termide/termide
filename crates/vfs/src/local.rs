//! Local filesystem VFS provider.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

use crate::error::{VfsError, VfsResult};
use crate::traits::VfsProvider;
use crate::types::{
    AuthMethod, ConnectOptions, ConnectionState, CopyProgress, VfsCopyOperation, VfsEntry,
    VfsMetadata, VfsOperation, VfsPath,
};

use crate::MAX_RECURSION_DEPTH;

/// Map an `io::Error` to a `VfsError`, using `NotFound` / `PermissionDenied`
/// variants when the error kind matches.
fn map_io_error(e: std::io::Error, path: PathBuf) -> VfsError {
    match e.kind() {
        std::io::ErrorKind::NotFound => VfsError::NotFound { path },
        std::io::ErrorKind::PermissionDenied => VfsError::PermissionDenied { path },
        _ => VfsError::Io(e),
    }
}

/// Whether `path` is the root of a Windows drive or UNC share (`C:\`,
/// `\\server\share\`): a local path with no parent that still is not the whole
/// filesystem, because the other drives sit beside it rather than above it.
/// Always false on Unix, where `/` has no prefix.
pub fn is_drive_root(path: &Path) -> bool {
    path.parent().is_none()
        && matches!(
            path.components().next(),
            Some(std::path::Component::Prefix(_))
        )
}

/// Roots of the mounted drives (`C:\`, `D:\`, ...), in letter order. Empty on
/// Unix, where every mount already hangs below `/`.
///
/// Reads the drive bitmask only: no drive is touched, so an empty card reader
/// or a disconnected network letter costs nothing here.
pub fn drive_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        // SAFETY: takes no arguments and only returns a bitmask.
        let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
        (0..26u8)
            .filter(|bit| mask & (1 << bit) != 0)
            .map(|bit| PathBuf::from(format!("{}:\\", (b'A' + bit) as char)))
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Local filesystem provider.
///
/// This provider wraps the standard library's filesystem operations
/// and implements the VfsProvider trait for consistency with remote providers.
#[derive(Default)]
pub struct LocalFileSystem {
    /// Always connected for local filesystem.
    connected: bool,
}

impl LocalFileSystem {
    /// Create a new local filesystem provider.
    pub fn new() -> Self {
        Self { connected: true }
    }

    /// Convert VfsPath to local PathBuf, validating it's a local path.
    fn to_local_path(path: &VfsPath) -> VfsResult<&Path> {
        if !path.is_local() {
            return Err(VfsError::InvalidPath(format!(
                "Expected local path, got: {}",
                path
            )));
        }
        Ok(&path.path)
    }

    /// Convert fs::Metadata and path to VfsEntry.
    fn metadata_to_entry(name: &str, path: &VfsPath, metadata: fs::Metadata) -> VfsEntry {
        VfsEntry::new(name, path.clone(), VfsMetadata::from(metadata))
    }

    /// Read directory entries (internal helper).
    fn read_dir_entries(path: &Path) -> VfsResult<Vec<VfsEntry>> {
        let vfs_path = VfsPath::local(path);
        let mut entries = Vec::new();

        let read_dir = fs::read_dir(path).map_err(|e| map_io_error(e, path.to_path_buf()))?;

        for entry in read_dir {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let entry_path = vfs_path.join(&name);

            // Get metadata, handling symlinks
            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    log::warn!("Failed to get metadata for {:?}: {}", entry.path(), e);
                    continue;
                }
            };

            entries.push(Self::metadata_to_entry(&name, &entry_path, metadata));
        }

        // Sort: directories first, then by name
        entries.sort_by(|a, b| match (a.is_dir(), b.is_dir()) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });

        Ok(entries)
    }

    /// Copy directory recursively (internal helper).
    fn copy_dir_recursive(src: &Path, dst: &Path, depth: usize) -> VfsResult<()> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(VfsError::RemoteError {
                message: format!("Directory nesting too deep (> {})", MAX_RECURSION_DEPTH),
            });
        }

        fs::create_dir_all(dst)?;

        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let entry_path = entry.path();
            let dest_path = dst.join(entry.file_name());

            let metadata = fs::symlink_metadata(&entry_path)?;

            if metadata.is_symlink() {
                // Copy symlink
                #[cfg(unix)]
                {
                    let target = fs::read_link(&entry_path)?;
                    std::os::unix::fs::symlink(target, &dest_path)?;
                }
                #[cfg(not(unix))]
                {
                    // On Windows, copy as regular file
                    fs::copy(&entry_path, &dest_path)?;
                }
            } else if metadata.is_dir() {
                Self::copy_dir_recursive(&entry_path, &dest_path, depth + 1)?;
            } else {
                fs::copy(&entry_path, &dest_path)?;
            }
        }

        Ok(())
    }

    /// Delete directory recursively (internal helper).
    fn delete_dir_recursive(path: &Path) -> VfsResult<()> {
        fs::remove_dir_all(path).map_err(|e| map_io_error(e, path.to_path_buf()))
    }

    /// Count files and total size in a directory (internal helper).
    fn count_directory_contents(path: &Path, depth: usize) -> VfsResult<(usize, u64)> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(VfsError::RemoteError {
                message: format!("Directory nesting too deep (> {})", MAX_RECURSION_DEPTH),
            });
        }

        let mut file_count = 0;
        let mut total_bytes = 0u64;

        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;

            if metadata.is_dir() && !metadata.is_symlink() {
                let (sub_count, sub_bytes) =
                    Self::count_directory_contents(&entry.path(), depth + 1)?;
                file_count += sub_count;
                total_bytes += sub_bytes;
            } else if metadata.is_file() {
                file_count += 1;
                total_bytes += metadata.len();
            } else if metadata.is_symlink() {
                file_count += 1; // Count symlinks as files
            }
        }

        Ok((file_count, total_bytes))
    }

    /// Copy file with chunked I/O, progress reporting, and pause/cancel support.
    #[allow(clippy::too_many_arguments)]
    fn copy_file_chunked(
        src: &Path,
        dst: &Path,
        pause_flag: &Arc<AtomicBool>,
        cancel_flag: &Arc<AtomicBool>,
        progress_tx: &mpsc::Sender<CopyProgress>,
        bytes_offset: u64,
        total_bytes: u64,
        files_copied: usize,
        total_files: usize,
    ) -> VfsResult<u64> {
        const CHUNK_SIZE: usize = 1024 * 1024; // 1MB chunks

        let mut src_file = fs::File::open(src)?;
        let file_size = src_file.metadata()?.len();

        let mut dst_file = fs::File::create(dst)?;
        let mut buffer = vec![0u8; CHUNK_SIZE];
        let mut bytes_copied_local = 0u64;

        loop {
            // Check for cancellation
            if cancel_flag.load(Ordering::Relaxed) {
                return Err(VfsError::RemoteError {
                    message: "Operation cancelled by user".to_string(),
                });
            }

            // Wait while paused
            while pause_flag.load(Ordering::Relaxed) {
                if cancel_flag.load(Ordering::Relaxed) {
                    return Err(VfsError::RemoteError {
                        message: "Operation cancelled by user".to_string(),
                    });
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            let bytes_read = src_file.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }

            dst_file.write_all(&buffer[..bytes_read])?;
            bytes_copied_local += bytes_read as u64;

            // Send progress update
            let _ = progress_tx.send(CopyProgress {
                bytes_copied: bytes_offset + bytes_copied_local,
                total_bytes,
                files_copied,
                total_files,
                current_file: Some(
                    src.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                ),
            });
        }

        Ok(file_size)
    }

    /// Copy directory with progress reporting and pause/cancel support.
    #[allow(clippy::too_many_arguments)]
    fn copy_dir_with_progress(
        src: &Path,
        dst: &Path,
        pause_flag: &Arc<AtomicBool>,
        cancel_flag: &Arc<AtomicBool>,
        progress_tx: &mpsc::Sender<CopyProgress>,
        bytes_offset: &mut u64,
        total_bytes: u64,
        files_copied: &mut usize,
        total_files: usize,
        depth: usize,
    ) -> VfsResult<()> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(VfsError::RemoteError {
                message: format!("Directory nesting too deep (> {})", MAX_RECURSION_DEPTH),
            });
        }

        fs::create_dir_all(dst)?;

        for entry in fs::read_dir(src)? {
            // Check for cancellation
            if cancel_flag.load(Ordering::Relaxed) {
                return Err(VfsError::RemoteError {
                    message: "Operation cancelled by user".to_string(),
                });
            }

            let entry = entry?;
            let entry_path = entry.path();
            let dest_path = dst.join(entry.file_name());
            let metadata = fs::symlink_metadata(&entry_path)?;

            if metadata.is_symlink() {
                // Copy symlink
                #[cfg(unix)]
                {
                    let target = fs::read_link(&entry_path)?;
                    std::os::unix::fs::symlink(target, &dest_path)?;
                }
                #[cfg(not(unix))]
                {
                    fs::copy(&entry_path, &dest_path)?;
                }
                *files_copied += 1;
            } else if metadata.is_dir() {
                Self::copy_dir_with_progress(
                    &entry_path,
                    &dest_path,
                    pause_flag,
                    cancel_flag,
                    progress_tx,
                    bytes_offset,
                    total_bytes,
                    files_copied,
                    total_files,
                    depth + 1,
                )?;
            } else {
                // Copy file with chunked I/O
                let file_size = Self::copy_file_chunked(
                    &entry_path,
                    &dest_path,
                    pause_flag,
                    cancel_flag,
                    progress_tx,
                    *bytes_offset,
                    total_bytes,
                    *files_copied,
                    total_files,
                )?;
                *bytes_offset += file_size;
                *files_copied += 1;
            }
        }

        Ok(())
    }
}

impl VfsProvider for LocalFileSystem {
    fn name(&self) -> &'static str {
        "local"
    }

    fn connection_state(&self) -> ConnectionState {
        if self.connected {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }

    fn connect(&mut self, _options: ConnectOptions) -> VfsOperation<()> {
        // Local filesystem is always connected
        self.connected = true;
        VfsOperation::ready(Ok(()))
    }

    fn disconnect(&mut self) {
        // No-op for local filesystem
        self.connected = false;
    }

    fn list_dir(&self, path: &VfsPath) -> VfsOperation<Vec<VfsEntry>> {
        let local_path = match Self::to_local_path(path) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };

        // Run in thread to match async pattern
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = Self::read_dir_entries(&local_path);
            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn create_dir(&self, path: &VfsPath) -> VfsOperation<()> {
        let local_path = match Self::to_local_path(path) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = fs::create_dir(&local_path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    VfsError::AlreadyExists { path: local_path }
                } else {
                    VfsError::Io(e)
                }
            });
            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn create_dir_all(&self, path: &VfsPath) -> VfsOperation<()> {
        let local_path = match Self::to_local_path(path) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = fs::create_dir_all(&local_path).map_err(VfsError::Io);
            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn exists(&self, path: &VfsPath) -> VfsOperation<bool> {
        let local_path = match Self::to_local_path(path) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };

        // Simple synchronous check, wrapped in operation for consistency
        VfsOperation::ready(Ok(local_path.exists()))
    }

    fn metadata(&self, path: &VfsPath) -> VfsOperation<VfsMetadata> {
        let local_path = match Self::to_local_path(path) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = fs::metadata(&local_path)
                .map(VfsMetadata::from)
                .map_err(|e| map_io_error(e, local_path));
            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn symlink_metadata(&self, path: &VfsPath) -> VfsOperation<VfsMetadata> {
        let local_path = match Self::to_local_path(path) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };
        let result = fs::symlink_metadata(&local_path)
            .map(VfsMetadata::from)
            .map_err(|e| map_io_error(e, local_path));
        VfsOperation::ready(result)
    }

    fn read_file(&self, path: &VfsPath) -> VfsOperation<Vec<u8>> {
        let local_path = match Self::to_local_path(path) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = fs::read(&local_path).map_err(|e| map_io_error(e, local_path));
            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn write_file(&self, path: &VfsPath, data: &[u8]) -> VfsOperation<()> {
        let local_path = match Self::to_local_path(path) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };

        let data = data.to_vec();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = fs::write(&local_path, data).map_err(|e| map_io_error(e, local_path));
            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn delete(&self, path: &VfsPath) -> VfsOperation<()> {
        let local_path = match Self::to_local_path(path) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            // A symlink to a directory is unlinked, not treated as a dir.
            let is_dir = fs::symlink_metadata(&local_path).is_ok_and(|m| m.is_dir());
            let result = if is_dir {
                fs::remove_dir(&local_path)
            } else {
                fs::remove_file(&local_path)
            }
            .map_err(|e| map_io_error(e, local_path));
            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn delete_recursive(&self, path: &VfsPath) -> VfsOperation<()> {
        let local_path = match Self::to_local_path(path) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = Self::delete_dir_recursive(&local_path);
            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn rename(&self, from: &VfsPath, to: &VfsPath) -> VfsOperation<()> {
        let from_path = match Self::to_local_path(from) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };
        let to_path = match Self::to_local_path(to) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = fs::rename(&from_path, &to_path).map_err(|e| map_io_error(e, from_path));
            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn copy(&self, from: &VfsPath, to: &VfsPath) -> VfsOperation<()> {
        let from_path = match Self::to_local_path(from) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };
        let to_path = match Self::to_local_path(to) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = if from_path.is_dir() {
                Self::copy_dir_recursive(&from_path, &to_path, 0)
            } else {
                fs::copy(&from_path, &to_path)
                    .map(|_| ())
                    .map_err(|e| map_io_error(e, from_path))
            };
            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn copy_with_progress(&self, from: &VfsPath, to: &VfsPath) -> VfsCopyOperation {
        let from_path = match Self::to_local_path(from) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsCopyOperation::error(e),
        };
        let to_path = match Self::to_local_path(to) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsCopyOperation::error(e),
        };

        let (tx_complete, rx_complete) = mpsc::channel();
        let (tx_progress, rx_progress) = mpsc::channel();

        let pause_flag = Arc::new(AtomicBool::new(false));
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let pause_flag_clone = Arc::clone(&pause_flag);
        let cancel_flag_clone = Arc::clone(&cancel_flag);

        thread::spawn(move || {
            let result = if from_path.is_dir() {
                // Count total files and size first
                match Self::count_directory_contents(&from_path, 0) {
                    Ok((total_files, total_bytes)) => {
                        let mut bytes_offset = 0u64;
                        let mut files_copied = 0usize;

                        Self::copy_dir_with_progress(
                            &from_path,
                            &to_path,
                            &pause_flag_clone,
                            &cancel_flag_clone,
                            &tx_progress,
                            &mut bytes_offset,
                            total_bytes,
                            &mut files_copied,
                            total_files,
                            0,
                        )
                    }
                    Err(e) => Err(e),
                }
            } else {
                // Single file copy
                let file_size = match fs::metadata(&from_path) {
                    Ok(m) => m.len(),
                    Err(e) => {
                        let _ = tx_complete.send(Err(VfsError::Io(e)));
                        return;
                    }
                };

                Self::copy_file_chunked(
                    &from_path,
                    &to_path,
                    &pause_flag_clone,
                    &cancel_flag_clone,
                    &tx_progress,
                    0,
                    file_size,
                    0,
                    1,
                )
                .map(|_| ())
            };

            let _ = tx_complete.send(result);
        });

        VfsCopyOperation::new(rx_complete, rx_progress, pause_flag, cancel_flag)
    }

    fn download(&self, remote: &VfsPath, local: &Path) -> VfsOperation<PathBuf> {
        // For local filesystem, "download" is just a copy
        let from_path = match Self::to_local_path(remote) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };
        let to_path = local.to_path_buf();

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = fs::copy(&from_path, &to_path)
                .map(|_| to_path.clone())
                .map_err(VfsError::Io);
            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn upload(&self, local: &Path, remote: &VfsPath) -> VfsOperation<()> {
        // For local filesystem, "upload" is just a copy
        let from_path = local.to_path_buf();
        let to_path = match Self::to_local_path(remote) {
            Ok(p) => p.to_path_buf(),
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = fs::copy(&from_path, &to_path)
                .map(|_| ())
                .map_err(VfsError::Io);
            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn supported_auth_methods(&self) -> Vec<AuthMethod> {
        vec![AuthMethod::None]
    }

    fn supports_recursive(&self) -> bool {
        true
    }

    fn home_dir(&self) -> Option<VfsPath> {
        dirs::home_dir().map(VfsPath::local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::VfsFileType;
    use tempfile::TempDir;

    fn create_test_provider() -> (LocalFileSystem, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let provider = LocalFileSystem::new();
        (provider, temp_dir)
    }

    #[test]
    fn unix_root_is_not_a_drive_root() {
        assert!(!is_drive_root(Path::new("/")));
        assert!(!is_drive_root(Path::new("/home")));
    }

    #[cfg(windows)]
    #[test]
    fn drive_and_share_roots_are_drive_roots() {
        assert!(is_drive_root(Path::new(r"C:\")));
        assert!(is_drive_root(Path::new(r"\\server\share\")));
        assert!(!is_drive_root(Path::new(r"C:\Users")));
    }

    #[cfg(windows)]
    #[test]
    fn drive_roots_include_the_system_drive() {
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        assert!(drive_roots().contains(&PathBuf::from(format!("{system}\\"))));
    }

    #[test]
    fn test_local_provider_name() {
        let provider = LocalFileSystem::new();
        assert_eq!(provider.name(), "local");
    }

    #[test]
    fn test_local_provider_always_connected() {
        let provider = LocalFileSystem::new();
        assert!(provider.is_connected());
    }

    #[test]
    fn test_list_dir() {
        let (provider, temp_dir) = create_test_provider();

        // Create test files
        fs::write(temp_dir.path().join("file1.txt"), "content1").unwrap();
        fs::write(temp_dir.path().join("file2.txt"), "content2").unwrap();
        fs::create_dir(temp_dir.path().join("subdir")).unwrap();

        let path = VfsPath::local(temp_dir.path());
        let entries = provider.list_dir(&path).recv().unwrap();

        assert_eq!(entries.len(), 3);
        // Directories should be first
        assert!(entries[0].is_dir());
        assert_eq!(entries[0].name, "subdir");
    }

    #[test]
    fn test_read_write_file() {
        let (provider, temp_dir) = create_test_provider();

        let file_path = VfsPath::local(temp_dir.path().join("test.txt"));
        let content = b"Hello, VFS!";

        // Write
        provider.write_file(&file_path, content).recv().unwrap();

        // Read
        let read_content = provider.read_file(&file_path).recv().unwrap();
        assert_eq!(read_content, content);
    }

    #[test]
    fn test_create_and_delete_dir() {
        let (provider, temp_dir) = create_test_provider();

        let dir_path = VfsPath::local(temp_dir.path().join("newdir"));

        // Create
        provider.create_dir(&dir_path).recv().unwrap();
        assert!(provider.exists(&dir_path).recv().unwrap());

        // Delete
        provider.delete(&dir_path).recv().unwrap();
        assert!(!provider.exists(&dir_path).recv().unwrap());
    }

    #[test]
    fn test_copy_file() {
        let (provider, temp_dir) = create_test_provider();

        let src = VfsPath::local(temp_dir.path().join("src.txt"));
        let dst = VfsPath::local(temp_dir.path().join("dst.txt"));

        provider.write_file(&src, b"copy me").recv().unwrap();
        provider.copy(&src, &dst).recv().unwrap();

        let content = provider.read_file(&dst).recv().unwrap();
        assert_eq!(content, b"copy me");
    }

    #[test]
    fn test_rename_file() {
        let (provider, temp_dir) = create_test_provider();

        let old = VfsPath::local(temp_dir.path().join("old.txt"));
        let new = VfsPath::local(temp_dir.path().join("new.txt"));

        provider.write_file(&old, b"content").recv().unwrap();
        provider.rename(&old, &new).recv().unwrap();

        assert!(!provider.exists(&old).recv().unwrap());
        assert!(provider.exists(&new).recv().unwrap());
    }

    #[test]
    fn test_metadata() {
        let (provider, temp_dir) = create_test_provider();

        let file_path = VfsPath::local(temp_dir.path().join("meta.txt"));
        provider.write_file(&file_path, b"12345").recv().unwrap();

        let meta = provider.metadata(&file_path).recv().unwrap();
        assert_eq!(meta.file_type, VfsFileType::File);
        assert_eq!(meta.size, 5);
    }

    #[test]
    fn test_home_dir() {
        let provider = LocalFileSystem::new();
        let home = provider.home_dir();
        assert!(home.is_some());
        assert!(home.unwrap().is_local());
    }
}
