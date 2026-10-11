use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use super::*;
use crate::VfsManager;

const README: &[u8] = b"hello from the archive\n";

mod encryption {
    use super::*;

    /// Written by Info-ZIP `zip -P hunter2`: `secret/note.txt` encrypted
    /// with ZipCrypto, `readme.txt` plain.
    const ZIPCRYPTO: &[u8] = include_bytes!("fixtures/zipcrypto.zip");

    fn connect(archive: &Path, password: Option<&str>) -> VfsResult<ArchiveProvider> {
        let mut provider = ArchiveProvider::new(VfsPath::local(archive));
        let options = match password {
            Some(p) => ConnectOptions::with_password(p),
            None => ConnectOptions::default(),
        };
        provider.connect(options).recv()?;
        Ok(provider)
    }

    fn aes_zip(path: &Path) {
        use zip::write::SimpleFileOptions;
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        zip.start_file(
            "secret/note.txt",
            SimpleFileOptions::default().with_aes_encryption(zip::AesMode::Aes256, "hunter2"),
        )
        .unwrap();
        zip.write_all(b"top secret\n").unwrap();
        zip.start_file("readme.txt", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"plain\n").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn an_encrypted_zip_asks_for_the_password_and_checks_it() {
        let dir = tempfile::tempdir().unwrap();
        let zipcrypto = dir.path().join("zipcrypto.zip");
        fs::write(&zipcrypto, ZIPCRYPTO).unwrap();
        let aes = dir.path().join("aes.zip");
        aes_zip(&aes);

        for archive in [&zipcrypto, &aes] {
            let what = archive.display().to_string();
            assert!(
                matches!(connect(archive, None), Err(VfsError::PasswordRequired)),
                "{what}"
            );
            assert!(
                matches!(connect(archive, Some("nope")), Err(VfsError::WrongPassword)),
                "{what}"
            );

            let provider = connect(archive, Some("hunter2")).unwrap();
            let secret = provider
                .read_file(&at(archive, "/secret/note.txt"))
                .recv()
                .unwrap();
            assert_eq!(secret, b"top secret\n", "{what}");
            let plain = provider
                .read_file(&at(archive, "/readme.txt"))
                .recv()
                .unwrap();
            assert_eq!(plain, b"plain\n", "{what}");

            let dest = dir.path().join(format!(
                "out-{}",
                archive.file_name().unwrap().to_string_lossy()
            ));
            provider.download(&at(archive, "/"), &dest).recv().unwrap();
            assert_eq!(
                fs::read(dest.join("secret/note.txt")).unwrap(),
                b"top secret\n"
            );
        }
    }

    #[test]
    fn check_password_tells_missing_wrong_and_right_apart() {
        let dir = tempfile::tempdir().unwrap();
        let aes = dir.path().join("aes.zip");
        aes_zip(&aes);
        assert!(matches!(
            check_password(&aes, None),
            Err(VfsError::PasswordRequired)
        ));
        assert!(matches!(
            check_password(&aes, Some("nope")),
            Err(VfsError::WrongPassword)
        ));
        assert!(check_password(&aes, Some("hunter2")).is_ok());

        let plain = dir.path().join("plain.zip");
        write_zip(&plain);
        assert!(check_password(&plain, None).is_ok());
    }

    #[test]
    fn a_plain_archive_ignores_a_password() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("plain.zip");
        write_zip(&archive);
        let provider = connect(&archive, Some("unused")).unwrap();
        let data = provider
            .read_file(&at(&archive, "/docs/readme.md"))
            .recv()
            .unwrap();
        assert_eq!(data, README);
    }
}

fn big() -> Vec<u8> {
    (0..600_000u32).map(|i| (i % 251) as u8).collect()
}

fn names(entries: &[VfsEntry]) -> Vec<String> {
    entries.iter().map(|e| e.name.clone()).collect()
}

fn opened(archive: &Path) -> ArchiveProvider {
    let mut provider = ArchiveProvider::new(VfsPath::local(archive));
    provider.connect(ConnectOptions::default()).recv().unwrap();
    provider
}

