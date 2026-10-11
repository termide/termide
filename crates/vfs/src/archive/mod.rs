//! Read-only archive provider: a zip or tar file or an ISO image browsed as a
//! directory tree.
//!
//! "Connecting" reads the archive's table of contents into memory, so
//! listing and metadata are instant afterwards. Reading an entry goes back to
//! the file: directly for zip and ISO, by streaming from the start for tar.
//! Every mutating operation fails with [`VfsError::NotSupported`].
//!
//! An archive on a remote host or inside another archive is read from a
//! local copy the manager fetches on connect; the copy lives as long as the
//! provider.

mod extract;
mod format;
mod index;
mod iso;
mod names;
pub mod pack;
mod source;

pub use format::{ArchiveFormat, TarCompression};

/// Check whether the local archive at `path` opens with `password`:
/// `Err(PasswordRequired)` when it has encrypted entries and none was given,
/// `Err(WrongPassword)` when the one given is rejected, `Ok` otherwise.
///
/// Only zip archives can be encrypted, and for them this reads the central
/// directory and decrypts the smallest encrypted entry, so it is cheap
/// enough to run before starting an extraction. Other formats are not read
/// past their leading bytes.
pub fn check_password(path: &Path, password: Option<&str>) -> VfsResult<()> {
    if ArchiveFormat::detect(path)? != Some(ArchiveFormat::Zip) {
        return Ok(());
    }
    let (_, mut backend) = source::open(path, ArchiveFormat::Zip)?;
    backend.unlock(password)
}

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::error::{VfsError, VfsResult};
use crate::traits::VfsProvider;
use crate::types::{
    AuthMethod, ConnectOptions, ConnectionState, VfsDownloadOperation, VfsEntry, VfsMetadata,
    VfsOperation, VfsPath,
};
use extract::Control;
use index::ArchiveIndex;
use source::Backend;

const READ_ONLY: &str = "Archives are read-only";

/// An opened archive: its table of contents and a way back to its bytes.
pub(crate) struct OpenArchive {
    index: ArchiveIndex,
    backend: Backend,
    /// Size and modification time of the archive file when it was indexed.
    stamp: Stamp,
}

type Stamp = (u64, Option<SystemTime>);

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

/// Where the provider reads the archive file from.
enum Source {
    /// The container is a local file.
    Local,
    /// The container lives elsewhere and is read from a local copy. `fetch`
    /// is the transfer producing it, awaited on connect; the directory holding
    /// the copy is removed with the provider.
    Copy {
        fetch: Mutex<Option<VfsOperation<PathBuf>>>,
        path: PathBuf,
        _dir: tempfile::TempDir,
    },
}

/// VFS provider for one archive file.
pub struct ArchiveProvider {
    container: VfsPath,
    source: Source,
    open: Option<Arc<OpenArchive>>,
}

impl ArchiveProvider {
    /// A provider for the local archive file `container`; nothing is read
    /// until [`VfsProvider::connect`].
    pub fn new(container: VfsPath) -> Self {
        Self {
            container,
            source: Source::Local,
            open: None,
        }
    }

    /// A provider for `container` read from the local copy `path` inside
    /// `dir`, which `fetch` is writing. Connecting waits for it.
    pub fn from_copy(
        container: VfsPath,
        fetch: VfsOperation<PathBuf>,
        dir: tempfile::TempDir,
        path: PathBuf,
    ) -> Self {
        Self {
            container,
            source: Source::Copy {
                fetch: Mutex::new(Some(fetch)),
                path,
                _dir: dir,
            },
            open: None,
        }
    }

    /// The local file holding the archive.
    fn local_path(&self) -> &Path {
        match &self.source {
            Source::Local => self.container.path(),
            Source::Copy { path, .. } => path,
        }
    }

    fn opened(&self) -> VfsResult<Arc<OpenArchive>> {
        self.open.clone().ok_or(VfsError::NotConnected)
    }

    fn open_archive(&self, password: Option<&str>) -> VfsResult<OpenArchive> {
        match &self.source {
            Source::Local if !self.container.is_local() => {
                return Err(VfsError::NotSupported(
                    "A non-local archive is opened from a local copy".to_string(),
                ));
            }
            Source::Local => {}
            Source::Copy { fetch, .. } => {
                let pending = fetch.lock().ok().and_then(|mut f| f.take());
                if let Some(fetch) = pending {
                    fetch.recv()?;
                }
            }
        }
        let path = self.local_path();
        let stamp = stamp(path).ok_or_else(|| VfsError::NotFound {
            path: path.to_path_buf(),
        })?;
        let format = ArchiveFormat::detect(path)?.ok_or_else(|| {
            VfsError::Archive(format!("{} is not a supported archive", path.display()))
        })?;
        let (index, mut backend) = source::open(path, format)?;
        backend.unlock(password)?;
        Ok(OpenArchive {
            index,
            backend,
            stamp,
        })
    }

    /// Run `work` on a background thread against the opened archive.
    fn spawn<T: Send + 'static>(
        &self,
        work: impl FnOnce(&OpenArchive) -> VfsResult<T> + Send + 'static,
    ) -> VfsOperation<T> {
        let archive = match self.opened() {
            Ok(archive) => archive,
            Err(e) => return VfsOperation::error(e),
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work(&archive));
        });
        VfsOperation::new(rx)
    }

    fn read_only<T>() -> VfsOperation<T> {
        VfsOperation::error(VfsError::NotSupported(READ_ONLY.to_string()))
    }
}

