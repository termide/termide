//! Archive format detection.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// Compression wrapped around a tar stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TarCompression {
    /// Plain `.tar`.
    None,
    /// `.tar.gz`, `.tgz`.
    Gzip,
    /// `.tar.bz2`, `.tbz2`.
    Bzip2,
    /// `.tar.xz`, `.txz`.
    Xz,
    /// `.tar.zst`, `.tzst`.
    Zstd,
}

/// Archive formats the archive provider can browse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    /// ZIP and the formats built on it (jar, apk, whl).
    Zip,
    /// A tar stream, possibly compressed.
    Tar(TarCompression),
    /// An ISO 9660 image, with Joliet and Rock Ridge.
    Iso,
}

/// Offset of the `CD001` identifier of the first ISO 9660 volume
/// descriptor, which follows a 32 KiB system area.
const ISO_MAGIC_OFFSET: u64 = 16 * 2048 + 1;

/// Name suffixes recognised as archives, longest first so `.tar.gz` wins
/// over a hypothetical `.gz`.
const SUFFIXES: &[(&str, ArchiveFormat)] = &[
    (".tar.gz", ArchiveFormat::Tar(TarCompression::Gzip)),
    (".tar.bz2", ArchiveFormat::Tar(TarCompression::Bzip2)),
    (".tar.xz", ArchiveFormat::Tar(TarCompression::Xz)),
    (".tar.zst", ArchiveFormat::Tar(TarCompression::Zstd)),
    (".tbz2", ArchiveFormat::Tar(TarCompression::Bzip2)),
    (".tzst", ArchiveFormat::Tar(TarCompression::Zstd)),
    (".tgz", ArchiveFormat::Tar(TarCompression::Gzip)),
    (".tbz", ArchiveFormat::Tar(TarCompression::Bzip2)),
    (".txz", ArchiveFormat::Tar(TarCompression::Xz)),
    (".tar", ArchiveFormat::Tar(TarCompression::None)),
    (".zip", ArchiveFormat::Zip),
    (".jar", ArchiveFormat::Zip),
    (".war", ArchiveFormat::Zip),
    (".apk", ArchiveFormat::Zip),
    (".whl", ArchiveFormat::Zip),
    (".iso", ArchiveFormat::Iso),
];

impl ArchiveFormat {
    /// Guess the format from a file name alone. Cheap enough to run on every
    /// entry of a directory listing; [`ArchiveFormat::detect`] is the
    /// authoritative check once the archive is opened.
    pub fn from_file_name(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        SUFFIXES
            .iter()
            .find(|(suffix, _)| lower.len() > suffix.len() && lower.ends_with(suffix))
            .map(|&(_, format)| format)
    }

    /// The name without its archive suffix (`docs.tar.gz` -> `docs`), the
    /// natural name of the directory it unpacks into. `None` when the name
    /// carries no recognised suffix.
    pub fn strip_suffix(name: &str) -> Option<&str> {
        let lower = name.to_ascii_lowercase();
        SUFFIXES
            .iter()
            .find(|(suffix, _)| lower.len() > suffix.len() && lower.ends_with(suffix))
            .map(|(suffix, _)| &name[..name.len() - suffix.len()])
    }

    /// Whether [`pack`](super::pack::pack) can write this format; ISO
    /// images are only read.
    pub fn can_pack(self) -> bool {
        !matches!(self, Self::Iso)
    }

