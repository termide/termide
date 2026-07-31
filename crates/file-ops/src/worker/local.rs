//! Local file copy and delete workers.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

use super::{ConflictAction, ConflictContext, OperationWorker, CHUNK_SIZE};

/// Number of files between progress updates during scanning phase.
const PROGRESS_THROTTLE_FILES: usize = 50;
use crate::types::{
    OperationControl, OperationError, OperationPhase, OperationProgress, OperationResult,
};
/// Worker for local file/directory copy operations.
pub struct LocalCopyWorker {
    /// Source paths.
    sources: Vec<PathBuf>,
    /// Destination path.
    destination: PathBuf,
    /// Whether to delete source after copy (move).
    is_move: bool,
}

impl LocalCopyWorker {
    /// Create a new local copy worker.
    pub fn new(sources: Vec<PathBuf>, destination: PathBuf, is_move: bool) -> Self {
        Self {
            sources,
            destination,
            is_move,
        }
    }

    /// Scan directory to count files and total size with progress reporting.
    #[allow(clippy::only_used_in_recursion)]
    fn scan_directory(
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
                self.scan_directory(
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

                if (*accumulated_files).is_multiple_of(PROGRESS_THROTTLE_FILES) {
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

    /// Copy a single file with progress.
    #[allow(clippy::too_many_arguments)]
    fn copy_file(
        &self,
        source: &Path,
        dest: &Path,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
        bytes_copied: &mut u64,
        total_bytes: u64,
        files_copied: &mut usize,
        total_files: usize,
        start_time: Instant,
    ) -> Result<(), OperationError> {
        control.check_cancelled()?;
        control.wait_if_paused()?;

        let metadata = fs::symlink_metadata(source)?;

        if metadata.is_symlink() {
            // Copy symlink
            #[cfg(unix)]
            {
                let link_target = fs::read_link(source)?;
                std::os::unix::fs::symlink(&link_target, dest)?;
            }
            #[cfg(not(unix))]
            {
                fs::copy(source, dest)?;
            }
            *files_copied += 1;
        } else {
            // Copy regular file with chunked reading
            let file_size = metadata.len();
            let mut source_file = File::open(source)?;
            let mut dest_file = File::create(dest)?;

            let mut buffer = vec![0u8; CHUNK_SIZE];
            let mut file_bytes_copied = 0u64;

            loop {
                control.check_cancelled()?;
                control.wait_if_paused()?;

                let bytes_read = source_file.read(&mut buffer)?;
                if bytes_read == 0 {
                    break;
                }

                dest_file.write_all(&buffer[..bytes_read])?;
                file_bytes_copied += bytes_read as u64;
                *bytes_copied += bytes_read as u64;

                // Calculate speed and ETA
                let elapsed = start_time.elapsed().as_secs_f64();
                let speed_bps = if elapsed > 0.0 {
                    *bytes_copied as f64 / elapsed
                } else {
                    0.0
                };
                let remaining_bytes = total_bytes.saturating_sub(*bytes_copied);
                let eta_seconds = if speed_bps > 0.0 {
                    Some((remaining_bytes as f64 / speed_bps) as u64)
                } else {
                    None
                };

                // Send progress
                let _ = progress_tx.send(OperationProgress {
                    phase: OperationPhase::Transferring,
                    bytes_transferred: *bytes_copied,
                    total_bytes,
                    files_completed: *files_copied,
                    total_files,
                    current_item: source
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(String::from),
                    speed_bps,
                    eta_seconds,
                    individual_file_bytes: 0,
                    individual_file_total: 0,
                });
            }

            *files_copied += 1;

            // Verify file size
            if file_bytes_copied != file_size {
                return Err(OperationError::Io(std::io::Error::other(format!(
                    "File size mismatch for {}: expected {}, got {}",
                    source.display(),
                    file_size,
                    file_bytes_copied
                ))));
            }

            // Preserve permissions from source
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                let perms = std::fs::Permissions::from_mode(metadata.mode());
                if let Err(e) = fs::set_permissions(dest, perms) {
                    log::warn!("failed to preserve permissions on {dest:?}: {e}");
                }
            }
        }

        Ok(())
    }

    /// Copy a directory recursively.
    #[allow(clippy::too_many_arguments)]
    fn copy_directory(
        &self,
        source: &Path,
        dest: &Path,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
        bytes_copied: &mut u64,
        total_bytes: u64,
        files_copied: &mut usize,
        total_files: usize,
        start_time: Instant,
        depth: usize,
    ) -> Result<(), OperationError> {
        if depth > termide_vfs::MAX_RECURSION_DEPTH {
            return Err(OperationError::Invalid(format!(
                "Directory nesting too deep (> {})",
                termide_vfs::MAX_RECURSION_DEPTH
            )));
        }

        control.check_cancelled()?;
        control.wait_if_paused()?;

        // Create destination directory
        fs::create_dir_all(dest)?;

        for entry in fs::read_dir(source)? {
            control.check_cancelled()?;
            let entry = entry?;
            let entry_path = entry.path();
            let dest_path = dest.join(entry.file_name());
            let metadata = fs::symlink_metadata(&entry_path)?;

            if metadata.is_dir() && !metadata.is_symlink() {
                self.copy_directory(
                    &entry_path,
                    &dest_path,
                    control,
                    progress_tx,
                    bytes_copied,
                    total_bytes,
                    files_copied,
                    total_files,
                    start_time,
                    depth + 1,
                )?;
            } else {
                self.copy_file(
                    &entry_path,
                    &dest_path,
                    control,
                    progress_tx,
                    bytes_copied,
                    total_bytes,
                    files_copied,
                    total_files,
                    start_time,
                )?;
            }
        }

        Ok(())
    }
}

impl OperationWorker for LocalCopyWorker {
    fn execute(
        &mut self,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
    ) -> OperationResult {
        // Default implementation without conflict checking
        self.execute_with_conflicts(control, progress_tx, None)
    }

    fn execute_with_conflicts(
        &mut self,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
        mut conflict_ctx: Option<&mut ConflictContext>,
    ) -> OperationResult {
        let start_time = Instant::now();

        // Phase 1: Scan to get totals
        let _ = progress_tx.send(OperationProgress::scanning());

        let mut total_files: usize = 0;
        let mut total_bytes: u64 = 0;

        for source in &self.sources {
            if control.is_cancelled() {
                return OperationResult::Cancelled;
            }

            let metadata = match fs::symlink_metadata(source) {
                Ok(m) => m,
                Err(e) => return OperationResult::Failed(e.to_string()),
            };

            if metadata.is_dir() {
                match self.scan_directory(
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
            } else {
                total_files += 1;
                if !metadata.is_symlink() {
                    total_bytes += metadata.len();
                }
            }
        }

        // Send final scanning progress before switching to transfer phase
        let _ = progress_tx.send(OperationProgress::scanning_details(
            total_files,
            total_bytes,
            None,
        ));

        // Phase 2: Copy files
        let mut bytes_copied = 0u64;
        let mut files_copied = 0usize;
        let mut skipped_files = 0usize;
        // Track sources that were successfully copied (for move cleanup)
        let mut copied_sources: Vec<PathBuf> = Vec::new();

        for source in &self.sources {
            if control.is_cancelled() {
                return OperationResult::Cancelled;
            }

            let metadata = match fs::symlink_metadata(source) {
                Ok(m) => m,
                Err(e) => return OperationResult::Failed(e.to_string()),
            };

            let dest = if self.destination.is_dir() || self.sources.len() > 1 {
                self.destination
                    .join(source.file_name().unwrap_or_default())
            } else {
                self.destination.clone()
            };

            // Check for conflict at top level before copying
            // Determine final destination (may be renamed)
            let final_dest = if let Some(ref mut ctx) = conflict_ctx {
                let remaining = total_files.saturating_sub(files_copied + skipped_files);
                match ctx.check_conflict(source, &dest, remaining) {
                    Ok(ConflictAction::Proceed) => dest.clone(),
                    Ok(ConflictAction::Skip) => {
                        // Skip this item
                        skipped_files += 1;
                        continue;
                    }
                    Ok(ConflictAction::RenameAs(new_dest)) => new_dest,
                    Err(OperationError::Cancelled) => return OperationResult::Cancelled,
                    Err(e) => return OperationResult::Failed(e.to_string()),
                }
            } else {
                dest.clone()
            };

            // For move: try fs::rename first (atomic, preserves all metadata)
            // Falls back to copy+delete on cross-device moves (EXDEV)
            if self.is_move {
                match fs::rename(source, &final_dest) {
                    Ok(()) => {
                        files_copied += 1;
                        if !metadata.is_symlink() {
                            bytes_copied += metadata.len();
                        }
                        let _ = progress_tx.send(OperationProgress {
                            phase: OperationPhase::Transferring,
                            bytes_transferred: bytes_copied,
                            total_bytes,
                            files_completed: files_copied,
                            total_files,
                            current_item: source
                                .file_name()
                                .and_then(|n| n.to_str())
                                .map(String::from),
                            speed_bps: 0.0,
                            eta_seconds: None,
                            individual_file_bytes: 0,
                            individual_file_total: 0,
                        });
                        // Already moved — no need to track for delete phase
                        continue;
                    }
                    #[cfg(unix)]
                    Err(e) if e.raw_os_error() == Some(18 /* EXDEV */) => {
                        // Cross-device move — fall through to copy+delete
                    }
                    #[cfg(not(unix))]
                    Err(e) if e.kind() == std::io::ErrorKind::Other => {
                        // Cross-device move on non-Unix — fall through to copy+delete
                    }
                    Err(e) => return OperationResult::Failed(e.to_string()),
                }
            }

            let result = if metadata.is_dir() && !metadata.is_symlink() {
                self.copy_directory(
                    source,
                    &final_dest,
                    control,
                    progress_tx,
                    &mut bytes_copied,
                    total_bytes,
                    &mut files_copied,
                    total_files,
                    start_time,
                    0,
                )
            } else {
                self.copy_file(
                    source,
                    &final_dest,
                    control,
                    progress_tx,
                    &mut bytes_copied,
                    total_bytes,
                    &mut files_copied,
                    total_files,
                    start_time,
                )
            };

            match result {
                Ok(()) => {
                    copied_sources.push(source.clone());
                }
                Err(OperationError::Cancelled) => return OperationResult::Cancelled,
                Err(e) => return OperationResult::Failed(e.to_string()),
            }
        }

        // Phase 3: Delete source if move (only for successfully copied sources)
        if self.is_move && !copied_sources.is_empty() {
            let _ = progress_tx.send(OperationProgress {
                phase: OperationPhase::Cleaning,
                bytes_transferred: bytes_copied,
                total_bytes,
                files_completed: files_copied,
                total_files,
                current_item: None,
                speed_bps: 0.0,
                eta_seconds: None,
                individual_file_bytes: 0,
                individual_file_total: 0,
            });

            for source in &copied_sources {
                if control.is_cancelled() {
                    return OperationResult::Cancelled;
                }

                let result = if source.is_dir() {
                    fs::remove_dir_all(source)
                } else {
                    fs::remove_file(source)
                };

                if let Err(e) = result {
                    return OperationResult::Failed(format!(
                        "Failed to delete source {}: {}",
                        source.display(),
                        e
                    ));
                }
            }
        }

        // Complete
        let _ = progress_tx.send(OperationProgress::completed(
            bytes_copied,
            files_copied,
            total_files,
        ));

        if skipped_files > 0 {
            OperationResult::PartialSuccess {
                completed: files_copied,
                skipped: skipped_files,
                failed: 0,
                failed_files: Vec::new(),
            }
        } else {
            OperationResult::SuccessWithPath(self.destination.clone())
        }
    }
}

/// Worker for local file/directory delete operations.
pub struct LocalDeleteWorker {
    /// Paths to delete.
    paths: Vec<PathBuf>,
}

impl LocalDeleteWorker {
    /// Create a new local delete worker.
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self { paths }
    }

    /// Count files in directory.
    #[allow(clippy::only_used_in_recursion)]
    fn count_files(
        &self,
        path: &Path,
        control: &OperationControl,
    ) -> Result<usize, OperationError> {
        control.check_cancelled()?;

        let mut count = 0;
        for entry in fs::read_dir(path)? {
            control.check_cancelled()?;
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.is_dir() && !meta.is_symlink() {
                count += self.count_files(&entry.path(), control)?;
            } else {
                count += 1;
            }
        }
        // Count directory itself
        count += 1;
        Ok(count)
    }

    /// Delete directory recursively with progress.
    #[allow(clippy::only_used_in_recursion)]
    fn delete_directory(
        &self,
        path: &Path,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
        files_deleted: &mut usize,
        total_files: usize,
    ) -> Result<(), OperationError> {
        control.check_cancelled()?;
        control.wait_if_paused()?;

        for entry in fs::read_dir(path)? {
            control.check_cancelled()?;
            let entry = entry?;
            let entry_path = entry.path();

            // Send progress
            let _ = progress_tx.send(OperationProgress {
                phase: OperationPhase::Cleaning,
                bytes_transferred: 0,
                total_bytes: 0,
                files_completed: *files_deleted,
                total_files,
                current_item: Some(entry_path.display().to_string()),
                speed_bps: 0.0,
                eta_seconds: None,
                individual_file_bytes: 0,
                individual_file_total: 0,
            });

            let meta = fs::symlink_metadata(&entry_path)?;
            if meta.is_dir() && !meta.is_symlink() {
                self.delete_directory(
                    &entry_path,
                    control,
                    progress_tx,
                    files_deleted,
                    total_files,
                )?;
            } else {
                fs::remove_file(&entry_path)?;
                *files_deleted += 1;
            }
        }

        fs::remove_dir(path)?;
        *files_deleted += 1;

        Ok(())
    }
}

impl OperationWorker for LocalDeleteWorker {
    fn execute(
        &mut self,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
    ) -> OperationResult {
        // Phase 1: Count files
        let _ = progress_tx.send(OperationProgress::scanning());

        let mut total_files = 0;
        for path in &self.paths {
            if control.is_cancelled() {
                return OperationResult::Cancelled;
            }

            let meta = fs::symlink_metadata(path).map_err(OperationError::Io);
            let is_real_dir = meta.as_ref().is_ok_and(|m| m.is_dir() && !m.is_symlink());
            if is_real_dir {
                match self.count_files(path, control) {
                    Ok(count) => total_files += count,
                    Err(OperationError::Cancelled) => return OperationResult::Cancelled,
                    Err(e) => return OperationResult::Failed(e.to_string()),
                }
            } else {
                total_files += 1;
            }
        }

        // Phase 2: Delete files
        let mut files_deleted = 0;
        for path in &self.paths {
            if control.is_cancelled() {
                return OperationResult::Cancelled;
            }

            let meta = fs::symlink_metadata(path).map_err(OperationError::Io);
            let is_real_dir = meta.as_ref().is_ok_and(|m| m.is_dir() && !m.is_symlink());
            let result = if is_real_dir {
                self.delete_directory(path, control, progress_tx, &mut files_deleted, total_files)
            } else {
                let _ = progress_tx.send(OperationProgress {
                    phase: OperationPhase::Cleaning,
                    bytes_transferred: 0,
                    total_bytes: 0,
                    files_completed: files_deleted,
                    total_files,
                    current_item: Some(path.display().to_string()),
                    speed_bps: 0.0,
                    eta_seconds: None,
                    individual_file_bytes: 0,
                    individual_file_total: 0,
                });

                match fs::remove_file(path) {
                    Ok(()) => {
                        files_deleted += 1;
                        Ok(())
                    }
                    Err(e) => Err(OperationError::Io(e)),
                }
            };

            match result {
                Ok(()) => {}
                Err(OperationError::Cancelled) => return OperationResult::Cancelled,
                Err(e) => return OperationResult::Failed(e.to_string()),
            }
        }

        // Complete
        let _ = progress_tx.send(OperationProgress::completed(0, files_deleted, total_files));

        OperationResult::Success
    }
}
