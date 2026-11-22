//! Remote file upload worker.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use termide_vfs::{VfsManager, VfsPath};

use super::OperationWorker;
use crate::retry::{is_retryable_error, RetryPolicy, RetryState};
use crate::types::{
    OperationControl, OperationError, OperationPhase, OperationProgress, OperationResult,
};
/// Worker for uploading files from local to remote.
///
/// Supports both single and multiple file uploads. For single file:
/// use `sources` with one element. For batch: use multiple sources.
pub struct UploadWorker {
    /// VFS manager for remote operations.
    vfs_manager: std::sync::Arc<VfsManager>,
    /// Local source paths.
    sources: Vec<PathBuf>,
    /// Remote destination base path (directory for batch, file for single).
    dest_base: VfsPath,
    /// Whether to delete sources after upload (move).
    is_move: bool,
    /// Retry policy for network errors.
    retry_policy: RetryPolicy,
}

impl UploadWorker {
    /// Create a new upload worker.
    pub fn new(
        vfs_manager: std::sync::Arc<VfsManager>,
        sources: Vec<PathBuf>,
        dest_base: VfsPath,
        is_move: bool,
    ) -> Self {
        Self {
            vfs_manager,
            sources,
            dest_base,
            is_move,
            retry_policy: RetryPolicy::network(),
        }
    }

    /// Create a new upload worker with custom retry policy.
    pub fn with_retry_policy(
        vfs_manager: std::sync::Arc<VfsManager>,
        sources: Vec<PathBuf>,
        dest_base: VfsPath,
        is_move: bool,
        retry_policy: RetryPolicy,
    ) -> Self {
        Self {
            vfs_manager,
            sources,
            dest_base,
            is_move,
            retry_policy,
        }
    }

    /// Scan local directory to count files and total size with progress reporting.
    #[allow(clippy::only_used_in_recursion)]
    fn scan_local_directory(
        &self,
        path: &Path,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
        accumulated_files: &mut usize,
        accumulated_bytes: &mut u64,
    ) -> Result<(), OperationError> {
        control.check_cancelled()?;

        for entry in fs::read_dir(path)? {
            control.check_cancelled()?;
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;

            if metadata.is_dir() && !metadata.is_symlink() {
                self.scan_local_directory(
                    &entry.path(),
                    control,
                    progress_tx,
                    accumulated_files,
                    accumulated_bytes,
                )?;
            } else {
                *accumulated_files += 1;
                if !metadata.is_symlink() {
                    *accumulated_bytes += metadata.len();
                }

                // Throttle: send progress every 50 files
                if (*accumulated_files).is_multiple_of(50) {
                    let _ = progress_tx.send(OperationProgress::scanning_details(
                        *accumulated_files,
                        *accumulated_bytes,
                        Some(path.to_string_lossy().into_owned()),
                    ));
                }
            }
        }

        Ok(())
    }

    /// Check if any source is a directory
    fn has_directories(&self) -> bool {
        self.sources.iter().any(|s| s.is_dir())
    }
}