fn at(archive: &Path, inner: &str) -> VfsPath {
    VfsPath::archive(VfsPath::local(archive), inner)
}

fn write_zip(path: &Path) {
    use zip::write::SimpleFileOptions;
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let deflated = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o640);
    zip.add_directory("docs/", SimpleFileOptions::default())
        .unwrap();
    zip.start_file("docs/readme.md", deflated).unwrap();
    zip.write_all(README).unwrap();
    // No explicit entry for `src/` or `src/deep/`.
    zip.start_file("src/deep/big.bin", deflated).unwrap();
    zip.write_all(&big()).unwrap();
    zip.add_symlink("docs/link.md", "readme.md", SimpleFileOptions::default())
        .unwrap();
    zip.finish().unwrap();
}

/// A tar stream with the same tree as [`write_zip`], plus a hard link.
fn tar_bytes() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    append_dir(&mut builder, "docs/");
    append_file(&mut builder, "docs/readme.md", README);
    append_file(&mut builder, "src/deep/big.bin", &big());
    append_link(
        &mut builder,
        tar::EntryType::Symlink,
        "docs/link.md",
        "readme.md",
    );
    append_link(
        &mut builder,
        tar::EntryType::Link,
        "docs/hard.md",
        "docs/readme.md",
    );
    builder.into_inner().unwrap()
}

fn header(kind: tar::EntryType, size: u64) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_size(size);
    header.set_mode(0o640);
    header.set_mtime(1_700_000_000);
    header
}

/// Write the raw name, bypassing the builder's path checks, so tests can
/// plant names a hostile archive would carry.
fn set_raw_name(header: &mut tar::Header, name: &str) {
    let field = &mut header.as_old_mut().name;
    field.fill(0);
    field[..name.len()].copy_from_slice(name.as_bytes());
}

fn append_file(builder: &mut tar::Builder<Vec<u8>>, name: &str, data: &[u8]) {
    let mut header = header(tar::EntryType::Regular, data.len() as u64);
    set_raw_name(&mut header, name);
    header.set_cksum();
    builder.append(&header, data).unwrap();
}

fn append_dir(builder: &mut tar::Builder<Vec<u8>>, name: &str) {
    let mut header = header(tar::EntryType::Directory, 0);
    set_raw_name(&mut header, name);
    header.set_cksum();
    builder.append(&header, std::io::empty()).unwrap();
}

fn append_link(
    builder: &mut tar::Builder<Vec<u8>>,
    kind: tar::EntryType,
    name: &str,
    target: &str,
) {
    let mut header = header(kind, 0);
    set_raw_name(&mut header, name);
    header.set_link_name(target).unwrap();
    header.set_cksum();
    builder.append(&header, std::io::empty()).unwrap();
}

fn compress(data: &[u8], compression: TarCompression) -> Vec<u8> {
    match compression {
        TarCompression::None => data.to_vec(),
        TarCompression::Gzip => {
            let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            enc.write_all(data).unwrap();
            enc.finish().unwrap()
        }
        TarCompression::Bzip2 => {
            let mut enc = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
            enc.write_all(data).unwrap();
            enc.finish().unwrap()
        }
        TarCompression::Xz => {
            let mut enc =
                lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(1))
                    .unwrap();
            enc.write_all(data).unwrap();
            enc.finish().unwrap()
        }
        TarCompression::Zstd => {
            ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest)
        }
    }
}

/// Every supported format, built in `dir`, with the tree of [`write_zip`].
fn all_formats(dir: &Path) -> Vec<PathBuf> {
    let zip = dir.join("tree.zip");
    write_zip(&zip);
    let mut archives = vec![zip];
    for (name, compression) in [
        ("tree.tar", TarCompression::None),
        ("tree.tar.gz", TarCompression::Gzip),
        ("tree.tar.bz2", TarCompression::Bzip2),
        ("tree.tar.xz", TarCompression::Xz),
        ("tree.tar.zst", TarCompression::Zstd),
    ] {
        let path = dir.join(name);
        fs::write(&path, compress(&tar_bytes(), compression)).unwrap();
        archives.push(path);
    }
    archives
}

