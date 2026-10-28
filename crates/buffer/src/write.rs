//! Crash-safe file writing for buffer saves.
//!
//! The new contents go to a temporary file next to the target, are flushed to
//! disk and then renamed over the target, so a crash or a full disk leaves
//! either the old file or the new one, never a truncated mix.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// How many symlink hops to follow before giving up on resolution.
const MAX_SYMLINK_HOPS: usize = 40;

/// Write `contents` to `path`, replacing it atomically where possible.
///
/// - A symlink is followed: its final target is replaced and the link stays.
/// - The permissions (and, on Unix, the owner when allowed) of an existing
///   file carry over to the new one.
/// - A file with several hard links, or one in a directory where no
///   temporary file can be created, is rewritten in place, since a rename
///   would split the links or is impossible there.
pub(crate) fn write_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let target = resolve_symlinks(path)?;
    let existing = fs::metadata(&target).ok();

    if existing.as_ref().is_some_and(has_other_hard_links) {
        return write_in_place(&target, contents);
    }

    let dir = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let (temp_path, mut temp) = match create_temp(&dir, &target) {
        Ok(t) => t,
        // A writable file in a read-only directory can still be saved.
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied && existing.is_some() => {
            return write_in_place(&target, contents);
        }
        Err(e) => return Err(e),
    };

    let result = (|| {
        temp.write_all(contents)?;
        if let Some(meta) = &existing {
            copy_metadata(&temp, meta)?;
        }
        temp.sync_all()?;
        drop(temp);
        fs::rename(&temp_path, &target)?;
        sync_dir(&dir);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

/// Follow `path` through symlinks to the file that actually holds the data.
/// A dangling link resolves to the path it points at, so saving creates it.
fn resolve_symlinks(path: &Path) -> io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..MAX_SYMLINK_HOPS {
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let link = fs::read_link(&current)?;
                current = match current.parent() {
                    Some(parent) if link.is_relative() => parent.join(link),
                    _ => link,
                };
            }
            _ => return Ok(current),
        }
    }
    Err(io::Error::other(format!(
        "Too many levels of symbolic links: {}",
        path.display()
    )))
}

fn create_temp(dir: &Path, target: &Path) -> io::Result<(PathBuf, File)> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    loop {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_path = dir.join(format!(".{name}.{}.{n}.termide-tmp", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

fn write_in_place(target: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(target)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(unix)]
fn has_other_hard_links(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.is_file() && meta.nlink() > 1
}

#[cfg(not(unix))]
fn has_other_hard_links(_meta: &fs::Metadata) -> bool {
    false
}

fn copy_metadata(file: &File, meta: &fs::Metadata) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Only root may give a file away; for everyone else the new file
        // already has the right owner, so a failure here is expected.
        let _ = std::os::unix::fs::fchown(file, Some(meta.uid()), Some(meta.gid()));
    }
    file.set_permissions(meta.permissions())
}

/// Make the rename itself durable. Best effort: not every platform or
/// filesystem lets a directory be opened and synced.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".termide-tmp"))
            .collect()
    }

    #[test]
    fn creates_a_new_file_and_leaves_no_temp_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        write_file(&path, b"hello").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"hello");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn replaces_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        fs::write(&path, "a much longer old content").unwrap();
        write_file(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn keeps_the_permissions_of_the_existing_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.sh");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o751)).unwrap();
        write_file(&path, b"new").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o751);
    }

    #[cfg(unix)]
    #[test]
    fn writes_through_a_symlink_and_keeps_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.txt");
        let link = dir.path().join("link.txt");
        fs::write(&real, "old").unwrap();
        std::os::unix::fs::symlink("real.txt", &link).unwrap();

        write_file(&link, b"new").unwrap();

        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(&real).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn keeps_hard_links_together() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, "old").unwrap();
        fs::hard_link(&a, &b).unwrap();

        write_file(&a, b"new").unwrap();

        assert_eq!(fs::read(&b).unwrap(), b"new");
    }
}
