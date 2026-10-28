//! Remote file delete worker.

use std::sync::mpsc;

use termide_vfs::{VfsManager, VfsPath};

use super::OperationWorker;
use crate::retry::{is_retryable_error, RetryPolicy, RetryState};
use crate::types::{
    OperationControl, OperationError, OperationPhase, OperationProgress, OperationResult,
};

/// Worker for deleting remote files/directories.
///
/// Implements a two-phase approach similar to `LocalDeleteWorker`:
/// 1. **Scanning**: Recursively counts all files/directories to determine total
/// 2. **Deleting**: Deletes files one by one with per-file progress and pause support
pub struct RemoteDeleteWorker {
    /// VFS manager for remote operations.
    vfs_manager: std::sync::Arc<VfsManager>,
    /// Remote paths to delete.
    paths: Vec<VfsPath>,
    /// Retry policy for network errors.
    retry_policy: RetryPolicy,
}

impl RemoteDeleteWorker {
    /// Create a new remote delete worker.
    pub fn new(vfs_manager: std::sync::Arc<VfsManager>, paths: Vec<VfsPath>) -> Self {
        Self {
            vfs_manager,
            paths,
            retry_policy: RetryPolicy::network(),
        }
    }
}

impl OperationWorker for RemoteDeleteWorker {
    fn execute(
        &mut self,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
    ) -> OperationResult {
        // Phase 1: Scanning — count all files recursively
        let _ = progress_tx.send(OperationProgress::scanning());

        let mut total_files: usize = 0;
        for path in &self.paths {
            if control.is_cancelled() {
                return OperationResult::Cancelled;
            }

            match self.count_remote_files(path, control, progress_tx) {
                Ok(count) => total_files += count,
                Err(OperationError::Cancelled) => return OperationResult::Cancelled,
                Err(e) => return OperationResult::Failed(format!("Scanning failed: {}", e)),
            }
        }

        // Phase 2: Deleting — delete files with per-file progress
        let mut files_deleted: usize = 0;
        let mut failed_files: Vec<String> = Vec::new();

        for path in &self.paths {
            if control.is_cancelled() {
                return OperationResult::Cancelled;
            }

            match self.delete_recursive_with_progress(
                path,
                control,
                progress_tx,
                &mut files_deleted,
                total_files,
            ) {
                Ok(()) => {}
                Err(OperationError::Cancelled) => {
                    return OperationResult::Cancelled;
                }
                Err(e) => {
                    failed_files.push(format!("{}: {}", path.to_url_string(), e));
                }
            }
        }

        // Send final progress
        let _ = progress_tx.send(OperationProgress::completed(0, files_deleted, total_files));

        if failed_files.is_empty() {
            OperationResult::Success
        } else if files_deleted > 0 {
            OperationResult::PartialSuccess {
                completed: files_deleted,
                skipped: 0,
                failed: failed_files.len(),
                failed_files,
            }
        } else {
            OperationResult::Failed(failed_files.join("; "))
        }
    }
}

impl RemoteDeleteWorker {
    /// Count files recursively in a remote path (for scanning phase).
    fn count_remote_files(
        &self,
        path: &VfsPath,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
    ) -> Result<usize, OperationError> {
        control.check_cancelled()?;

        // lstat: a symlink to a directory is deleted as a link, never
        // descended into (that would delete the target's contents).
        let metadata = self
            .vfs_manager
            .symlink_metadata(path)
            .recv()
            .map_err(OperationError::Vfs)?;

        if !metadata.file_type.is_dir() {
            // Single file — count as 1
            return Ok(1);
        }

        // List directory entries
        let entries = self
            .vfs_manager
            .list_dir(path)
            .recv()
            .map_err(OperationError::Vfs)?;

        let mut count: usize = 0;
        for entry in &entries {
            control.check_cancelled()?;

            if entry.name == "." || entry.name == ".." {
                continue;
            }

            if entry.is_dir() {
                count += self.count_remote_files(&entry.path, control, progress_tx)?;
            } else {
                count += 1;
            }

            // Send scanning progress
            let _ = progress_tx.send(OperationProgress::scanning_details(
                count,
                0,
                Some(path.path.display().to_string()),
            ));
        }

        // Count the directory itself
        count += 1;
        Ok(count)
    }

