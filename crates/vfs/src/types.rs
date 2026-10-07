//! VFS type definitions.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::SystemTime;

use crate::error::{VfsError, VfsResult};

/// Supported VFS protocols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VfsProtocol {
    /// Local filesystem.
    Local,
    /// SFTP (SSH File Transfer Protocol).
    Sftp,
    /// FTP (File Transfer Protocol).
    Ftp,
    /// FTPS (FTP over TLS/SSL).
    Ftps,
    /// SMB/CIFS (Server Message Block).
    Smb,
    /// NFS (Network File System) via FUSE mount.
    Nfs,
    /// An archive file browsed as a directory tree. The archive itself is
    /// [`VfsPath::container`]; [`VfsPath::path`] is the path inside it.
    Archive,
}

impl VfsProtocol {
    /// Get the URL scheme for this protocol.
    pub fn scheme(&self) -> &'static str {
        match self {
            Self::Local => "file",
            Self::Sftp => "sftp",
            Self::Ftp => "ftp",
            Self::Ftps => "ftps",
            Self::Smb => "smb",
            Self::Nfs => "nfs",
            Self::Archive => "archive",
        }
    }

    /// Parse a scheme string into a protocol.
    pub fn from_scheme(scheme: &str) -> Option<Self> {
        match scheme.to_lowercase().as_str() {
            "file" | "" => Some(Self::Local),
            "sftp" => Some(Self::Sftp),
            "ftp" => Some(Self::Ftp),
            "ftps" => Some(Self::Ftps),
            "smb" | "cifs" => Some(Self::Smb),
            "nfs" => Some(Self::Nfs),
            "archive" => Some(Self::Archive),
            _ => None,
        }
    }

    /// Check if paths of this protocol are served by a VFS provider rather
    /// than the local filesystem: every network protocol, and archives even
    /// when the archive file itself is local.
    pub fn is_remote(&self) -> bool {
        !matches!(self, Self::Local)
    }
}

/// A path in the virtual filesystem.
///
/// Represents either a local path or a remote path with connection info.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VfsPath {
    /// The protocol/scheme.
    pub protocol: VfsProtocol,
    /// Host for remote paths (None for local).
    pub host: Option<String>,
    /// Port for remote paths (None uses default).
    pub port: Option<u16>,
    /// Username for remote paths.
    pub username: Option<String>,
    /// The actual path component. For an archive path this is the absolute
    /// path inside the archive, `/` being the archive root.
    pub path: PathBuf,
    /// The archive file an [`VfsProtocol::Archive`] path lives in; `None` for
    /// every other protocol. It is a full `VfsPath` so that an archive on a
    /// remote host, or inside another archive, stays expressible. Archive
    /// paths carry no host, port or username of their own.
    pub container: Option<Box<VfsPath>>,
}

impl VfsPath {
    /// Create a local filesystem path.
    pub fn local<P: AsRef<Path>>(path: P) -> Self {
        Self {
            protocol: VfsProtocol::Local,
            host: None,
            port: None,
            username: None,
            path: path.as_ref().to_path_buf(),
            container: None,
        }
    }

    /// Create a remote path.
    pub fn remote(protocol: VfsProtocol, host: impl Into<String>, path: impl AsRef<Path>) -> Self {
        Self {
            protocol,
            host: Some(host.into()),
            port: None,
            username: None,
            path: path.as_ref().to_path_buf(),
            container: None,
        }
    }

    /// Create a path inside the archive file `container`. A relative `inner`
    /// is taken from the archive root.
    pub fn archive(container: VfsPath, inner: impl AsRef<Path>) -> Self {
        Self {
            protocol: VfsProtocol::Archive,
            host: None,
            port: None,
            username: None,
            path: Path::new("/").join(inner),
            container: Some(Box::new(container)),
        }
    }

    /// Set the port.
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Set the username.
    pub fn with_username(mut self, username: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self
    }

    /// Check if this is a local path.
    pub fn is_local(&self) -> bool {
        self.protocol == VfsProtocol::Local
    }

    /// Check if this is a remote path.
    pub fn is_remote(&self) -> bool {
        self.protocol.is_remote()
    }

