//! Unpack worker: local archives into a local directory.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use termide_vfs::archive::ArchiveFormat;
use termide_vfs::{ConnectOptions, VfsError, VfsManager, VfsPath};

use super::{OperationWorker, SpeedTracker};
use crate::types::{ArchivePasswords, OperationControl, OperationProgress, OperationResult};

/// How often the worker looks at the extraction for progress, pause and
/// cancel.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Where each archive goes: a single archive straight into `destination`,
/// several into a directory each under it, named after the archive.
pub fn unpack_targets(archives: &[PathBuf], destination: &Path) -> Vec<(PathBuf, PathBuf)> {
    match archives {
        [single] => vec![(single.clone(), destination.to_path_buf())],
        many => many
            .iter()
            .map(|archive| (archive.clone(), destination.join(unpack_dir_name(archive))))
            .collect(),
    }
}

/// The directory name an archive unpacks into: its name without the archive
/// suffix (`docs.tar.gz` -> `docs`).
pub fn unpack_dir_name(archive: &Path) -> String {
    let name = archive.file_name().unwrap_or_default().to_string_lossy();
    ArchiveFormat::strip_suffix(&name)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{name}.d"))
}

/// Extracts every entry of each archive into its target directory, creating
/// it as needed. Files already there under the same name are replaced; the
/// caller confirms that before starting, and asks beforehand for the
/// passwords of encrypted archives, since a background run cannot.
pub struct UnpackWorker {
    jobs: Vec<(PathBuf, PathBuf)>,
    passwords: ArchivePasswords,
}

impl UnpackWorker {
    pub fn new(archives: Vec<PathBuf>, destination: PathBuf, passwords: ArchivePasswords) -> Self {
        Self {
            jobs: unpack_targets(&archives, &destination),
            passwords,
        }
    }