#[test]
fn every_format_lists_and_reads_the_same_tree() {
    let dir = tempfile::tempdir().unwrap();
    for archive in all_formats(dir.path()) {
        let what = archive.display().to_string();
        let provider = opened(&archive);
        assert_eq!(
            provider.connection_state(),
            ConnectionState::Connected,
            "{what}"
        );

        let root = provider.list_dir(&at(&archive, "/")).recv().unwrap();
        assert_eq!(names(&root), ["docs", "src"], "{what}");
        assert!(root.iter().all(VfsEntry::is_dir), "{what}");
        assert_eq!(root[0].path, at(&archive, "/docs"), "{what}");

        let docs = provider.list_dir(&at(&archive, "/docs")).recv().unwrap();
        let link = docs.iter().find(|e| e.name == "link.md").unwrap();
        assert!(link.is_symlink(), "listing reports the link itself: {what}");

        let readme = provider
            .read_file(&at(&archive, "/docs/readme.md"))
            .recv()
            .unwrap();
        assert_eq!(readme, README, "{what}");
        let via_link = provider
            .read_file(&at(&archive, "/docs/link.md"))
            .recv()
            .unwrap();
        assert_eq!(via_link, README, "{what}");

        let big_meta = provider
            .metadata(&at(&archive, "/src/deep/big.bin"))
            .recv()
            .unwrap();
        assert_eq!(big_meta.size, big().len() as u64, "{what}");
        assert!(big_meta.readonly, "{what}");
        let big_data = provider
            .read_file(&at(&archive, "src/deep/big.bin"))
            .recv()
            .unwrap();
        assert_eq!(big_data, big(), "{what}");

        assert!(provider.exists(&at(&archive, "/src/deep")).recv().unwrap());
        assert!(!provider.exists(&at(&archive, "/nope")).recv().unwrap());
        assert!(provider.read_file(&at(&archive, "/src")).recv().is_err());
    }
}

#[test]
fn tar_hard_links_read_their_target() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("t.tar");
    fs::write(&archive, tar_bytes()).unwrap();
    let provider = opened(&archive);
    let data = provider
        .read_file(&at(&archive, "/docs/hard.md"))
        .recv()
        .unwrap();
    assert_eq!(data, README);
}

#[test]
fn extracting_a_directory_rebuilds_the_tree_with_progress() {
    let dir = tempfile::tempdir().unwrap();
    for archive in all_formats(dir.path()) {
        let what = archive.display().to_string();
        let provider = opened(&archive);
        let dest = dir.path().join(format!(
            "out-{}",
            archive.file_name().unwrap().to_string_lossy()
        ));

        let op = provider.download_with_progress(&at(&archive, "/"), &dest);
        let result = loop {
            if let Some(result) = op.try_recv() {
                break result;
            }
            std::thread::yield_now();
        };
        assert_eq!(result.unwrap(), dest, "{what}");
        let last = op.drain_progress().unwrap();
        assert_eq!(last.bytes_downloaded, last.total_bytes, "{what}");
        assert_eq!(last.files_downloaded, last.total_files, "{what}");

        assert_eq!(
            fs::read(dest.join("docs/readme.md")).unwrap(),
            README,
            "{what}"
        );
        assert_eq!(
            fs::read(dest.join("src/deep/big.bin")).unwrap(),
            big(),
            "{what}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let link = fs::read_link(dest.join("docs/link.md")).unwrap();
            assert_eq!(link, PathBuf::from("readme.md"), "{what}");
            let mode = fs::metadata(dest.join("docs/readme.md"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o640, "{what}");
        }
    }
}

#[test]
fn extracting_a_file_writes_it_at_the_destination() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("t.zip");
    write_zip(&archive);
    let provider = opened(&archive);
    let dest = dir.path().join("fresh/dir/readme.md");
    provider
        .download(&at(&archive, "/docs/readme.md"), &dest)
        .recv()
        .unwrap();
    assert_eq!(fs::read(&dest).unwrap(), README);
}