    /// Check if this is a path inside an archive.
    pub fn is_archive(&self) -> bool {
        self.protocol == VfsProtocol::Archive
    }

    /// The archive file this path lives in, for an archive path.
    pub fn container(&self) -> Option<&VfsPath> {
        self.container.as_deref()
    }

    /// The same filesystem location (connection or archive) with a different
    /// path component.
    pub fn with_path(&self, path: impl Into<PathBuf>) -> Self {
        Self {
            protocol: self.protocol,
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            path: path.into(),
            container: self.container.clone(),
        }
    }

    /// Get the path component.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get the file name (last component).
    pub fn file_name(&self) -> Option<&std::ffi::OsStr> {
        self.path.file_name()
    }

    /// Get the parent path.
    /// Returns None if already at root. The root of an archive has no parent
    /// either: leaving the archive goes through [`VfsPath::container`].
    pub fn parent(&self) -> Option<VfsPath> {
        let parent = self.path.parent()?;
        // For root path "/" parent() returns Some("") - treat as no parent
        if parent.as_os_str().is_empty() || parent == self.path {
            return None;
        }
        Some(self.with_path(parent))
    }

    /// Join a path component.
    pub fn join<P: AsRef<Path>>(&self, path: P) -> Self {
        self.with_path(self.path.join(path))
    }

    /// Convert to a URL string.
    ///
    /// An archive path is `archive://<container>!<inner>`, e.g.
    /// `archive:///home/x/a.zip!/docs`. `!` and `%` are percent-encoded in
    /// the container part, so the first `!` always ends it and the string
    /// parses back to the same path.
    pub fn to_url_string(&self) -> String {
        if let Some(container) = self.container() {
            format!(
                "{}://{}!{}",
                VfsProtocol::Archive.scheme(),
                crate::url::encode_archive_container(&container.to_url_string()),
                self.path.display()
            )
        } else if self.is_local() {
            self.path.display().to_string()
        } else {
            let mut url = format!("{}://", self.protocol.scheme());

            if let Some(ref user) = self.username {
                url.push_str(user);
                url.push('@');
            }

            if let Some(ref host) = self.host {
                url.push_str(host);
            }

            if let Some(port) = self.port {
                url.push(':');
                url.push_str(&port.to_string());
            }

            url.push_str(&self.path.display().to_string());
            url
        }
    }

    /// Get a connection key for caching providers.
    /// This uniquely identifies a connection (protocol + host + port + user).
    /// Uses effective_port() to normalize keys (e.g., sftp://host and sftp://host:22 are the same).
    /// An open archive is its own connection, keyed by where the archive file
    /// lives and its path there.
    pub fn connection_key(&self) -> String {
        if let Some(container) = self.container() {
            format!(
                "{}:{}{}",
                VfsProtocol::Archive.scheme(),
                container.connection_key(),
                container.path.display()
            )
        } else if self.is_local() {
            "local".to_string()
        } else {
            format!(
                "{}://{}@{}:{}",
                self.protocol.scheme(),
                self.username.as_deref().unwrap_or(""),
                self.host.as_deref().unwrap_or(""),
                self.effective_port().unwrap_or(0)
            )
        }
    }

    /// Get a log-safe connection identifier (username redacted).
    ///
    /// Delegates to a free function taking only non-sensitive fields
    /// to avoid CodeQL taint propagation through `&self.username`.
    pub fn log_safe_key(&self) -> String {
        if let Some(container) = self.container() {
            let name = container.path.file_name().unwrap_or_default();
            return format!(
                "{}:{}/{}",
                VfsProtocol::Archive.scheme(),
                container.log_safe_key(),
                name.to_string_lossy()
            );
        }
        log_safe_connection_key(self.protocol, self.host.as_deref(), self.effective_port())
    }

    /// Get the default port for this protocol.
    pub fn default_port(&self) -> Option<u16> {
        match self.protocol {
            VfsProtocol::Local | VfsProtocol::Archive => None,
            VfsProtocol::Sftp => Some(22),
            VfsProtocol::Ftp => Some(21),
            VfsProtocol::Ftps => Some(990),
            VfsProtocol::Smb => Some(445),
            VfsProtocol::Nfs => Some(2049),
        }
    }