impl VfsProvider for ArchiveProvider {
    fn name(&self) -> &'static str {
        "archive"
    }

    /// Connected while the archive file is unchanged on disk. A rewritten or
    /// removed file reports `Failed`, which makes the manager drop this
    /// provider so the next access re-reads the table of contents.
    fn connection_state(&self) -> ConnectionState {
        match &self.open {
            None => ConnectionState::Disconnected,
            Some(open) if stamp(self.local_path()) == Some(open.stamp) => {
                ConnectionState::Connected
            }
            Some(_) => {
                log::info!(
                    "archive: {} changed on disk, closing it",
                    self.container.log_safe_key()
                );
                ConnectionState::Failed
            }
        }
    }

    /// A password in `options` unlocks an archive with encrypted entries;
    /// without one such an archive fails with [`VfsError::PasswordRequired`].
    fn connect(&mut self, options: ConnectOptions) -> VfsOperation<()> {
        let password = match &options.auth {
            AuthMethod::Password(password) => Some(password.as_str()),
            _ => None,
        };
        // The manager already runs connect on a background thread.
        let result = self.open_archive(password).map(|open| {
            self.open = Some(Arc::new(open));
        });
        VfsOperation::ready(result)
    }

    fn disconnect(&mut self) {
        self.open = None;
    }

    fn list_dir(&self, path: &VfsPath) -> VfsOperation<Vec<VfsEntry>> {
        let result = self.opened().and_then(|archive| {
            let not_dir = || VfsError::NotFound {
                path: path.path.clone(),
            };
            let (key, _) = archive.index.resolve(&path.path).ok_or_else(not_dir)?;
            let names = archive.index.children(&key).ok_or_else(not_dir)?;
            Ok(names
                .iter()
                .filter_map(|name| {
                    let (_, node) = archive.index.get(&key.join(name))?;
                    Some(VfsEntry::new(
                        name.clone(),
                        path.join(name),
                        node.meta.clone(),
                    ))
                })
                .collect())
        });
        VfsOperation::ready(result)
    }

    fn create_dir(&self, _path: &VfsPath) -> VfsOperation<()> {
        Self::read_only()
    }

    fn create_dir_all(&self, _path: &VfsPath) -> VfsOperation<()> {
        Self::read_only()
    }

    fn exists(&self, path: &VfsPath) -> VfsOperation<bool> {
        VfsOperation::ready(
            self.opened()
                .map(|archive| archive.index.resolve(&path.path).is_some()),
        )
    }

    /// Follows symlinks, like `stat`; a listing reports links as themselves.
    fn metadata(&self, path: &VfsPath) -> VfsOperation<VfsMetadata> {
        VfsOperation::ready(self.opened().and_then(|archive| {
            archive
                .index
                .resolve(&path.path)
                .map(|(_, node)| node.meta.clone())
                .ok_or_else(|| VfsError::NotFound {
                    path: path.path.clone(),
                })
        }))
    }

    fn read_file(&self, path: &VfsPath) -> VfsOperation<Vec<u8>> {
        let key = path.path.clone();
        self.spawn(move |archive| extract::read_to_vec(archive, &key))
    }

    fn write_file(&self, _path: &VfsPath, _data: &[u8]) -> VfsOperation<()> {
        Self::read_only()
    }

    fn delete(&self, _path: &VfsPath) -> VfsOperation<()> {
        Self::read_only()
    }

    fn delete_recursive(&self, _path: &VfsPath) -> VfsOperation<()> {
        Self::read_only()
    }

    fn rename(&self, _from: &VfsPath, _to: &VfsPath) -> VfsOperation<()> {
        Self::read_only()
    }

    fn copy(&self, _from: &VfsPath, _to: &VfsPath) -> VfsOperation<()> {
        Self::read_only()
    }

    fn download(&self, remote: &VfsPath, local: &Path) -> VfsOperation<PathBuf> {
        let key = remote.path.clone();
        let dest = local.to_path_buf();
        self.spawn(move |archive| extract::extract(archive, &key, &dest, &Control::none(), None))
    }

    fn upload(&self, _local: &Path, _remote: &VfsPath) -> VfsOperation<()> {
        Self::read_only()
    }

    fn download_with_progress(&self, remote: &VfsPath, local: &Path) -> VfsDownloadOperation {
        let archive = match self.opened() {
            Ok(archive) => archive,
            Err(e) => return VfsDownloadOperation::error(e),
        };
        let key = remote.path.clone();
        let dest = local.to_path_buf();
        let control = Control {
            pause: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let (pause, cancel) = (Arc::clone(&control.pause), Arc::clone(&control.cancel));
        let (done_tx, done_rx) = mpsc::channel();
        let (progress_tx, progress_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = extract::extract(&archive, &key, &dest, &control, Some(&progress_tx));
            let _ = done_tx.send(result);
        });
        VfsDownloadOperation::new(done_rx, progress_rx, pause, cancel)
    }

    fn home_dir(&self) -> Option<VfsPath> {
        Some(VfsPath::archive(self.container.clone(), "/"))
    }
}

#[cfg(test)]
mod tests;