#[test]
fn hostile_names_and_links_stay_inside_the_destination() {
    let dir = tempfile::tempdir().unwrap();
    let mut builder = tar::Builder::new(Vec::new());
    append_file(&mut builder, "../escaped.txt", b"x");
    append_file(&mut builder, "ok/../../escaped2.txt", b"x");
    append_file(&mut builder, "/abs/file.txt", b"inside");
    append_link(
        &mut builder,
        tar::EntryType::Symlink,
        "abs/out",
        "../../../etc",
    );
    append_link(&mut builder, tar::EntryType::Symlink, "abs/root", "/etc");
    append_link(&mut builder, tar::EntryType::Symlink, "abs/in", "file.txt");
    let archive = dir.path().join("evil.tar");
    fs::write(&archive, builder.into_inner().unwrap()).unwrap();

    let provider = opened(&archive);
    let root = provider.list_dir(&at(&archive, "/")).recv().unwrap();
    assert_eq!(names(&root), ["abs"]);

    let dest = dir.path().join("sandbox/out");
    provider.download(&at(&archive, "/"), &dest).recv().unwrap();
    assert_eq!(fs::read(dest.join("abs/file.txt")).unwrap(), b"inside");
    assert!(!dir.path().join("sandbox/escaped.txt").exists());
    assert!(!dir.path().join("escaped2.txt").exists());
    assert!(fs::symlink_metadata(dest.join("abs/out")).is_err());
    assert!(fs::symlink_metadata(dest.join("abs/root")).is_err());
    #[cfg(unix)]
    assert!(fs::symlink_metadata(dest.join("abs/in"))
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn a_link_out_of_an_extracted_subtree_is_not_created() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("t.tar");
    let mut builder = tar::Builder::new(Vec::new());
    append_file(&mut builder, "shared.txt", b"s");
    append_link(
        &mut builder,
        tar::EntryType::Symlink,
        "sub/up",
        "../shared.txt",
    );
    fs::write(&archive, builder.into_inner().unwrap()).unwrap();

    let provider = opened(&archive);
    let dest = dir.path().join("sub-only");
    provider
        .download(&at(&archive, "/sub"), &dest)
        .recv()
        .unwrap();
    assert!(dest.is_dir());
    assert!(fs::symlink_metadata(dest.join("up")).is_err());
}

#[test]
fn a_later_tar_entry_with_the_same_name_wins() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("t.tar");
    let mut builder = tar::Builder::new(Vec::new());
    append_file(&mut builder, "f.txt", b"old");
    append_file(&mut builder, "f.txt", b"newer");
    fs::write(&archive, builder.into_inner().unwrap()).unwrap();

    let provider = opened(&archive);
    let data = provider.read_file(&at(&archive, "/f.txt")).recv().unwrap();
    assert_eq!(data, b"newer");
}

#[test]
fn cancelling_removes_the_partial_file() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("t.zip");
    write_zip(&archive);
    let provider = opened(&archive);
    let open = provider.opened().unwrap();

    let control = Control::none();
    control.cancel.store(true, Ordering::Relaxed);
    let dest = dir.path().join("big.bin");
    let result = extract::extract(&open, Path::new("/src/deep/big.bin"), &dest, &control, None);
    assert!(matches!(result, Err(VfsError::Cancelled)));
    assert!(!dest.exists());
}

#[test]
fn every_write_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("t.zip");
    write_zip(&archive);
    let provider = opened(&archive);
    let file = at(&archive, "/docs/readme.md");

    let refused = |r: VfsResult<()>| matches!(r, Err(VfsError::NotSupported(_)));
    assert!(refused(provider.write_file(&file, b"x").recv()));
    assert!(refused(provider.delete(&file).recv()));
    assert!(refused(provider.rename(&file, &at(&archive, "/x")).recv()));
    assert!(refused(provider.create_dir(&at(&archive, "/new")).recv()));
    assert!(refused(provider.upload(&archive, &file).recv()));
}

