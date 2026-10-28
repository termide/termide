//! Crash-safe replacement of the files in the project store.
//!
//! The same approach as the editor's file save (`termide-buffer`'s `write`
//! module), without its handling of symlinks, hard links and foreign
//! permissions: everything written here is a plain file termide owns.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Write `contents` to `path` through a temporary sibling that is flushed to
/// disk and renamed over it, so a crash, a kill or a full disk leaves the
/// previous file or the new one, never a truncated one.
///
/// The temporary name carries the process id and a counter: two instances,
/// or two threads of one, may write the same file at once.
pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (temp_path, mut temp) = loop {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_path = dir.join(format!(".{name}.{}.{n}.tmp", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => break (temp_path, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    let result = (|| {
        temp.write_all(contents)?;
        temp.sync_all()?;
        drop(temp);
        fs::rename(&temp_path, path)?;
        // Make the rename itself durable; not every platform can sync a
        // directory, hence best effort.
        #[cfg(unix)]
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn creates_then_replaces_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.toml");
        write_atomic(&path, b"a long first version").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(names(dir.path()), ["session.toml"]);
    }

    /// A write that cannot complete leaves the previous file whole.
    #[test]
    fn a_failed_write_keeps_the_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.toml");
        fs::write(&path, "previous").unwrap();
        // Renaming a file over a non-empty directory fails after the
        // temporary file was written; the temporary file must go and nothing
        // else change.
        let blocked = dir.path().join("blocked");
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("x"), "").unwrap();
        assert!(write_atomic(&blocked, b"new").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "previous");
        assert_eq!(names(dir.path()), ["blocked", "session.toml"]);
    }
}
