//! SFTP (SSH File Transfer Protocol) VFS provider.
//!
//! Uses `russh` + `russh-sftp` (pure-Rust SSH stack) so the workspace can
//! build statically against musl without pulling OpenSSL/libssh2.
//!
//! Internally an async tokio actor task owns the `SftpSession`. The
//! synchronous `VfsProvider` surface communicates with it through
//! `mpsc<Command>` + `oneshot<Reply>`, blocking the calling thread on a
//! global tokio runtime created lazily through `OnceLock`. From the
//! outside, callers see the same blocking API as before.
//!
//! Phase 2a scope: connect/disconnect, password/ssh-key/agent/auto auth,
//! basic file operations, recursive delete. Progress reporting and
//! pause/resume for download/upload are stubbed to plain transfer in
//! this phase — added in subsequent phases.
//!
//! NOTE: known_hosts verification is intentionally not enforced yet
//! (accept-all server keys) — matches previous ssh2 behavior. Hardening
//! this is a separate task.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use tokio::runtime::Runtime;
use tokio::sync::mpsc as async_mpsc;

use crate::error::{VfsError, VfsResult};
use crate::traits::{DiskSpace, VfsProvider};
use crate::types::{
    AuthMethod, ConnectOptions, ConnectionState, DownloadProgress, UploadProgress,
    VfsDownloadOperation, VfsEntry, VfsFileType, VfsMetadata, VfsOperation, VfsPath, VfsProtocol,
    VfsUploadOperation,
};

mod actor;
mod connect;
mod transfer;

use actor::{sftp_actor, Reply, SftpCommand, SftpHandle};
use connect::{do_connect, fallback_username};
use transfer::{
    count_local_files_sync, worker_count_remote, worker_download_dir, worker_download_file,
    worker_upload_dir, worker_upload_file, DlState, UlState,
};

/// Default connection timeout in seconds (matches the previous ssh2-based impl).
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Chunk size for chunked I/O operations (64KB) — matches old behavior.
const CHUNK_SIZE: usize = 64 * 1024;

/// Bounded time we give `file.shutdown()` after a transfer to flush
/// pending acks and close the remote handle. This is what keeps the
/// `russh-sftp` request-id space clean across cancels.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

// ============================================================================
// Global tokio runtime
// ============================================================================

static SFTP_RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn runtime() -> &'static Runtime {
    SFTP_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("vfs-sftp")
            .enable_all()
            .build()
            .expect("failed to build SFTP tokio runtime")
    })
}

/// Run a future to completion on the global SFTP runtime, blocking the
/// calling (sync) thread. Safe to call from any non-tokio thread.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    runtime().block_on(fut)
}

// ============================================================================
// Public SftpProvider
// ============================================================================

struct SftpInner {
    state: ConnectionState,
    handle: Option<SftpHandle>,
    home_dir: Option<String>,
    connect_started: Option<Instant>,
    cancelled: Arc<AtomicBool>,
    /// Last `ConnectOptions` used to bring the session up. Cached so
    /// the actor can transparently reconnect — for example after a
    /// cancel that left the SFTP session in an unknown state — without
    /// having to round-trip through the UI for credentials. Cleared
    /// and zeroed on Drop.
    cached_options: Option<ConnectOptions>,
    /// Username effectively used at connect time (after fallback to
    /// `$USER` / `$USERNAME` / `"root"`). Needed alongside
    /// `cached_options` to repeat the handshake.
    cached_username: Option<String>,
}

impl SftpInner {
    fn new() -> Self {
        Self {
            state: ConnectionState::Disconnected,
            handle: None,
            home_dir: None,
            connect_started: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            cached_options: None,
            cached_username: None,
        }
    }
}

/// SFTP filesystem provider.
pub struct SftpProvider {
    host: String,
    port: u16,
    username: Option<String>,
    inner: Arc<Mutex<SftpInner>>,
}

