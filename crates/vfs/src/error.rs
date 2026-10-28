//! VFS error types.

use std::io;
use std::path::PathBuf;
use thiserror::Error;

/// Virtual filesystem error type.
#[derive(Debug, Error)]
pub enum VfsError {
    /// I/O error during file operation.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Path parsing or validation error.
    #[error("Invalid path: {0}")]
    InvalidPath(String),

    /// URL parsing error.
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    /// Unsupported protocol scheme.
    #[error("Unsupported protocol: {0}")]
    UnsupportedProtocol(String),

    /// Connection error.
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    /// Authentication error.
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    /// Operation timeout.
    #[error("Operation timed out: {0}")]
    Timeout(String),

    /// File or directory not found.
    #[error("Not found: {path}")]
    NotFound { path: PathBuf },

    /// Permission denied.
    #[error("Permission denied: {path}")]
    PermissionDenied { path: PathBuf },

    /// Already exists (for create operations).
    #[error("Already exists: {path}")]
    AlreadyExists { path: PathBuf },

    /// Directory is not empty (for delete operations).
    #[error("Directory not empty: {path}")]
    DirectoryNotEmpty { path: PathBuf },

    /// Operation not supported by this provider.
    #[error("Operation not supported: {0}")]
    NotSupported(String),

    /// Provider is not connected.
    #[error("Not connected")]
    NotConnected,

    /// Operation was cancelled by user.
    #[error("Operation cancelled")]
    Cancelled,

    /// The resource is encrypted and no password was given.
    #[error("Password required")]
    PasswordRequired,

    /// The password given for an encrypted resource was rejected.
    #[error("Wrong password")]
    WrongPassword,

    /// The SSH server offered a key that no `known_hosts` file records for
    /// the host. Nothing was sent to the server; trust the key with
    /// `VfsManager::trust_host_key` and connect again.
    #[error("Unknown host key for {}:{} ({} {})", .0.host, .0.port, .0.algorithm, .0.fingerprint)]
    UnknownHostKey(Box<crate::types::HostKey>),

    /// The SSH server offered a key that no `known_hosts` file records for
    /// the host, and `StrictHostKeyChecking yes` in the SSH configuration
    /// forbids asking. Nothing was sent to the server.
    #[error(
        "No host key for {} is known and StrictHostKeyChecking is yes ({} {})",
        .0.known_as, .0.algorithm, .0.fingerprint
    )]
    HostKeyNotRecorded(Box<crate::types::HostKey>),

    /// The SSH server offered a different key than the one recorded for the
    /// host: someone may be intercepting the connection. `known_as` is the
    /// name the host is recorded under; `recorded_in` is the `file:line` of
    /// the recorded key, `None` when it was accepted earlier in this run.
    #[error(
        "Host key for {host}:{port} does not match the one recorded in {}",
        recorded_in.as_deref().unwrap_or("this session")
    )]
    HostKeyChanged {
        host: String,
        port: u16,
        known_as: String,
        recorded_in: Option<String>,
    },

    /// The SSH server offered a key marked `@revoked` at `location`.
    #[error("Host key for {host}:{port} is revoked in {location}")]
    HostKeyRevoked {
        host: String,
        port: u16,
        known_as: String,
        location: String,
    },

    /// Provider is already connected.
    #[error("Already connected")]
    AlreadyConnected,

    /// Remote operation error with provider-specific details.
    #[error("Remote error: {message}")]
    RemoteError { message: String },

    /// SFTP-specific error.
    #[cfg(feature = "sftp")]
    #[error("SFTP error: {0}")]
    Sftp(String),

    /// FTP-specific error.
    #[cfg(feature = "ftp")]
    #[error("FTP error: {0}")]
    Ftp(String),

    /// SMB-specific error.
    #[cfg(feature = "smb")]
    #[error("SMB error: {0}")]
    Smb(String),

    /// NFS/FUSE mount error.
    #[cfg(feature = "nfs")]
    #[error("NFS mount error: {0}")]
    NfsMount(String),

    /// An archive could not be read: corrupt, truncated, encrypted or using
    /// an unsupported compression method.
    #[cfg(feature = "archive")]
    #[error("Archive error: {0}")]
    Archive(String),
}

impl VfsError {
    /// True when an archive could not be read (corrupt, truncated,
    /// encrypted, unsupported), as opposed to a connection problem.
    pub fn is_archive_error(&self) -> bool {
        #[cfg(feature = "archive")]
        return matches!(self, VfsError::Archive(_));
        #[cfg(not(feature = "archive"))]
        false
    }

    /// True when the error means the remote *session* is gone (timed out,
    /// reset, closed) rather than a benign per-operation failure like
    /// permission-denied or not-found. Callers use this to offer a reconnect
    /// instead of just reporting — a closed session and a benign per-file
    /// error both surface as a protocol error (`Sftp`/`Ftp`/`Smb`), so the
    /// distinction is the wording / IO kind.
    pub fn is_connection_lost(&self) -> bool {
        // A truncated archive reports "unexpected EOF", which is about the
        // file, not a session.
        #[cfg(feature = "archive")]
        if matches!(self, VfsError::Archive(_)) {
            return false;
        }
        if matches!(self, VfsError::ConnectionFailed(_) | VfsError::Timeout(_)) {
            return true;
        }
        if let VfsError::Io(e) = self {
            use std::io::ErrorKind::*;
            if matches!(
                e.kind(),
                BrokenPipe | ConnectionReset | ConnectionAborted | NotConnected | UnexpectedEof
            ) {
                return true;
            }
        }
        let msg = self.to_string().to_lowercase();
        [
            "session closed",
            "session is closed",
            "channel closed",
            "connection closed",
            "connection reset",
            "disconnect",
            "broken pipe",
            "not connected",
            "unexpected eof",
        ]
        .iter()
        .any(|needle| msg.contains(needle))
    }
}

/// Alias for VFS operation results.
pub type VfsResult<T> = Result<T, VfsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_lost_distinguishes_session_death_from_benign_errors() {
        // Connection-class variants and broken-pipe IO are session death.
        assert!(VfsError::ConnectionFailed("x".into()).is_connection_lost());
        assert!(VfsError::Timeout("x".into()).is_connection_lost());
        assert!(VfsError::Io(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "broken pipe"
        ))
        .is_connection_lost());
        // Message wording flags a closed session even on a generic error.
        assert!(VfsError::RemoteError {
            message: "session closed".into()
        }
        .is_connection_lost());

        // Benign per-operation failures must NOT offer a reconnect.
        assert!(!VfsError::NotFound {
            path: std::path::PathBuf::from("/x")
        }
        .is_connection_lost());
        assert!(!VfsError::PermissionDenied {
            path: std::path::PathBuf::from("/x")
        }
        .is_connection_lost());
        assert!(!VfsError::RemoteError {
            message: "no such file".into()
        }
        .is_connection_lost());
    }
}
