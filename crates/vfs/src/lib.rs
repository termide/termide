//! Virtual File System (VFS) abstraction for TermIDE.
//!
//! This crate provides a unified interface for accessing local and remote filesystems,
//! enabling seamless file operations across different protocols (SFTP, FTP, SMB, NFS).
//!
//! # Architecture
//!
//! The VFS is built around the [`VfsProvider`] trait, which defines the common interface
//! for all filesystem implementations. Operations are asynchronous via channels to avoid
//! blocking the UI thread during network operations.
//!
//! # URL Format
//!
//! Remote paths are specified using URLs:
//! - `sftp://user@host:port/path` - SFTP (SSH File Transfer Protocol)
//! - `ftp://host/path` - FTP
//! - `smb://server/share/path` - SMB/CIFS
//! - `nfs://server/export/path` - NFS (via FUSE)
//! - `/local/path` - Local filesystem
//!
//! # Example
//!
//! ```ignore
//! use termide_vfs::{VfsManager, parse_vfs_url};
//!
//! let manager = VfsManager::new();
//!
//! // Parse a URL and get the appropriate provider
//! let path = parse_vfs_url("sftp://user@host/home/user")?;
//! let provider = manager.provider_for(&path)?;
//!
//! // List directory contents
//! let entries = provider.list_dir(&path).recv()?;
//! for entry in entries {
//!     println!("{}: {:?}", entry.name, entry.metadata.file_type);
//! }
//! ```

pub mod cache;
pub mod error;
pub mod local;
pub mod traits;
pub mod types;
pub mod url;

#[cfg(feature = "sftp")]
pub mod sftp;
#[cfg(feature = "sftp")]
pub mod ssh_config;

#[cfg(feature = "ftp")]
pub mod ftp;

#[cfg(feature = "smb")]
pub mod smb;

#[cfg(feature = "nfs")]
pub mod fuse_mount;

// Re-exports for convenience
pub use cache::DirCache;
pub use error::{VfsError, VfsResult};
pub use local::LocalFileSystem;
pub use traits::{DiskSpace, VfsProvider, VfsProviderSync};
pub use types::{
    AuthMethod, ConnectOptions, ConnectionState, CopyProgress, DownloadProgress, UploadProgress,
    VfsCopyOperation, VfsDownloadOperation, VfsEntry, VfsFileType, VfsMetadata, VfsOperation,
    VfsPath, VfsProtocol, VfsUploadOperation,
};
pub use url::{is_vfs_url, parse_vfs_url, UrlComponents};

/// Maximum directory nesting depth for recursive operations (safety guard).
pub const MAX_RECURSION_DEPTH: usize = 100;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

/// Central manager for VFS providers.
///
/// The VfsManager maintains a pool of connected providers and handles
/// connection caching, authentication, and provider lifecycle.
pub struct VfsManager {
    /// Local filesystem provider (always available).
    local: LocalFileSystem,
    /// Connected remote providers, keyed by connection string.
    remote_providers: Arc<RwLock<HashMap<String, Box<dyn VfsProvider>>>>,
    /// Directory cache for all providers.
    cache: DirCache,
    /// Pending authentication requests.
    pending_auth: Arc<RwLock<Vec<AuthRequest>>>,
}

impl std::fmt::Debug for VfsManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VfsManager")
            .field(
                "remote_providers_count",
                &self.remote_providers.read().map(|p| p.len()).unwrap_or(0),
            )
            .finish_non_exhaustive()
    }
}

/// Authentication request for user interaction.
#[derive(Debug, Clone)]
pub struct AuthRequest {
    /// The path that requires authentication.
    pub path: VfsPath,
    /// Message to display to user.
    pub message: String,
    /// Whether password is required.
    pub needs_password: bool,
}

impl Default for VfsManager {
    fn default() -> Self {
        Self::new()
    }
}