impl SftpProvider {
    /// Create a new SFTP provider.
    pub fn new(host: &str, port: u16, username: Option<&str>) -> Self {
        Self {
            host: host.to_string(),
            port,
            username: username.map(String::from),
            inner: Arc::new(Mutex::new(SftpInner::new())),
        }
    }

    fn effective_username(&self) -> String {
        match &self.username {
            Some(u) if !u.is_empty() => u.clone(),
            _ => fallback_username(),
        }
    }

    fn to_remote_path(path: &VfsPath) -> VfsResult<PathBuf> {
        if !matches!(path.protocol, VfsProtocol::Sftp) {
            return Err(VfsError::InvalidPath(format!(
                "Expected SFTP path, got: {path}"
            )));
        }
        Ok(path.path.clone())
    }

    /// True while a connect attempt is in flight (for UI spinner).
    pub fn is_connecting(&self) -> bool {
        self.inner
            .lock()
            .map(|i| i.state == ConnectionState::Connecting)
            .unwrap_or(false)
    }

    /// Time elapsed since the current connect attempt started.
    pub fn connection_elapsed(&self) -> Option<Duration> {
        self.inner
            .lock()
            .ok()
            .and_then(|i| i.connect_started.map(|t| t.elapsed()))
    }

    /// Signal cancellation of the in-flight connect attempt.
    pub fn cancel_connection(&self) {
        if let Ok(i) = self.inner.lock() {
            i.cancelled.store(true, Ordering::SeqCst);
        }
    }

    fn get_handle(&self) -> VfsResult<SftpHandle> {
        let guard = self.inner.lock().map_err(|e| {
            log::warn!("sftp inner mutex poisoned: {e}");
            VfsError::RemoteError {
                message: "SFTP state poisoned".into(),
            }
        })?;
        match &guard.handle {
            Some(h) => Ok(SftpHandle {
                cmd_tx: h.cmd_tx.clone(),
            }),
            None => Err(VfsError::NotConnected),
        }
    }

    fn dispatch_op<T, F>(&self, build: F) -> VfsOperation<T>
    where
        T: Send + 'static,
        F: FnOnce(Reply<T>) -> SftpCommand + Send + 'static,
    {
        let handle = match self.get_handle() {
            Ok(h) => h,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        let (tx, rx) = std_mpsc::channel();
        thread::spawn(move || {
            let res = handle.dispatch(build);
            let _ = tx.send(res);
        });
        VfsOperation::new(rx)
    }
}

impl Drop for SftpProvider {
    fn drop(&mut self) {
        // Best-effort shutdown of the actor.
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(handle) = inner.handle.take() {
                let _ = block_on(handle.cmd_tx.send(SftpCommand::Shutdown));
            }
            inner.state = ConnectionState::Disconnected;
            // Zero out any cached password before letting the cache go.
            if let Some(ref mut opts) = inner.cached_options {
                if let AuthMethod::Password(ref mut pw) = opts.auth {
                    // SAFETY: zeroing owned String bytes valid for pw.len().
                    unsafe {
                        std::ptr::write_bytes(pw.as_mut_vec().as_mut_ptr(), 0, pw.len());
                    }
                }
            }
            inner.cached_options = None;
            inner.cached_username = None;
        }
    }
}

// ============================================================================
// VfsProvider impl
// ============================================================================