    /// Detect the format from the file's leading bytes, falling back to the
    /// name for an old-style tar without the `ustar` magic.
    pub fn detect(path: &Path) -> io::Result<Option<Self>> {
        let mut file = File::open(path)?;
        let mut head = [0u8; 512];
        let len = read_up_to(&mut file, &mut head)?;
        let head = &head[..len];

        let by_magic = if head.starts_with(b"PK\x03\x04")
            || head.starts_with(b"PK\x05\x06")
            || head.starts_with(b"PK\x07\x08")
        {
            Some(Self::Zip)
        } else if head.starts_with(&[0x1f, 0x8b]) {
            Some(Self::Tar(TarCompression::Gzip))
        } else if head.starts_with(b"BZh") {
            Some(Self::Tar(TarCompression::Bzip2))
        } else if head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
            Some(Self::Tar(TarCompression::Xz))
        } else if head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
            Some(Self::Tar(TarCompression::Zstd))
        } else if head.get(257..262) == Some(b"ustar") {
            Some(Self::Tar(TarCompression::None))
        } else if has_iso_magic(&mut file)? {
            // Checked last: a hybrid image starts with a boot sector that
            // none of the magics above can match.
            Some(Self::Iso)
        } else {
            None
        };

        Ok(by_magic.or_else(|| {
            let name = path.file_name()?.to_str()?;
            (Self::from_file_name(name) == Some(Self::Tar(TarCompression::None)))
                .then_some(Self::Tar(TarCompression::None))
        }))
    }
}

fn has_iso_magic(file: &mut File) -> io::Result<bool> {
    file.seek(SeekFrom::Start(ISO_MAGIC_OFFSET))?;
    let mut magic = [0u8; 5];
    Ok(read_up_to(file, &mut magic)? == magic.len() && &magic == b"CD001")
}

fn read_up_to(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_suffix_removes_the_whole_archive_suffix() {
        assert_eq!(ArchiveFormat::strip_suffix("docs.tar.gz"), Some("docs"));
        assert_eq!(ArchiveFormat::strip_suffix("Docs.v2.ZIP"), Some("Docs.v2"));
        assert_eq!(ArchiveFormat::strip_suffix("отчёт.tgz"), Some("отчёт"));
        assert_eq!(ArchiveFormat::strip_suffix("notes.txt"), None);
        assert_eq!(ArchiveFormat::strip_suffix(".zip"), None);
    }

    #[test]
    fn file_name_picks_the_longest_suffix() {
        assert_eq!(
            ArchiveFormat::from_file_name("src-1.0.TAR.GZ"),
            Some(ArchiveFormat::Tar(TarCompression::Gzip))
        );
        assert_eq!(
            ArchiveFormat::from_file_name("a.tar"),
            Some(ArchiveFormat::Tar(TarCompression::None))
        );
        assert_eq!(
            ArchiveFormat::from_file_name("app.jar"),
            Some(ArchiveFormat::Zip)
        );
        assert_eq!(
            ArchiveFormat::from_file_name("Debian.ISO"),
            Some(ArchiveFormat::Iso)
        );
        assert_eq!(ArchiveFormat::from_file_name("notes.txt"), None);
        assert_eq!(ArchiveFormat::from_file_name(".zip"), None);
        assert_eq!(ArchiveFormat::from_file_name("log.gz"), None);
    }

    #[test]
    fn detect_trusts_content_over_the_name() {
        let dir = tempfile::tempdir().unwrap();
        let misnamed = dir.path().join("really-gzip.zip");
        std::fs::write(&misnamed, [0x1f, 0x8b, 8, 0]).unwrap();
        assert_eq!(
            ArchiveFormat::detect(&misnamed).unwrap(),
            Some(ArchiveFormat::Tar(TarCompression::Gzip))
        );

        let mut image = vec![0u8; 40_000];
        image[ISO_MAGIC_OFFSET as usize..][..5].copy_from_slice(b"CD001");
        let iso = dir.path().join("disc.img");
        std::fs::write(&iso, image).unwrap();
        assert_eq!(
            ArchiveFormat::detect(&iso).unwrap(),
            Some(ArchiveFormat::Iso)
        );

        let text = dir.path().join("notes.zip");
        std::fs::write(&text, b"just text").unwrap();
        assert_eq!(ArchiveFormat::detect(&text).unwrap(), None);
    }
}