impl OperationWorker for UploadWorker {
    fn execute(
        &mut self,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
    ) -> OperationResult {
        // Phase 1: Scan local directories if any
        let has_dirs = self.has_directories();
        let mut total_files: usize = 0;
        let mut total_bytes: u64 = 0;

        if has_dirs {
            let _ = progress_tx.send(OperationProgress::scanning());

            for source in &self.sources {
                if control.is_cancelled() {
                    return OperationResult::Cancelled;
                }

                if source.is_dir() {
                    match self.scan_local_directory(
                        source,
                        control,
                        progress_tx,
                        &mut total_files,
                        &mut total_bytes,
                    ) {
                        Ok(()) => {}
                        Err(OperationError::Cancelled) => return OperationResult::Cancelled,
                        Err(e) => return OperationResult::Failed(e.to_string()),
                    }
                } else if let Ok(metadata) = fs::metadata(source) {
                    total_files += 1;
                    total_bytes += metadata.len();
                }
            }

            // Send final scanning progress
            let _ = progress_tx.send(OperationProgress::scanning_details(
                total_files,
                total_bytes,
                None,
            ));
        } else {
            // No directories - calculate totals directly
            total_files = self.sources.len();
            total_bytes = self
                .sources
                .iter()
                .filter_map(|p| fs::metadata(p).ok().map(|m| m.len()))
                .sum();
        }

        // Single file case: simpler path
        if self.sources.len() == 1 && !self.sources[0].is_dir() {
            return self.execute_single_file(control, progress_tx);
        }

        // Phase 2: Multiple files/directories batch mode
        let mut files_completed = 0;
        let mut files_skipped = 0;
        let mut failed_files: Vec<String> = Vec::new();
        let mut bytes_transferred = 0u64;

        // For single source, dest_base already includes the destination name
        // For multiple sources, dest_base is the target directory and we need to join file names
        let single_source = self.sources.len() == 1;

        for (idx, source) in self.sources.iter().enumerate() {
            if control.is_cancelled() {
                return OperationResult::Cancelled;
            }

            // Build destination path
            let file_name = source
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file");
            let dest_path = if single_source {
                // For single source, dest_base is the full destination path
                self.dest_base.clone()
            } else {
                // For multiple sources, join file name to dest_base directory
                self.dest_base.join(file_name)
            };
            let file_size = fs::metadata(source).map(|m| m.len()).unwrap_or(0);

            // Report progress before starting this file
            let _ = progress_tx.send(OperationProgress {
                phase: OperationPhase::Transferring,
                bytes_transferred,
                total_bytes,
                files_completed,
                total_files,
                current_item: Some(format!("{} ({}/{})", file_name, idx + 1, total_files)),
                speed_bps: 0.0,
                eta_seconds: None,
                individual_file_bytes: 0,
                individual_file_total: file_size,
            });

            // Upload this file with retry
            let result = self.upload_single_with_retry(
                source,
                &dest_path,
                control,
                progress_tx,
                |sub_progress| {
                    // Combine outer batch progress with VFS-level file progress.
                    // For directory sources: files_completed=0 (outer) + sub_progress.files_completed (VFS tracks inner files).
                    // For file sources: files_completed=N (outer) + sub_progress.files_completed=0 (single file in progress).
                    let combined_files = files_completed + sub_progress.files_completed;
                    let combined_bytes = bytes_transferred + sub_progress.bytes_transferred;

                    // Send aggregated progress
                    let _ = progress_tx.send(OperationProgress {
                        phase: OperationPhase::Transferring,
                        bytes_transferred: combined_bytes,
                        total_bytes,
                        files_completed: combined_files,
                        total_files,
                        current_item: sub_progress.current_item.clone().or_else(|| {
                            Some(format!("{} ({}/{})", file_name, idx + 1, total_files))
                        }),
                        speed_bps: sub_progress.speed_bps,
                        eta_seconds: if sub_progress.speed_bps > 0.0 {
                            let remaining = total_bytes.saturating_sub(combined_bytes);
                            Some((remaining as f64 / sub_progress.speed_bps) as u64)
                        } else {
                            None
                        },
                        individual_file_bytes: sub_progress.individual_file_bytes,
                        individual_file_total: sub_progress.individual_file_total,
                    });
                },
            );

            match result {
                OperationResult::Success | OperationResult::SuccessWithPath(_) => {
                    files_completed += 1;
                    bytes_transferred += file_size;

                    // If move, delete what was uploaded of the source
                    if self.is_move {
                        if let Err(e) = remove_uploaded(source) {
                            // Log but don't fail the batch
                            failed_files
                                .push(format!("{}: failed to delete source: {}", file_name, e));
                        }
                    }
                }
                OperationResult::Cancelled => {
                    return OperationResult::Cancelled;
                }
                OperationResult::PartialSuccess { skipped, .. } => {
                    files_completed += 1;
                    files_skipped += skipped;
                    bytes_transferred += file_size;
                }
                OperationResult::Failed(err) => {
                    failed_files.push(format!("{}: {}", file_name, err));
                }
            }
        }

        // Final progress
        let _ = progress_tx.send(OperationProgress::completed(
            total_bytes,
            files_completed,
            total_files,
        ));

        if failed_files.is_empty() {
            OperationResult::Success
        } else if files_completed > 0 {
            OperationResult::PartialSuccess {
                completed: files_completed,
                skipped: files_skipped,
                failed: failed_files.len(),
                failed_files,
            }
        } else {
            OperationResult::Failed(failed_files.join("; "))
        }
    }
}

