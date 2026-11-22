//! Keeping a config file that did not load before it is replaced.

use std::io;
use std::path::{Path, PathBuf};

/// Before `path` is replaced by a file rebuilt from what was loaded, copy it
/// aside when it did not load: what loaded from it was then nothing, and
/// the rewrite would lose every entry the user had. `loads` says whether the
/// text loads. The copy goes next to it as `<name>.bak` (or `<name>.bak.N`
/// when that is taken, so an older copy is never replaced). An error means
/// no copy was made, and the caller must not write.
pub(crate) fn back_up_unloadable(path: &Path, loads: impl Fn(&str) -> bool) -> io::Result<()> {
    let loaded = match std::fs::read_to_string(path) {
        Ok(text) => loads(&text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        // Unreadable as text (not UTF-8, say): it did not load either.
        Err(_) => false,
    };
    if loaded {
        return Ok(());
    }
    let backup = free_backup_path(path)?;
    std::fs::copy(path, &backup)?;
    log::warn!(
        "{} did not load; kept a copy as {} before replacing it",
        path.display(),
        backup.display()
    );
    Ok(())
}

fn free_backup_path(path: &Path) -> io::Result<PathBuf> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    let first = path.with_file_name(&name);
    if std::fs::symlink_metadata(&first).is_err() {
        return Ok(first);
    }
    for n in 1..1000 {
        let mut numbered = name.clone();
        numbered.push(format!(".{n}"));
        let candidate = path.with_file_name(numbered);
        if std::fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("no free backup name next to {}", path.display()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_that_loads_or_is_missing_is_not_copied() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.toml");
        back_up_unloadable(&path, |_| false).unwrap();
        std::fs::write(&path, "ok").unwrap();
        back_up_unloadable(&path, |_| true).unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_file_that_does_not_load_is_copied_without_replacing_older_copies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.toml");
        std::fs::write(&path, "first").unwrap();
        back_up_unloadable(&path, |_| false).unwrap();
        std::fs::write(&path, "second").unwrap();
        back_up_unloadable(&path, |_| false).unwrap();
        let read = |name: &str| std::fs::read_to_string(dir.path().join(name)).unwrap();
        assert_eq!(read("x.toml.bak"), "first");
        assert_eq!(read("x.toml.bak.1"), "second");
    }
}
