//! Async SFTP actor: command/reply protocol and the message loop that
//! owns the `SftpSession`, plus the low-level per-command operation
//! primitives it dispatches to.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, FileType as SftpFileType, OpenFlags};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc as async_mpsc, oneshot};

use crate::error::{VfsError, VfsResult};
use crate::types::{ConnectionState, VfsFileType, VfsMetadata};

use super::{block_on, SftpInner, CHUNK_SIZE, SHUTDOWN_TIMEOUT};

// ============================================================================
// Actor: commands, replies, task loop
// ============================================================================

pub(super) type Reply<T> = oneshot::Sender<VfsResult<T>>;

/// SFTP entry as crossed-over from the actor to the sync side.
/// Decoupled from russh_sftp's `DirEntry` so callers don't carry the dep.
pub(super) struct ActorEntry {
    pub(super) name: String,
    pub(super) metadata: VfsMetadata,
}

pub(super) enum SftpCommand {
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
pub(super) struct SftpHandle {
    pub(super) cmd_tx: async_mpsc::Sender<SftpCommand>,
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
    pub(super) fn dispatch<T, F>(&self, build: F) -> VfsResult<T>
    where
        F: FnOnce(Reply<T>) -> SftpCommand,
    {
        let (tx, rx) = oneshot::channel();
        let cmd = build(tx);
        block_on(async move {
            self.cmd_tx.send(cmd).await.map_err(|e| {
                log::debug!("sftp dispatch send failed (actor gone): {e}");
                VfsError::NotConnected
            })?;
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
pub(super) async fn sftp_actor(
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