impl VfsProvider for SftpProvider {
    fn name(&self) -> &'static str {
        "sftp"
    }

    fn connection_state(&self) -> ConnectionState {
        self.inner
            .lock()
            .map(|i| i.state)
            .unwrap_or(ConnectionState::Failed)
    }

    fn connect(&mut self, options: ConnectOptions) -> VfsOperation<()> {
        let cancelled = {
            let mut inner = match self.inner.lock() {
                Ok(g) => g,
                Err(_) => {
                    return VfsOperation::ready(Err(VfsError::RemoteError {
                        message: "SFTP state poisoned".into(),
                    }))
                }
            };
            if inner.state == ConnectionState::Connected {
                return VfsOperation::ready(Err(VfsError::RemoteError {
                    message: "Already connected".into(),
                }));
            }
            inner.state = ConnectionState::Connecting;
            inner.connect_started = Some(Instant::now());
            inner.cancelled = Arc::new(AtomicBool::new(false));
            inner.home_dir = None;
            inner.handle = None;
            Arc::clone(&inner.cancelled)
        };

        let host = self.host.clone();
        let port = self.port;
        let username = self.effective_username();
        let inner_arc = Arc::clone(&self.inner);
        // Stash creds before the move into the worker thread so we can
        // hand them to the reconnect path later. Cloning ConnectOptions
        // is cheap (small enum + maybe a String).
        let options_for_cache = options.clone();
        let username_for_cache = username.clone();

        let (tx, rx) = std_mpsc::channel();

        thread::spawn(move || {
            let result = block_on(do_connect(
                host.clone(),
                port,
                username.clone(),
                options,
                cancelled,
            ));

            match result {
                Ok((sftp, home_dir)) => {
                    let (cmd_tx, cmd_rx) = async_mpsc::channel::<SftpCommand>(32);
                    runtime().spawn(sftp_actor(sftp, cmd_rx, Arc::clone(&inner_arc)));
                    if let Ok(mut inner) = inner_arc.lock() {
                        inner.state = ConnectionState::Connected;
                        inner.handle = Some(SftpHandle { cmd_tx });
                        inner.home_dir = home_dir;
                        inner.cached_options = Some(options_for_cache);
                        inner.cached_username = Some(username_for_cache);
                    }
                    let _ = tx.send(Ok(()));
                }
                Err(e) => {
                    if let Ok(mut inner) = inner_arc.lock() {
                        inner.state = ConnectionState::Failed;
                    }
                    let _ = tx.send(Err(e));
                }
            }
        });

        VfsOperation::new(rx)
    }

    fn disconnect(&mut self) {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if let Some(handle) = inner.handle.take() {
            let _ = block_on(handle.cmd_tx.send(SftpCommand::Shutdown));
        }
        inner.state = ConnectionState::Disconnected;
        inner.home_dir = None;
    }

    fn list_dir(&self, path: &VfsPath) -> VfsOperation<Vec<VfsEntry>> {
        let remote_path = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        let parent = path.clone();
        let handle = match self.get_handle() {
            Ok(h) => h,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        let (tx, rx) = std_mpsc::channel();
        thread::spawn(move || {
            let res = handle.dispatch(|reply| SftpCommand::ListDir {
                path: remote_path,
                reply,
            });
            let entries = res.map(|raw| {
                let mut entries: Vec<VfsEntry> = raw
                    .into_iter()
                    .map(|e| {
                        let p = parent.join(&e.name);
                        VfsEntry::new(e.name, p, e.metadata)
                    })
                    .collect();
                entries.sort_by(|a, b| match (a.is_dir(), b.is_dir()) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                });
                entries
            });
            let _ = tx.send(entries);
        });
        VfsOperation::new(rx)
    }

    fn create_dir(&self, path: &VfsPath) -> VfsOperation<()> {
        let p = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        self.dispatch_op(move |reply| SftpCommand::Mkdir { path: p, reply })
    }

    fn create_dir_all(&self, path: &VfsPath) -> VfsOperation<()> {
        let p = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        self.dispatch_op(move |reply| SftpCommand::MkdirRecursive { path: p, reply })
    }

    fn exists(&self, path: &VfsPath) -> VfsOperation<bool> {
        let p = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        self.dispatch_op(move |reply| SftpCommand::Exists { path: p, reply })
    }

    fn metadata(&self, path: &VfsPath) -> VfsOperation<VfsMetadata> {
        let p = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        self.dispatch_op(move |reply| SftpCommand::Stat { path: p, reply })
    }

    fn read_file(&self, path: &VfsPath) -> VfsOperation<Vec<u8>> {
        let p = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        self.dispatch_op(move |reply| SftpCommand::Read { path: p, reply })
    }

    fn write_file(&self, path: &VfsPath, data: &[u8]) -> VfsOperation<()> {
        let p = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        let data = data.to_vec();
        self.dispatch_op(move |reply| SftpCommand::Write {
            path: p,
            data,
            reply,
        })
    }

    fn delete(&self, path: &VfsPath) -> VfsOperation<()> {
        // Match previous behavior: delete is recursive on SFTP.
        self.delete_recursive(path)
    }

    fn delete_recursive(&self, path: &VfsPath) -> VfsOperation<()> {
        let p = match Self::to_remote_path(path) {
            Ok(p) => p,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        self.dispatch_op(move |reply| SftpCommand::DeleteRecursive {
            path: p,
            depth_limit: crate::MAX_RECURSION_DEPTH,
            reply,
        })
    }

    fn rename(&self, from: &VfsPath, to: &VfsPath) -> VfsOperation<()> {
        let from_p = match Self::to_remote_path(from) {
            Ok(p) => p,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        let to_p = match Self::to_remote_path(to) {
            Ok(p) => p,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        self.dispatch_op(move |reply| SftpCommand::Rename {
            from: from_p,
            to: to_p,
            reply,
        })
    }

    fn copy(&self, from: &VfsPath, to: &VfsPath) -> VfsOperation<()> {
        let from_p = match Self::to_remote_path(from) {
            Ok(p) => p,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        let to_p = match Self::to_remote_path(to) {
            Ok(p) => p,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        self.dispatch_op(move |reply| SftpCommand::CopyFile {
            from: from_p,
            to: to_p,
            reply,
        })
    }

    fn download(&self, remote: &VfsPath, local: &Path) -> VfsOperation<PathBuf> {
        let remote_p = match Self::to_remote_path(remote) {
            Ok(p) => p,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        let local_path = local.to_path_buf();
        let result_local = local_path.clone();
        let handle = match self.get_handle() {
            Ok(h) => h,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        let (tx, rx) = std_mpsc::channel();
        let (_progress_tx, _progress_rx) = std_mpsc::channel::<DownloadProgress>();
        let pause_flag = Arc::new(AtomicBool::new(false));
        let cancel_flag = Arc::new(AtomicBool::new(false));
        thread::spawn(move || {
            let result: VfsResult<PathBuf> = (|| -> VfsResult<PathBuf> {
                let p = remote_p.clone();
                let meta = handle.dispatch(move |reply| SftpCommand::Stat { path: p, reply })?;
                let mut state = DlState {
                    total_files: 1,
                    total_bytes: meta.size,
                    files_done: 0,
                    bytes_done: 0,
                };
                if matches!(meta.file_type, VfsFileType::Directory) {
                    let (tf, tb) = worker_count_remote(
                        &handle,
                        &remote_p,
                        &cancel_flag,
                        crate::MAX_RECURSION_DEPTH,
                    )?;
                    state.total_files = tf;
                    state.total_bytes = tb;
                    worker_download_dir(
                        &handle,
                        &remote_p,
                        &local_path,
                        &pause_flag,
                        &cancel_flag,
                        &_progress_tx,
                        &mut state,
                        crate::MAX_RECURSION_DEPTH,
                    )?;
                } else {
                    worker_download_file(
                        &handle,
                        &remote_p,
                        &local_path,
                        &pause_flag,
                        &cancel_flag,
                        &_progress_tx,
                        &mut state,
                    )?;
                }
                Ok(result_local)
            })();
            let _ = tx.send(result);
        });
        VfsOperation::new(rx)
    }

    fn upload(&self, local: &Path, remote: &VfsPath) -> VfsOperation<()> {
        let remote_p = match Self::to_remote_path(remote) {
            Ok(p) => p,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        let local_path = local.to_path_buf();
        let handle = match self.get_handle() {
            Ok(h) => h,
            Err(e) => return VfsOperation::ready(Err(e)),
        };
        let (tx, rx) = std_mpsc::channel();
        let (_progress_tx, _progress_rx) = std_mpsc::channel::<UploadProgress>();
        let pause_flag = Arc::new(AtomicBool::new(false));
        let cancel_flag = Arc::new(AtomicBool::new(false));
        thread::spawn(move || {
            let result: VfsResult<()> = (|| -> VfsResult<()> {
                let meta = std::fs::metadata(&local_path).map_err(VfsError::Io)?;
                let is_dir = meta.is_dir();
                let mut state = UlState {
                    total_files: 1,
                    total_bytes: meta.len(),
                    files_done: 0,
                    bytes_done: 0,
                };
                if is_dir {
                    let (tf, tb) = count_local_files_sync(&local_path, &cancel_flag)?;
                    state.total_files = tf;
                    state.total_bytes = tb;
                    worker_upload_dir(
                        &handle,
                        &local_path,
                        &remote_p,
                        &pause_flag,
                        &cancel_flag,
                        &_progress_tx,
                        &mut state,
                    )?;
                } else {
                    worker_upload_file(
                        &handle,
                        &local_path,
                        &remote_p,
                        &pause_flag,
                        &cancel_flag,
                        &_progress_tx,
                        &mut state,
                    )?;
                }
                Ok(())
            })();
            let _ = tx.send(result);
        });
        VfsOperation::new(rx)
    }

    fn upload_with_progress(&self, local: &Path, remote: &VfsPath) -> VfsUploadOperation {
        let remote_p = match Self::to_remote_path(remote) {
            Ok(p) => p,
            Err(e) => return VfsUploadOperation::error(e),
        };
        let local_path = local.to_path_buf();
        let handle = match self.get_handle() {
            Ok(h) => h,
            Err(e) => return VfsUploadOperation::error(e),
        };
        let (completion_tx, completion_rx) = std_mpsc::channel();
        let (progress_tx, progress_rx) = std_mpsc::channel();
        let pause_flag = Arc::new(AtomicBool::new(false));
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let pause_for_worker = Arc::clone(&pause_flag);
        let cancel_for_worker = Arc::clone(&cancel_flag);

        thread::spawn(move || {
            let result: VfsResult<()> = (|| -> VfsResult<()> {
                let meta = std::fs::metadata(&local_path).map_err(VfsError::Io)?;
                let is_dir = meta.is_dir();
                let (total_files, total_bytes) = if is_dir {
                    count_local_files_sync(&local_path, &cancel_for_worker)?
                } else {
                    (1usize, meta.len())
                };
                let mut state = UlState {
                    total_files,
                    total_bytes,
                    files_done: 0,
                    bytes_done: 0,
                };
                if is_dir {
                    worker_upload_dir(
                        &handle,
                        &local_path,
                        &remote_p,
                        &pause_for_worker,
                        &cancel_for_worker,
                        &progress_tx,
                        &mut state,
                    )
                } else {
                    worker_upload_file(
                        &handle,
                        &local_path,
                        &remote_p,
                        &pause_for_worker,
                        &cancel_for_worker,
                        &progress_tx,
                        &mut state,
                    )
                }
            })();
            let _ = completion_tx.send(result);
        });

        VfsUploadOperation::new(completion_rx, progress_rx, pause_flag, cancel_flag)
    }

    fn download_with_progress(&self, remote: &VfsPath, local: &Path) -> VfsDownloadOperation {
        let remote_p = match Self::to_remote_path(remote) {
            Ok(p) => p,
            Err(e) => return VfsDownloadOperation::error(e),
        };
        let local_path = local.to_path_buf();
        let result_local = local_path.clone();
        let handle = match self.get_handle() {
            Ok(h) => h,
            Err(e) => return VfsDownloadOperation::error(e),
        };
        let (completion_tx, completion_rx) = std_mpsc::channel();
        let (progress_tx, progress_rx) = std_mpsc::channel();
        let pause_flag = Arc::new(AtomicBool::new(false));
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let pause_for_worker = Arc::clone(&pause_flag);
        let cancel_for_worker = Arc::clone(&cancel_flag);

        thread::spawn(move || {
            let result: VfsResult<PathBuf> = (|| -> VfsResult<PathBuf> {
                let p = remote_p.clone();
                let meta = handle.dispatch(move |reply| SftpCommand::Stat { path: p, reply })?;
                let is_dir = matches!(meta.file_type, VfsFileType::Directory);
                let (total_files, total_bytes) = if is_dir {
                    worker_count_remote(
                        &handle,
                        &remote_p,
                        &cancel_for_worker,
                        crate::MAX_RECURSION_DEPTH,
                    )?
                } else {
                    (1usize, meta.size)
                };
                let mut state = DlState {
                    total_files,
                    total_bytes,
                    files_done: 0,
                    bytes_done: 0,
                };
                if is_dir {
                    worker_download_dir(
                        &handle,
                        &remote_p,
                        &local_path,
                        &pause_for_worker,
                        &cancel_for_worker,
                        &progress_tx,
                        &mut state,
                        crate::MAX_RECURSION_DEPTH,
                    )?;
                } else {
                    worker_download_file(
                        &handle,
                        &remote_p,
                        &local_path,
                        &pause_for_worker,
                        &cancel_for_worker,
                        &progress_tx,
                        &mut state,
                    )?;
                }
                Ok(result_local)
            })();
            let _ = completion_tx.send(result);
        });

        VfsDownloadOperation::new(completion_rx, progress_rx, pause_flag, cancel_flag)
    }

    fn supported_auth_methods(&self) -> Vec<AuthMethod> {
        vec![
            AuthMethod::SshAgent,
            AuthMethod::SshKey {
                private_key: PathBuf::new(),
                passphrase: None,
            },
            AuthMethod::Password(String::new()),
            AuthMethod::Auto,
        ]
    }

    fn supports_recursive(&self) -> bool {
        true
    }

    fn home_dir(&self) -> Option<VfsPath> {
        let home = self.inner.lock().ok()?.home_dir.clone()?;
        Some(
            VfsPath::remote(VfsProtocol::Sftp, &self.host, Path::new(&home))
                .with_port(self.port)
                .with_username(self.effective_username()),
        )
    }

    fn disk_space(&self, _path: &VfsPath) -> Option<DiskSpace> {
        // Could be implemented via SSH exec "df" but not used by termide yet.
        None
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sftp_provider_creation() {
        let provider = SftpProvider::new("example.com", 22, Some("alice"));
        assert_eq!(provider.name(), "sftp");
        assert_eq!(provider.connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_effective_username() {
        let p1 = SftpProvider::new("h", 22, Some("alice"));
        assert_eq!(p1.effective_username(), "alice");

        let p2 = SftpProvider::new("h", 22, None);
        assert!(!p2.effective_username().is_empty());
    }

    #[test]
    fn test_to_remote_path() {
        let sftp_path = VfsPath::remote(VfsProtocol::Sftp, "example.com", Path::new("/var/log"));
        assert!(SftpProvider::to_remote_path(&sftp_path).is_ok());

        let local_path = VfsPath::local("/tmp");
        assert!(SftpProvider::to_remote_path(&local_path).is_err());
    }

    #[test]
    fn test_supported_auth_methods() {
        let provider = SftpProvider::new("h", 22, None);
        let methods = provider.supported_auth_methods();
        assert!(!methods.is_empty());
    }
}
