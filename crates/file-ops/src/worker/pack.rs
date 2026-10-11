//! Pack worker: local files and directories into a new archive.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use termide_vfs::archive::pack;
use termide_vfs::VfsError;

use super::{OperationWorker, SpeedTracker};
use crate::types::{OperationControl, OperationProgress, OperationResult};

/// Minimum interval between progress messages: the packer reports every
/// chunk, far more often than the panel can show.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Writes `sources` into the archive `destination`, whose name picks the
/// format. The archive only appears once complete; an existing file is
/// never replaced.
pub struct PackWorker {
    sources: Vec<PathBuf>,
    destination: PathBuf,
    level: pack::CompressionLevel,
}

impl PackWorker {
    pub fn new(sources: Vec<PathBuf>, destination: PathBuf, level: pack::CompressionLevel) -> Self {
        Self {
            sources,
            destination,
            level,
        }
    }
}

impl PackWorker {
    /// The failure as the operations panel shows it, in the user's language.
    fn failed(&self, error: &VfsError) -> OperationResult {
        let name = self
            .destination
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        OperationResult::Failed(
            termide_i18n::t().op_pack_failed(&name, &crate::messages::vfs_error_text(error)),
        )
    }
}

impl OperationWorker for PackWorker {
    fn execute(
        &mut self,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
    ) -> OperationResult {
        let _ = progress_tx.send(OperationProgress::scanning());
        let items = match pack::collect(&self.sources, &self.destination) {
            Ok(items) => items,
            Err(e) => return self.failed(&e),
        };

        let mut speed = SpeedTracker::new();
        let mut last_sent: Option<Instant> = None;
        let mut last = pack::PackProgress::for_items(&items);
        let result = pack::pack(&items, &self.destination, self.level, &mut |p| {
            control
                .check_cancelled()
                .and_then(|()| control.wait_if_paused())
                .map_err(|_| VfsError::Cancelled)?;
            last = p.clone();
            if last_sent.is_none_or(|t| t.elapsed() >= PROGRESS_INTERVAL) {
                last_sent = Some(Instant::now());
                let bps = speed.update(p.bytes_done);
                let mut progress = OperationProgress::transferring(
                    p.bytes_done,
                    p.total_bytes,
                    p.files_done,
                    p.total_files,
                )
                .with_speed(bps, speed.eta(p.bytes_done, p.total_bytes));
                if let Some(current) = &p.current {
                    progress = progress.with_item(current.clone());
                }
                let _ = progress_tx.send(progress);
            }
            Ok(())
        });

        match result {
            Ok(()) => {
                let _ = progress_tx.send(OperationProgress::completed(
                    last.total_bytes,
                    last.total_files,
                    last.total_files,
                ));
                OperationResult::SuccessWithPath(self.destination.clone())
            }
            Err(VfsError::Cancelled) => OperationResult::Cancelled,
            Err(e) => self.failed(&e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(worker: &mut PackWorker, control: &OperationControl) -> OperationResult {
        let (tx, _rx) = mpsc::channel();
        worker.execute(control, &tx)
    }

    #[test]
    fn packs_the_sources_into_the_named_archive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs/a.txt"), "a").unwrap();
        let dest = dir.path().join("docs.tar.gz");

        let mut worker = PackWorker::new(
            vec![dir.path().join("docs")],
            dest.clone(),
            Default::default(),
        );
        let result = run(&mut worker, &OperationControl::new());
        assert!(matches!(result, OperationResult::SuccessWithPath(ref p) if *p == dest));
        assert!(dest.is_file());
    }

    #[test]
    fn a_cancelled_run_leaves_no_archive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        let dest = dir.path().join("a.zip");
        let control = OperationControl::new();
        control.cancel();

        let mut worker = PackWorker::new(
            vec![dir.path().join("a.txt")],
            dest.clone(),
            Default::default(),
        );
        assert!(matches!(
            run(&mut worker, &control),
            OperationResult::Cancelled
        ));
        assert!(!dest.exists());
    }

    #[test]
    fn an_existing_destination_fails_the_operation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        let dest = dir.path().join("a.zip");
        std::fs::write(&dest, "keep").unwrap();

        let mut worker = PackWorker::new(
            vec![dir.path().join("a.txt")],
            dest.clone(),
            Default::default(),
        );
        assert!(matches!(
            run(&mut worker, &OperationControl::new()),
            OperationResult::Failed(_)
        ));
        assert_eq!(std::fs::read(&dest).unwrap(), b"keep");
    }
}
