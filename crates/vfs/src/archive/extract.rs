//! Copying archive entries out to the local filesystem.
//!
//! Invariants:
//! - Nothing is written through a symlink found on disk below the
//!   destination's parent: every directory on the way is checked with
//!   lstat, and files are opened with `O_NOFOLLOW`. A link already sitting
//!   where a file goes is replaced; one where a directory goes fails the
//!   extraction.
//! - Symlinks from the archive are created only after every file has been
//!   written, and only when the kernel's resolution of the target — links
//!   in the archive included — stays inside the extracted subtree.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use super::index::{EntrySource, Node, NodeKind};
use super::source::copy_error;
use super::OpenArchive;
use crate::error::{VfsError, VfsResult};
use crate::types::{DownloadProgress, VfsMetadata};

const COPY_BUFFER: usize = 256 * 1024;

/// Pause and cancel flags shared with the operation handle.
pub(crate) struct Control {
    pub(crate) pause: Arc<AtomicBool>,
    pub(crate) cancel: Arc<AtomicBool>,
}

impl Control {
    pub(crate) fn none() -> Self {
        Self {
            pause: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Block while paused; fail once cancelled.
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

/// One archive entry and every local file that receives its bytes (more
/// than one when hard links point at it).
struct Job {
    size: u64,
    dests: Vec<(PathBuf, VfsMetadata)>,
}

struct Progress<'a> {
    tx: Option<&'a Sender<DownloadProgress>>,
    state: DownloadProgress,
}

impl Progress<'_> {
    fn start_file(&mut self, name: &Path, size: u64) {
        self.state.current_file = name.file_name().map(|n| n.to_string_lossy().into_owned());
        self.state.current_file_bytes = 0;
        self.state.current_file_total = size;
        self.send();
    }

    fn advance(&mut self, bytes: u64) {
        self.state.bytes_downloaded += bytes;
        self.state.current_file_bytes += bytes;
        self.send();
    }

    fn finish_file(&mut self) {
        self.state.files_downloaded += 1;
        self.send();
    }

    fn send(&self) {
        if let Some(tx) = self.tx {
            let _ = tx.send(self.state.clone());
        }
    }
}

/// Extract the entry at inner path `key` (a file or a whole directory) to
/// the local path `dest`, which names the extracted item itself.
pub(crate) fn extract(
    archive: &OpenArchive,
    key: &Path,
    dest: &Path,
    control: &Control,
    progress_tx: Option<&Sender<DownloadProgress>>,
) -> VfsResult<PathBuf> {
    let (top, node) = archive
        .index
        .resolve(key)
        .ok_or_else(|| VfsError::NotFound {
            path: key.to_path_buf(),
        })?;

    // The destination's parent is the user's choice and trusted as is;
    // everything from `dest` down is checked before it is written.
    let base = dest.parent().unwrap_or(dest);
    fs::create_dir_all(base)?;

    let mut jobs: BTreeMap<EntrySource, Job> = BTreeMap::new();
    let mut dirs = Vec::new();
    let mut links = Vec::new();

    match node.kind {
        NodeKind::File(source) => add_job(&mut jobs, source, node, dest.to_path_buf()),
        NodeKind::Dir => {
            for (path, node) in archive.index.subtree(&top) {
                let rel = path.strip_prefix(&top).unwrap_or(Path::new(""));
                let target = dest.join(rel);
                match &node.kind {
                    NodeKind::Dir => dirs.push(target),
                    NodeKind::File(source) => add_job(&mut jobs, *source, node, target),
                    NodeKind::HardLink(_) => match archive.index.resolve(path) {
                        Some((_, resolved)) => match resolved.kind {
                            NodeKind::File(source) => add_job(&mut jobs, source, resolved, target),
                            _ => log::warn!("archive: hard link to a non-file {}", path.display()),
                        },
                        None => log::warn!("archive: dangling hard link {}", path.display()),
                    },
                    NodeKind::Symlink(link) => {
                        match archive.index.link_target_within(path, link, &top) {
                            Some(_) => links.push((target, link.clone())),
                            None => log::warn!(
                                "archive: not creating {} -> {link}, it leaves the extracted tree",
                                path.display()
                            ),
                        }
                    }
                }
            }
        }
        NodeKind::Symlink(_) | NodeKind::HardLink(_) => unreachable!("resolve follows links"),
    }

    let mut progress = Progress {
        tx: progress_tx,
        state: DownloadProgress {
            bytes_downloaded: 0,
            total_bytes: jobs.values().map(|j| j.size * j.dests.len() as u64).sum(),
            current_file: None,
            files_downloaded: 0,
            total_files: jobs.values().map(|j| j.dests.len()).sum(),
            current_file_bytes: 0,
            current_file_total: 0,
        },
    };
    progress.send();

    for dir in &dirs {
        ensure_dir(base, dir)?;
    }

    let sources: Vec<EntrySource> = jobs.keys().copied().collect();
    archive.backend.read_sources(&sources, |source, reader| {
        let job = &jobs[&source];
        let (first, first_meta) = &job.dests[0];
        progress.start_file(first, job.size);
        let out = write_entry(reader, base, first, job.size, control, &mut progress)?;
        apply_metadata(&out, first_meta);
        progress.finish_file();
        for (copy, meta) in &job.dests[1..] {
            control.checkpoint()?;
            progress.start_file(copy, job.size);
            let mut out = create_file(base, copy)?;
            std::io::copy(&mut File::open(first)?, &mut out)?;
            apply_metadata(&out, meta);
            progress.advance(job.size);
            progress.finish_file();
        }
        Ok(())
    })?;

    for (target, link) in links {
        match target.parent().map(|parent| ensure_dir(base, parent)) {
            Some(Err(e)) => log::warn!("archive: not creating {}: {e}", target.display()),
            _ => create_symlink(&link, &target),
        }
    }
    Ok(dest.to_path_buf())
}

/// Read one whole file entry into memory.
pub(crate) fn read_to_vec(archive: &OpenArchive, key: &Path) -> VfsResult<Vec<u8>> {
    let (_, node) = archive
        .index
        .resolve(key)
        .ok_or_else(|| VfsError::NotFound {
            path: key.to_path_buf(),
        })?;
    let NodeKind::File(source) = node.kind else {
        return Err(VfsError::InvalidPath(format!(
            "{} is a directory",
            key.display()
        )));
    };
    let size = node.meta.size;
    let mut data = Vec::new();
    archive.backend.read_sources(&[source], |_, reader| {
        let mut limited = reader.take(size.saturating_add(1));
        limited.read_to_end(&mut data).map_err(copy_error)?;
        check_size(data.len() as u64, size)
    })?;
    Ok(data)
}

fn add_job(jobs: &mut BTreeMap<EntrySource, Job>, source: EntrySource, node: &Node, dest: PathBuf) {
    jobs.entry(source)
        .or_insert_with(|| Job {
            size: node.meta.size,
            dests: Vec::new(),
        })
        .dests
        .push((dest, node.meta.clone()));
}

/// Stream one entry into `dest`, never writing more than its declared size:
/// an entry that decompresses to more than it claims (a zip bomb) fails
/// instead of filling the disk. A partial file is removed on failure.
fn write_entry(
    reader: &mut dyn Read,
    base: &Path,
    dest: &Path,
    size: u64,
    control: &Control,
    progress: &mut Progress<'_>,
) -> VfsResult<File> {
    let mut out = create_file(base, dest)?;
    let result = (|| {
        let mut buf = vec![0u8; COPY_BUFFER];
        let mut written = 0u64;
        loop {
            control.checkpoint()?;
            let n = reader.read(&mut buf).map_err(copy_error)?;
            if n == 0 {
                break;
            }
            written += n as u64;
            check_size_bound(written, size)?;
            out.write_all(&buf[..n])?;
            progress.advance(n as u64);
        }
        check_size(written, size)
    })();
    match result {
        Ok(()) => Ok(out),
        Err(e) => {
            drop(out);
            let _ = fs::remove_file(dest);
            Err(e)
        }
    }
}

/// Make the directory `dir` (at or below `base`) exist without following a
/// symlink: each component below `base` must be a real directory or absent.
fn ensure_dir(base: &Path, dir: &Path) -> VfsResult<()> {
    let rel = dir.strip_prefix(base).map_err(|_| {
        VfsError::Archive(format!("{} is outside {}", dir.display(), base.display()))
    })?;
    let mut current = base.to_path_buf();
    for component in rel.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() => {}
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(VfsError::Archive(format!(
                    "refusing to extract through the symbolic link {}",
                    current.display()
                )));
            }
            Ok(_) => {
                return Err(VfsError::Archive(format!(
                    "{} exists and is not a directory",
                    current.display()
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir(&current) {
                    Ok(()) => {}
                    // Lost a race to a concurrent creator: check it again.
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        if !fs::symlink_metadata(&current)?.is_dir() {
                            return Err(e.into());
                        }
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Create (or truncate) the file `path` below `base` without writing
/// through a symlink: a link at `path` itself is replaced, as tar does.
fn create_file(base: &Path, path: &Path) -> VfsResult<File> {
    if let Some(parent) = path.parent() {
        ensure_dir(base, parent)?;
    }
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        fs::remove_file(path)?;
    }
    let mut options = File::options();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    Ok(options.open(path)?)
}

fn check_size_bound(read: u64, declared: u64) -> VfsResult<()> {
    if read > declared {
        return Err(VfsError::Archive(format!(
            "entry is larger than its declared size of {declared} bytes"
        )));
    }
    Ok(())
}

fn check_size(read: u64, declared: u64) -> VfsResult<()> {
    check_size_bound(read, declared)?;
    if read < declared {
        return Err(VfsError::Archive(format!(
            "entry is truncated: {read} of {declared} bytes"
        )));
    }
    Ok(())
}

/// Best-effort: restore the modification time and the permission bits
/// (setuid, setgid and sticky are dropped).
/// Applied through the open handle, so it reaches the file just written.
fn apply_metadata(file: &File, meta: &VfsMetadata) {
    if let Some(modified) = meta.modified {
        let _ = file.set_modified(modified);
    }
    #[cfg(unix)]
    if let Some(mode) = meta.permissions {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(fs::Permissions::from_mode(mode & 0o777));
    }
}

#[cfg(unix)]
fn create_symlink(link: &str, target: &Path) {
    if let Err(e) = std::os::unix::fs::symlink(link, target) {
        log::warn!(
            "archive: cannot create symlink {} -> {link}: {e}",
            target.display()
        );
    }
}

#[cfg(not(unix))]
fn create_symlink(link: &str, target: &Path) {
    log::warn!(
        "archive: symlinks are not extracted on this platform: {} -> {link}",
        target.display()
    );
}