#[test]
fn rewriting_the_archive_fails_the_connection() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("t.tar");
    fs::write(&archive, tar_bytes()).unwrap();
    let provider = opened(&archive);
    assert_eq!(provider.connection_state(), ConnectionState::Connected);

    fs::write(&archive, compress(&tar_bytes(), TarCompression::Gzip)).unwrap();
    assert_eq!(provider.connection_state(), ConnectionState::Failed);
}

#[test]
fn opening_something_that_is_not_an_archive_fails() {
    let dir = tempfile::tempdir().unwrap();
    let text = dir.path().join("notes.zip");
    fs::write(&text, "plain text").unwrap();
    let mut provider = ArchiveProvider::new(VfsPath::local(&text));
    let result = provider.connect(ConnectOptions::default()).recv();
    assert!(matches!(result, Err(VfsError::Archive(_))));

    let mut missing = ArchiveProvider::new(VfsPath::local(dir.path().join("gone.zip")));
    let result = missing.connect(ConnectOptions::default()).recv();
    assert!(matches!(result, Err(VfsError::NotFound { .. })));
}

#[test]
fn a_remote_archive_is_only_read_from_a_local_copy() {
    let container = VfsPath::remote(crate::VfsProtocol::Sftp, "host", "/a.zip");
    let mut provider = ArchiveProvider::new(container);
    let result = provider.connect(ConnectOptions::default()).recv();
    assert!(matches!(result, Err(VfsError::NotSupported(_))));
}

#[test]
fn a_local_copy_is_removed_with_its_provider() {
    let dir = tempfile::tempdir().unwrap();
    let copy_dir = tempfile::tempdir_in(dir.path()).unwrap();
    let copy = copy_dir.path().join("remote.zip");
    write_zip(&copy);
    let copy_dir_path = copy_dir.path().to_path_buf();
    let container = VfsPath::remote(crate::VfsProtocol::Sftp, "host", "/srv/remote.zip");

    let mut provider = ArchiveProvider::from_copy(
        container.clone(),
        VfsOperation::ready(Ok(copy.clone())),
        copy_dir,
        copy,
    );
    provider.connect(ConnectOptions::default()).recv().unwrap();
    let root = VfsPath::archive(container, "/");
    let entries = provider.list_dir(&root).recv().unwrap();
    assert_eq!(names(&entries), ["docs", "src"]);

    drop(provider);
    assert!(!copy_dir_path.exists());
}

#[test]
fn a_failed_copy_fails_the_connection() {
    let copy_dir = tempfile::tempdir().unwrap();
    let copy = copy_dir.path().join("remote.zip");
    let container = VfsPath::remote(crate::VfsProtocol::Sftp, "host", "/srv/remote.zip");
    let mut provider = ArchiveProvider::from_copy(
        container,
        VfsOperation::error(VfsError::NotConnected),
        copy_dir,
        copy,
    );
    let result = provider.connect(ConnectOptions::default()).recv();
    assert!(matches!(result, Err(VfsError::NotConnected)));
}

#[test]
fn an_archive_inside_an_open_archive_opens_through_the_manager() {
    let dir = tempfile::tempdir().unwrap();
    let inner = dir.path().join("inner.zip");
    write_zip(&inner);
    let outer = dir.path().join("outer.tar.gz");
    let mut builder = tar::Builder::new(Vec::new());
    append_file(&mut builder, "lib/inner.zip", &fs::read(&inner).unwrap());
    fs::write(
        &outer,
        compress(&builder.into_inner().unwrap(), TarCompression::Gzip),
    )
    .unwrap();

    let manager = VfsManager::new();
    let outer_root = at(&outer, "/");
    manager
        .connect_archive(&outer_root, ConnectOptions::default())
        .recv()
        .unwrap();
    let nested_root = VfsPath::archive(at(&outer, "/lib/inner.zip"), "/");
    manager
        .connect_archive(&nested_root, ConnectOptions::default())
        .recv()
        .unwrap();

    let entries = manager.list_dir(&nested_root).recv().unwrap();
    assert_eq!(names(&entries), ["docs", "src"]);
    let data = manager
        .read_file(&nested_root.join("docs/readme.md"))
        .recv()
        .unwrap();
    assert_eq!(data, README);
    assert_ne!(nested_root.connection_key(), outer_root.connection_key());
}

