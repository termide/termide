//! FTP (File Transfer Protocol) VFS provider.
//!
//! Uses the `suppaftp` crate for FTP connectivity.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::SystemTime;

use suppaftp::rustls::{ClientConfig, RootCertStore};
use suppaftp::{RustlsConnector, RustlsFtpStream};

use crate::error::{VfsError, VfsResult};
use crate::traits::{DiskSpace, VfsProvider};
use crate::types::{
    AuthMethod, ConnectOptions, ConnectionState, VfsEntry, VfsFileType, VfsMetadata, VfsOperation,
    VfsPath, VfsProtocol,
};

/// Default FTP port.
const DEFAULT_PORT: u16 = 21;

/// Acquire FTP stream mutex lock, converting poison error to VfsError.
fn lock_ftp(
    stream: &Arc<Mutex<RustlsFtpStream>>,
) -> VfsResult<std::sync::MutexGuard<'_, RustlsFtpStream>> {
    stream.lock().map_err(|e| VfsError::RemoteError {
        message: format!("Failed to acquire FTP stream lock: {e}"),
    })
}

/// FTP filesystem provider.
pub struct FtpProvider {
    /// FTP host.
    host: String,
    /// FTP port.
    port: u16,
    /// Username.
    username: Option<String>,
    /// Password (stored in memory for reconnection).
    password: Option<String>,
    /// Whether to use TLS (FTPS).
    use_tls: bool,
    /// Current connection state.
    state: ConnectionState,
    /// FTP stream (shared for thread safety).
    stream: Option<Arc<Mutex<RustlsFtpStream>>>,
}

impl FtpProvider {
    /// Create a new FTP provider.
    pub fn new(host: &str, port: Option<u16>, username: Option<&str>, use_tls: bool) -> Self {
        Self {
            host: host.to_string(),
            port: port.unwrap_or(if use_tls { 990 } else { DEFAULT_PORT }),
            username: username.map(String::from),
            password: None,
            use_tls,
            state: ConnectionState::Disconnected,
            stream: None,
        }
    }

    /// Convert VfsPath to remote path string.
    fn to_remote_path(path: &VfsPath) -> VfsResult<String> {
        if !matches!(path.protocol, VfsProtocol::Ftp | VfsProtocol::Ftps) {
            return Err(VfsError::InvalidPath(format!(
                "Expected FTP/FTPS path, got: {}",
                path
            )));
        }
        Ok(path.path.display().to_string())
    }

    /// Check if connected and return the stream.
    fn get_stream(&self) -> VfsResult<Arc<Mutex<RustlsFtpStream>>> {
        match &self.stream {
            Some(stream) => Ok(Arc::clone(stream)),
            None => Err(VfsError::NotConnected),
        }
    }

    /// Parse FTP LIST line into VfsEntry.
    fn parse_list_line(&self, line: &str, parent_path: &VfsPath) -> Option<VfsEntry> {
        // FTP LIST output varies by server, but common format is Unix-like:
        // drwxr-xr-x  2 user group  4096 Jan 01 12:00 dirname
        // -rw-r--r--  1 user group 12345 Jan 01 12:00 filename
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            return None;
        }

        let perms = parts[0];
        let size: u64 = parts[4].parse().unwrap_or(0);
        let name = parts[8..].join(" ");

        // Skip . and ..
        if name == "." || name == ".." {
            return None;
        }

        let file_type = if perms.starts_with('d') {
            VfsFileType::Directory
        } else if perms.starts_with('l') {
            VfsFileType::Symlink
        } else {
            VfsFileType::File
        };

        let path = parent_path.join(&name);

        let metadata = VfsMetadata {
            file_type,
            size,
            modified: None,
            created: None,
            accessed: None,
            readonly: false,
            permissions: None,
        };

        Some(VfsEntry::new(name, path, metadata))
    }

    /// Extract password from ConnectOptions.
    fn extract_password(options: &ConnectOptions) -> String {
        match &options.auth {
            AuthMethod::Password(pwd) => pwd.clone(),
            AuthMethod::None => "anonymous@".to_string(),
            _ => "anonymous@".to_string(),
        }
    }
}

impl Drop for FtpProvider {
    fn drop(&mut self) {
        // Zero out password bytes in memory before deallocation
        if let Some(ref mut pw) = self.password {
            // SAFETY: zeroing owned String bytes that are valid for pw.len()
            unsafe {
                std::ptr::write_bytes(pw.as_mut_vec().as_mut_ptr(), 0, pw.len());
            }
        }
    }
}

