//! Sync worker helpers driving download/upload transfers and recursive
//! walks by dispatching short atomic chunk-as-command primitives to the
//! actor, so pause/cancel and cross-panel work stay responsive.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc};
use std::time::Duration;

use crate::error::{VfsError, VfsResult};
use crate::types::{DownloadProgress, UploadProgress, VfsFileType};

use super::actor::{SftpCommand, SftpHandle};
use super::CHUNK_SIZE;

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
pub(super) fn worker_count_remote(
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

pub(super) fn count_local_files_sync(
    path: &Path,
    cancel: &Arc<AtomicBool>,
) -> VfsResult<(usize, u64)> {
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
pub(super) struct DlState {
    pub(super) total_files: usize,
    pub(super) total_bytes: u64,
    pub(super) files_done: usize,
    pub(super) bytes_done: u64,
}

pub(super) struct UlState {
    pub(super) total_files: usize,
    pub(super) total_bytes: u64,
    pub(super) files_done: usize,
    pub(super) bytes_done: u64,
}

pub(super) fn worker_download_file(
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
pub(super) fn worker_download_dir(
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

pub(super) fn worker_upload_file(
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

pub(super) fn worker_upload_dir(
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
