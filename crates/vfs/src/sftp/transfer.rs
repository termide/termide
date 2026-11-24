//! Sync worker helpers driving download/upload transfers and recursive
//! walks by dispatching short atomic chunk-as-command primitives to the
//! actor, so pause/cancel and cross-panel work stay responsive.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc};
use std::time::Duration;

use crate::error::{VfsError, VfsResult};
use crate::local_part::{is_plain_entry_name, LocalPart};
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
        // Mirror `worker_upload_dir`: descend only real directories and
        // count only regular files. Following symlinks here overcounts
        // and loops forever on a cycle such as `link -> ..`.
        let ft = entry.file_type().map_err(VfsError::Io)?;
        let (c, b) = if ft.is_dir() {
            count_local_files_sync(&entry.path(), cancel)?
        } else if ft.is_file() {
            (1, entry.metadata().map(|m| m.len()).unwrap_or(0))
        } else {
            (0, 0)
        };
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

    // Into a sibling, put in place once whole: a cancelled or failed
    // download leaves the file it was to replace as it was, and nothing
    // half-written (the sibling goes when dropped).
    let mut dst = LocalPart::create(local).map_err(VfsError::Io)?;
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
    dst.persist(local).map_err(VfsError::Io)?;
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
        if !is_plain_entry_name(&entry.name) {
            return Err(VfsError::InvalidPath(entry.name));
        }
        let remote_child = remote.join(&entry.name);
        let local_child = local.join(&entry.name);
        if matches!(entry.metadata.file_type, VfsFileType::Directory) {
            // Defensive depth guard mirroring `worker_count_remote`. An
            // error, not a skip: a move deletes its source once this returns.
            if depth == 0 {
                return Err(VfsError::InvalidPath(format!(
                    "{}: nested too deep",
                    remote_child.display()
                )));
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

/// A hidden name next to `remote`, unique to this transfer: `.name.<tag>.<n>`.
fn remote_sibling(remote: &Path, tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = remote.file_name().unwrap_or_default().to_string_lossy();
    remote.with_file_name(format!(".{name}.{tag}.{}-{n}", std::process::id()))
}

/// Upload `local` to `remote`. A regular file there, or none, is replaced
/// only once the new content is whole on the server: it goes to a hidden
/// sibling first, then takes the old file's place, so a cancelled or cut
/// upload leaves the old file as it was. A server offering
/// `posix-rename@openssh.com` (OpenSSH does) swaps them in one step. SFTP
/// v3 itself renames only onto a free name, so elsewhere the old file
/// steps aside for the moment of the swap and comes back if the swap
/// fails. A symlink at `remote` is written through, as before, rather than
/// replaced by a file.
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
    let p = remote.to_path_buf();
    let existing = handle
        .dispatch(move |reply| SftpCommand::Lstat { path: p, reply })
        .ok();
    let replaces = match &existing {
        None => true,
        Some(meta) => matches!(meta.file_type, VfsFileType::File),
    };
    if !replaces {
        return upload_to(
            handle,
            local,
            remote,
            false,
            None,
            pause,
            cancel,
            progress_tx,
            state,
        );
    }

    let part = remote_sibling(remote, "termide-part");
    let remove = |path: &Path| {
        let p = path.to_path_buf();
        handle.dispatch(move |reply| SftpCommand::DeleteRecursive {
            path: p,
            depth_limit: 1,
            reply,
        })
    };
    let rename = |from: &Path, to: &Path| {
        let (from, to) = (from.to_path_buf(), to.to_path_buf());
        handle.dispatch(move |reply| SftpCommand::Rename { from, to, reply })
    };
    // The file replaced keeps its permission bits. The part is created
    // with them, so it is never more open than the file while it is being
    // written (the server's umask can only narrow them), and set to them
    // exactly before the swap: a part left narrower stops the replace.
    let mode = existing
        .as_ref()
        .and_then(|m| m.permissions)
        .map(|m| m & 0o7777);
    if let Err(e) = upload_to(
        handle,
        local,
        &part,
        true,
        mode.map(|m| m & 0o777),
        pause,
        cancel,
        progress_tx,
        state,
    ) {
        let _ = remove(&part);
        return Err(e);
    }
    if let Some(mode) = mode {
        if let Err(e) = restore_mode(handle, &part, mode) {
            let _ = remove(&part);
            return Err(VfsError::RemoteError {
                message: format!(
                    "{}: could not keep its mode {mode:o}: {e}",
                    remote.display()
                ),
            });
        }
    }
    if existing.is_some() {
        // One step where the server offers it: the name holds the old file
        // until it holds the new one.
        let (from, to) = (part.clone(), remote.to_path_buf());
        match handle.dispatch(move |reply| SftpCommand::RenameOver { from, to, reply }) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(e) => {
                let _ = remove(&part);
                return Err(e);
            }
        }
    }
    let aside = existing
        .is_some()
        .then(|| remote_sibling(remote, "termide-old"));
    if let Some(aside) = &aside {
        if let Err(e) = rename(remote, aside) {
            let _ = remove(&part);
            return Err(e);
        }
    }
    if let Err(e) = rename(&part, remote) {
        let _ = remove(&part);
        if let Some(aside) = &aside {
            if let Err(back) = rename(aside, remote) {
                // The original is not lost, only renamed: say where it is.
                return Err(VfsError::RemoteError {
                    message: format!(
                        "{e}; the original is kept as {} (could not rename it back: {back})",
                        aside.display()
                    ),
                });
            }
        }
        return Err(e);
    }
    if let Some(aside) = &aside {
        if let Err(e) = remove(aside) {
            log::warn!("Could not remove the replaced {}: {e}", aside.display());
        }
    }
    Ok(())
}

