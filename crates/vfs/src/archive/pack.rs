//! Writing local files and directories into a new archive.
//!
//! The archive is written to a temporary file next to the destination and
//! renamed into place only once complete, so a failed or cancelled run never
//! leaves a truncated archive behind, and an existing file is never replaced.

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::format::{ArchiveFormat, TarCompression};
use super::source::civil_from_days;
use crate::error::{VfsError, VfsResult};

const COPY_BUFFER: usize = 256 * 1024;

/// What an item becomes in the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackKind {
    File,
    Dir,
    /// A symbolic link, stored as a link with this target.
    Symlink(PathBuf),
}

/// One filesystem entry to store.
#[derive(Debug, Clone)]
pub struct PackItem {
    /// Where the entry is read from.
    pub path: PathBuf,
    /// Its relative name inside the archive.
    pub name: PathBuf,
    pub kind: PackKind,
    /// Size in bytes for a file, 0 otherwise.
    pub size: u64,
    /// Unix permission bits, when the platform has them.
    pub mode: Option<u32>,
    pub modified: Option<SystemTime>,
}

/// How hard to compress. Each format maps it onto its own scale; a plain
/// `.tar` ignores it. bzip2 and xz have no stored mode, so `Store` gives
/// their weakest level; zstd only tells `Store` from the rest, since its
/// pure-Rust encoder has a single compressing level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompressionLevel {
    /// No compression: entries are stored as they are.
    Store,
    Fast,
    #[default]
    Normal,
    Best,
}

impl CompressionLevel {
    /// Every level, from none to the strongest.
    pub const ALL: [Self; 4] = [Self::Store, Self::Fast, Self::Normal, Self::Best];

    /// The 0–9 level of the deflate, bzip2 and xz encoders.
    fn numeric(self) -> u32 {
        match self {
            Self::Store => 0,
            Self::Fast => 1,
            Self::Normal => 6,
            Self::Best => 9,
        }
    }
}

/// Progress of a pack run, reported after every chunk.
#[derive(Debug, Clone, Default)]
pub struct PackProgress {
    pub files_done: usize,
    pub total_files: usize,
    pub bytes_done: u64,
    pub total_bytes: u64,
    /// Name of the entry being written.
    pub current: Option<String>,
}

impl PackProgress {
    /// Totals for `items`: files and bytes to write.
    pub fn for_items(items: &[PackItem]) -> Self {
        Self {
            total_files: items.iter().filter(|i| i.kind == PackKind::File).count(),
            total_bytes: items.iter().map(|i| i.size).sum(),
            ..Self::default()
        }
    }
}

/// List everything under `sources`, without following symlinks. Names are
/// relative to the deepest directory holding all sources, so entries picked
/// from different subdirectories keep them apart. `skip` (the archive being
/// written) is left out when it lies inside a source.
pub fn collect(sources: &[PathBuf], skip: &Path) -> VfsResult<Vec<PackItem>> {
    let base = common_parent(sources)
        .ok_or_else(|| VfsError::InvalidPath("Nothing to pack".to_string()))?;
    let mut items = Vec::new();
    for source in sources {
        walk(source, &base, skip, &mut items, crate::MAX_RECURSION_DEPTH)?;
    }
    Ok(items)
}

fn walk(
    path: &Path,
    base: &Path,
    skip: &Path,
    items: &mut Vec<PackItem>,
    depth: usize,
) -> VfsResult<()> {
    if path == skip {
        return Ok(());
    }
    let meta = fs::symlink_metadata(path)?;
    let name = path
        .strip_prefix(base)
        .map_err(|_| VfsError::InvalidPath(path.display().to_string()))?
        .to_path_buf();
    let file_type = meta.file_type();
    let kind = if file_type.is_symlink() {
        PackKind::Symlink(fs::read_link(path)?)
    } else if file_type.is_dir() {
        PackKind::Dir
    } else if file_type.is_file() {
        PackKind::File
    } else {
        log::warn!("pack: skipping special file {}", path.display());
        return Ok(());
    };
    items.push(PackItem {
        path: path.to_path_buf(),
        name,
        size: if kind == PackKind::File {
            meta.len()
        } else {
            0
        },
        kind: kind.clone(),
        mode: mode_of(&meta),
        modified: meta.modified().ok(),
    });
    if kind == PackKind::Dir {
        if depth == 0 {
            return Err(VfsError::InvalidPath(format!(
                "{} is nested too deeply",
                path.display()
            )));
        }
        let mut children: Vec<PathBuf> = fs::read_dir(path)?
            .map(|entry| entry.map(|e| e.path()))
            .collect::<io::Result<_>>()?;
        children.sort();
        for child in children {
            walk(&child, base, skip, items, depth - 1)?;
        }
    }
    Ok(())
}