#[test]
fn the_manager_opens_and_serves_an_archive() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("t.zip");
    write_zip(&archive);
    let manager = VfsManager::new();
    let root = at(&archive, "/");

    assert!(!manager.is_connected(&root));
    manager
        .connect_archive(&root, ConnectOptions::default())
        .recv()
        .unwrap();
    assert!(manager.is_connected(&at(&archive, "/docs")));
    assert_eq!(manager.get_home_dir(&root), Some(root.clone()));

    let entries = manager.list_dir(&root).recv().unwrap();
    assert_eq!(names(&entries), ["docs", "src"]);
    let data = manager
        .read_file(&at(&archive, "/docs/readme.md"))
        .recv()
        .unwrap();
    assert_eq!(data, README);
}

/// A stored zip whose names are raw bytes with no UTF-8 flag, the way zip
/// tools on Windows write them.
fn legacy_zip(entries: &[(&[u8], &[u8])]) -> Vec<u8> {
    fn u16le(out: &mut Vec<u8>, v: u16) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    fn u32le(out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    const DOS_DATE_1980_01_01: u16 = 0x21;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let mut crc = flate2::Crc::new();
        crc.update(data);
        let offset = out.len() as u32;
        for (buf, central_entry) in [(&mut out, false), (&mut central, true)] {
            u32le(
                buf,
                if central_entry {
                    0x0201_4b50
                } else {
                    0x0403_4b50
                },
            );
            if central_entry {
                u16le(buf, 20); // version made by
            }
            u16le(buf, 20); // version needed
            u16le(buf, 0); // flags: no UTF-8 bit
            u16le(buf, 0); // stored
            u16le(buf, 0); // time
            u16le(buf, DOS_DATE_1980_01_01);
            u32le(buf, crc.sum());
            u32le(buf, data.len() as u32);
            u32le(buf, data.len() as u32);
            u16le(buf, name.len() as u16);
            u16le(buf, 0); // extra field length
            if central_entry {
                u16le(buf, 0); // comment length
                u16le(buf, 0); // disk
                u16le(buf, 0); // internal attributes
                u32le(buf, 0); // external attributes
                u32le(buf, offset);
            }
            buf.extend_from_slice(name);
        }
        out.extend_from_slice(data);
    }
    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);
    u32le(&mut out, 0x0605_4b50);
    u16le(&mut out, 0);
    u16le(&mut out, 0);
    u16le(&mut out, entries.len() as u16);
    u16le(&mut out, entries.len() as u16);
    u32le(&mut out, central_size);
    u32le(&mut out, central_offset);
    u16le(&mut out, 0);
    out
}

#[test]
fn zip_names_from_russian_windows_read_as_cp866() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("windows.zip");
    // "Документы/отчёт.txt" in CP866, plus a plain ASCII entry.
    let report: &[u8] = b"\x84\xae\xaa\xe3\xac\xa5\xad\xe2\xeb/\xae\xe2\xe7\xf1\xe2.txt";
    fs::write(
        &archive,
        legacy_zip(&[(report, b"report"), (b"readme.txt", b"hi")]),
    )
    .unwrap();

    let provider = opened(&archive);
    let root = provider.list_dir(&at(&archive, "/")).recv().unwrap();
    assert_eq!(names(&root), ["readme.txt", "Документы"]);
    let data = provider
        .read_file(&at(&archive, "/Документы/отчёт.txt"))
        .recv()
        .unwrap();
    assert_eq!(data, b"report");
}

mod packing {
    use super::*;
    use crate::archive::pack::{self, CompressionLevel, PackProgress};

