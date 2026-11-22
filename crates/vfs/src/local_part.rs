//! The local end of a download, shared by the remote providers.
//!
//! - A name a server lists is used as a local name only when it is one
//!   plain entry of its directory ([`is_plain_entry_name`]).
//! - A file is written under a hidden name beside its destination and put
//!   in place only once whole ([`LocalPart`]): a cancelled or failed
//!   download leaves the file it was to replace as it was, and a link
//!   planted at the destination is replaced, never written through.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Component, Path};

/// Whether `name`, as a server listed it, is one plain entry of its
/// directory: a name holding a separator, a drive or a parent would put a
/// download outside the folder it goes to.
#[cfg_attr(
    not(any(feature = "sftp", feature = "smb", feature = "nfs")),
    allow(dead_code)
)]
pub(crate) fn is_plain_entry_name(name: &str) -> bool {
    let mut parts = Path::new(name).components();
    matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(_)), None)
    ) && !name.contains(['/', '\\', '\0'])
}

/// A downloaded file being written beside its destination. Dropped without
/// [`persist`](Self::persist), it is removed.
pub(crate) struct LocalPart(tempfile::NamedTempFile);

impl LocalPart {
    /// A fresh hidden file next to `local`, `.name.XXXX.termide-part`,
    /// created anew (never a link followed) with the usual mode.
    pub(crate) fn create(local: &Path) -> io::Result<Self> {
        let parent = local
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let prefix = format!(
            ".{}.",
            local.file_name().unwrap_or_default().to_string_lossy()
        );
        tempfile::Builder::new()
            .prefix(&prefix)
            .suffix(".termide-part")
            .make_in(parent, |path| {
                File::options().write(true).create_new(true).open(path)
            })
            .map(Self)
    }

    /// Put the whole file in place of `local`. A regular file replaced
    /// keeps its mode, as it did when written in place; a link there is
    /// replaced, not followed.
    pub(crate) fn persist(self, local: &Path) -> io::Result<()> {
        if let Ok(meta) = std::fs::symlink_metadata(local) {
            if meta.is_file() {
                std::fs::set_permissions(self.0.path(), meta.permissions())?;
            }
        }
        self.0.persist(local).map(drop).map_err(|e| e.error)
    }
}

impl Write for LocalPart {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_entry_name_is_downloaded() {
        assert!(is_plain_entry_name("notes.txt"));
        assert!(is_plain_entry_name(".hidden"));
        assert!(!is_plain_entry_name(".."));
        assert!(!is_plain_entry_name("."));
        assert!(!is_plain_entry_name("a/../../x"));
        assert!(!is_plain_entry_name("/home/u/.ssh/authorized_keys"));
        assert!(!is_plain_entry_name("a\\b"));
        assert!(!is_plain_entry_name("a\0b"));
        assert!(!is_plain_entry_name(""));
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_part_dropped_unpersisted_leaves_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("data");
        std::fs::write(&local, b"old").unwrap();
        let mut part = LocalPart::create(&local).unwrap();
        part.write_all(b"half").unwrap();
        drop(part);
        assert_eq!(std::fs::read(&local).unwrap(), b"old");
        assert_eq!(names(dir.path()), ["data"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_persisted_part_keeps_the_mode_and_replaces_a_link() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("secret");
        std::fs::write(&local, b"old").unwrap();
        std::fs::set_permissions(&local, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut part = LocalPart::create(&local).unwrap();
        part.write_all(b"new").unwrap();
        part.persist(&local).unwrap();
        assert_eq!(std::fs::read(&local).unwrap(), b"new");
        let mode = std::fs::metadata(&local).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        // A link planted at the destination is replaced, its target untouched.
        let target = dir.path().join("target");
        std::fs::write(&target, b"theirs").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let mut part = LocalPart::create(&link).unwrap();
        part.write_all(b"mine").unwrap();
        part.persist(&link).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_file());
        assert_eq!(std::fs::read(&link).unwrap(), b"mine");
        assert_eq!(std::fs::read(&target).unwrap(), b"theirs");
        assert_eq!(names(dir.path()), ["link", "secret", "target"]);
    }
}