/// The deepest directory that contains every path in `sources`.
fn common_parent(sources: &[PathBuf]) -> Option<PathBuf> {
    let mut parents = sources.iter().map(|s| s.parent());
    let mut base: Vec<Component<'_>> = parents.next()??.components().collect();
    for parent in parents {
        let parent: Vec<Component<'_>> = parent?.components().collect();
        let shared = base.iter().zip(&parent).take_while(|(a, b)| a == b).count();
        base.truncate(shared);
    }
    Some(base.iter().collect())
}

#[cfg(unix)]
fn mode_of(meta: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn mode_of(_meta: &fs::Metadata) -> Option<u32> {
    None
}

/// Write `items` into a new archive at `dest`, in the format its name asks
/// for, compressed at `level`. `progress` runs after every chunk; returning
/// an error (such as [`VfsError::Cancelled`]) stops the run, and blocking in
/// it pauses it.
pub fn pack(
    items: &[PackItem],
    dest: &Path,
    level: CompressionLevel,
    progress: &mut dyn FnMut(&PackProgress) -> VfsResult<()>,
) -> VfsResult<()> {
    let format = dest
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(ArchiveFormat::from_file_name)
        .ok_or_else(|| {
            VfsError::InvalidPath(format!("{} is not an archive name", dest.display()))
        })?;
    if fs::symlink_metadata(dest).is_ok() {
        return Err(VfsError::AlreadyExists {
            path: dest.to_path_buf(),
        });
    }
    let dir = match dest.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let temp = tempfile::Builder::new()
        .prefix(".termide-pack-")
        .tempfile_in(dir)?;
    let file = temp.as_file().try_clone()?;

    let mut state = Progress {
        report: progress,
        state: PackProgress::for_items(items),
        aborted: None,
    };
    let written = match format {
        ArchiveFormat::Zip => write_zip(items, file, level, &mut state),
        ArchiveFormat::Tar(compression) => write_tar(items, file, compression, level, &mut state),
        ArchiveFormat::Iso => Err(VfsError::NotSupported(
            "ISO images can be browsed, not packed".to_string(),
        )),
    };
    if let Err(e) = written {
        return Err(state.aborted.take().unwrap_or(e));
    }
    temp.as_file().sync_all()?;
    // `persist_noclobber` refuses to replace a file created at `dest` since
    // the check above.
    temp.persist_noclobber(dest).map_err(|e| e.error)?;
    Ok(())
}

/// Progress bookkeeping shared by the writers. The report callback's error
/// is parked in `aborted` while the I/O stack unwinds with a plain error.
struct Progress<'a> {
    report: &'a mut dyn FnMut(&PackProgress) -> VfsResult<()>,
    state: PackProgress,
    aborted: Option<VfsError>,
}

impl Progress<'_> {
    fn start(&mut self, item: &PackItem) -> VfsResult<()> {
        self.state.current = Some(item.name.to_string_lossy().into_owned());
        self.report()
    }

    fn advance(&mut self, bytes: u64) -> io::Result<()> {
        self.state.bytes_done += bytes;
        self.report().map_err(|_| io::Error::other("pack aborted"))
    }

    fn finish_file(&mut self) -> VfsResult<()> {
        self.state.files_done += 1;
        self.report()
    }

    fn report(&mut self) -> VfsResult<()> {
        if let Err(e) = (self.report)(&self.state) {
            let message = e.to_string();
            self.aborted = Some(e);
            return Err(VfsError::RemoteError { message });
        }
        Ok(())
    }
}

/// Copy exactly `size` bytes of `path` into `out`: a file that shrinks or
/// grows while it is read fails the entry, since tar has already written its
/// size.
fn copy_file(
    path: &Path,
    size: u64,
    out: &mut dyn Write,
    progress: &mut Progress<'_>,
) -> VfsResult<()> {
    let mut reader = File::open(path)?.take(size.saturating_add(1));
    let mut buf = vec![0u8; COPY_BUFFER];
    let mut copied = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        copied += n as u64;
        if copied > size {
            break;
        }
        out.write_all(&buf[..n])?;
        progress.advance(n as u64)?;
    }
    if copied != size {
        return Err(VfsError::Archive(format!(
            "{} changed while it was being packed",
            path.display()
        )));
    }
    Ok(())
}