    /// A small tree with a nested file, an empty directory and a symlink.
    fn source_tree(root: &Path) -> PathBuf {
        let src = root.join("project");
        fs::create_dir_all(src.join("src/deep")).unwrap();
        fs::create_dir_all(src.join("empty")).unwrap();
        fs::write(src.join("README.md"), README).unwrap();
        fs::write(src.join("src/deep/big.bin"), big()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("README.md", src.join("link.md")).unwrap();
        src
    }

    fn pack_all(sources: &[PathBuf], dest: &Path) -> VfsResult<PackProgress> {
        pack_at(sources, dest, CompressionLevel::default())
    }

    fn pack_at(
        sources: &[PathBuf],
        dest: &Path,
        level: CompressionLevel,
    ) -> VfsResult<PackProgress> {
        let items = pack::collect(sources, dest)?;
        let mut last = PackProgress::default();
        pack::pack(&items, dest, level, &mut |p| {
            last = p.clone();
            Ok(())
        })?;
        Ok(last)
    }

    fn temp_leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".termide-pack-"))
            .collect()
    }

    #[test]
    fn every_writable_format_round_trips_through_the_reader() {
        let dir = tempfile::tempdir().unwrap();
        let src = source_tree(dir.path());
        for name in [
            "out.zip",
            "out.tar",
            "out.tar.gz",
            "out.tar.bz2",
            "out.tar.xz",
            "out.tar.zst",
        ] {
            let dest = dir.path().join(name);
            let last = pack_all(std::slice::from_ref(&src), &dest).unwrap();
            assert_eq!(last.files_done, last.total_files, "{name}");
            assert_eq!(last.bytes_done, last.total_bytes, "{name}");
            assert_eq!(
                last.total_bytes,
                (README.len() + big().len()) as u64,
                "{name}"
            );

            let provider = opened(&dest);
            let root = provider.list_dir(&at(&dest, "/")).recv().unwrap();
            assert_eq!(names(&root), ["project"], "{name}");
            let top = provider.list_dir(&at(&dest, "/project")).recv().unwrap();
            #[cfg(unix)]
            assert_eq!(
                names(&top),
                ["README.md", "empty", "link.md", "src"],
                "{name}"
            );
            #[cfg(not(unix))]
            assert_eq!(names(&top), ["README.md", "empty", "src"], "{name}");
            let data = provider
                .read_file(&at(&dest, "/project/src/deep/big.bin"))
                .recv()
                .unwrap();
            assert_eq!(data, big(), "{name}");
            #[cfg(unix)]
            {
                let link = top.iter().find(|e| e.name == "link.md").unwrap();
                assert!(link.is_symlink(), "{name}");
                let via_link = provider
                    .read_file(&at(&dest, "/project/link.md"))
                    .recv()
                    .unwrap();
                assert_eq!(via_link, README, "{name}");
            }
            assert!(temp_leftovers(dir.path()).is_empty(), "{name}");
        }
    }

    /// Every level writes a readable archive in every format, and storing
    /// leaves the compressible test data larger than compressing it does.
    #[test]
    fn every_level_writes_a_readable_archive() {
        let dir = tempfile::tempdir().unwrap();
        let src = source_tree(dir.path());
        for ext in ["zip", "tar.gz", "tar.bz2", "tar.xz", "tar.zst"] {
            let mut sizes = Vec::new();
            for (i, level) in CompressionLevel::ALL.into_iter().enumerate() {
                let dest = dir.path().join(format!("l{i}.{ext}"));
                pack_at(std::slice::from_ref(&src), &dest, level).unwrap();
                let data = opened(&dest)
                    .read_file(&at(&dest, "/project/src/deep/big.bin"))
                    .recv()
                    .unwrap();
                assert_eq!(data, big(), "{ext} at {level:?}");
                sizes.push(fs::metadata(&dest).unwrap().len());
            }
            let stored = sizes[0];
            let best = sizes[3];
            assert!(stored > best, "{ext}: stored {stored}, best {best}");
            // bzip2 and xz have no stored mode; their weakest level stands in.
            if !matches!(ext, "tar.bz2" | "tar.xz") {
                assert!(stored > big().len() as u64, "{ext}: stored {stored}");
            }
        }
    }

    #[test]
    fn names_are_relative_to_the_common_parent_of_the_sources() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("a")).unwrap();
        fs::create_dir_all(dir.path().join("b")).unwrap();
        fs::write(dir.path().join("a/x.txt"), "a").unwrap();
        fs::write(dir.path().join("b/x.txt"), "b").unwrap();
        let dest = dir.path().join("two.zip");
        pack_all(
            &[dir.path().join("a/x.txt"), dir.path().join("b/x.txt")],
            &dest,
        )
        .unwrap();

        let provider = opened(&dest);
        let root = provider.list_dir(&at(&dest, "/")).recv().unwrap();
        assert_eq!(names(&root), ["a", "b"]);
        let b = provider.read_file(&at(&dest, "/b/x.txt")).recv().unwrap();
        assert_eq!(b, b"b");
    }

    #[test]
    fn the_archive_does_not_contain_itself() {
        let dir = tempfile::tempdir().unwrap();
        let src = source_tree(dir.path());
        let dest = src.join("self.tar");
        pack_all(std::slice::from_ref(&src), &dest).unwrap();
        let provider = opened(&dest);
        let inside = provider.exists(&at(&dest, "/project/self.tar")).recv();
        assert!(!inside.unwrap());
    }

    #[test]
    fn an_existing_destination_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let src = source_tree(dir.path());
        let dest = dir.path().join("taken.zip");
        fs::write(&dest, "keep me").unwrap();
        let result = pack_all(std::slice::from_ref(&src), &dest);
        assert!(matches!(result, Err(VfsError::AlreadyExists { .. })));
        assert_eq!(fs::read(&dest).unwrap(), b"keep me");
    }

    #[test]
    fn cancelling_leaves_neither_the_archive_nor_a_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = source_tree(dir.path());
        for name in ["c.zip", "c.tar.gz", "c.tar.zst"] {
            let dest = dir.path().join(name);
            let items = pack::collect(std::slice::from_ref(&src), &dest).unwrap();
            let result = pack::pack(&items, &dest, CompressionLevel::default(), &mut |p| {
                if p.bytes_done > 0 {
                    Err(VfsError::Cancelled)
                } else {
                    Ok(())
                }
            });
            assert!(
                matches!(result, Err(VfsError::Cancelled)),
                "{name}: {result:?}"
            );
            assert!(!dest.exists(), "{name}");
        }
        assert!(temp_leftovers(dir.path()).is_empty());
    }

    /// DOS time has no zone, so a zip timestamp is only exact through the
    /// extended timestamp field the packer adds; tar stores UTC seconds.
    #[test]
    fn modification_times_survive_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("dated.txt");
        fs::write(&file, "x").unwrap();
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_001);
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(when)
            .unwrap();
        for name in ["t.zip", "t.tar.gz"] {
            let dest = dir.path().join(name);
            pack_all(std::slice::from_ref(&file), &dest).unwrap();
            let meta = opened(&dest)
                .metadata(&at(&dest, "/dated.txt"))
                .recv()
                .unwrap();
            assert_eq!(meta.modified, Some(when), "{name}");
        }
    }

    #[test]
    fn a_name_that_is_not_an_archive_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let src = source_tree(dir.path());
        let result = pack_all(&[src], &dir.path().join("out.rar"));
        assert!(matches!(result, Err(VfsError::InvalidPath(_))));
    }

    #[test]
    fn an_iso_image_is_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let src = source_tree(dir.path());
        let dest = dir.path().join("out.iso");
        let result = pack_all(&[src], &dest);
        assert!(matches!(result, Err(VfsError::NotSupported(_))));
        assert!(!dest.exists());
        assert!(temp_leftovers(dir.path()).is_empty());
    }
}