    /// Delete a remote path recursively with per-file progress and pause support.
    fn delete_recursive_with_progress(
        &self,
        path: &VfsPath,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
        files_deleted: &mut usize,
        total_files: usize,
    ) -> Result<(), OperationError> {
        control.check_cancelled()?;
        control.wait_if_paused()?;

        // lstat: a symlink to a directory is deleted as a link, never
        // descended into (that would delete the target's contents).
        let metadata = self
            .vfs_manager
            .symlink_metadata(path)
            .recv()
            .map_err(OperationError::Vfs)?;

        if metadata.file_type.is_dir() {
            // List directory entries
            let entries = self
                .vfs_manager
                .list_dir(path)
                .recv()
                .map_err(OperationError::Vfs)?;

            // Delete children first (depth-first)
            for entry in &entries {
                control.check_cancelled()?;

                if entry.name == "." || entry.name == ".." {
                    continue;
                }

                self.delete_recursive_with_progress(
                    &entry.path,
                    control,
                    progress_tx,
                    files_deleted,
                    total_files,
                )?;
            }

            // Delete the now-empty directory
            control.wait_if_paused()?;

            let _ = progress_tx.send(OperationProgress {
                phase: OperationPhase::Cleaning,
                bytes_transferred: 0,
                total_bytes: 0,
                files_completed: *files_deleted,
                total_files,
                current_item: path.file_name().and_then(|n| n.to_str()).map(String::from),
                speed_bps: 0.0,
                eta_seconds: None,
                individual_file_bytes: 0,
                individual_file_total: 0,
            });

            self.delete_single_with_retry(path, control)?;
            *files_deleted += 1;
        } else {
            // Delete single file
            control.wait_if_paused()?;

            let _ = progress_tx.send(OperationProgress {
                phase: OperationPhase::Cleaning,
                bytes_transferred: 0,
                total_bytes: 0,
                files_completed: *files_deleted,
                total_files,
                current_item: path.file_name().and_then(|n| n.to_str()).map(String::from),
                speed_bps: 0.0,
                eta_seconds: None,
                individual_file_bytes: 0,
                individual_file_total: 0,
            });

            self.delete_single_with_retry(path, control)?;
            *files_deleted += 1;
        }

        Ok(())
    }

    /// Delete a single path (file or empty directory) with retry logic.
    fn delete_single_with_retry(
        &self,
        path: &VfsPath,
        control: &OperationControl,
    ) -> Result<(), OperationError> {
        let mut retry_state = RetryState::new(self.retry_policy.clone());

        loop {
            if control.is_cancelled() {
                return Err(OperationError::Cancelled);
            }

            // Wait for retry delay if this is a retry attempt
            if retry_state.attempt() > 0 {
                let delay = retry_state.next_delay();
                std::thread::sleep(delay);
            }

            // Attempt the delete — use VFS delete (non-recursive for files/empty dirs)
            let operation = self.vfs_manager.delete(path);

            let result = loop {
                if control.is_cancelled() {
                    break Err(OperationError::Cancelled);
                }
                if let Some(result) = operation.try_recv() {
                    break match result {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            if matches!(e, termide_vfs::VfsError::Cancelled) {
                                Err(OperationError::Cancelled)
                            } else {
                                Err(OperationError::Vfs(e))
                            }
                        }
                    };
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            };

            let retryable_msg: Option<String> = match &result {
                Err(OperationError::Vfs(e)) => Some(e.to_string()),
                Err(OperationError::Io(e)) => Some(e.to_string()),
                _ => None,
            };
            if let Some(msg) = retryable_msg {
                if is_retryable_error(&msg) && retry_state.record_failure(&msg) {
                    continue;
                }
            }
            return result;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::types::OperationControl;
    use std::fs;

    /// The worker drives the VFS the same way for every protocol; local
    /// paths exercise its directory/symlink decisions without a server.
    #[test]
    fn deleting_a_dir_with_a_symlinked_dir_keeps_the_link_target() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("www");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("index.html"), b"keep").unwrap();
        let victim = tmp.path().join("victim");
        fs::create_dir(&victim).unwrap();
        fs::write(victim.join("f.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(&target, victim.join("data")).unwrap();
        std::os::unix::fs::symlink(&target, tmp.path().join("top-link")).unwrap();

        let manager = std::sync::Arc::new(VfsManager::new());
        let mut worker = RemoteDeleteWorker::new(
            manager,
            vec![
                VfsPath::local(&victim),
                VfsPath::local(tmp.path().join("top-link")),
            ],
        );
        let (tx, _rx) = mpsc::channel();
        let result = worker.execute(&OperationControl::new(), &tx);

        assert!(matches!(result, OperationResult::Success), "{result:?}");
        assert!(!victim.exists());
        assert!(fs::symlink_metadata(tmp.path().join("top-link")).is_err());
        assert_eq!(fs::read(target.join("index.html")).unwrap(), b"keep");
    }
}