/// The name as stored in a zip: `/`-separated on every platform.
fn zip_name(name: &Path) -> String {
    name.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Header ID of the Info-ZIP extended timestamp field.
const EXTENDED_TIMESTAMP: u16 = 0x5455;

fn write_zip(
    items: &[PackItem],
    file: File,
    level: CompressionLevel,
    progress: &mut Progress<'_>,
) -> VfsResult<()> {
    use zip::write::FullFileOptions;

    let (method, numeric) = match level {
        CompressionLevel::Store => (zip::CompressionMethod::Stored, None),
        _ => (
            zip::CompressionMethod::Deflated,
            Some(i64::from(level.numeric())),
        ),
    };
    let mut zip = zip::ZipWriter::new(BufWriter::with_capacity(COPY_BUFFER, file));
    for item in items {
        progress.start(item)?;
        let mut options = FullFileOptions::default()
            .compression_method(method)
            .compression_level(numeric)
            .large_file(item.size >= u64::from(u32::MAX));
        if let Some(mode) = item.mode {
            options = options.unix_permissions(mode);
        }
        if let Some(time) = item.modified {
            if let Some(dos) = dos_time(time) {
                options = options.last_modified_time(dos);
            }
            // DOS time has no zone; the extended timestamp carries the exact
            // UTC second, which unzip, bsdtar and this reader prefer.
            if let Some(secs) = time
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|d| u32::try_from(d.as_secs()).ok())
            {
                let mut field = vec![0x01];
                field.extend_from_slice(&secs.to_le_bytes());
                options
                    .add_extra_data(EXTENDED_TIMESTAMP, field, false)
                    .map_err(zip_error)?;
            }
        }
        let name = zip_name(&item.name);
        match &item.kind {
            PackKind::Dir => zip.add_directory(name, options).map_err(zip_error)?,
            PackKind::Symlink(target) => zip
                .add_symlink(name, zip_name(target), options)
                .map_err(zip_error)?,
            PackKind::File => {
                zip.start_file(name, options).map_err(zip_error)?;
                copy_file(&item.path, item.size, &mut zip, progress)?;
                progress.finish_file()?;
            }
        }
    }
    zip.finish()
        .map_err(zip_error)?
        .into_inner()
        .map_err(|e| e.into_error())?;
    Ok(())
}

fn write_tar(
    items: &[PackItem],
    file: File,
    compression: TarCompression,
    level: CompressionLevel,
    progress: &mut Progress<'_>,
) -> VfsResult<()> {
    let mut builder = tar::Builder::new(Compressor::new(file, compression, level)?);
    for item in items {
        progress.start(item)?;
        let mut header = tar::Header::new_gnu();
        header.set_mode(item.mode.unwrap_or(match item.kind {
            PackKind::Dir => 0o755,
            _ => 0o644,
        }));
        if let Some(secs) = item
            .modified
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        {
            header.set_mtime(secs.as_secs());
        }
        match &item.kind {
            PackKind::Dir => {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                builder.append_data(&mut header, &item.name, io::empty())?;
            }
            PackKind::Symlink(target) => {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                builder.append_link(&mut header, &item.name, target)?;
            }
            PackKind::File => {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(item.size);
                let reader = ItemReader::new(item, progress)?;
                builder.append_data(&mut header, &item.name, reader)?;
                progress.finish_file()?;
            }
        }
    }
    builder.into_inner()?.finish()?;
    Ok(())
}

/// Reads one file for the tar builder: reports progress per chunk and fails
/// once the file turns out longer or shorter than its declared size.
struct ItemReader<'p, 'a> {
    file: io::Take<File>,
    path: PathBuf,
    size: u64,
    read: u64,
    progress: &'p mut Progress<'a>,
}

impl<'p, 'a> ItemReader<'p, 'a> {
    fn new(item: &PackItem, progress: &'p mut Progress<'a>) -> VfsResult<Self> {
        Ok(Self {
            file: File::open(&item.path)?.take(item.size.saturating_add(1)),
            path: item.path.clone(),
            size: item.size,
            read: 0,
            progress,
        })
    }