/// Refuse a pipe, socket or device as an upload source: opening a FIFO
/// blocks until a writer comes and a device such as /dev/zero never ends.
/// The source is followed through a link, as the upload opens it. Inside a
/// directory the providers' walks skip such entries themselves; a source
/// that cannot be read is left for the upload to report.
fn refuse_special_file(source: &Path) -> Result<(), OperationError> {
    match fs::metadata(source) {
        Ok(meta) if !meta.is_file() && !meta.is_dir() => {
            Err(OperationError::SpecialFile(source.to_path_buf()))
        }
        _ => Ok(()),
    }
}

/// Remove the source of a move once it has been uploaded: the file, or of a
/// directory the regular files and the directories they leave empty. An
/// upload skips symbolic links and special files inside a directory, so
/// those stay, with the directories that hold them, and the move reports
/// them instead of losing them.
fn remove_uploaded(source: &Path) -> std::io::Result<()> {
    let kept = if fs::symlink_metadata(source)?.is_dir() {
        remove_uploaded_dir(source)?
    } else {
        fs::remove_file(source)?;
        0
    };
    if kept > 0 {
        return Err(std::io::Error::other(format!(
            "{kept} entries were not uploaded (symbolic links or special files) and stay"
        )));
    }
    Ok(())
}

/// Remove the regular files under `dir` and the directories left empty;
/// returns how many entries stayed.
fn remove_uploaded_dir(dir: &Path) -> std::io::Result<usize> {
    let mut kept = 0;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            kept += remove_uploaded_dir(&entry.path())?;
        } else if file_type.is_file() {
            fs::remove_file(entry.path())?;
        } else {
            kept += 1;
        }
    }
    if kept == 0 {
        fs::remove_dir(dir)?;
    }
    Ok(kept)
}

impl UploadWorker {
    /// Execute upload for a single file (optimized path).
    fn execute_single_file(
        &self,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
    ) -> OperationResult {
        let source = &self.sources[0];
        let dest_path = if self.sources.len() == 1 {
            // For single file, dest_base could be full path or directory
            // Check if it looks like a directory by trying to use the filename
            self.dest_base.clone()
        } else {
            let file_name = source
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file");
            self.dest_base.join(file_name)
        };

        let result =
            self.upload_single_with_retry(source, &dest_path, control, progress_tx, |progress| {
                let _ = progress_tx.send(progress);
            });

        // Handle move cleanup for single file or directory
        if self.is_move && result.is_success() {
            if let Err(e) = remove_uploaded(source) {
                return OperationResult::Failed(format!(
                    "Upload succeeded but failed to delete source: {}",
                    e
                ));
            }
        }

        result
    }

    /// Upload a single file with retry logic.
    fn upload_single_with_retry<F>(
        &self,
        source: &Path,
        dest: &VfsPath,
        control: &OperationControl,
        _progress_tx: &mpsc::Sender<OperationProgress>,
        on_progress: F,
    ) -> OperationResult
    where
        F: Fn(OperationProgress),
    {
        if let Err(e) = refuse_special_file(source) {
            return OperationResult::Failed(e.to_string());
        }

        let mut retry_state = RetryState::new(self.retry_policy.clone());

        loop {
            if control.is_cancelled() {
                return OperationResult::Cancelled;
            }

            // Wait for retry delay if this is a retry attempt
            if retry_state.attempt() > 0 {
                let delay = retry_state.next_delay();
                std::thread::sleep(delay);
            }

            let result = self.execute_upload_attempt(source, dest, control, &on_progress);

            match &result {
                OperationResult::Failed(error_msg) => {
                    if is_retryable_error(error_msg) && retry_state.record_failure(error_msg) {
                        continue; // Retry
                    }
                    return result;
                }
                _ => return result,
            }
        }
    }

