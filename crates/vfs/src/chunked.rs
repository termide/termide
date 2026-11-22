//! Uploads and downloads of a file or a whole tree, with progress, pause
//! and cancel, for providers that read and write a file a piece at a time
//! (SMB, NFS).
//!
//! A transfer runs on a thread of its own and makes blocking calls on the
//! provider's [`ChunkedRemote`]; each piece is one short call, so other
//! panels' requests on the same connection get their turn between pieces,
//! and a pause holds nothing on the server but the open file.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use crate::error::{VfsError, VfsResult};
use crate::local_part::{is_plain_entry_name, LocalPart};
use crate::types::{
    DownloadProgress, UploadProgress, VfsDownloadOperation, VfsEntry, VfsFileType, VfsMetadata,
    VfsOperation, VfsPath, VfsUploadOperation,
};
use crate::MAX_RECURSION_DEPTH;

/// Size of the pieces a local file is read in for an upload.
const UPLOAD_PIECE: usize = 1024 * 1024;

/// A remote file being read.
pub(crate) trait ChunkRead: Send {
    /// The next piece, `None` at the end.
    fn next_chunk(&mut self) -> VfsResult<Option<Vec<u8>>>;
}

/// A remote file being written from its start; what it held is gone.
pub(crate) trait ChunkWrite: Send {
    fn write_chunk(&mut self, data: &[u8]) -> VfsResult<()>;
    /// Make what was written stay, and close the file.
    fn finish(self: Box<Self>) -> VfsResult<()>;
    /// Close the file after a cancel or a failure, keeping what got there.
    fn abort(self: Box<Self>);
}

/// What a provider gives a transfer. Every call blocks; transfers make them
/// from their own thread, never from the provider's runtime.
pub(crate) trait ChunkedRemote: Send + Sync + 'static {
    fn metadata(&self, path: &VfsPath) -> VfsResult<VfsMetadata>;
    fn list(&self, path: &VfsPath) -> VfsResult<Vec<VfsEntry>>;
    fn create_dir_all(&self, path: &VfsPath) -> VfsResult<()>;
    fn open_read(&self, path: &VfsPath) -> VfsResult<Box<dyn ChunkRead>>;
    fn open_write(&self, path: &VfsPath) -> VfsResult<Box<dyn ChunkWrite>>;
}

