//! FTP (File Transfer Protocol) VFS provider.
//!
//! Uses the `suppaftp` crate for FTP connectivity.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::SystemTime;

use suppaftp::rustls::{ClientConfig, RootCertStore};
use suppaftp::{RustlsConnector, RustlsFtpStream};

use crate::error::{VfsError, VfsResult};
use crate::local_part::LocalPart;
use crate::traits::VfsProvider;
use crate::types::{
    AuthMethod, ConnectOptions, ConnectionState, DownloadProgress, UploadProgress,
    VfsDownloadOperation, VfsEntry, VfsFileType, VfsMetadata, VfsOperation, VfsPath, VfsProtocol,
    VfsUploadOperation,
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
        let file_type = if perms.starts_with('d') {
            VfsFileType::Directory
        } else if perms.starts_with('l') {
            VfsFileType::Symlink
        } else {
            VfsFileType::File
        };
        let name = list_line_name(line, file_type)?;

        // Skip . and ..
        if name == "." || name == ".." {
            return None;
        }

        let path = parent_path.join(name);

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

    /// Download the file `remote` to `local` on a thread of its own: a
    /// piece at a time, with progress, pause and cancel, into a sibling of
    /// `local` that takes its place once whole.
    fn start_download(&self, remote: &VfsPath, local: &Path) -> Started<PathBuf, DownloadProgress> {
        let (done_tx, done) = std::sync::mpsc::channel();
        let (progress_tx, progress) = std::sync::mpsc::channel();
        let pause = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let target = self
            .get_stream()
            .and_then(|stream| Ok((stream, Self::to_remote_path(remote)?)));
        let (local, flags) = (local.to_path_buf(), (pause.clone(), cancel.clone()));
        thread::spawn(move || {
            let result = target.and_then(|(stream, remote)| {
                let mut ftp = lock_ftp(&stream)?;
                download_file(&mut ftp, &remote, &local, &flags.0, &flags.1, &progress_tx)
                    .map(|()| local)
            });
            let _ = done_tx.send(result);
        });
        Started {
            done,
            progress,
            pause,
            cancel,
        }
    }

    /// Upload the file `local` to `remote` on a thread of its own, a piece
    /// at a time, with progress, pause and cancel.
    fn start_upload(&self, local: &Path, remote: &VfsPath) -> Started<(), UploadProgress> {
        let (done_tx, done) = std::sync::mpsc::channel();
        let (progress_tx, progress) = std::sync::mpsc::channel();
        let pause = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let target = self
            .get_stream()
            .and_then(|stream| Ok((stream, Self::to_remote_path(remote)?)));
        let (local, flags) = (local.to_path_buf(), (pause.clone(), cancel.clone()));
        thread::spawn(move || {
            let result = target.and_then(|(stream, remote)| {
                let mut ftp = lock_ftp(&stream)?;
                upload_file(&mut ftp, &local, &remote, &flags.0, &flags.1, &progress_tx)
            });
            let _ = done_tx.send(result);
        });
        Started {
            done,
            progress,
            pause,
            cancel,
        }
    }
}

/// Size of the pieces a transfer moves between checks for pause and cancel.
const PIECE: usize = 256 * 1024;

/// A transfer started on its thread.
struct Started<T, P> {
    done: Receiver<VfsResult<T>>,
    progress: Receiver<P>,
    pause: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

/// Copy `from` into `to` a piece at a time, waiting out a pause and
/// stopping with `Cancelled` on a cancel between pieces; `moved` hears of
/// each piece through.
fn pump(
    from: &mut dyn Read,
    to: &mut dyn Write,
    pause: &AtomicBool,
    cancel: &AtomicBool,
    mut moved: impl FnMut(u64),
) -> VfsResult<()> {
    let mut buf = vec![0; PIECE];
    loop {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(VfsError::Cancelled);
            }
            if !pause.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(50));
        }
        let n = match from.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(VfsError::Io(e)),
        };
        if n == 0 {
            to.flush()?;
            return Ok(());
        }
        to.write_all(&buf[..n])?;
        moved(n as u64);
    }
}

