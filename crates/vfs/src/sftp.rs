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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use russh::client;
use russh::keys::{load_secret_key, PrivateKey, PrivateKeyWithHashAlg};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, FileType as SftpFileType, OpenFlags};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc as async_mpsc, oneshot};

use crate::error::{VfsError, VfsResult};
use crate::traits::{DiskSpace, VfsProvider};
use crate::types::{
    AuthMethod, ConnectOptions, ConnectionState, DownloadProgress, UploadProgress,
    VfsDownloadOperation, VfsEntry, VfsFileType, VfsMetadata, VfsOperation, VfsPath, VfsProtocol,
    VfsUploadOperation,
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
// Actor: commands, replies, task loop
// ============================================================================

type Reply<T> = oneshot::Sender<VfsResult<T>>;

/// SFTP entry as crossed-over from the actor to the sync side.
/// Decoupled from russh_sftp's `DirEntry` so callers don't carry the dep.
struct ActorEntry {
    name: String,
    metadata: VfsMetadata,
}

enum SftpCommand {
    ListDir {
        path: PathBuf,
        reply: Reply<Vec<ActorEntry>>,
    },
    Stat {
        path: PathBuf,
        reply: Reply<VfsMetadata>,
    },
    Exists {
        path: PathBuf,
        reply: Reply<bool>,
    },
    Mkdir {
        path: PathBuf,
        reply: Reply<()>,
    },
    MkdirRecursive {
        path: PathBuf,
        reply: Reply<()>,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
        reply: Reply<()>,
    },
    Read {
        path: PathBuf,
        reply: Reply<Vec<u8>>,
    },
    Write {
        path: PathBuf,
        data: Vec<u8>,
        reply: Reply<()>,
    },
    /// Recursive delete (file or directory), with depth limit.
    DeleteRecursive {
        path: PathBuf,
        depth_limit: usize,
        reply: Reply<()>,
    },
    /// SFTP-side copy via streaming (no temp file).
    CopyFile {
        from: PathBuf,
        to: PathBuf,
        reply: Reply<()>,
    },
    // Atomic chunk-as-command primitives. Transfers and recursive walks
    // live on the sync (worker) side so the actor never sits in a long
    // loop — pause and cross-panel work stay responsive.
    /// Open a remote file for reading. Returns an opaque handle id
    /// that subsequent ReadChunk / CloseHandle commands target.
    OpenRead {
        path: PathBuf,
        reply: Reply<u64>,
    },
    /// Open a remote file for writing (CREATE | WRITE | TRUNCATE).
    OpenWrite {
        path: PathBuf,
        reply: Reply<u64>,
    },
    /// Read up to `max_bytes` from the file at `handle`. Empty Vec = EOF.
    ReadChunk {
        handle: u64,
        max_bytes: usize,
        reply: Reply<Vec<u8>>,
    },
    /// Append `data` to the file at `handle`.
    WriteChunk {
        handle: u64,
        data: Vec<u8>,
        reply: Reply<()>,
    },
    /// Shut down the file at `handle` (flush + close) and drop it.
    CloseHandle {
        handle: u64,
        reply: Reply<()>,
    },
    /// Tear down the actor cleanly.
    Shutdown,
}

/// Handle to the long-lived SFTP actor task.
struct SftpHandle {
    cmd_tx: async_mpsc::Sender<SftpCommand>,
}

/// Per-command timeout for sync dispatches. The actor only processes
/// one command at a time, so if a previous transfer left it stuck on
/// the server (e.g. an open file handle that the server hasn't closed
/// yet after a cancel), all subsequent UI calls — `metadata`,
/// `exists`, `list_dir` — would otherwise block the UI thread
/// forever. This is a safety net, not the happy path.
const DISPATCH_TIMEOUT: Duration = Duration::from_secs(30);

impl SftpHandle {
    /// Send a command and block for the reply on the SFTP runtime.
    fn dispatch<T, F>(&self, build: F) -> VfsResult<T>
    where
        F: FnOnce(Reply<T>) -> SftpCommand,
    {
        let (tx, rx) = oneshot::channel();
        let cmd = build(tx);
        block_on(async move {
            self.cmd_tx
                .send(cmd)
                .await
                .map_err(|_| VfsError::NotConnected)?;
            match tokio::time::timeout(DISPATCH_TIMEOUT, rx).await {
                Ok(Ok(res)) => res,
                Ok(Err(_)) => Err(VfsError::NotConnected),
                // Avoid the substring "timed out" — file-ops retry
                // policy treats that as a transient network failure and
                // would auto-retry the operation that just bailed out.
                Err(_) => Err(VfsError::RemoteError {
                    message: "SFTP backend not responding within deadline".into(),
                }),
            }
        })
    }
}

/// Async actor task: owns the SftpSession and serves commands until the
/// channel closes or `Shutdown` is received. `inner` lets the actor
/// publish state transitions (Disconnected) so the rest of the VFS
/// sees a coherent picture on teardown.
async fn sftp_actor(
    initial: SftpSession,
    mut rx: async_mpsc::Receiver<SftpCommand>,
    inner: Arc<Mutex<SftpInner>>,
) {
    // Held as Option so a Reconnect attempt can take the old session
    // out by value (close() consumes it) and put a fresh one back.
    let mut sftp_opt: Option<SftpSession> = Some(initial);

    // Open remote file handles, keyed by an opaque u64 the sync worker
    // refers to. Held inside the actor task — single owner, no Mutex.
    let mut open_files: HashMap<u64, russh_sftp::client::fs::File> = HashMap::new();
    let mut next_handle_id: u64 = 1;

    // Convenience: pull the live session reference for a command. The
    // actor only enters the next iteration if a previous iteration's
    // reconnect succeeded, so unwrap is safe by construction.
    macro_rules! sftp {
        () => {
            sftp_opt
                .as_ref()
                .expect("SFTP session must exist while actor runs")
        };
    }

    while let Some(cmd) = rx.recv().await {
        match cmd {
            SftpCommand::ListDir { path, reply } => {
                let _ = reply.send(actor_list_dir(sftp!(), &path).await);
            }
            SftpCommand::Stat { path, reply } => {
                let _ = reply.send(actor_stat(sftp!(), &path).await);
            }
            SftpCommand::Exists { path, reply } => {
                let _ = reply.send(Ok(sftp!().metadata(path_to_string(&path)).await.is_ok()));
            }
            SftpCommand::Mkdir { path, reply } => {
                let _ = reply.send(map_sftp_unit(
                    sftp!().create_dir(path_to_string(&path)).await,
                ));
            }
            SftpCommand::MkdirRecursive { path, reply } => {
                let _ = reply.send(actor_mkdir_recursive(sftp!(), &path).await);
            }
            SftpCommand::Rename { from, to, reply } => {
                let _ = reply.send(map_sftp_unit(
                    sftp!()
                        .rename(path_to_string(&from), path_to_string(&to))
                        .await,
                ));
            }
            SftpCommand::Read { path, reply } => {
                let _ = reply.send(actor_read_file(sftp!(), &path).await);
            }
            SftpCommand::Write { path, data, reply } => {
                let _ = reply.send(actor_write_file(sftp!(), &path, &data).await);
            }
            SftpCommand::DeleteRecursive {
                path,
                depth_limit,
                reply,
            } => {
                let _ = reply.send(actor_delete_recursive(sftp!(), &path, depth_limit).await);
            }
            SftpCommand::CopyFile { from, to, reply } => {
                let _ = reply.send(actor_copy_file(sftp!(), &from, &to).await);
            }
            SftpCommand::OpenRead { path, reply } => {
                let res = sftp!()
                    .open(path_to_string(&path))
                    .await
                    .map_err(map_sftp_err);
                match res {
                    Ok(file) => {
                        let id = next_handle_id;
                        next_handle_id += 1;
                        open_files.insert(id, file);
                        let _ = reply.send(Ok(id));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
            }
            SftpCommand::OpenWrite { path, reply } => {
                let res = sftp!()
                    .open_with_flags(
                        path_to_string(&path),
                        OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
                    )
                    .await
                    .map_err(map_sftp_err);
                match res {
                    Ok(file) => {
                        let id = next_handle_id;
                        next_handle_id += 1;
                        open_files.insert(id, file);
                        let _ = reply.send(Ok(id));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
            }
            SftpCommand::ReadChunk {
                handle,
                max_bytes,
                reply,
            } => {
                let result = match open_files.get_mut(&handle) {
                    Some(file_ref) => {
                        // Reborrow into a fresh mutable binding — tokio's
                        // AsyncReadExt::read takes &mut self, so the
                        // binding has to be declared mut to allow the
                        // implicit reborrow inside the await.
                        let mut buf = vec![0u8; max_bytes];
                        let res = file_ref.read(&mut buf).await;
                        match res {
                            Ok(n) => {
                                buf.truncate(n);
                                Ok(buf)
                            }
                            Err(e) => Err(map_sftp_err(e)),
                        }
                    }
                    None => Err(VfsError::RemoteError {
                        message: format!("unknown SFTP handle {handle}"),
                    }),
                };
                let _ = reply.send(result);
            }
            SftpCommand::WriteChunk {
                handle,
                data,
                reply,
            } => {
                let result = match open_files.get_mut(&handle) {
                    Some(file_ref) => file_ref.write_all(&data).await.map_err(map_sftp_err),
                    None => Err(VfsError::RemoteError {
                        message: format!("unknown SFTP handle {handle}"),
                    }),
                };
                let _ = reply.send(result);
            }
            SftpCommand::CloseHandle { handle, reply } => {
                let result = if let Some(mut file) = open_files.remove(&handle) {
                    match tokio::time::timeout(SHUTDOWN_TIMEOUT, file.shutdown()).await {
                        Ok(Ok(())) => Ok(()),
                        Ok(Err(e)) => Err(map_sftp_err(e)),
                        Err(_) => Err(VfsError::RemoteError {
                            message: "SFTP close took too long".into(),
                        }),
                    }
                } else {
                    // Closing an unknown handle is benign — likely a
                    // double-close from worker cleanup.
                    Ok(())
                };
                let _ = reply.send(result);
            }
            SftpCommand::Shutdown => break,
        }
    }
    // Tear down any handles the worker forgot to close. shutdown() is
    // best-effort with a bounded timeout — we still want to call it so
    // the server releases the handle promptly.
    for (_id, mut file) in open_files.drain() {
        let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, file.shutdown()).await;
    }
    if let Some(s) = sftp_opt.take() {
        let _ = s.close().await;
    }
    if let Ok(mut g) = inner.lock() {
        g.state = ConnectionState::Disconnected;
        g.handle = None;
        g.home_dir = None;
    }
}

// ============================================================================
// Actor operation primitives
// ============================================================================

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn map_sftp_err<E: std::fmt::Display>(e: E) -> VfsError {
    VfsError::Sftp(e.to_string())
}

fn map_sftp_unit<T, E: std::fmt::Display>(r: Result<T, E>) -> VfsResult<()> {
    r.map(|_| ()).map_err(map_sftp_err)
}

fn attrs_to_metadata(attrs: &FileAttributes) -> VfsMetadata {
    let file_type = match attrs.file_type() {
        SftpFileType::Dir => VfsFileType::Directory,
        SftpFileType::Symlink => VfsFileType::Symlink,
        SftpFileType::File => VfsFileType::File,
        _ => VfsFileType::Other,
    };

    let modified = attrs
        .mtime
        .map(|secs| UNIX_EPOCH + Duration::from_secs(secs as u64));

    VfsMetadata {
        file_type,
        size: attrs.size.unwrap_or(0),
        modified,
        created: None,
        accessed: attrs
            .atime
            .map(|secs| UNIX_EPOCH + Duration::from_secs(secs as u64)),
        readonly: attrs.permissions.is_some_and(|p| p & 0o200 == 0),
        permissions: attrs.permissions,
    }
}

async fn actor_list_dir(sftp: &SftpSession, path: &Path) -> VfsResult<Vec<ActorEntry>> {
    let entries = sftp
        .read_dir(path_to_string(path))
        .await
        .map_err(map_sftp_err)?;
    let mut out = Vec::new();
    for entry in entries {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        out.push(ActorEntry {
            metadata: attrs_to_metadata(&entry.metadata()),
            name,
        });
    }
    Ok(out)
}

async fn actor_stat(sftp: &SftpSession, path: &Path) -> VfsResult<VfsMetadata> {
    let attrs = sftp
        .metadata(path_to_string(path))
        .await
        .map_err(map_sftp_err)?;
    Ok(attrs_to_metadata(&attrs))
}

/// Create directory and all parents, ignoring "already exists" errors.
async fn actor_mkdir_recursive(sftp: &SftpSession, path: &Path) -> VfsResult<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if current.as_os_str() == "/" {
            continue;
        }
        match sftp.create_dir(path_to_string(&current)).await {
            Ok(_) => {}
            Err(_) => {
                // Check whether it already exists as a directory.
                match sftp.metadata(path_to_string(&current)).await {
                    Ok(attrs) if matches!(attrs.file_type(), SftpFileType::Dir) => {}
                    Ok(_) => {
                        return Err(VfsError::Sftp(format!(
                            "Path '{}' exists but is not a directory",
                            current.display()
                        )));
                    }
                    Err(e) => {
                        return Err(VfsError::Sftp(format!(
                            "Failed to create remote directory '{}': {}",
                            current.display(),
                            e
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

async fn actor_read_file(sftp: &SftpSession, path: &Path) -> VfsResult<Vec<u8>> {
    let mut file = sftp
        .open(path_to_string(path))
        .await
        .map_err(map_sftp_err)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).await.map_err(map_sftp_err)?;
    Ok(buf)
}

async fn actor_write_file(sftp: &SftpSession, path: &Path, data: &[u8]) -> VfsResult<()> {
    let mut file = sftp
        .open_with_flags(
            path_to_string(path),
            OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
        )
        .await
        .map_err(map_sftp_err)?;
    file.write_all(data).await.map_err(map_sftp_err)?;
    file.flush().await.map_err(map_sftp_err)?;
    file.shutdown().await.map_err(map_sftp_err)?;
    Ok(())
}

async fn actor_delete_recursive(
    sftp: &SftpSession,
    path: &Path,
    depth_limit: usize,
) -> VfsResult<()> {
    if depth_limit == 0 {
        return Err(VfsError::Sftp(format!(
            "delete recursion limit reached at {}",
            path.display()
        )));
    }
    let attrs = sftp
        .metadata(path_to_string(path))
        .await
        .map_err(map_sftp_err)?;
    if matches!(attrs.file_type(), SftpFileType::Dir) {
        let entries = sftp
            .read_dir(path_to_string(path))
            .await
            .map_err(map_sftp_err)?;
        for entry in entries {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let child = path.join(&name);
            Box::pin(actor_delete_recursive(sftp, &child, depth_limit - 1)).await?;
        }
        sftp.remove_dir(path_to_string(path))
            .await
            .map_err(map_sftp_err)?;
    } else {
        sftp.remove_file(path_to_string(path))
            .await
            .map_err(map_sftp_err)?;
    }
    Ok(())
}

/// SFTP has no native copy — stream read + write through chunks.
async fn actor_copy_file(sftp: &SftpSession, from: &Path, to: &Path) -> VfsResult<()> {
    let mut src = sftp
        .open(path_to_string(from))
        .await
        .map_err(map_sftp_err)?;
    let mut dst = sftp
        .open_with_flags(
            path_to_string(to),
            OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
        )
        .await
        .map_err(map_sftp_err)?;
    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        let n = src.read(&mut buf).await.map_err(map_sftp_err)?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await.map_err(map_sftp_err)?;
    }
    dst.flush().await.map_err(map_sftp_err)?;
    dst.shutdown().await.map_err(map_sftp_err)?;
    Ok(())
}

// ============================================================================
// Sync worker helpers for chunk-as-command transfers.
//
// Transfers run on a sync thread on the SftpProvider side. The thread
// dispatches short atomic commands to the actor — OpenRead / OpenWrite,
// ReadChunk / WriteChunk, CloseHandle. Pause/cancel polling lives in
// the worker so the actor stays free to serve metadata/list_dir from
// other panels while a transfer is paused.
// ============================================================================

/// Poll the cancel/pause flags between chunks. Returns Cancelled if the
/// user cancelled; spins on a coarse sleep while paused. The sleep is
/// `std::thread::sleep` — it runs on the worker, not the actor, so the
/// actor remains responsive for unrelated commands during a pause.
fn wait_or_cancel_sync(pause: &Arc<AtomicBool>, cancel: &Arc<AtomicBool>) -> VfsResult<()> {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(VfsError::Cancelled);
        }
        if !pause.load(Ordering::Relaxed) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Sync recursive walk of a remote subtree to total up files/bytes.
/// Uses `Stat` + `ListDir` atomic dispatches — each is a short command
/// the actor finishes immediately, so a multi-second walk does not
/// block the actor for the whole walk.
fn worker_count_remote(
    handle: &SftpHandle,
    path: &Path,
    cancel: &Arc<AtomicBool>,
    depth: usize,
) -> VfsResult<(usize, u64)> {
    if cancel.load(Ordering::Relaxed) {
        return Err(VfsError::Cancelled);
    }
    let p1 = path.to_path_buf();
    let meta = handle.dispatch(move |reply| SftpCommand::Stat { path: p1, reply })?;
    if !matches!(meta.file_type, VfsFileType::Directory) {
        return Ok((1, meta.size));
    }
    // Stop descending past the recursion limit. `Stat` above follows
    // symlinks, so without this guard a symlink cycle (e.g. `link -> ..`)
    // makes the count walk recurse forever and the download operation
    // appears to hang indefinitely.
    if depth == 0 {
        return Ok((0, 0));
    }
    let p2 = path.to_path_buf();
    let entries = handle.dispatch(move |reply| SftpCommand::ListDir { path: p2, reply })?;
    let mut count = 0;
    let mut bytes = 0u64;
    for entry in entries {
        let child = path.join(&entry.name);
        // Only descend into real directories; never follow symlinks. This
        // mirrors what `worker_download_dir` actually transfers (it also
        // keys off the listing's file type) and prevents cycles.
        let (c, b) = if matches!(entry.metadata.file_type, VfsFileType::Directory) {
            worker_count_remote(handle, &child, cancel, depth - 1)?
        } else {
            (1, entry.metadata.size)
        };
        count += c;
        bytes += b;
    }
    Ok((count, bytes))
}

fn count_local_files_sync(path: &Path, cancel: &Arc<AtomicBool>) -> VfsResult<(usize, u64)> {
    if cancel.load(Ordering::Relaxed) {
        return Err(VfsError::Cancelled);
    }
    let meta = std::fs::metadata(path).map_err(VfsError::Io)?;
    if !meta.is_dir() {
        return Ok((1, meta.len()));
    }
    let mut count = 0;
    let mut bytes = 0u64;
    for entry in std::fs::read_dir(path).map_err(VfsError::Io)? {
        let entry = entry.map_err(VfsError::Io)?;
        let (c, b) = count_local_files_sync(&entry.path(), cancel)?;
        count += c;
        bytes += b;
    }
    Ok((count, bytes))
}

/// RAII guard that closes a remote file handle when the worker scope
/// exits — keeps the actor's open_files map clean across the early-
/// return / cancel / panic paths.
struct RemoteHandleGuard<'a> {
    handle: &'a SftpHandle,
    id: Option<u64>,
}

impl<'a> RemoteHandleGuard<'a> {
    fn close(mut self) -> VfsResult<()> {
        if let Some(id) = self.id.take() {
            self.handle
                .dispatch(|reply| SftpCommand::CloseHandle { handle: id, reply })?;
        }
        Ok(())
    }
}

impl<'a> Drop for RemoteHandleGuard<'a> {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            // Best-effort close on early return / panic. Ignore the
            // result — the dispatch may already be unhealthy if we are
            // tearing down due to a session error.
            let _ = self
                .handle
                .dispatch(|reply| SftpCommand::CloseHandle { handle: id, reply });
        }
    }
}

/// Rolling state shared across one transfer so progress events show the
/// whole batch's totals (bytes_done / files_done), not just the current
/// file's contribution.
struct DlState {
    total_files: usize,
    total_bytes: u64,
    files_done: usize,
    bytes_done: u64,
}

struct UlState {
    total_files: usize,
    total_bytes: u64,
    files_done: usize,
    bytes_done: u64,
}

fn worker_download_file(
    handle: &SftpHandle,
    remote: &Path,
    local: &Path,
    pause: &Arc<AtomicBool>,
    cancel: &Arc<AtomicBool>,
    progress_tx: &std_mpsc::Sender<DownloadProgress>,
    state: &mut DlState,
) -> VfsResult<()> {
    if let Some(parent) = local.parent() {
        std::fs::create_dir_all(parent).map_err(VfsError::Io)?;
    }
    let p1 = remote.to_path_buf();
    let meta = handle.dispatch(move |reply| SftpCommand::Stat { path: p1, reply })?;
    let file_total = meta.size;

    let p2 = remote.to_path_buf();
    let remote_id = handle.dispatch(move |reply| SftpCommand::OpenRead { path: p2, reply })?;
    let guard = RemoteHandleGuard {
        handle,
        id: Some(remote_id),
    };

    let mut dst = std::fs::File::create(local).map_err(VfsError::Io)?;
    let mut current_bytes = 0u64;
    let current_name = remote.file_name().map(|s| s.to_string_lossy().into_owned());

    let _ = progress_tx.send(DownloadProgress {
        bytes_downloaded: state.bytes_done,
        total_bytes: state.total_bytes,
        current_file: current_name.clone(),
        files_downloaded: state.files_done,
        total_files: state.total_files,
        current_file_bytes: 0,
        current_file_total: file_total,
    });

    let mut cancelled = false;
    loop {
        if let Err(_e) = wait_or_cancel_sync(pause, cancel) {
            cancelled = true;
            break;
        }
        let chunk = handle.dispatch(|reply| SftpCommand::ReadChunk {
            handle: remote_id,
            max_bytes: CHUNK_SIZE,
            reply,
        })?;
        if chunk.is_empty() {
            break;
        }
        let n = chunk.len();
        use std::io::Write as _;
        dst.write_all(&chunk).map_err(VfsError::Io)?;
        current_bytes += n as u64;
        state.bytes_done += n as u64;
        let _ = progress_tx.send(DownloadProgress {
            bytes_downloaded: state.bytes_done,
            total_bytes: state.total_bytes,
            current_file: current_name.clone(),
            files_downloaded: state.files_done,
            total_files: state.total_files,
            current_file_bytes: current_bytes,
            current_file_total: file_total,
        });
    }
    // std::fs::File is unbuffered; closing on Drop is enough. We rely
    // on the explicit guard.close() below to surface server-side close
    // errors to the caller — the RAII drop is a best-effort fallback.
    guard.close()?;
    if cancelled {
        return Err(VfsError::Cancelled);
    }
    state.files_done += 1;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn worker_download_dir(
    handle: &SftpHandle,
    remote: &Path,
    local: &Path,
    pause: &Arc<AtomicBool>,
    cancel: &Arc<AtomicBool>,
    progress_tx: &std_mpsc::Sender<DownloadProgress>,
    state: &mut DlState,
    depth: usize,
) -> VfsResult<()> {
    std::fs::create_dir_all(local).map_err(VfsError::Io)?;
    let p = remote.to_path_buf();
    let entries = handle.dispatch(move |reply| SftpCommand::ListDir { path: p, reply })?;
    for entry in entries {
        if cancel.load(Ordering::Relaxed) {
            return Err(VfsError::Cancelled);
        }
        let remote_child = remote.join(&entry.name);
        let local_child = local.join(&entry.name);
        if matches!(entry.metadata.file_type, VfsFileType::Directory) {
            // Defensive depth guard mirroring `worker_count_remote`.
            if depth == 0 {
                continue;
            }
            worker_download_dir(
                handle,
                &remote_child,
                &local_child,
                pause,
                cancel,
                progress_tx,
                state,
                depth - 1,
            )?;
        } else {
            worker_download_file(
                handle,
                &remote_child,
                &local_child,
                pause,
                cancel,
                progress_tx,
                state,
            )?;
        }
    }
    Ok(())
}

fn worker_upload_file(
    handle: &SftpHandle,
    local: &Path,
    remote: &Path,
    pause: &Arc<AtomicBool>,
    cancel: &Arc<AtomicBool>,
    progress_tx: &std_mpsc::Sender<UploadProgress>,
    state: &mut UlState,
) -> VfsResult<()> {
    if let Some(parent) = remote.parent() {
        if parent.as_os_str() != "" && parent.as_os_str() != "/" {
            let p = parent.to_path_buf();
            handle.dispatch(move |reply| SftpCommand::MkdirRecursive { path: p, reply })?;
        }
    }
    let file_total = std::fs::metadata(local).map(|m| m.len()).unwrap_or(0);
    let mut src = std::fs::File::open(local).map_err(VfsError::Io)?;

    let p = remote.to_path_buf();
    let remote_id = handle.dispatch(move |reply| SftpCommand::OpenWrite { path: p, reply })?;
    let guard = RemoteHandleGuard {
        handle,
        id: Some(remote_id),
    };

    let mut current_bytes = 0u64;
    let current_name = local.file_name().map(|s| s.to_string_lossy().into_owned());

    let _ = progress_tx.send(UploadProgress {
        bytes_uploaded: state.bytes_done,
        total_bytes: state.total_bytes,
        current_file: current_name.clone(),
        files_uploaded: state.files_done,
        total_files: state.total_files,
        current_file_bytes: 0,
        current_file_total: file_total,
    });

    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut cancelled = false;
    loop {
        if let Err(_e) = wait_or_cancel_sync(pause, cancel) {
            cancelled = true;
            break;
        }
        use std::io::Read as _;
        let n = src.read(&mut buf).map_err(VfsError::Io)?;
        if n == 0 {
            break;
        }
        let chunk = buf[..n].to_vec();
        handle.dispatch(move |reply| SftpCommand::WriteChunk {
            handle: remote_id,
            data: chunk,
            reply,
        })?;
        current_bytes += n as u64;
        state.bytes_done += n as u64;
        let _ = progress_tx.send(UploadProgress {
            bytes_uploaded: state.bytes_done,
            total_bytes: state.total_bytes,
            current_file: current_name.clone(),
            files_uploaded: state.files_done,
            total_files: state.total_files,
            current_file_bytes: current_bytes,
            current_file_total: file_total,
        });
    }
    guard.close()?;
    if cancelled {
        return Err(VfsError::Cancelled);
    }
    state.files_done += 1;
    Ok(())
}

fn worker_upload_dir(
    handle: &SftpHandle,
    local: &Path,
    remote: &Path,
    pause: &Arc<AtomicBool>,
    cancel: &Arc<AtomicBool>,
    progress_tx: &std_mpsc::Sender<UploadProgress>,
    state: &mut UlState,
) -> VfsResult<()> {
    let p = remote.to_path_buf();
    handle.dispatch(move |reply| SftpCommand::MkdirRecursive { path: p, reply })?;
    for entry in std::fs::read_dir(local).map_err(VfsError::Io)? {
        let entry = entry.map_err(VfsError::Io)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(VfsError::Cancelled);
        }
        let name = entry.file_name();
        let local_child = entry.path();
        let remote_child = remote.join(&name);
        let ft = entry.file_type().map_err(VfsError::Io)?;
        if ft.is_dir() {
            worker_upload_dir(
                handle,
                &local_child,
                &remote_child,
                pause,
                cancel,
                progress_tx,
                state,
            )?;
        } else if ft.is_file() {
            worker_upload_file(
                handle,
                &local_child,
                &remote_child,
                pause,
                cancel,
                progress_tx,
                state,
            )?;
        }
    }
    Ok(())
}

// ============================================================================
// SSH handshake / auth (runs on the runtime, ferries the session to actor)
// ============================================================================

/// Accept-all handler — matches previous ssh2 behavior (no known_hosts check).
struct AcceptAllHandler;

impl client::Handler for AcceptAllHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// Locate default SSH private key files for current user, in priority
/// order. Mirrors what the previous Auto auth would attempt.
fn default_key_files() -> Vec<PathBuf> {
    let mut keys = Vec::new();
    if let Some(home) = dirs::home_dir() {
        for name in ["id_ed25519", "id_rsa", "id_ecdsa", "id_dsa"] {
            let p = home.join(".ssh").join(name);
            if p.exists() {
                keys.push(p);
            }
        }
    }
    keys
}

/// Fallback username when none is provided: $USER → $USERNAME → "root".
fn fallback_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".to_string())
}

/// Expand a leading `~` in a path using `$HOME`. Returns the input
/// unchanged if there's no tilde or no home directory.
fn expand_tilde(p: &Path) -> PathBuf {
    let s = match p.to_str() {
        Some(s) => s,
        None => return p.to_path_buf(),
    };
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    p.to_path_buf()
}

/// Try every reasonable auth method until one succeeds. Stops at the
/// first success; returns AuthenticationFailed if none work.
///
/// `host` is used to look up Host-specific entries in `~/.ssh/config`
/// (IdentityFile / IdentitiesOnly) when `auth == Auto`.
async fn try_authenticate(
    session: &mut client::Handle<AcceptAllHandler>,
    host: &str,
    username: &str,
    auth: &AuthMethod,
    cancelled: &Arc<AtomicBool>,
) -> VfsResult<()> {
    let cancelled_check = || -> VfsResult<()> {
        if cancelled.load(Ordering::SeqCst) {
            Err(VfsError::ConnectionFailed("Connection cancelled".into()))
        } else {
            Ok(())
        }
    };

    cancelled_check()?;

    match auth {
        AuthMethod::None => {
            // Most permissive interpretation of "None": try SSH agent.
            try_auth_agent(session, username).await
        }
        AuthMethod::Password(password) => try_auth_password(session, username, password).await,
        AuthMethod::SshKey {
            private_key,
            passphrase,
        } => try_auth_keyfile(session, username, private_key, passphrase.as_deref()).await,
        AuthMethod::SshAgent => try_auth_agent(session, username).await,
        AuthMethod::Auto => {
            // Phase 2b: honor ~/.ssh/config IdentityFile / IdentitiesOnly.
            // Priority order matches OpenSSH's default behavior:
            //   1. SSH agent (unless IdentitiesOnly=yes)
            //   2. Keys from IdentityFile entries in ssh_config
            //   3. Default keys in ~/.ssh/id_*
            let host_cfg = crate::ssh_config::SshConfig::from_default_path()
                .map(|cfg| cfg.get_host_config(host))
                .unwrap_or_default();

            if !host_cfg.identities_only && try_auth_agent(session, username).await.is_ok() {
                return Ok(());
            }
            cancelled_check()?;

            // Try keys from ssh_config first (they're authoritative).
            for raw_key in &host_cfg.identity_files {
                let key_path = expand_tilde(raw_key);
                if !key_path.exists() {
                    continue;
                }
                if try_auth_keyfile(session, username, &key_path, None)
                    .await
                    .is_ok()
                {
                    return Ok(());
                }
                cancelled_check()?;
            }

            // Fallback to default keys unless IdentitiesOnly forbids it.
            if !host_cfg.identities_only {
                for key_path in default_key_files() {
                    if try_auth_keyfile(session, username, &key_path, None)
                        .await
                        .is_ok()
                    {
                        return Ok(());
                    }
                    cancelled_check()?;
                }
            }

            Err(VfsError::AuthenticationFailed(
                "No authentication method succeeded (tried SSH agent and SSH keys). \
                 Provide a password or specific key."
                    .into(),
            ))
        }
    }
}

async fn try_auth_password(
    session: &mut client::Handle<AcceptAllHandler>,
    username: &str,
    password: &str,
) -> VfsResult<()> {
    let res = session
        .authenticate_password(username, password)
        .await
        .map_err(|e| VfsError::AuthenticationFailed(format!("password auth error: {e}")))?;
    if res.success() {
        Ok(())
    } else {
        Err(VfsError::AuthenticationFailed("Password rejected".into()))
    }
}

async fn try_auth_keyfile(
    session: &mut client::Handle<AcceptAllHandler>,
    username: &str,
    key_path: &Path,
    passphrase: Option<&str>,
) -> VfsResult<()> {
    let key = load_secret_key(key_path, passphrase).map_err(|e| {
        VfsError::AuthenticationFailed(format!(
            "Failed to load private key '{}': {}",
            key_path.display(),
            e
        ))
    })?;
    authenticate_with_key(session, username, key).await
}

async fn authenticate_with_key(
    session: &mut client::Handle<AcceptAllHandler>,
    username: &str,
    key: PrivateKey,
) -> VfsResult<()> {
    let key_with_alg = PrivateKeyWithHashAlg::new(Arc::new(key), None);
    let res = session
        .authenticate_publickey(username, key_with_alg)
        .await
        .map_err(|e| VfsError::AuthenticationFailed(format!("publickey auth error: {e}")))?;
    if res.success() {
        Ok(())
    } else {
        Err(VfsError::AuthenticationFailed("Public key rejected".into()))
    }
}

#[cfg(unix)]
async fn try_auth_agent(
    session: &mut client::Handle<AcceptAllHandler>,
    username: &str,
) -> VfsResult<()> {
    use russh::keys::agent::client::AgentClient;
    if std::env::var_os("SSH_AUTH_SOCK").is_none() {
        return Err(VfsError::AuthenticationFailed(
            "SSH agent not available (SSH_AUTH_SOCK unset)".into(),
        ));
    }
    let mut agent = AgentClient::connect_env()
        .await
        .map_err(|e| VfsError::AuthenticationFailed(format!("agent connect failed: {e}")))?;
    let identities = agent
        .request_identities()
        .await
        .map_err(|e| VfsError::AuthenticationFailed(format!("agent identities failed: {e}")))?;
    for identity in identities {
        let pk = identity.public_key().into_owned();
        let res = session
            .authenticate_publickey_with(username, pk, None, &mut agent)
            .await;
        if let Ok(auth_result) = res {
            if auth_result.success() {
                return Ok(());
            }
        }
    }
    Err(VfsError::AuthenticationFailed(
        "SSH agent had no usable keys".into(),
    ))
}

#[cfg(not(unix))]
async fn try_auth_agent(
    _session: &mut client::Handle<AcceptAllHandler>,
    _username: &str,
) -> VfsResult<()> {
    Err(VfsError::AuthenticationFailed(
        "SSH agent not supported on this platform".into(),
    ))
}

/// Top-level async connect routine. Runs on the global runtime.
async fn do_connect(
    host: String,
    port: u16,
    username: String,
    options: ConnectOptions,
    cancelled: Arc<AtomicBool>,
) -> VfsResult<(SftpSession, Option<String>)> {
    if cancelled.load(Ordering::SeqCst) {
        return Err(VfsError::ConnectionFailed("Connection cancelled".into()));
    }
    let timeout_secs = options.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(timeout_secs * 5)),
        ..Default::default()
    });

    let handler = AcceptAllHandler;
    let addr = (host.as_str(), port);

    let connect_fut = client::connect(config, addr, handler);
    let mut session = tokio::time::timeout(Duration::from_secs(timeout_secs), connect_fut)
        .await
        .map_err(|_| VfsError::ConnectionFailed(format!("Connection to {host}:{port} timed out")))?
        .map_err(|e| VfsError::ConnectionFailed(format!("SSH connect failed: {e}")))?;

    if cancelled.load(Ordering::SeqCst) {
        return Err(VfsError::ConnectionFailed("Connection cancelled".into()));
    }

    try_authenticate(&mut session, &host, &username, &options.auth, &cancelled).await?;

    if cancelled.load(Ordering::SeqCst) {
        return Err(VfsError::ConnectionFailed("Connection cancelled".into()));
    }

    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| VfsError::ConnectionFailed(format!("SSH channel open failed: {e}")))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| VfsError::ConnectionFailed(format!("SFTP subsystem request failed: {e}")))?;

    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| VfsError::ConnectionFailed(format!("SFTP session init failed: {e}")))?;

    let home_dir = sftp
        .canonicalize(".")
        .await
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| Some(format!("/home/{username}")));

    Ok((sftp, home_dir))
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
        let guard = self.inner.lock().map_err(|_| VfsError::RemoteError {
            message: "SFTP state poisoned".into(),
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