    fn changed(&mut self) -> io::Error {
        let e = VfsError::Archive(format!(
            "{} changed while it was being packed",
            self.path.display()
        ));
        let message = e.to_string();
        self.progress.aborted = Some(e);
        io::Error::other(message)
    }
}

impl Read for ItemReader<'_, '_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read(buf)?;
        self.read += n as u64;
        if self.read > self.size || (n == 0 && self.read < self.size) {
            return Err(self.changed());
        }
        if n > 0 {
            self.progress.advance(n as u64)?;
        }
        Ok(n)
    }
}

/// The compression stage under a tar stream.
enum Compressor {
    None(BufWriter<File>),
    Gzip(flate2::write::GzEncoder<BufWriter<File>>),
    Bzip2(bzip2::write::BzEncoder<BufWriter<File>>),
    Xz(Box<lzma_rust2::XzWriter<BufWriter<File>>>),
    /// ruzstd compresses from a reader, so the tar stream goes through a
    /// pipe into a compressing thread.
    Zstd {
        pipe: io::PipeWriter,
        worker: std::thread::JoinHandle<()>,
    },
}

impl Compressor {
    fn new(file: File, compression: TarCompression, level: CompressionLevel) -> io::Result<Self> {
        let out = BufWriter::with_capacity(COPY_BUFFER, file);
        let numeric = level.numeric();
        Ok(match compression {
            TarCompression::None => Self::None(out),
            TarCompression::Gzip => Self::Gzip(flate2::write::GzEncoder::new(
                out,
                flate2::Compression::new(numeric),
            )),
            TarCompression::Bzip2 => Self::Bzip2(bzip2::write::BzEncoder::new(
                out,
                bzip2::Compression::new(numeric.max(1)),
            )),
            TarCompression::Xz => Self::Xz(Box::new(lzma_rust2::XzWriter::new(
                out,
                lzma_rust2::XzOptions::with_preset(numeric),
            )?)),
            TarCompression::Zstd => {
                let zstd_level = match level {
                    CompressionLevel::Store => ruzstd::encoding::CompressionLevel::Uncompressed,
                    _ => ruzstd::encoding::CompressionLevel::Fastest,
                };
                let (reader, pipe) = io::pipe()?;
                let worker = std::thread::spawn(move || {
                    let mut out = out;
                    ruzstd::encoding::compress(reader, &mut out, zstd_level);
                    if let Err(e) = out.flush() {
                        // Surfaces as a failed join below.
                        panic!("zstd output: {e}");
                    }
                });
                Self::Zstd { pipe, worker }
            }
        })
    }

    /// Flush every stage down to the file.
    fn finish(self) -> io::Result<()> {
        let mut out = match self {
            Self::None(out) => out,
            Self::Gzip(enc) => enc.finish()?,
            Self::Bzip2(enc) => enc.finish()?,
            Self::Xz(enc) => enc.finish()?,
            Self::Zstd { pipe, worker } => {
                drop(pipe);
                return worker
                    .join()
                    .map_err(|_| io::Error::other("zstd compression failed"));
            }
        };
        out.flush()
    }
}

impl Write for Compressor {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::None(w) => w.write(buf),
            Self::Gzip(w) => w.write(buf),
            Self::Bzip2(w) => w.write(buf),
            Self::Xz(w) => w.write(buf),
            Self::Zstd { pipe, .. } => pipe.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::None(w) => w.flush(),
            Self::Gzip(w) => w.flush(),
            Self::Bzip2(w) => w.flush(),
            Self::Xz(w) => w.flush(),
            Self::Zstd { pipe, .. } => pipe.flush(),
        }
    }
}

/// A point in time as a zip timestamp (UTC, like the reader assumes), or
/// `None` outside the 1980–2107 range zip can store.
fn dos_time(time: SystemTime) -> Option<zip::DateTime> {
    let secs = i64::try_from(time.duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()?;
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    zip::DateTime::from_date_and_time(
        u16::try_from(year).ok()?,
        month as u8,
        day as u8,
        (rem / 3600) as u8,
        (rem % 3600 / 60) as u8,
        (rem % 60) as u8,
    )
    .ok()
}

fn zip_error(e: zip::result::ZipError) -> VfsError {
    VfsError::Archive(e.to_string())
}