fn download_file(
    ftp: &mut RustlsFtpStream,
    remote: &str,
    local: &Path,
    pause: &AtomicBool,
    cancel: &AtomicBool,
    progress: &Sender<DownloadProgress>,
) -> VfsResult<()> {
    let total = ftp.size(remote).map(|s| s as u64).unwrap_or(0);
    let mut report = DownloadProgress {
        bytes_downloaded: 0,
        total_bytes: total,
        current_file: Path::new(remote)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned()),
        files_downloaded: 0,
        total_files: 1,
        current_file_bytes: 0,
        current_file_total: total,
    };
    let _ = progress.send(report.clone());
    let mut part = LocalPart::create(local)?;
    let mut data = ftp
        .retr_as_stream(remote)
        .map_err(|e| VfsError::Ftp(format!("retr {remote}: {e}")))?;
    let pumped = pump(&mut data, &mut part, pause, cancel, |n| {
        report.bytes_downloaded += n;
        report.current_file_bytes += n;
        let _ = progress.send(report.clone());
    });
    match pumped {
        Ok(()) => ftp
            .finalize_retr_stream(data)
            .map_err(|e| VfsError::Ftp(format!("retr {remote}: {e}")))?,
        Err(e) => {
            // The part goes with `part`; the control channel is told.
            let _ = ftp.abort(data);
            return Err(e);
        }
    }
    part.persist(local)?;
    report.files_downloaded = 1;
    let _ = progress.send(report);
    Ok(())
}

