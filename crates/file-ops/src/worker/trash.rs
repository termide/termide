//! Moving local files to the system trash.

use std::path::PathBuf;
use std::sync::mpsc;

use super::OperationWorker;
use crate::types::{OperationControl, OperationPhase, OperationProgress, OperationResult};

/// Worker moving local files and directories to the system trash, one
/// source at a time: the system moves a directory as a whole, so there is
/// nothing to count inside it.
pub struct TrashWorker {
    paths: Vec<PathBuf>,
}

impl TrashWorker {
    /// Create a worker moving `paths` to the trash.
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self { paths }
    }
}

impl OperationWorker for TrashWorker {
    fn execute(
        &mut self,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
    ) -> OperationResult {
        let total = self.paths.len();
        for (done, path) in self.paths.iter().enumerate() {
            if control.is_cancelled() {
                return OperationResult::Cancelled;
            }
            if control.wait_if_paused().is_err() {
                return OperationResult::Cancelled;
            }
            let _ = progress_tx.send(OperationProgress {
                phase: OperationPhase::Cleaning,
                files_completed: done,
                total_files: total,
                current_item: Some(path.display().to_string()),
                ..Default::default()
            });
            // A path that is gone is reported, not skipped: the user asked
            // for it by name.
            if let Err(e) = trash::delete(path) {
                return OperationResult::Failed(format!(
                    "Could not move {} to the trash: {e}",
                    path.display()
                ));
            }
        }
        let _ = progress_tx.send(OperationProgress::completed(0, total, total));
        OperationResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Moving a real file to the trash is left to the live check: on macOS
    // it would land in the user's own trash.

    #[test]
    fn a_missing_path_fails_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone.txt");
        let (tx, _rx) = mpsc::channel();
        let result = TrashWorker::new(vec![missing]).execute(&OperationControl::new(), &tx);
        match result {
            OperationResult::Failed(message) => assert!(message.contains("gone.txt"), "{message}"),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn a_cancelled_operation_moves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("kept.txt");
        std::fs::write(&file, "x").unwrap();
        let control = OperationControl::new();
        control.cancel();
        let (tx, _rx) = mpsc::channel();
        let result = TrashWorker::new(vec![file.clone()]).execute(&control, &tx);
        assert!(matches!(result, OperationResult::Cancelled));
        assert!(file.exists());
    }
}