    /// Open one archive and extract its root into `dest`, forwarding
    /// progress offset by what earlier archives already did.
    fn unpack_one(
        &self,
        archive: &Path,
        dest: &Path,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
        speed: &mut SpeedTracker,
        done: (u64, usize),
    ) -> Result<(u64, usize), OperationResult> {
        let vfs = VfsManager::new();
        let root = VfsPath::archive(VfsPath::local(archive), "/");
        let name = archive
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        let failed = |error: &VfsError| {
            OperationResult::Failed(
                termide_i18n::t().op_unpack_failed(&name, &crate::messages::vfs_error_text(error)),
            )
        };
        let options = match self.passwords.get(archive) {
            Some(password) => ConnectOptions::with_password(password),
            None => ConnectOptions::default(),
        };
        let connect = vfs.connect_archive(&root, options);
        loop {
            if control.is_cancelled() {
                return Err(OperationResult::Cancelled);
            }
            if let Some(result) = connect.try_recv() {
                match result {
                    Ok(()) => break,
                    Err(e) => return Err(failed(&e)),
                }
            }
            std::thread::sleep(POLL_INTERVAL);
        }

        let operation = vfs.download_with_progress(&root, dest);
        let mut last = (0u64, 0usize);
        loop {
            if control.is_cancelled() {
                operation.cancel();
                return Err(OperationResult::Cancelled);
            }
            operation.set_paused(control.is_paused());
            if let Some(result) = operation.try_recv() {
                return match result {
                    Ok(_) => Ok((done.0 + last.0, done.1 + last.1)),
                    Err(VfsError::Cancelled) => Err(OperationResult::Cancelled),
                    Err(e) => Err(failed(&e)),
                };
            }
            if let Some(p) = operation.drain_progress() {
                last = (p.bytes_downloaded, p.files_downloaded);
                let bytes = done.0 + p.bytes_downloaded;
                let total = done.0 + p.total_bytes;
                let bps = speed.update(bytes);
                let mut progress = OperationProgress::transferring(
                    bytes,
                    total,
                    done.1 + p.files_downloaded,
                    done.1 + p.total_files,
                )
                .with_speed(bps, speed.eta(bytes, total));
                if let Some(current) = p.current_file {
                    progress = progress.with_item(current);
                }
                let _ = progress_tx.send(progress);
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

impl OperationWorker for UnpackWorker {
    fn execute(
        &mut self,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
    ) -> OperationResult {
        let _ = progress_tx.send(OperationProgress::scanning());
        let mut speed = SpeedTracker::new();
        let mut done = (0u64, 0usize);
        for (archive, dest) in &self.jobs {
            match self.unpack_one(archive, dest, control, progress_tx, &mut speed, done) {
                Ok(after) => done = after,
                Err(result) => return result,
            }
        }
        let _ = progress_tx.send(OperationProgress::completed(done.0, done.1, done.1));
        match &self.jobs[..] {
            [(_, dest)] => OperationResult::SuccessWithPath(dest.clone()),
            _ => OperationResult::Success,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::PackWorker;

    fn run(worker: &mut dyn OperationWorker, control: &OperationControl) -> OperationResult {
        let (tx, _rx) = mpsc::channel();
        worker.execute(control, &tx)
    }

    /// A `docs.zip` holding `docs/a.txt` and `docs/sub/b.txt`.
    fn sample_archive(dir: &Path, name: &str) -> PathBuf {
        let src = dir.join("src").join(name);
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), "a").unwrap();
        std::fs::write(src.join("sub/b.txt"), "b").unwrap();
        let archive = dir.join(format!("{name}.zip"));
        let mut pack = PackWorker::new(
            vec![src],
            archive.clone(),
            termide_vfs::archive::pack::CompressionLevel::Normal,
        );
        assert!(run(&mut pack, &OperationControl::new()).is_success());
        archive
    }

    #[test]
    fn targets_put_one_archive_in_place_and_several_in_their_own_dirs() {
        let dest = Path::new("/out");
        let one = vec![PathBuf::from("/a/docs.tar.gz")];
        assert_eq!(
            unpack_targets(&one, dest),
            vec![(one[0].clone(), dest.into())]
        );
        let two = vec![PathBuf::from("/a/docs.tar.gz"), PathBuf::from("/a/img.zip")];
        assert_eq!(
            unpack_targets(&two, dest),
            vec![
                (two[0].clone(), PathBuf::from("/out/docs")),
                (two[1].clone(), PathBuf::from("/out/img")),
            ]
        );
    }

    #[test]
    fn unpacks_the_whole_archive_into_a_new_directory() {
        let dir = tempfile::tempdir().unwrap();
        let archive = sample_archive(dir.path(), "docs");
        let dest = dir.path().join("out/docs");

        let mut worker =
            UnpackWorker::new(vec![archive], dest.clone(), ArchivePasswords::default());
        let result = run(&mut worker, &OperationControl::new());
        assert!(matches!(result, OperationResult::SuccessWithPath(ref p) if *p == dest));
        assert_eq!(std::fs::read(dest.join("docs/a.txt")).unwrap(), b"a");
        assert_eq!(std::fs::read(dest.join("docs/sub/b.txt")).unwrap(), b"b");
    }

    #[test]
    fn several_archives_land_in_a_directory_each() {
        let dir = tempfile::tempdir().unwrap();
        let docs = sample_archive(dir.path(), "docs");
        let img = sample_archive(dir.path(), "img");
        let dest = dir.path().join("out");

        let mut worker =
            UnpackWorker::new(vec![docs, img], dest.clone(), ArchivePasswords::default());
        assert!(run(&mut worker, &OperationControl::new()).is_success());
        assert!(dest.join("docs/docs/a.txt").is_file());
        assert!(dest.join("img/img/sub/b.txt").is_file());
    }

    #[test]
    fn a_broken_archive_fails_the_operation() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("bad.zip");
        std::fs::write(&archive, "not a zip").unwrap();

        let mut worker = UnpackWorker::new(
            vec![archive],
            dir.path().join("out"),
            ArchivePasswords::default(),
        );
        assert!(matches!(
            run(&mut worker, &OperationControl::new()),
            OperationResult::Failed(_)
        ));
    }

    /// An AES-encrypted `secret.zip` holding `note.txt`, password `hunter2`.
    fn encrypted_archive(dir: &Path) -> PathBuf {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        let path = dir.join("secret.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file(
            "note.txt",
            SimpleFileOptions::default().with_aes_encryption(zip::AesMode::Aes256, "hunter2"),
        )
        .unwrap();
        zip.write_all(b"top secret").unwrap();
        zip.finish().unwrap();
        path
    }

    #[test]
    fn an_encrypted_archive_unpacks_with_its_password() {
        let dir = tempfile::tempdir().unwrap();
        let archive = encrypted_archive(dir.path());
        let dest = dir.path().join("out");

        let mut worker = UnpackWorker::new(
            vec![archive.clone()],
            dest.clone(),
            ArchivePasswords::default(),
        );
        match run(&mut worker, &OperationControl::new()) {
            OperationResult::Failed(message) => assert!(
                message.contains("secret.zip") && message.contains("no password"),
                "{message}"
            ),
            other => panic!("expected a failure, got {other:?}"),
        }

        let mut passwords = ArchivePasswords::default();
        passwords.insert(archive.clone(), "hunter2".to_string());
        let mut worker = UnpackWorker::new(vec![archive], dest.clone(), passwords);
        assert!(run(&mut worker, &OperationControl::new()).is_success());
        assert_eq!(std::fs::read(dest.join("note.txt")).unwrap(), b"top secret");
    }

    #[test]
    fn a_cancelled_run_reports_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let archive = sample_archive(dir.path(), "docs");
        let control = OperationControl::new();
        control.cancel();

        let mut worker = UnpackWorker::new(
            vec![archive],
            dir.path().join("out"),
            ArchivePasswords::default(),
        );
        assert!(matches!(
            run(&mut worker, &control),
            OperationResult::Cancelled
        ));
    }
}