impl VfsManager {
    /// Create a new VFS manager.
    pub fn new() -> Self {
        Self {
            local: LocalFileSystem::new(),
            remote_providers: Arc::new(RwLock::new(HashMap::new())),
            cache: DirCache::new(),
            pending_auth: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Get the local filesystem provider.
    pub fn local(&self) -> &LocalFileSystem {
        &self.local
    }

    /// Get the directory cache.
    pub fn cache(&self) -> &DirCache {
        &self.cache
    }

    /// Check if a path is local.
    pub fn is_local(path: &VfsPath) -> bool {
        path.is_local()
    }

    /// Get or create a provider for the given path.
    ///
    /// Spawn a background thread that builds a provider, connects it, and on
    /// success stores it under `key`. Shared by every remote `connect_*`
    /// method — only the provider constructor differs.
    #[cfg(any(feature = "sftp", feature = "ftp", feature = "smb"))]
    fn spawn_connect<P, F>(
        &self,
        options: ConnectOptions,
        path: &VfsPath,
        make: F,
    ) -> VfsOperation<()>
    where
        P: VfsProvider + 'static,
        F: FnOnce() -> P + Send + 'static,
    {
        let providers = Arc::clone(&self.remote_providers);
        let (tx, rx) = std::sync::mpsc::channel();

        // `connection_key` embeds the username so providers can be looked up
        // per account; it must never reach a log. Diagnostics use the redacted
        // `log_safe_key` instead.
        let key = path.connection_key();
        let log_key = path.log_safe_key();

        std::thread::spawn(move || {
            let mut provider = make();
            let result = provider.connect(options).recv();

            if let Err(e) = &result {
                log::error!("VfsManager: connection failed for {log_key}: {e}");
            }
            if result.is_ok() {
                if let Ok(mut providers) = providers.write() {
                    providers.insert(key, Box::new(provider));
                }
            }
            if let Err(e) = tx.send(result) {
                log::error!("VfsManager: failed to send result for {log_key}: {e:?}");
            }
        });

        VfsOperation::new(rx)
    }

    /// For local paths, returns the local provider.
    /// Get a mutable reference to create/connect a provider.
    ///
    /// This method handles the creation and connection of remote providers.
    #[cfg(feature = "sftp")]
    pub fn connect_sftp(&self, path: &VfsPath, options: ConnectOptions) -> VfsOperation<()> {
        use crate::sftp::SftpProvider;

        if !matches!(path.protocol, VfsProtocol::Sftp) {
            return VfsOperation::error(VfsError::InvalidPath("Expected SFTP path".to_string()));
        }

        let host = match &path.host {
            Some(h) => h.clone(),
            None => {
                return VfsOperation::error(VfsError::InvalidPath(
                    "Missing host in SFTP path".to_string(),
                ))
            }
        };

        let port = path.effective_port().unwrap_or(22);
        let username = path.username.clone();

        self.spawn_connect(options, path, move || {
            SftpProvider::new(&host, port, username.as_deref())
        })
    }

    /// Connect to an FTP or FTPS server.
    #[cfg(feature = "ftp")]
    pub fn connect_ftp(&self, path: &VfsPath, options: ConnectOptions) -> VfsOperation<()> {
        use crate::ftp::FtpProvider;

        let use_tls = matches!(path.protocol, VfsProtocol::Ftps);

        if !matches!(path.protocol, VfsProtocol::Ftp | VfsProtocol::Ftps) {
            return VfsOperation::error(VfsError::InvalidPath(
                "Expected FTP or FTPS path".to_string(),
            ));
        }

        let host = match &path.host {
            Some(h) => h.clone(),
            None => {
                return VfsOperation::error(VfsError::InvalidPath(
                    "Missing host in FTP path".to_string(),
                ))
            }
        };

        let port = path.effective_port();
        let username = path.username.clone();

        self.spawn_connect(options, path, move || {
            FtpProvider::new(&host, port, username.as_deref(), use_tls)
        })
    }

    /// Connect to an SMB/CIFS server.
    #[cfg(feature = "smb")]
    pub fn connect_smb(&self, path: &VfsPath, options: ConnectOptions) -> VfsOperation<()> {
        use crate::smb::SmbProvider;

        if !matches!(path.protocol, VfsProtocol::Smb) {
            return VfsOperation::error(VfsError::InvalidPath("Expected SMB path".to_string()));
        }

        let host = match &path.host {
            Some(h) => h.clone(),
            None => {
                return VfsOperation::error(VfsError::InvalidPath(
                    "Missing host in SMB path".to_string(),
                ))
            }
        };

        let port = path.effective_port();
        let username = path.username.clone();

        // Extract share from first path component: /share/rest/of/path
        let path_str = path.path.display().to_string();
        let trimmed = path_str.trim_start_matches('/');
        let share = trimmed.split('/').next().filter(|s| !s.is_empty());

        let share_owned = share.map(String::from);
        self.spawn_connect(options, path, move || {
            SmbProvider::new(
                &host,
                port,
                share_owned.as_deref(),
                username.as_deref(),
                None,
            )
        })
    }

    /// Connect to an SMB/CIFS server (stub when smb feature is disabled).
    #[cfg(not(feature = "smb"))]
    pub fn connect_smb(&self, _path: &VfsPath, _options: ConnectOptions) -> VfsOperation<()> {
        VfsOperation::error(VfsError::NotSupported(
            "SMB support not compiled. Enable the 'smb' feature.".to_string(),
        ))
    }

    /// Disconnect and remove a provider for the given connection key.
    pub fn disconnect(&self, connection_key: &str) {
        if let Ok(mut providers) = self.remote_providers.write() {
            if let Some(mut provider) = providers.remove(connection_key) {
                provider.disconnect();
            }
        }

        // Invalidate cache for this connection
        self.cache.invalidate_connection(connection_key);
    }

    /// Disconnect all remote providers.
    pub fn disconnect_all(&self) {
        if let Ok(mut providers) = self.remote_providers.write() {
            for (_, mut provider) in providers.drain() {
                provider.disconnect();
            }
        }

        self.cache.clear();
    }

    /// Check for pending authentication requests.
    pub fn pending_auth_requests(&self) -> Vec<AuthRequest> {
        self.pending_auth
            .read()
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    /// Clear pending authentication requests.
    pub fn clear_pending_auth(&self) {
        if let Ok(mut pending) = self.pending_auth.write() {
            pending.clear();
        }
    }

    /// Get list of active connections.
    pub fn active_connections(&self) -> Vec<String> {
        self.remote_providers
            .read()
            .map(|p| p.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Check if a remote path is connected.
    pub fn is_connected(&self, path: &VfsPath) -> bool {
        if path.is_local() {
            return true;
        }

        let key = path.connection_key();

        self.remote_providers
            .read()
            .map(|p| p.contains_key(&key))
            .unwrap_or(false)
    }

    /// Get the home directory for a connected remote path.
    ///
    /// For local paths, returns the user's home directory.
    /// For remote paths, returns the home directory from the connected provider.
    pub fn get_home_dir(&self, path: &VfsPath) -> Option<VfsPath> {
        if path.is_local() {
            return dirs::home_dir().map(VfsPath::local);
        }

        let key = path.connection_key();
        if let Ok(providers) = self.remote_providers.read() {
            if let Some(provider) = providers.get(&key) {
                return provider.home_dir();
            }
        }
        None
    }

    // === Convenience methods for common operations ===

    /// Dispatch an operation to the connected remote provider keyed by `path`.
    ///
    /// Returns `on_not_connected()` if no provider is registered for that connection
    /// or if the read lock is poisoned. The read guard is held while `on_remote`
    /// runs, matching the existing per-method behaviour.
    fn dispatch_remote<R, F, E>(&self, path: &VfsPath, on_remote: F, on_not_connected: E) -> R
    where
        F: FnOnce(&dyn VfsProvider) -> R,
        E: FnOnce() -> R,
    {
        let key = path.connection_key();

        // Fast read path: if the cached provider is still healthy, run
        // the op against it directly. The actor publishes Disconnected
        // / Failed on `SftpInner` when its session is dead — in that
        // case we evict the zombie provider and report NotConnected so
        // the UI re-runs Connect.
        if let Ok(providers) = self.remote_providers.read() {
            if let Some(provider) = providers.get(&key) {
                match provider.connection_state() {
                    ConnectionState::Connected | ConnectionState::Connecting => {
                        return on_remote(&**provider);
                    }
                    ConnectionState::Disconnected | ConnectionState::Failed => {
                        // Drop the guard before taking the write lock.
                    }
                }
            } else {
                return on_not_connected();
            }
        } else {
            return on_not_connected();
        }

        if let Ok(mut providers) = self.remote_providers.write() {
            if let Some(provider) = providers.get(&key) {
                if matches!(
                    provider.connection_state(),
                    ConnectionState::Disconnected | ConnectionState::Failed
                ) {
                    providers.remove(&key);
                }
            }
        }
        on_not_connected()
    }

    /// List directory contents with caching.
    pub fn list_dir(&self, path: &VfsPath) -> VfsOperation<Vec<VfsEntry>> {
        // Check cache first
        if let Some(entries) = self.cache.get(path) {
            return VfsOperation::ready(Ok(entries));
        }

        // Use local provider for local paths
        if path.is_local() {
            // Cache will be updated when result is received by caller
            return self.local.list_dir(path);
        }

        self.dispatch_remote(
            path,
            |p| p.list_dir(path),
            || VfsOperation::error(VfsError::NotConnected),
        )
    }

    /// Read a file.
    pub fn read_file(&self, path: &VfsPath) -> VfsOperation<Vec<u8>> {
        if path.is_local() {
            return self.local.read_file(path);
        }

        self.dispatch_remote(
            path,
            |p| p.read_file(path),
            || VfsOperation::error(VfsError::NotConnected),
        )
    }

    /// Write a file.
    pub fn write_file(&self, path: &VfsPath, data: &[u8]) -> VfsOperation<()> {
        if path.is_local() {
            self.cache.invalidate_with_parent(path);
            return self.local.write_file(path, data);
        }

        self.dispatch_remote(
            path,
            |p| {
                self.cache.invalidate_with_parent(path);
                p.write_file(path, data)
            },
            || VfsOperation::error(VfsError::NotConnected),
        )
    }

    /// Create a directory.
    pub fn create_dir(&self, path: &VfsPath) -> VfsOperation<()> {
        if path.is_local() {
            self.cache.invalidate_with_parent(path);
            return self.local.create_dir(path);
        }

        self.dispatch_remote(
            path,
            |p| {
                self.cache.invalidate_with_parent(path);
                p.create_dir(path)
            },
            || VfsOperation::error(VfsError::NotConnected),
        )
    }

    /// Check if a path exists.
    pub fn exists(&self, path: &VfsPath) -> VfsOperation<bool> {
        if path.is_local() {
            return self.local.exists(path);
        }

        self.dispatch_remote(
            path,
            |p| p.exists(path),
            || VfsOperation::error(VfsError::NotConnected),
        )
    }

    /// Get metadata for a path.
    pub fn metadata(&self, path: &VfsPath) -> VfsOperation<VfsMetadata> {
        if path.is_local() {
            return self.local.metadata(path);
        }

        self.dispatch_remote(
            path,
            |p| p.metadata(path),
            || VfsOperation::error(VfsError::NotConnected),
        )
    }

    /// Delete a file or directory.
    pub fn delete(&self, path: &VfsPath) -> VfsOperation<()> {
        if path.is_local() {
            self.cache.invalidate_with_parent(path);
            return self.local.delete(path);
        }

        self.dispatch_remote(
            path,
            |p| {
                self.cache.invalidate_with_parent(path);
                p.delete(path)
            },
            || VfsOperation::error(VfsError::NotConnected),
        )
    }

    /// Rename / move a file or directory within the same provider.
    ///
    /// On SFTP and FTP this is an atomic server-side operation (no
    /// data is transferred), so this should be preferred over
    /// download+upload whenever source and destination share the same
    /// connection.
    pub fn rename(&self, from: &VfsPath, to: &VfsPath) -> VfsOperation<()> {
        if from.is_local() && to.is_local() {
            self.cache.invalidate_with_parent(from);
            self.cache.invalidate_with_parent(to);
            return self.local.rename(from, to);
        }

        self.dispatch_remote(
            from,
            |p| {
                self.cache.invalidate_with_parent(from);
                self.cache.invalidate_with_parent(to);
                p.rename(from, to)
            },
            || VfsOperation::error(VfsError::NotConnected),
        )
    }

    /// Download a remote file to a local path.
    ///
    /// Returns the path to the downloaded local file.
    pub fn download(&self, remote: &VfsPath, local: &Path) -> VfsOperation<std::path::PathBuf> {
        if remote.is_local() {
            // For local paths, just copy
            return self.local.download(remote, local);
        }

        self.dispatch_remote(
            remote,
            |p| p.download(remote, local),
            || VfsOperation::error(VfsError::NotConnected),
        )
    }

    /// Download a remote file to local temp directory.
    ///
    /// Returns the path to the downloaded local file.
    pub fn download_to_temp(&self, remote: &VfsPath) -> VfsOperation<std::path::PathBuf> {
        if remote.is_local() {
            // Already local, just return the path
            return VfsOperation::ready(Ok(remote.path.clone()));
        }

        // Create temp file path
        let temp_dir = std::env::temp_dir().join("termide-vfs");
        let _ = std::fs::create_dir_all(&temp_dir);

        let filename = remote
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "download".to_string());

        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
        let temp_path = temp_dir.join(format!("{}_{}", timestamp, filename));

        // Use the download method
        self.download(remote, &temp_path)
    }

    /// Upload a local file to remote path.
    pub fn upload(&self, local: &Path, remote: &VfsPath) -> VfsOperation<()> {
        if remote.is_local() {
            // Just copy locally
            return self.local.upload(local, remote);
        }

        self.dispatch_remote(
            remote,
            |p| {
                self.cache.invalidate_with_parent(remote);
                p.upload(local, remote)
            },
            || VfsOperation::error(VfsError::NotConnected),
        )
    }

    /// Upload a local file to remote path with progress reporting.
    pub fn upload_with_progress(&self, local: &Path, remote: &VfsPath) -> VfsUploadOperation {
        if remote.is_local() {
            // For local, use default implementation (no progress)
            return self.local.upload_with_progress(local, remote);
        }

        self.dispatch_remote(
            remote,
            |p| {
                self.cache.invalidate_with_parent(remote);
                p.upload_with_progress(local, remote)
            },
            || VfsUploadOperation::error(VfsError::NotConnected),
        )
    }

    /// Download a remote file/directory to local path with progress and pause/cancel support.
    pub fn download_with_progress(&self, remote: &VfsPath, local: &Path) -> VfsDownloadOperation {
        if remote.is_local() {
            // For local, use default implementation
            return self.local.download_with_progress(remote, local);
        }

        self.dispatch_remote(
            remote,
            |p| p.download_with_progress(remote, local),
            || VfsDownloadOperation::error(VfsError::NotConnected),
        )
    }

    /// Copy a file/directory with progress and pause/cancel support.
    /// Works for both local and remote paths.
    pub fn copy_with_progress(&self, from: &VfsPath, to: &VfsPath) -> VfsCopyOperation {
        // Both must be local OR both must be on the same remote connection
        if from.is_local() && to.is_local() {
            return self.local.copy_with_progress(from, to);
        }

        if from.is_local() != to.is_local() {
            // Cross-protocol copy - not directly supported, use download/upload instead
            return VfsCopyOperation::error(VfsError::NotSupported(
                "Cross-protocol copy not supported. Use download/upload instead.".to_string(),
            ));
        }

        // Both remote - must be same connection
        let from_key = from.connection_key();
        let to_key = to.connection_key();
        if from_key != to_key {
            return VfsCopyOperation::error(VfsError::NotSupported(
                "Copy between different remote connections not supported".to_string(),
            ));
        }

        // Use the remote provider
        if let Ok(providers) = self.remote_providers.read() {
            if let Some(provider) = providers.get(&from_key) {
                return provider.copy_with_progress(from, to);
            }
        }

        VfsCopyOperation::error(VfsError::NotConnected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vfs_manager_creation() {
        let manager = VfsManager::new();
        assert!(manager.local().is_connected());
        assert!(manager.active_connections().is_empty());
    }

    #[test]
    fn test_local_path_detection() {
        let local = VfsPath::local("/home/user");
        let remote = VfsPath::remote(VfsProtocol::Sftp, "host", "/path");

        assert!(VfsManager::is_local(&local));
        assert!(!VfsManager::is_local(&remote));
    }

    #[test]
    fn test_is_connected() {
        let manager = VfsManager::new();

        let local = VfsPath::local("/home/user");
        assert!(manager.is_connected(&local));

        let remote = VfsPath::remote(VfsProtocol::Sftp, "host", "/path");
        assert!(!manager.is_connected(&remote));
    }

    #[test]
    fn test_list_local_dir() {
        let manager = VfsManager::new();
        let path = VfsPath::local(std::env::temp_dir());

        let result = manager.list_dir(&path).recv();
        assert!(result.is_ok());
    }
}