    /// Execute a single upload attempt.
    fn execute_upload_attempt<F>(
        &self,
        source: &Path,
        dest: &VfsPath,
        control: &OperationControl,
        on_progress: &F,
    ) -> OperationResult
    where
        F: Fn(OperationProgress),
    {
        // Use VFS upload_with_progress
        let operation = self.vfs_manager.upload_with_progress(source, dest);

        // Speed/ETA tracking
        let mut speed_tracker = super::SpeedTracker::new();

        loop {
            // Check for cancellation
            if control.is_cancelled() {
                operation.cancel();
                return OperationResult::Cancelled;
            }

            // Handle pause
            if control.is_paused() {
                operation.set_paused(true);
            } else {
                operation.set_paused(false);
            }

            // Check for completion
            if let Some(result) = operation.try_recv() {
                return match result {
                    Ok(()) => OperationResult::Success,
                    Err(e) => {
                        if matches!(e, termide_vfs::VfsError::Cancelled) {
                            OperationResult::Cancelled
                        } else {
                            OperationResult::Failed(e.to_string())
                        }
                    }
                };
            }

            // Forward progress with speed/ETA calculation
            if let Some(progress) = operation.drain_progress() {
                let bytes_transferred = progress.bytes_uploaded;
                let total_bytes = progress.total_bytes;

                let current_speed = speed_tracker.update(bytes_transferred);
                let eta_seconds = speed_tracker.eta(bytes_transferred, total_bytes);

                on_progress(OperationProgress {
                    phase: OperationPhase::Transferring,
                    bytes_transferred,
                    total_bytes,
                    files_completed: progress.files_uploaded,
                    total_files: progress.total_files,
                    current_item: progress.current_file.clone(),
                    speed_bps: current_speed,
                    eta_seconds,
                    individual_file_bytes: progress.current_file_bytes,
                    individual_file_total: progress.current_file_total,
                });
            }

            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A socket, and a link to a device, are refused before the upload opens
    /// them (a FIFO would block it forever, /dev/zero never end), and the
    /// refusal is not retried as a network error.
    #[test]
    fn special_files_are_not_uploaded() {
        let tmp = tempfile::tempdir().unwrap();
        let socket = tmp.path().join("s.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let device = tmp.path().join("null");
        std::os::unix::fs::symlink("/dev/null", &device).unwrap();
        let dest = VfsPath::remote(termide_vfs::VfsProtocol::Sftp, "nowhere", "/up");

        for sources in [vec![socket.clone()], vec![socket, device]] {
            let mut worker = UploadWorker::new(
                std::sync::Arc::new(VfsManager::new()),
                sources,
                dest.clone(),
                false,
            );
            let (tx, _rx) = mpsc::channel();
            match worker.execute(&OperationControl::new(), &tx) {
                OperationResult::Failed(message) => {
                    assert!(message.contains("not a regular file"), "{message}");
                    assert!(!message.contains("connect"), "{message}");
                }
                other => panic!("special file was uploaded: {other:?}"),
            }
        }
    }

    #[test]
    fn a_move_keeps_what_the_upload_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("src");
        fs::create_dir_all(source.join("plain/deep")).unwrap();
        fs::create_dir_all(source.join("links")).unwrap();
        fs::write(source.join("a.txt"), "a").unwrap();
        fs::write(source.join("plain/deep/b.txt"), "b").unwrap();
        fs::write(source.join("links/c.txt"), "c").unwrap();
        std::os::unix::fs::symlink("../a.txt", source.join("links/to-a")).unwrap();

        let error = remove_uploaded(&source).unwrap_err();
        assert!(error.to_string().contains("1 entries"), "{error}");
        assert!(!source.join("a.txt").exists());
        assert!(!source.join("plain").exists());
        assert!(!source.join("links/c.txt").exists());
        assert!(fs::symlink_metadata(source.join("links/to-a")).is_ok());
    }

    #[test]
    fn a_move_of_plain_files_removes_the_whole_source() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("src");
        fs::create_dir_all(source.join("sub")).unwrap();
        fs::write(source.join("sub/b.txt"), "b").unwrap();
        remove_uploaded(&source).unwrap();
        assert!(!source.exists());

        let file = tmp.path().join("f.txt");
        fs::write(&file, "f").unwrap();
        remove_uploaded(&file).unwrap();
        assert!(!file.exists());
    }
}