/// Set `path`'s permission bits to `mode`. A server refusing SETSTAT is
/// forgiven when the file already has them (created with `mode` under a
/// umask that took nothing away).
fn restore_mode(handle: &SftpHandle, path: &Path, mode: u32) -> VfsResult<()> {
    let p = path.to_path_buf();
    let set = handle.dispatch(move |reply| SftpCommand::SetMode {
        path: p,
        mode,
        reply,
    });
    let Err(e) = set else { return Ok(()) };
    let p = path.to_path_buf();
    let has = handle
        .dispatch(move |reply| SftpCommand::Lstat { path: p, reply })
        .ok()
        .and_then(|m| m.permissions)
        .map(|m| m & 0o7777);
    if has == Some(mode) {
        Ok(())
    } else {
        Err(e)
    }
}

/// Write `local` into `remote`, a fresh file when `exclusive`; a file
/// created gets `mode` (less the server's umask).
#[allow(clippy::too_many_arguments)]
fn upload_to(
    handle: &SftpHandle,
    local: &Path,
    remote: &Path,
    exclusive: bool,
    mode: Option<u32>,
    pause: &Arc<AtomicBool>,
    cancel: &Arc<AtomicBool>,
    progress_tx: &std_mpsc::Sender<UploadProgress>,
    state: &mut UlState,
) -> VfsResult<()> {
    let file_total = std::fs::metadata(local).map(|m| m.len()).unwrap_or(0);
    let mut src = std::fs::File::open(local).map_err(VfsError::Io)?;

    let p = remote.to_path_buf();
    let remote_id = handle.dispatch(move |reply| SftpCommand::OpenWrite {
        path: p,
        exclusive,
        mode,
        reply,
    })?;
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_remote_sibling_is_hidden_unique_and_beside_the_target() {
        let target = Path::new("/srv/www/index.html");
        let a = remote_sibling(target, "termide-part");
        let b = remote_sibling(target, "termide-part");
        assert_ne!(a, b);
        assert_eq!(a.parent(), target.parent());
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with(".index.html.termide-part."), "{name}");
    }

    fn sftp_server_path() -> Option<&'static str> {
        [
            "/usr/libexec/sftp-server",
            "/usr/lib/openssh/sftp-server",
            "/usr/lib/ssh/sftp-server",
            "/usr/libexec/openssh/sftp-server",
        ]
        .into_iter()
        .find(|p| Path::new(p).exists())
    }

    /// A fresh sftp-server's stdio. The child lives as long as the
    /// session's runtime task.
    fn spawn_sftp_server(
        server: &str,
    ) -> Option<impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> {
        let mut child = tokio::process::Command::new(server)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        let stream = tokio::io::join(child.stdout.take()?, child.stdin.take()?);
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        Some(stream)
    }

    /// OpenSSH's sftp-server where this system has one, with an actor
    /// serving it, as a connection would set up: the second session for
    /// `posix-rename@openssh.com` is another sftp-server.
    fn local_sftp_server() -> Option<SftpHandle> {
        local_sftp_server_with(true)
    }

    /// As `local_sftp_server`, without the second session when
    /// `posix_rename` is false: as a server that does not offer it.
    fn local_sftp_server_with(posix_rename: bool) -> Option<SftpHandle> {
        let server = sftp_server_path()?;
        let rt = super::super::try_runtime().ok()?;
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let session = rt.block_on(async {
            let stream = spawn_sftp_server(server)?;
            russh_sftp::client::SftpSession::new(stream).await.ok()
        })?;
        let opener = posix_rename.then(|| -> super::super::actor::RawSessionOpener {
            Box::new(move || {
                Box::pin(async move {
                    let stream = spawn_sftp_server(server)?;
                    Some(russh_sftp::client::RawSftpSession::new(stream))
                })
            })
        });
        let inner = Arc::new(std::sync::Mutex::new(super::super::SftpInner::new()));
        rt.spawn(super::super::actor::sftp_actor(session, rx, inner, opener));
        Some(SftpHandle { cmd_tx: tx })
    }

    fn upload(handle: &SftpHandle, local: &Path, remote: &Path) -> VfsResult<()> {
        let (tx, _rx) = std_mpsc::channel();
        let no = Arc::new(AtomicBool::new(false));
        worker_upload_file(
            handle,
            local,
            remote,
            &no,
            &no,
            &tx,
            &mut UlState {
                total_files: 1,
                total_bytes: 0,
                files_done: 0,
                bytes_done: 0,
            },
        )
    }

    /// In one step with `posix-rename@openssh.com`, and in two without.
    #[test]
    fn an_upload_replaces_the_file_whole_and_keeps_its_mode() {
        for posix_rename in [true, false] {
            let Some(handle) = local_sftp_server_with(posix_rename) else {
                eprintln!("no sftp-server here; skipped");
                return;
            };
            upload_replaces_whole_and_keeps_mode(&handle);
        }
    }

    fn upload_replaces_whole_and_keeps_mode(handle: &SftpHandle) {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let remote = dir.join("run.sh");
        std::fs::write(&remote, b"old content, longer than the new").unwrap();
        std::fs::set_permissions(&remote, std::fs::Permissions::from_mode(0o750)).unwrap();
        let local = dir.join("local.sh");
        std::fs::write(&local, b"new").unwrap();

        upload(handle, &local, &remote).unwrap();
        assert_eq!(std::fs::read(&remote).unwrap(), b"new");
        let mode = std::fs::metadata(&remote).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o750);
        let mut left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["local.sh", "run.sh"]);

        // A new name is created; a symlink is written through.
        let fresh = dir.join("fresh");
        upload(handle, &local, &fresh).unwrap();
        assert_eq!(std::fs::read(&fresh).unwrap(), b"new");
        let target = dir.join("target");
        std::fs::write(&target, b"old").unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        std::fs::write(&local, b"through").unwrap();
        upload(handle, &local, &link).unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(&target).unwrap(), b"through");
    }

    /// The part written for a private file is private from its creation,
    /// not only once the content is whole.
    #[test]
    fn the_part_of_a_private_file_is_private_while_written() {
        use std::os::unix::fs::PermissionsExt as _;
        let Some(handle) = local_sftp_server() else {
            eprintln!("no sftp-server here; skipped");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let remote = dir.join("secret");
        std::fs::write(&remote, b"old").unwrap();
        std::fs::set_permissions(&remote, std::fs::Permissions::from_mode(0o600)).unwrap();
        let local = dir.join("local");
        std::fs::write(&local, b"new").unwrap();

        let (tx, rx) = std_mpsc::channel();
        let pause = Arc::new(AtomicBool::new(true));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut state = UlState {
            total_files: 1,
            total_bytes: 3,
            files_done: 0,
            bytes_done: 0,
        };
        std::thread::scope(|s| {
            let upload = s.spawn(|| {
                worker_upload_file(&handle, &local, &remote, &pause, &cancel, &tx, &mut state)
            });
            // The first progress comes once the part is open, before any data.
            rx.recv().unwrap();
            let part = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().path())
                .find(|p| p.to_string_lossy().contains("termide-part"))
                .expect("the part file");
            let mode = std::fs::metadata(&part).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            pause.store(false, Ordering::Relaxed);
            upload.join().unwrap().unwrap();
        });
        assert_eq!(std::fs::read(&remote).unwrap(), b"new");
        let mode = std::fs::metadata(&remote).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_cancelled_upload_leaves_the_old_file() {
        let Some(handle) = local_sftp_server() else {
            eprintln!("no sftp-server here; skipped");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let remote = dir.join("data");
        std::fs::write(&remote, b"old").unwrap();
        let local = dir.join("local");
        std::fs::write(&local, b"new").unwrap();

        let (tx, _rx) = std_mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(true));
        let pause = Arc::new(AtomicBool::new(false));
        let mut state = UlState {
            total_files: 1,
            total_bytes: 3,
            files_done: 0,
            bytes_done: 0,
        };
        let result = worker_upload_file(&handle, &local, &remote, &pause, &cancel, &tx, &mut state);
        assert!(matches!(result, Err(VfsError::Cancelled)), "{result:?}");
        assert_eq!(std::fs::read(&remote).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    }

    fn rename_over(handle: &SftpHandle, from: &Path, to: &Path) -> VfsResult<bool> {
        let (from, to) = (from.to_path_buf(), to.to_path_buf());
        handle.dispatch(move |reply| SftpCommand::RenameOver { from, to, reply })
    }

    /// OpenSSH offers `posix-rename@openssh.com`: the new file takes the
    /// name in one step. Without the extension nothing is done.
    #[test]
    fn a_rename_over_replaces_in_one_step_where_offered() {
        let (Some(offered), Some(plain)) = (local_sftp_server(), local_sftp_server_with(false))
        else {
            eprintln!("no sftp-server here; skipped");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let (part, target) = (tmp.path().join("part"), tmp.path().join("target"));
        std::fs::write(&part, b"new").unwrap();
        std::fs::write(&target, b"old").unwrap();

        assert!(!rename_over(&plain, &part, &target).unwrap());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert!(part.exists());

        assert!(rename_over(&offered, &part, &target).unwrap());
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!part.exists());

        // A failed rename is an error, not a fallback.
        let missing = tmp.path().join("missing");
        assert!(rename_over(&offered, &missing, &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
    }

    fn create_new(handle: &SftpHandle, path: &Path) -> VfsResult<()> {
        let path = path.to_path_buf();
        handle.dispatch(move |reply| SftpCommand::CreateNew { path, reply })
    }

    /// "New file" on a taken name is refused by the server and reported as
    /// such; what has the name — a file, a dangling link — stays.
    #[test]
    fn an_exclusive_create_refuses_a_taken_name() {
        let Some(handle) = local_sftp_server_with(false) else {
            eprintln!("no sftp-server here; skipped");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("notes");
        std::fs::write(&file, b"keep me").unwrap();
        let taken = create_new(&handle, &file);
        assert!(
            matches!(taken, Err(VfsError::AlreadyExists { .. })),
            "{taken:?}"
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"keep me");

        let link = tmp.path().join("dangling");
        let target = tmp.path().join("target");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let taken = create_new(&handle, &link);
        assert!(
            matches!(taken, Err(VfsError::AlreadyExists { .. })),
            "{taken:?}"
        );
        assert!(!target.exists());

        let fresh = tmp.path().join("fresh");
        create_new(&handle, &fresh).unwrap();
        assert_eq!(std::fs::read(&fresh).unwrap(), b"");

        // A missing directory is not "already exists".
        let nowhere = tmp.path().join("no/such/dir");
        let missing = create_new(&handle, &nowhere);
        assert!(
            !matches!(missing, Err(VfsError::AlreadyExists { .. }) | Ok(())),
            "{missing:?}"
        );
    }

    #[test]
    fn local_upload_count_skips_symlinks_and_survives_cycles() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a"), b"12").unwrap();
        std::fs::write(root.join("sub/b"), b"345").unwrap();
        std::os::unix::fs::symlink("..", root.join("sub/loop")).unwrap();
        std::os::unix::fs::symlink("a", root.join("alias")).unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        assert_eq!(count_local_files_sync(&root, &cancel).unwrap(), (2, 5));
    }
}
