//! VFS provider trait definitions.

use std::path::Path;

use crate::types::{
    AuthMethod, ConnectOptions, ConnectionState, CopyProgress, VfsCopyOperation,
    VfsDownloadOperation, VfsEntry, VfsMetadata, VfsOperation, VfsPath, VfsUploadOperation,
};

/// Trait for virtual filesystem providers.
///
/// Each provider implements a specific protocol (local, SFTP, FTP, SMB, NFS).
/// Operations are asynchronous to avoid blocking the UI during network operations.
pub trait VfsProvider: Send + Sync {
    /// Get the provider name (e.g., "local", "sftp", "ftp").
    fn name(&self) -> &'static str;

    /// Get the current connection state.
    fn connection_state(&self) -> ConnectionState;

    /// Check if the provider is connected and ready.
    fn is_connected(&self) -> bool {
        self.connection_state() == ConnectionState::Connected
    }

    /// Connect to the remote filesystem.
    ///
    /// For local filesystem, this is a no-op that immediately returns success.
    fn connect(&mut self, options: ConnectOptions) -> VfsOperation<()>;

    /// Disconnect from the remote filesystem.
    fn disconnect(&mut self);

    // === Directory operations ===

    /// List contents of a directory.
    fn list_dir(&self, path: &VfsPath) -> VfsOperation<Vec<VfsEntry>>;

    /// Create a new directory.
    fn create_dir(&self, path: &VfsPath) -> VfsOperation<()>;

    /// Create a directory and all parent directories.
    fn create_dir_all(&self, path: &VfsPath) -> VfsOperation<()>;

    /// Check if a path exists.
    fn exists(&self, path: &VfsPath) -> VfsOperation<bool>;

    /// Get metadata for a path.
    fn metadata(&self, path: &VfsPath) -> VfsOperation<VfsMetadata>;

    /// Get metadata for a path without following a final symlink: a link
    /// reports `VfsFileType::Symlink`. Recursive deletes must decide from
    /// this, never from `metadata`, or they descend into a link's target.
    /// Providers without symlinks keep the default, which is `metadata`.
    fn symlink_metadata(&self, path: &VfsPath) -> VfsOperation<VfsMetadata> {
        self.metadata(path)
    }

    // === File operations ===

    /// Read entire file contents into memory.
    fn read_file(&self, path: &VfsPath) -> VfsOperation<Vec<u8>>;

    /// Write data to a file (creates or overwrites).
    fn write_file(&self, path: &VfsPath, data: &[u8]) -> VfsOperation<()>;

    /// Delete a file or empty directory.
    fn delete(&self, path: &VfsPath) -> VfsOperation<()>;

    /// Delete a directory and all its contents recursively.
    fn delete_recursive(&self, path: &VfsPath) -> VfsOperation<()>;

    /// Rename/move a file or directory.
    fn rename(&self, from: &VfsPath, to: &VfsPath) -> VfsOperation<()>;

    /// Copy a file.
    fn copy(&self, from: &VfsPath, to: &VfsPath) -> VfsOperation<()>;

    /// Copy a file or directory with progress reporting and pause/cancel support.
    /// Default implementation uses regular copy without progress.
    fn copy_with_progress(&self, from: &VfsPath, to: &VfsPath) -> VfsCopyOperation {
        // Default: wrap regular copy without progress
        let op = self.copy(from, to);
        let (tx, rx) = std::sync::mpsc::channel();
        let (_, progress_rx) = std::sync::mpsc::channel::<CopyProgress>();

        std::thread::spawn(move || {
            let result = op.recv();
            let _ = tx.send(result);
        });

        VfsCopyOperation::new(
            rx,
            progress_rx,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
    }

    // === Local transfer operations ===

    /// Download a remote file to local filesystem.
    ///
    /// Returns the path to the local file.
    fn download(&self, remote: &VfsPath, local: &Path) -> VfsOperation<std::path::PathBuf>;

    /// Upload a local file to remote filesystem.
    fn upload(&self, local: &Path, remote: &VfsPath) -> VfsOperation<()>;

    /// Upload a local file with progress reporting and pause/cancel support.
    /// Default implementation uses regular upload without progress.
    fn upload_with_progress(&self, local: &Path, remote: &VfsPath) -> VfsUploadOperation {
        // Default: wrap regular upload without progress
        let op = self.upload(local, remote);
        let (tx, rx) = std::sync::mpsc::channel();
        let (_, progress_rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = op.recv();
            let _ = tx.send(result);
        });

        VfsUploadOperation::new(
            rx,
            progress_rx,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
    }

    /// Download a remote file/directory with progress reporting and pause/cancel support.
    /// Default implementation uses regular download without progress/pause.
    fn download_with_progress(&self, remote: &VfsPath, local: &Path) -> VfsDownloadOperation {
        // Default: wrap regular download without progress
        let op = self.download(remote, local);
        let (tx, rx) = std::sync::mpsc::channel();
        let (_, progress_rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = op.recv();
            let _ = tx.send(result);
        });

        VfsDownloadOperation::new(
            rx,
            progress_rx,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
    }

    // === Optional operations ===

    /// Get supported authentication methods for this provider.
    fn supported_auth_methods(&self) -> Vec<AuthMethod> {
        vec![AuthMethod::None]
    }

    /// Check if this provider supports recursive operations natively.
    fn supports_recursive(&self) -> bool {
        false
    }

    /// Get the home directory for this connection (if applicable).
    fn home_dir(&self) -> Option<VfsPath> {
        None
    }
}