    /// Get the effective port (explicit or default).
    pub fn effective_port(&self) -> Option<u16> {
        self.port.or_else(|| self.default_port())
    }
}

/// Build a log-safe connection identifier from non-sensitive fields only.
///
/// This free function breaks CodeQL's taint chain: `VfsPath.username` never
/// enters scope here, so log output is not flagged as cleartext credential logging.
fn log_safe_connection_key(protocol: VfsProtocol, host: Option<&str>, port: Option<u16>) -> String {
    if protocol == VfsProtocol::Local {
        "local".to_string()
    } else {
        format!(
            "{}://***@{}:{}",
            protocol.scheme(),
            host.unwrap_or(""),
            port.unwrap_or(0)
        )
    }
}

impl std::fmt::Display for VfsPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_url_string())
    }
}

impl From<PathBuf> for VfsPath {
    fn from(path: PathBuf) -> Self {
        Self::local(path)
    }
}

impl From<&Path> for VfsPath {
    fn from(path: &Path) -> Self {
        Self::local(path)
    }
}

/// File type in the virtual filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsFileType {
    /// Regular file.
    File,
    /// Directory.
    Directory,
    /// Symbolic link.
    Symlink,
    /// Unknown or special file.
    Other,
}

impl VfsFileType {
    /// Check if this is a directory.
    pub fn is_dir(&self) -> bool {
        matches!(self, Self::Directory)
    }

    /// Check if this is a file.
    pub fn is_file(&self) -> bool {
        matches!(self, Self::File)
    }

    /// Check if this is a symlink.
    pub fn is_symlink(&self) -> bool {
        matches!(self, Self::Symlink)
    }
}

/// Metadata for a file or directory in the VFS.
#[derive(Debug, Clone)]
pub struct VfsMetadata {
    /// File type.
    pub file_type: VfsFileType,
    /// Size in bytes (0 for directories).
    pub size: u64,
    /// Last modification time.
    pub modified: Option<SystemTime>,
    /// Creation time (if available).
    pub created: Option<SystemTime>,
    /// Last access time (if available).
    pub accessed: Option<SystemTime>,
    /// Whether the file is read-only.
    pub readonly: bool,
    /// Unix permissions (if available).
    pub permissions: Option<u32>,
}

impl VfsMetadata {
    /// Create metadata for a directory.
    pub fn directory() -> Self {
        Self {
            file_type: VfsFileType::Directory,
            size: 0,
            modified: None,
            created: None,
            accessed: None,
            readonly: false,
            permissions: None,
        }
    }

    /// Create metadata for a file.
    pub fn file(size: u64) -> Self {
        Self {
            file_type: VfsFileType::File,
            size,
            modified: None,
            created: None,
            accessed: None,
            readonly: false,
            permissions: None,
        }
    }

    /// Set the modification time.
    pub fn with_modified(mut self, time: SystemTime) -> Self {
        self.modified = Some(time);
        self
    }

    /// Set readonly flag.
    pub fn with_readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    /// Set Unix permissions.
    pub fn with_permissions(mut self, perms: u32) -> Self {
        self.permissions = Some(perms);
        self
    }
}

impl From<std::fs::Metadata> for VfsMetadata {
    fn from(meta: std::fs::Metadata) -> Self {
        let file_type = if meta.is_dir() {
            VfsFileType::Directory
        } else if meta.is_symlink() {
            VfsFileType::Symlink
        } else if meta.is_file() {
            VfsFileType::File
        } else {
            VfsFileType::Other
        };

        #[cfg(unix)]
        let permissions = {
            use std::os::unix::fs::PermissionsExt;
            Some(meta.permissions().mode())
        };
        #[cfg(not(unix))]
        let permissions = None;

        Self {
            file_type,
            size: meta.len(),
            modified: meta.modified().ok(),
            created: meta.created().ok(),
            accessed: meta.accessed().ok(),
            readonly: meta.permissions().readonly(),
            permissions,
        }
    }
}

/// A directory entry in the VFS.
#[derive(Debug, Clone)]
pub struct VfsEntry {
    /// Entry name (file/directory name only, not full path).
    pub name: String,
    /// Full path of the entry.
    pub path: VfsPath,
    /// Metadata.
    pub metadata: VfsMetadata,
}