/// The pause and cancel flags of one transfer.
#[derive(Clone)]
struct Flags {
    pause: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl Flags {
    fn new() -> Self {
        Self {
            pause: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Wait out a pause; `Cancelled` once the transfer is cancelled.
    fn checkpoint(&self) -> VfsResult<()> {
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(VfsError::Cancelled);
            }
            if !self.pause.load(Ordering::Relaxed) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// The batch's totals and how far it got.
struct Tally {
    total_files: usize,
    total_bytes: u64,
    files_done: usize,
    bytes_done: u64,
}

/// One file of a batch: its name, size and how much of it is through.
struct Piecewise {
    name: Option<String>,
    total: u64,
    done: u64,
}

impl Tally {
    fn upload(&self, file: &Piecewise) -> UploadProgress {
        UploadProgress {
            bytes_uploaded: self.bytes_done,
            total_bytes: self.total_bytes,
            current_file: file.name.clone(),
            files_uploaded: self.files_done,
            total_files: self.total_files,
            current_file_bytes: file.done,
            current_file_total: file.total,
        }
    }

    fn download(&self, file: &Piecewise) -> DownloadProgress {
        DownloadProgress {
            bytes_downloaded: self.bytes_done,
            total_bytes: self.total_bytes,
            current_file: file.name.clone(),
            files_downloaded: self.files_done,
            total_files: self.total_files,
            current_file_bytes: file.done,
            current_file_total: file.total,
        }
    }
}

/// Files and bytes under a local path; links and special files are not
/// uploaded, so they are not counted.
fn count_local(path: &Path, flags: &Flags) -> VfsResult<(usize, u64)> {
    flags.checkpoint()?;
    let meta = fs::metadata(path)?;
    if !meta.is_dir() {
        return Ok((1, meta.len()));
    }
    let (mut files, mut bytes) = (0, 0);
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let (f, b) = if kind.is_dir() {
            count_local(&entry.path(), flags)?
        } else if kind.is_file() {
            (1, entry.metadata().map(|m| m.len()).unwrap_or(0))
        } else {
            (0, 0)
        };
        files += f;
        bytes += b;
    }
    Ok((files, bytes))
}

/// Files and bytes under a remote directory, links left out.
fn count_remote(
    remote: &dyn ChunkedRemote,
    path: &VfsPath,
    flags: &Flags,
    depth: usize,
) -> VfsResult<(usize, u64)> {
    flags.checkpoint()?;
    let (mut files, mut bytes) = (0, 0);
    for entry in remote.list(path)? {
        match entry.metadata.file_type {
            VfsFileType::Directory if depth < MAX_RECURSION_DEPTH => {
                let (f, b) = count_remote(remote, &entry.path, flags, depth + 1)?;
                files += f;
                bytes += b;
            }
            VfsFileType::File => {
                files += 1;
                bytes += entry.metadata.size;
            }
            _ => {}
        }
    }
    Ok((files, bytes))
}

fn upload_file(
    remote: &dyn ChunkedRemote,
    local: &Path,
    dest: &VfsPath,
    flags: &Flags,
    progress: &Sender<UploadProgress>,
    tally: &mut Tally,
) -> VfsResult<()> {
    let mut source = fs::File::open(local)?;
    let mut file = Piecewise {
        name: local.file_name().map(|n| n.to_string_lossy().into_owned()),
        total: source.metadata().map(|m| m.len()).unwrap_or(0),
        done: 0,
    };
    let _ = progress.send(tally.upload(&file));
    let mut writer = remote.open_write(dest)?;
    let mut buf = vec![0; UPLOAD_PIECE];
    let result = (|| loop {
        flags.checkpoint()?;
        let n = source.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        writer.write_chunk(&buf[..n])?;
        file.done += n as u64;
        tally.bytes_done += n as u64;
        let _ = progress.send(tally.upload(&file));
    })();
    match result {
        Ok(()) => writer.finish()?,
        Err(e) => {
            writer.abort();
            return Err(e);
        }
    }
    tally.files_done += 1;
    let _ = progress.send(tally.upload(&file));
    Ok(())
}

fn upload_tree(
    remote: &dyn ChunkedRemote,
    local: &Path,
    dest: &VfsPath,
    flags: &Flags,
    progress: &Sender<UploadProgress>,
    tally: &mut Tally,
) -> VfsResult<()> {
    remote.create_dir_all(dest)?;
    for entry in fs::read_dir(local)? {
        let entry = entry?;
        flags.checkpoint()?;
        let kind = entry.file_type()?;
        let name = entry.file_name();
        let child = dest.join(&name);
        if kind.is_dir() {
            upload_tree(remote, &entry.path(), &child, flags, progress, tally)?;
        } else if kind.is_file() {
            upload_file(remote, &entry.path(), &child, flags, progress, tally)?;
        }
    }
    Ok(())
}

fn run_upload(
    remote: &dyn ChunkedRemote,
    local: &Path,
    dest: &VfsPath,
    flags: &Flags,
    progress: &Sender<UploadProgress>,
) -> VfsResult<()> {
    let (total_files, total_bytes) = count_local(local, flags)?;
    let mut tally = Tally {
        total_files,
        total_bytes,
        files_done: 0,
        bytes_done: 0,
    };
    if fs::metadata(local)?.is_dir() {
        upload_tree(remote, local, dest, flags, progress, &mut tally)
    } else {
        if let Some(parent) = dest.parent() {
            if parent.path.components().count() > 1 {
                remote.create_dir_all(&parent)?;
            }
        }
        upload_file(remote, local, dest, flags, progress, &mut tally)
    }
}

fn download_file(
    remote: &dyn ChunkedRemote,
    source: &VfsPath,
    size: u64,
    local: &Path,
    flags: &Flags,
    progress: &Sender<DownloadProgress>,
    tally: &mut Tally,
) -> VfsResult<()> {
    if let Some(parent) = local.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = Piecewise {
        name: source.file_name().map(|n| n.to_string_lossy().into_owned()),
        total: size,
        done: 0,
    };
    let _ = progress.send(tally.download(&file));
    let mut reader = remote.open_read(source)?;
    // Into a sibling, put in place once whole; dropped on an early return.
    let mut target = LocalPart::create(local)?;
    loop {
        flags.checkpoint()?;
        let Some(piece) = reader.next_chunk()? else {
            break;
        };
        target.write_all(&piece)?;
        file.done += piece.len() as u64;
        tally.bytes_done += piece.len() as u64;
        let _ = progress.send(tally.download(&file));
    }
    target.flush()?;
    target.persist(local)?;
    tally.files_done += 1;
    let _ = progress.send(tally.download(&file));
    Ok(())
}

fn download_tree(
    remote: &dyn ChunkedRemote,
    source: &VfsPath,
    local: &Path,
    flags: &Flags,
    progress: &Sender<DownloadProgress>,
    tally: &mut Tally,
    depth: usize,
) -> VfsResult<()> {
    fs::create_dir_all(local)?;
    for entry in remote.list(source)? {
        flags.checkpoint()?;
        if !is_plain_entry_name(&entry.name) {
            return Err(VfsError::InvalidPath(entry.name));
        }
        let child = local.join(&entry.name);
        match entry.metadata.file_type {
            // An error, not a skip: a move deletes its source once this returns.
            VfsFileType::Directory if depth >= MAX_RECURSION_DEPTH => {
                return Err(VfsError::InvalidPath(format!(
                    "{}: nested too deep",
                    entry.path
                )));
            }
            VfsFileType::Directory => {
                download_tree(
                    remote,
                    &entry.path,
                    &child,
                    flags,
                    progress,
                    tally,
                    depth + 1,
                )?;
            }
            VfsFileType::File => {
                let size = entry.metadata.size;
                download_file(remote, &entry.path, size, &child, flags, progress, tally)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn run_download(
    remote: &dyn ChunkedRemote,
    source: &VfsPath,
    local: &Path,
    flags: &Flags,
    progress: &Sender<DownloadProgress>,
) -> VfsResult<PathBuf> {
    let meta = remote.metadata(source)?;
    let is_dir = meta.file_type == VfsFileType::Directory;
    let (total_files, total_bytes) = if is_dir {
        count_remote(remote, source, flags, 0)?
    } else {
        (1, meta.size)
    };
    let mut tally = Tally {
        total_files,
        total_bytes,
        files_done: 0,
        bytes_done: 0,
    };
    if is_dir {
        download_tree(remote, source, local, flags, progress, &mut tally, 0)?;
    } else {
        download_file(
            remote, source, meta.size, local, flags, progress, &mut tally,
        )?;
    }
    Ok(local.to_path_buf())
}

struct Started<T, P> {
    done: Receiver<VfsResult<T>>,
    progress: Receiver<P>,
    flags: Flags,
}

fn start_upload(
    remote: Arc<dyn ChunkedRemote>,
    local: &Path,
    dest: &VfsPath,
) -> Started<(), UploadProgress> {
    let (done_tx, done) = mpsc::channel();
    let (progress_tx, progress) = mpsc::channel();
    let flags = Flags::new();
    let (local, dest, worker_flags) = (local.to_path_buf(), dest.clone(), flags.clone());
    std::thread::spawn(move || {
        let result = run_upload(&*remote, &local, &dest, &worker_flags, &progress_tx);
        let _ = done_tx.send(result);
    });
    Started {
        done,
        progress,
        flags,
    }
}

fn start_download(
    remote: Arc<dyn ChunkedRemote>,
    source: &VfsPath,
    local: &Path,
) -> Started<PathBuf, DownloadProgress> {
    let (done_tx, done) = mpsc::channel();
    let (progress_tx, progress) = mpsc::channel();
    let flags = Flags::new();
    let (source, local, worker_flags) = (source.clone(), local.to_path_buf(), flags.clone());
    std::thread::spawn(move || {
        let result = run_download(&*remote, &source, &local, &worker_flags, &progress_tx);
        let _ = done_tx.send(result);
    });
    Started {
        done,
        progress,
        flags,
    }
}

/// Upload the local file or directory `local` to `dest`.
pub(crate) fn upload(
    remote: Arc<dyn ChunkedRemote>,
    local: &Path,
    dest: &VfsPath,
) -> VfsOperation<()> {
    VfsOperation::new(start_upload(remote, local, dest).done)
}

/// [`upload`], reporting progress and taking pause and cancel.
pub(crate) fn upload_with_progress(
    remote: Arc<dyn ChunkedRemote>,
    local: &Path,
    dest: &VfsPath,
) -> VfsUploadOperation {
    let started = start_upload(remote, local, dest);
    VfsUploadOperation::new(
        started.done,
        started.progress,
        started.flags.pause,
        started.flags.cancel,
    )
}

/// Download the remote file or directory `source` to `local`.
pub(crate) fn download(
    remote: Arc<dyn ChunkedRemote>,
    source: &VfsPath,
    local: &Path,
) -> VfsOperation<PathBuf> {
    VfsOperation::new(start_download(remote, source, local).done)
}

/// [`download`], reporting progress and taking pause and cancel.
pub(crate) fn download_with_progress(
    remote: Arc<dyn ChunkedRemote>,
    source: &VfsPath,
    local: &Path,
) -> VfsDownloadOperation {
    let started = start_download(remote, source, local);
    VfsDownloadOperation::new(
        started.done,
        started.progress,
        started.flags.pause,
        started.flags.cancel,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::VfsProtocol;

    /// A remote holding one directory `/d` with `entries`, every file of
    /// which reads as `pieces`, `Err` ending the read with a failure.
    struct Fake {
        entries: Vec<VfsEntry>,
        pieces: Vec<Option<Vec<u8>>>,
    }

    struct Pieces(std::vec::IntoIter<Option<Vec<u8>>>);

    impl ChunkRead for Pieces {
        fn next_chunk(&mut self) -> VfsResult<Option<Vec<u8>>> {
            match self.0.next() {
                Some(Some(piece)) => Ok(Some(piece)),
                Some(None) => Err(VfsError::Nfs("connection lost".into())),
                None => Ok(None),
            }
        }
    }

    impl ChunkedRemote for Fake {
        fn metadata(&self, path: &VfsPath) -> VfsResult<VfsMetadata> {
            Ok(if path.path == Path::new("/d") {
                VfsMetadata::directory()
            } else {
                VfsMetadata::file(3)
            })
        }
        fn list(&self, _: &VfsPath) -> VfsResult<Vec<VfsEntry>> {
            Ok(self.entries.clone())
        }
        fn create_dir_all(&self, _: &VfsPath) -> VfsResult<()> {
            Ok(())
        }
        fn open_read(&self, _: &VfsPath) -> VfsResult<Box<dyn ChunkRead>> {
            Ok(Box::new(Pieces(self.pieces.clone().into_iter())))
        }
        fn open_write(&self, _: &VfsPath) -> VfsResult<Box<dyn ChunkWrite>> {
            unimplemented!()
        }
    }

    fn at(path: &str) -> VfsPath {
        VfsPath::remote(VfsProtocol::Nfs, "host", path)
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_listed_name_with_a_parent_is_not_downloaded() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake {
            entries: vec![VfsEntry::new(
                "../escaped",
                at("/d/../escaped"),
                VfsMetadata::file(3),
            )],
            pieces: vec![Some(b"bad".to_vec())],
        };
        let (tx, _rx) = mpsc::channel();
        let dest = tmp.path().join("d");
        let result = run_download(&fake, &at("/d"), &dest, &Flags::new(), &tx);
        assert!(
            matches!(result, Err(VfsError::InvalidPath(_))),
            "{result:?}"
        );
        assert_eq!(names(tmp.path()), ["d"]);
        assert!(names(&dest).is_empty());
    }

    #[test]
    fn a_failed_download_leaves_the_file_it_was_to_replace() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("f");
        fs::write(&local, b"old content").unwrap();
        let (tx, _rx) = mpsc::channel();

        let cut = Fake {
            entries: vec![],
            pieces: vec![Some(b"ne".to_vec()), None],
        };
        assert!(run_download(&cut, &at("/f"), &local, &Flags::new(), &tx).is_err());
        assert_eq!(fs::read(&local).unwrap(), b"old content");
        assert_eq!(names(tmp.path()), ["f"]);

        let cancelled = Flags::new();
        cancelled.cancel.store(true, Ordering::Relaxed);
        let whole = Fake {
            entries: vec![],
            pieces: vec![Some(b"new".to_vec())],
        };
        let result = run_download(&whole, &at("/f"), &local, &cancelled, &tx);
        assert!(matches!(result, Err(VfsError::Cancelled)), "{result:?}");
        assert_eq!(fs::read(&local).unwrap(), b"old content");

        run_download(&whole, &at("/f"), &local, &Flags::new(), &tx).unwrap();
        assert_eq!(fs::read(&local).unwrap(), b"new");
        assert_eq!(names(tmp.path()), ["f"]);
    }
}