/// Upload `local` into `remote`. FTP's STOR writes the file in place, so
/// a cancel or a cut connection leaves what got there.
fn upload_file(
    ftp: &mut RustlsFtpStream,
    local: &Path,
    remote: &str,
    pause: &AtomicBool,
    cancel: &AtomicBool,
    progress: &Sender<UploadProgress>,
) -> VfsResult<()> {
    let mut source = std::fs::File::open(local)?;
    let total = source.metadata().map(|m| m.len()).unwrap_or(0);
    let mut report = UploadProgress {
        bytes_uploaded: 0,
        total_bytes: total,
        current_file: local.file_name().map(|n| n.to_string_lossy().into_owned()),
        files_uploaded: 0,
        total_files: 1,
        current_file_bytes: 0,
        current_file_total: total,
    };
    let _ = progress.send(report.clone());
    let mut data = ftp
        .put_with_stream(remote)
        .map_err(|e| VfsError::Ftp(format!("stor {remote}: {e}")))?;
    let pumped = pump(&mut source, &mut data, pause, cancel, |n| {
        report.bytes_uploaded += n;
        report.current_file_bytes += n;
        let _ = progress.send(report.clone());
    });
    // Closing the data channel ends the upload either way; the reply is
    // what tells a whole one.
    let finished = ftp
        .finalize_put_stream(data)
        .map_err(|e| VfsError::Ftp(format!("stor {remote}: {e}")));
    pumped?;
    finished?;
    report.files_uploaded = 1;
    let _ = progress.send(report);
    Ok(())
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

    /// FTP has no lstat: `SIZE` and `CWD` both follow a link. The parent's
    /// listing marks a link with `l`, so a link found there is reported as
    /// one; anything else falls back to `metadata`.
    fn symlink_metadata(&self, path: &VfsPath) -> VfsOperation<VfsMetadata> {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return self.metadata(path);
        };
        let name = name.to_string_lossy().into_owned();
        let listing = self.list_dir(&parent);
        let followed = self.metadata(path);

        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let link = listing.recv().ok().and_then(|entries| {
                entries.into_iter().find(|entry| {
                    entry.name == name && entry.metadata.file_type == VfsFileType::Symlink
                })
            });
            let result = match link {
                Some(entry) => Ok(entry.metadata),
                None => followed.recv(),
            };
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

    /// FTP has no exclusive create (`STOU` picks a name of its own), so the
    /// name is looked up first — the parent's listing too, which shows a
    /// dangling link — and written only when free. A name another client
    /// takes between the two is still replaced.
    fn create_file(&self, path: &VfsPath) -> VfsOperation<()> {
        let stream = match self.get_stream() {
            Ok(s) => s,
            Err(e) => return VfsOperation::error(e),
        };
        let remote_path = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::error(e),
        };
        let lookup = self.symlink_metadata(path);
        let shown = path.path.clone();

        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let result = (|| -> VfsResult<()> {
                match lookup.recv() {
                    Ok(_) => return Err(VfsError::AlreadyExists { path: shown }),
                    Err(VfsError::NotFound { .. }) => {}
                    Err(e) => return Err(e),
                }
                let mut ftp = lock_ftp(&stream)?;
                ftp.put_file(&remote_path, &mut std::io::empty())
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
        VfsOperation::new(self.start_download(remote, local).done)
    }

    fn download_with_progress(&self, remote: &VfsPath, local: &Path) -> VfsDownloadOperation {
        let started = self.start_download(remote, local);
        VfsDownloadOperation::new(
            started.done,
            started.progress,
            started.pause,
            started.cancel,
        )
    }

    fn upload(&self, local: &Path, remote: &VfsPath) -> VfsOperation<()> {
        VfsOperation::new(self.start_upload(local, remote).done)
    }

    fn upload_with_progress(&self, local: &Path, remote: &VfsPath) -> VfsUploadOperation {
        let started = self.start_upload(local, remote);
        VfsUploadOperation::new(
            started.done,
            started.progress,
            started.pause,
            started.cancel,
        )
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
}

/// The name in a Unix-style `LIST` line: everything after the eighth field,
/// spaces inside it kept as they are. A symlink's line ends in
/// `name -> target`; the target is not part of the name.
fn list_line_name(line: &str, file_type: VfsFileType) -> Option<&str> {
    let mut rest = line.trim_end_matches(['\r', '\n']);
    for _ in 0..8 {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace)?;
        rest = &rest[end..];
    }
    let mut name = rest.trim_start();
    if file_type == VfsFileType::Symlink {
        if let Some((link, _target)) = name.split_once(" -> ") {
            name = link;
        }
    }
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_line_name_keeps_inner_spaces() {
        assert_eq!(
            list_line_name(
                "-rw-r--r--  1 user group 12 Jan 01 12:00 two  spaces.txt\r",
                VfsFileType::File
            ),
            Some("two  spaces.txt")
        );
    }

    #[test]
    fn list_line_name_drops_a_symlink_target() {
        let line = "lrwxrwxrwx  1 user group 4 Jan 01 12:00 www -> /var/www";
        assert_eq!(list_line_name(line, VfsFileType::Symlink), Some("www"));
        let provider = FtpProvider::new("", None, None, false);
        let parent = VfsPath::remote(VfsProtocol::Ftp, "host", "/home");
        let entry = provider.parse_list_line(line, &parent).unwrap();
        assert_eq!(entry.name, "www");
        assert_eq!(entry.metadata.file_type, VfsFileType::Symlink);
        assert_eq!(entry.path, parent.join("www"));
    }

    #[test]
    fn list_line_name_needs_nine_fields() {
        assert_eq!(
            list_line_name("-rw-r--r-- 1 user group 12 Jan 01 12:00", VfsFileType::File),
            None
        );
    }

    #[test]
    fn a_transfer_moves_in_pieces_and_stops_on_a_cancel() {
        let data = vec![7u8; PIECE * 2 + 10];
        let (pause, cancel) = (AtomicBool::new(false), AtomicBool::new(false));
        let mut out = Vec::new();
        let mut pieces = Vec::new();
        pump(&mut &data[..], &mut out, &pause, &cancel, |n| {
            pieces.push(n)
        })
        .unwrap();
        assert_eq!(out, data);
        assert_eq!(pieces.iter().sum::<u64>(), data.len() as u64);
        assert!(pieces.len() >= 3, "{pieces:?}");

        // A cancel during the transfer stops it after the piece in hand.
        let mut out = Vec::new();
        let result = pump(&mut &data[..], &mut out, &pause, &cancel, |_| {
            cancel.store(true, Ordering::Relaxed)
        });
        assert!(matches!(result, Err(VfsError::Cancelled)), "{result:?}");
        assert_eq!(out.len(), PIECE);
    }

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