impl VfsEntry {
    /// Create a new VFS entry.
    pub fn new(name: impl Into<String>, path: VfsPath, metadata: VfsMetadata) -> Self {
        Self {
            name: name.into(),
            path,
            metadata,
        }
    }

    /// Check if this entry is a directory.
    pub fn is_dir(&self) -> bool {
        self.metadata.file_type.is_dir()
    }

    /// Check if this entry is a file.
    pub fn is_file(&self) -> bool {
        self.metadata.file_type.is_file()
    }

    /// Check if this entry is a symlink.
    pub fn is_symlink(&self) -> bool {
        self.metadata.file_type.is_symlink()
    }
}

/// Handle to an async VFS operation.
///
/// VFS operations are performed asynchronously via channels to avoid
/// blocking the UI thread during network operations.
pub struct VfsOperation<T> {
    /// Channel receiver for the operation result.
    receiver: mpsc::Receiver<VfsResult<T>>,
}

impl<T> VfsOperation<T> {
    /// Create a new VFS operation from a receiver.
    pub fn new(receiver: mpsc::Receiver<VfsResult<T>>) -> Self {
        Self { receiver }
    }

    /// Create an immediately completed operation.
    pub fn ready(result: VfsResult<T>) -> Self {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(result);
        Self { receiver: rx }
    }

    /// Create an error operation.
    pub fn error(error: VfsError) -> Self {
        Self::ready(Err(error))
    }

    /// Try to receive the result without blocking.
    pub fn try_recv(&self) -> Option<VfsResult<T>> {
        self.receiver.try_recv().ok()
    }

    /// Block until the result is available.
    pub fn recv(self) -> VfsResult<T> {
        self.receiver.recv().map_err(|e| VfsError::RemoteError {
            message: format!("Operation channel closed: {e}"),
        })?
    }

    /// Check if the operation is complete.
    pub fn is_complete(&self) -> bool {
        // Peek at the channel without consuming
        matches!(
            self.receiver.try_recv(),
            Ok(_) | Err(mpsc::TryRecvError::Disconnected)
        )
    }
}

/// Progress information for upload operations.
#[derive(Debug, Clone)]
pub struct UploadProgress {
    /// Total bytes uploaded so far (across all files).
    pub bytes_uploaded: u64,
    /// Total bytes to upload (all files).
    pub total_bytes: u64,
    /// Current file being uploaded (for directory uploads).
    pub current_file: Option<String>,
    /// Files uploaded so far (for directory uploads).
    pub files_uploaded: usize,
    /// Total files to upload (for directory uploads).
    pub total_files: usize,
    /// Bytes uploaded for the current file.
    pub current_file_bytes: u64,
    /// Total bytes of the current file.
    pub current_file_total: u64,
}

/// Handle to an async upload operation with progress reporting and pause/cancel.
pub struct VfsUploadOperation {
    /// Channel receiver for the operation result.
    completion: mpsc::Receiver<VfsResult<()>>,
    /// Channel receiver for progress updates.
    progress: mpsc::Receiver<UploadProgress>,
    /// Flag to pause the operation.
    pub pause_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Flag to cancel the operation.
    pub cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl VfsUploadOperation {
    /// Create a new upload operation from receivers.
    pub fn new(
        completion: mpsc::Receiver<VfsResult<()>>,
        progress: mpsc::Receiver<UploadProgress>,
        pause_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
        cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            completion,
            progress,
            pause_flag,
            cancel_flag,
        }
    }