impl VfsProvider for FtpProvider {
    fn name(&self) -> &'static str {
        if self.use_tls {
            "ftps"
        } else {
            "ftp"
        }
    }

    fn connection_state(&self) -> ConnectionState {
        self.state
    }

    fn connect(&mut self, options: ConnectOptions) -> VfsOperation<()> {
        let host = self.host.clone();
        let port = self.port;
        let use_tls = self.use_tls;
        let username = self
            .username
            .clone()
            .unwrap_or_else(|| "anonymous".to_string());
        let password = Self::extract_password(&options);

        // Store password for potential reconnection
        self.password = Some(password.clone());

        let (tx, rx) = std::sync::mpsc::channel();

        thread::spawn(move || {
            let result = (|| -> VfsResult<RustlsFtpStream> {
                // Connect to FTP server
                let addr = format!("{}:{}", host, port);
                log::info!(
                    "FTP: Connecting to {} (TLS: {})",
                    addr,
                    if use_tls { "yes" } else { "no" }
                );

                let mut stream = RustlsFtpStream::connect(&addr).map_err(|e| {
                    VfsError::ConnectionFailed(format!("FTP connection failed: {}", e))
                })?;

                // Upgrade to TLS if requested (explicit FTPS / STARTTLS).
                // Rustls + Mozilla CA bundle from webpki-roots — no system OpenSSL needed.
                if use_tls {
                    let root_store =
                        RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                    let config = ClientConfig::builder()
                        .with_root_certificates(root_store)
                        .with_no_client_auth();
                    let tls_connector = RustlsConnector::from(std::sync::Arc::new(config));
                    stream = stream.into_secure(tls_connector, &host).map_err(|e| {
                        VfsError::ConnectionFailed(format!("TLS upgrade failed: {}", e))
                    })?;
                    log::info!("FTP: TLS connection established to {}", addr);
                }

                // Login
                stream.login(&username, &password).map_err(|e| {
                    VfsError::AuthenticationFailed(format!("FTP login failed: {}", e))
                })?;

                // Set binary transfer mode
                stream.transfer_type(suppaftp::types::FileType::Binary).ok();

                log::info!("FTP: Connected and logged in to {}", addr);
                Ok(stream)
            })();

            let _ = tx.send(result);
        });

        // Wait for connection result
        match rx.recv() {
            Ok(Ok(stream)) => {
                self.stream = Some(Arc::new(Mutex::new(stream)));
                self.state = ConnectionState::Connected;
                VfsOperation::ready(Ok(()))
            }
            Ok(Err(e)) => {
                self.state = ConnectionState::Failed;
                VfsOperation::ready(Err(e))
            }
            Err(_) => {
                self.state = ConnectionState::Failed;
                VfsOperation::error(VfsError::RemoteError {
                    message: "Connection thread failed".to_string(),
                })
            }
        }
    }

    fn disconnect(&mut self) {
        if let Some(stream) = self.stream.take() {
            if let Ok(mut ftp) = stream.lock() {
                let _ = ftp.quit();
            }
        }
        self.state = ConnectionState::Disconnected;
    }

    fn list_dir(&self, path: &VfsPath) -> VfsOperation<Vec<VfsEntry>> {
        let stream = match self.get_stream() {
            Ok(s) => s,
            Err(e) => return VfsOperation::error(e),
        };
        let remote_path = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::error(e),
        };
        let parent_path = path.clone();

        let (tx, rx) = std::sync::mpsc::channel();

        thread::spawn(move || {
            let result = (|| -> VfsResult<Vec<VfsEntry>> {
                let mut ftp = lock_ftp(&stream)?;

                // Change to directory
                ftp.cwd(&remote_path)
                    .map_err(|e| VfsError::Ftp(format!("cwd to {remote_path}: {e}")))?;

                // Get directory listing
                let listing = ftp.list(None).map_err(|e| VfsError::RemoteError {
                    message: format!("Failed to list directory: {}", e),
                })?;

                // Parse listing into entries
                let provider = FtpProvider::new("", None, None, false);
                let entries: Vec<VfsEntry> = listing
                    .iter()
                    .filter_map(|line| provider.parse_list_line(line, &parent_path))
                    .collect();

                Ok(entries)
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn create_dir(&self, path: &VfsPath) -> VfsOperation<()> {
        let stream = match self.get_stream() {
            Ok(s) => s,
            Err(e) => return VfsOperation::error(e),
        };
        let remote_path = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = std::sync::mpsc::channel();

        thread::spawn(move || {
            let result = (|| -> VfsResult<()> {
                let mut ftp = lock_ftp(&stream)?;

                ftp.mkdir(&remote_path).map_err(|e| VfsError::RemoteError {
                    message: format!("Failed to create directory: {}", e),
                })?;

                Ok(())
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn create_dir_all(&self, path: &VfsPath) -> VfsOperation<()> {
        // FTP doesn't have mkdir -p, so we need to create each level
        // For simplicity, just try to create the final directory
        self.create_dir(path)
    }

    fn exists(&self, path: &VfsPath) -> VfsOperation<bool> {
        let stream = match self.get_stream() {
            Ok(s) => s,
            Err(e) => return VfsOperation::error(e),
        };
        let remote_path = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = std::sync::mpsc::channel();

        thread::spawn(move || {
            let result = (|| -> VfsResult<bool> {
                let mut ftp = lock_ftp(&stream)?;

                // Try to get size - if it works, file exists
                if ftp.size(&remote_path).is_ok() {
                    return Ok(true);
                }

                // Try to CWD - if it works, directory exists
                let cwd = ftp.pwd().ok();
                if ftp.cwd(&remote_path).is_ok() {
                    // Restore original directory
                    if let Some(dir) = cwd {
                        let _ = ftp.cwd(&dir);
                    }
                    return Ok(true);
                }

                Ok(false)
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn metadata(&self, path: &VfsPath) -> VfsOperation<VfsMetadata> {
        let stream = match self.get_stream() {
            Ok(s) => s,
            Err(e) => return VfsOperation::error(e),
        };
        let remote_path = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = std::sync::mpsc::channel();

        thread::spawn(move || {
            let result = (|| -> VfsResult<VfsMetadata> {
                let mut ftp = lock_ftp(&stream)?;

                // Try to get file size first
                if let Ok(size) = ftp.size(&remote_path) {
                    // Try to get modification time
                    let modified = ftp.mdtm(&remote_path).ok().map(|dt| {
                        SystemTime::UNIX_EPOCH
                            + std::time::Duration::from_secs(dt.and_utc().timestamp() as u64)
                    });

                    return Ok(VfsMetadata {
                        file_type: VfsFileType::File,
                        size: size as u64,
                        modified,
                        created: None,
                        accessed: None,
                        readonly: false,
                        permissions: None,
                    });
                }

                // Check if it's a directory
                let cwd = ftp.pwd().ok();
                if ftp.cwd(&remote_path).is_ok() {
                    // Restore original directory
                    if let Some(dir) = cwd {
                        let _ = ftp.cwd(&dir);
                    }
                    return Ok(VfsMetadata::directory());
                }

                Err(VfsError::NotFound {
                    path: PathBuf::from(&remote_path),
                })
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn read_file(&self, path: &VfsPath) -> VfsOperation<Vec<u8>> {
        let stream = match self.get_stream() {
            Ok(s) => s,
            Err(e) => return VfsOperation::error(e),
        };
        let remote_path = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = std::sync::mpsc::channel();

        thread::spawn(move || {
            let result = (|| -> VfsResult<Vec<u8>> {
                let mut ftp = lock_ftp(&stream)?;

                let mut data = Vec::new();
                let mut reader = ftp
                    .retr_as_buffer(&remote_path)
                    .map_err(|e| VfsError::Ftp(format!("retr {remote_path}: {e}")))?;

                reader.read_to_end(&mut data).map_err(VfsError::Io)?;

                Ok(data)
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn write_file(&self, path: &VfsPath, data: &[u8]) -> VfsOperation<()> {
        let stream = match self.get_stream() {
            Ok(s) => s,
            Err(e) => return VfsOperation::error(e),
        };
        let remote_path = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::error(e),
        };
        let data = data.to_vec();

        let (tx, rx) = std::sync::mpsc::channel();

        thread::spawn(move || {
            let result = (|| -> VfsResult<()> {
                let mut ftp = lock_ftp(&stream)?;

                let mut reader = std::io::Cursor::new(data);
                ftp.put_file(&remote_path, &mut reader)
                    .map_err(|e| VfsError::RemoteError {
                        message: format!("Failed to upload file: {}", e),
                    })?;

                Ok(())
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn delete(&self, path: &VfsPath) -> VfsOperation<()> {
        let stream = match self.get_stream() {
            Ok(s) => s,
            Err(e) => return VfsOperation::error(e),
        };
        let remote_path = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = std::sync::mpsc::channel();

        thread::spawn(move || {
            let result = (|| -> VfsResult<()> {
                let mut ftp = lock_ftp(&stream)?;

                // Try to delete as file first
                if ftp.rm(&remote_path).is_ok() {
                    return Ok(());
                }

                // Try to delete as directory
                ftp.rmdir(&remote_path).map_err(|e| VfsError::RemoteError {
                    message: format!("Failed to delete: {}", e),
                })?;

                Ok(())
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn delete_recursive(&self, path: &VfsPath) -> VfsOperation<()> {
        // FTP doesn't support recursive delete natively
        // Would need to implement by listing and deleting each item
        self.delete(path)
    }

    fn rename(&self, from: &VfsPath, to: &VfsPath) -> VfsOperation<()> {
        let stream = match self.get_stream() {
            Ok(s) => s,
            Err(e) => return VfsOperation::error(e),
        };
        let from_path = match Self::to_remote_path(from) {
            Ok(p) => p,
            Err(e) => return VfsOperation::error(e),
        };
        let to_path = match Self::to_remote_path(to) {
            Ok(p) => p,
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = std::sync::mpsc::channel();

        thread::spawn(move || {
            let result = (|| -> VfsResult<()> {
                let mut ftp = lock_ftp(&stream)?;

                ftp.rename(&from_path, &to_path)
                    .map_err(|e| VfsError::RemoteError {
                        message: format!("Failed to rename: {}", e),
                    })?;

                Ok(())
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn copy(&self, from: &VfsPath, to: &VfsPath) -> VfsOperation<()> {
        // FTP doesn't support server-side copy
        // Would need to download and re-upload
        let _ = (from, to);
        VfsOperation::error(VfsError::NotSupported(
            "FTP does not support server-side copy".to_string(),
        ))
    }

    fn download(&self, remote: &VfsPath, local: &Path) -> VfsOperation<PathBuf> {
        let stream = match self.get_stream() {
            Ok(s) => s,
            Err(e) => return VfsOperation::error(e),
        };
        let remote_path = match Self::to_remote_path(remote) {
            Ok(p) => p,
            Err(e) => return VfsOperation::error(e),
        };
        let local_path = local.to_path_buf();

        let (tx, rx) = std::sync::mpsc::channel();

        thread::spawn(move || {
            let result = (|| -> VfsResult<PathBuf> {
                let mut ftp = lock_ftp(&stream)?;

                let mut data = Vec::new();
                let mut reader = ftp
                    .retr_as_buffer(&remote_path)
                    .map_err(|e| VfsError::Ftp(format!("retr {remote_path}: {e}")))?;

                reader.read_to_end(&mut data).map_err(VfsError::Io)?;

                // Write to local file
                let mut file = std::fs::File::create(&local_path).map_err(VfsError::Io)?;
                file.write_all(&data).map_err(VfsError::Io)?;

                Ok(local_path)
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn upload(&self, local: &Path, remote: &VfsPath) -> VfsOperation<()> {
        let stream = match self.get_stream() {
            Ok(s) => s,
            Err(e) => return VfsOperation::error(e),
        };
        let remote_path = match Self::to_remote_path(remote) {
            Ok(p) => p,
            Err(e) => return VfsOperation::error(e),
        };
        let local_path = local.to_path_buf();

        let (tx, rx) = std::sync::mpsc::channel();

        thread::spawn(move || {
            let result = (|| -> VfsResult<()> {
                // Read local file
                let data = std::fs::read(&local_path).map_err(VfsError::Io)?;

                let mut ftp = lock_ftp(&stream)?;

                let mut reader = std::io::Cursor::new(data);
                ftp.put_file(&remote_path, &mut reader)
                    .map_err(|e| VfsError::RemoteError {
                        message: format!("Failed to upload file: {}", e),
                    })?;

                Ok(())
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn supported_auth_methods(&self) -> Vec<AuthMethod> {
        vec![AuthMethod::Password(String::new()), AuthMethod::None]
    }

    fn supports_recursive(&self) -> bool {
        false
    }

    fn home_dir(&self) -> Option<VfsPath> {
        None
    }

    fn disk_space(&self, _path: &VfsPath) -> Option<DiskSpace> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ftp_provider_creation() {
        let provider = FtpProvider::new("ftp.example.com", None, Some("user"), false);
        assert_eq!(provider.name(), "ftp");
        assert_eq!(provider.connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_to_remote_path() {
        let ftp_path = VfsPath::remote(VfsProtocol::Ftp, "host", "/pub/file.txt");
        let result = FtpProvider::to_remote_path(&ftp_path);
        assert!(result.is_ok());

        let local_path = VfsPath::local("/local/path");
        let result = FtpProvider::to_remote_path(&local_path);
        assert!(result.is_err());
    }
}