    /// Create an error operation.
    pub fn error(error: VfsError) -> Self {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(Err(error));
        let (_, progress_rx) = mpsc::channel();
        Self {
            completion: rx,
            progress: progress_rx,
            pause_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cancel_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Try to receive the result without blocking.
    pub fn try_recv(&self) -> Option<VfsResult<()>> {
        self.completion.try_recv().ok()
    }

    /// Try to receive progress update without blocking.
    pub fn try_recv_progress(&self) -> Option<UploadProgress> {
        self.progress.try_recv().ok()
    }

    /// Drain all pending progress updates and return the latest.
    pub fn drain_progress(&self) -> Option<UploadProgress> {
        let mut latest = None;
        while let Ok(p) = self.progress.try_recv() {
            latest = Some(p);
        }
        latest
    }

    /// Set pause state.
    pub fn set_paused(&self, paused: bool) {
        self.pause_flag
            .store(paused, std::sync::atomic::Ordering::Relaxed);
    }

    /// Check if paused.
    pub fn is_paused(&self) -> bool {
        self.pause_flag.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Cancel the operation.
    pub fn cancel(&self) {
        self.cancel_flag
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl std::fmt::Debug for VfsUploadOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VfsUploadOperation")
            .field("paused", &self.is_paused())
            .finish_non_exhaustive()
    }
}

/// Progress information for download operations.
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    /// Total bytes downloaded so far (across all files).
    pub bytes_downloaded: u64,
    /// Total bytes to download (all files).
    pub total_bytes: u64,
    /// Current file being downloaded (for directory downloads).
    pub current_file: Option<String>,
    /// Files downloaded so far (for directory downloads).
    pub files_downloaded: usize,
    /// Total files to download (for directory downloads).
    pub total_files: usize,
    /// Bytes downloaded for the current file.
    pub current_file_bytes: u64,
    /// Total bytes of the current file.
    pub current_file_total: u64,
}

/// Handle to an async download operation with progress reporting and pause/cancel.
pub struct VfsDownloadOperation {
    /// Channel receiver for the operation result.
    completion: mpsc::Receiver<VfsResult<PathBuf>>,
    /// Channel receiver for progress updates.
    progress: mpsc::Receiver<DownloadProgress>,
    /// Flag to pause the operation.
    pub pause_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Flag to cancel the operation.
    pub cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl VfsDownloadOperation {
    /// Create a new download operation from receivers.
    pub fn new(
        completion: mpsc::Receiver<VfsResult<PathBuf>>,
        progress: mpsc::Receiver<DownloadProgress>,
        pause_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
        cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            completion,
            progress,
            pause_flag,
            cancel_flag,
        }
    }

    /// Create an error operation.
    pub fn error(error: VfsError) -> Self {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(Err(error));
        let (_, progress_rx) = mpsc::channel();
        Self {
            completion: rx,
            progress: progress_rx,
            pause_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cancel_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Try to receive the result without blocking.
    pub fn try_recv(&self) -> Option<VfsResult<PathBuf>> {
        self.completion.try_recv().ok()
    }

    /// Try to receive progress update without blocking.
    pub fn try_recv_progress(&self) -> Option<DownloadProgress> {
        self.progress.try_recv().ok()
    }

    /// Drain all pending progress updates and return the latest.
    pub fn drain_progress(&self) -> Option<DownloadProgress> {
        let mut latest = None;
        while let Ok(p) = self.progress.try_recv() {
            latest = Some(p);
        }
        latest
    }

    /// Set pause state.
    pub fn set_paused(&self, paused: bool) {
        self.pause_flag
            .store(paused, std::sync::atomic::Ordering::Relaxed);
    }

    /// Check if paused.
    pub fn is_paused(&self) -> bool {
        self.pause_flag.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Cancel the operation.
    pub fn cancel(&self) {
        self.cancel_flag
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl std::fmt::Debug for VfsDownloadOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VfsDownloadOperation")
            .field("paused", &self.is_paused())
            .finish_non_exhaustive()
    }
}

/// Progress information for copy operations.
#[derive(Debug, Clone)]
pub struct CopyProgress {
    /// Bytes copied so far.
    pub bytes_copied: u64,
    /// Total bytes to copy.
    pub total_bytes: u64,
    /// Files copied so far (for directory copies).
    pub files_copied: usize,
    /// Total files to copy (for directory copies).
    pub total_files: usize,
    /// Current file being copied.
    pub current_file: Option<String>,
}

/// Handle to an async copy operation with progress reporting and pause/cancel.
pub struct VfsCopyOperation {
    /// Channel receiver for the operation result.
    completion: mpsc::Receiver<VfsResult<()>>,
    /// Channel receiver for progress updates.
    progress: mpsc::Receiver<CopyProgress>,
    /// Flag to pause the operation.
    pub pause_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Flag to cancel the operation.
    pub cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl VfsCopyOperation {
    /// Create a new copy operation from receivers.
    pub fn new(
        completion: mpsc::Receiver<VfsResult<()>>,
        progress: mpsc::Receiver<CopyProgress>,
        pause_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
        cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            completion,
            progress,
            pause_flag,
            cancel_flag,
        }
    }

    /// Create an error operation.
    pub fn error(error: VfsError) -> Self {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(Err(error));
        let (_, progress_rx) = mpsc::channel();
        Self {
            completion: rx,
            progress: progress_rx,
            pause_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cancel_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Try to receive the result without blocking.
    pub fn try_recv(&self) -> Option<VfsResult<()>> {
        self.completion.try_recv().ok()
    }

    /// Try to receive progress update without blocking.
    pub fn try_recv_progress(&self) -> Option<CopyProgress> {
        self.progress.try_recv().ok()
    }

    /// Drain all pending progress updates and return the latest.
    pub fn drain_progress(&self) -> Option<CopyProgress> {
        let mut latest = None;
        while let Ok(p) = self.progress.try_recv() {
            latest = Some(p);
        }
        latest
    }

    /// Set pause state.
    pub fn set_paused(&self, paused: bool) {
        self.pause_flag
            .store(paused, std::sync::atomic::Ordering::Relaxed);
    }

    /// Check if paused.
    pub fn is_paused(&self) -> bool {
        self.pause_flag.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Cancel the operation.
    pub fn cancel(&self) {
        self.cancel_flag
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl std::fmt::Debug for VfsCopyOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VfsCopyOperation")
            .field("paused", &self.is_paused())
            .finish_non_exhaustive()
    }
}

/// Connection state for a VFS provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Not connected.
    Disconnected,
    /// Connection in progress.
    Connecting,
    /// Connected and ready.
    Connected,
    /// Connection failed.
    Failed,
}

/// Authentication method for remote connections.
///
/// `Debug` is implemented by hand so passwords and passphrases never reach
/// a log line.
#[derive(Clone, Default)]
pub enum AuthMethod {
    /// No authentication.
    None,
    /// Password authentication.
    Password(String),
    /// SSH key authentication.
    SshKey {
        /// Path to private key file.
        private_key: PathBuf,
        /// Passphrase for encrypted key.
        passphrase: Option<String>,
    },
    /// SSH agent authentication.
    SshAgent,
    /// Try multiple methods in order.
    #[default]
    Auto,
}

impl std::fmt::Debug for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Password(_) => f.debug_tuple("Password").field(&"***").finish(),
            Self::SshKey {
                private_key,
                passphrase,
            } => f
                .debug_struct("SshKey")
                .field("private_key", private_key)
                .field("passphrase", &passphrase.as_ref().map(|_| "***"))
                .finish(),
            Self::SshAgent => f.write_str("SshAgent"),
            Self::Auto => f.write_str("Auto"),
        }
    }
}

/// Options for connecting to a remote filesystem.
#[derive(Debug, Clone, Default)]
pub struct ConnectOptions {
    /// Authentication method.
    pub auth: AuthMethod,
    /// Connection timeout in seconds.
    pub timeout_secs: Option<u64>,
    /// Whether to verify host key (SSH).
    pub verify_host: bool,
}

impl ConnectOptions {
    /// Create options with password authentication.
    pub fn with_password(password: impl Into<String>) -> Self {
        Self {
            auth: AuthMethod::Password(password.into()),
            ..Default::default()
        }
    }

    /// Create options with SSH key authentication.
    pub fn with_ssh_key(key_path: PathBuf, passphrase: Option<String>) -> Self {
        Self {
            auth: AuthMethod::SshKey {
                private_key: key_path,
                passphrase,
            },
            ..Default::default()
        }
    }

    /// Set connection timeout.
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = Some(secs);
        self
    }
}

#[cfg(test)]
mod auth_debug_tests {
    use super::*;

    #[test]
    fn debug_hides_password_and_passphrase() {
        let opts = ConnectOptions::with_password("hunter2");
        let shown = format!("{opts:?}");
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(shown.contains("***"));

        let key = ConnectOptions::with_ssh_key("/k".into(), Some("s3cret".into()));
        let shown = format!("{key:?}");
        assert!(!shown.contains("s3cret"), "{shown}");
        assert!(shown.contains("/k"));
    }
}
