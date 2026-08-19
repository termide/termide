//! Cross-protocol copy/move worker.

use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;

use termide_vfs::{VfsManager, VfsPath};

use super::{DownloadWorker, OperationWorker, UploadWorker};
use crate::retry::RetryPolicy;
use crate::types::{
    OperationControl, OperationError, OperationPath, OperationPhase, OperationProgress,
    OperationResult,
};
/// Direction of cross-protocol transfer.
#[derive(Debug, Clone, Copy)]
pub enum CrossProtocolDirection {
    /// Download: Remote → Local
    Download,
    /// Upload: Local → Remote
    Upload,
    /// Remote to Remote (via temp file)
    RemoteToRemote,
}

/// Worker for cross-protocol copy/move operations.
///
/// Handles transfers between local and remote filesystems:
/// - Remote → Local: uses VFS download
/// - Local → Remote: uses VFS upload
/// - Remote → Remote: download to temp, then upload
pub struct CrossProtocolWorker {
    /// VFS manager for remote operations.
    vfs_manager: std::sync::Arc<VfsManager>,
    /// Source path.
    source: OperationPath,
    /// Destination path.
    destination: OperationPath,
    /// Whether to delete source after copy (move).
    is_move: bool,
    /// Transfer direction.
    direction: CrossProtocolDirection,
    /// Retry policy for network errors.
    retry_policy: RetryPolicy,
}

impl CrossProtocolWorker {
    /// Create a new cross-protocol worker.
    ///
    /// Automatically determines the transfer direction based on source/destination types.
    pub fn new(
        vfs_manager: std::sync::Arc<VfsManager>,
        source: OperationPath,
        destination: OperationPath,
        is_move: bool,
    ) -> Result<Self, OperationError> {
        let direction = match (&source, &destination) {
            (OperationPath::Remote(_), OperationPath::Local(_)) => CrossProtocolDirection::Download,
            (OperationPath::Local(_), OperationPath::Remote(_)) => CrossProtocolDirection::Upload,
            (OperationPath::Remote(_), OperationPath::Remote(_)) => {
                CrossProtocolDirection::RemoteToRemote
            }
            (OperationPath::Local(_), OperationPath::Local(_)) => {
                return Err(OperationError::Invalid(
                    "Use LocalCopyWorker for local-to-local transfers".to_string(),
                ));
            }
        };

        Ok(Self {
            vfs_manager,
            source,
            destination,
            is_move,
            direction,
            retry_policy: RetryPolicy::network(),
        })
    }

    /// Create with custom retry policy.
    pub fn with_retry_policy(
        vfs_manager: std::sync::Arc<VfsManager>,
        source: OperationPath,
        destination: OperationPath,
        is_move: bool,
        retry_policy: RetryPolicy,
    ) -> Result<Self, OperationError> {
        let mut worker = Self::new(vfs_manager, source, destination, is_move)?;
        worker.retry_policy = retry_policy;
        Ok(worker)
    }
}

impl OperationWorker for CrossProtocolWorker {
    fn execute(
        &mut self,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
    ) -> OperationResult {
        match self.direction {
            CrossProtocolDirection::Download => self.execute_download(control, progress_tx),
            CrossProtocolDirection::Upload => self.execute_upload(control, progress_tx),
            CrossProtocolDirection::RemoteToRemote => {
                self.execute_remote_to_remote(control, progress_tx)
            }
        }
    }
}

impl CrossProtocolWorker {
    /// Execute download (remote → local).
    fn execute_download(
        &self,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
    ) -> OperationResult {
        let remote_path = match &self.source {
            OperationPath::Remote(p) => p.clone(),
            _ => return OperationResult::Failed("Source must be remote for download".to_string()),
        };

        let local_path = match &self.destination {
            OperationPath::Local(p) => p.clone(),
            _ => {
                return OperationResult::Failed(
                    "Destination must be local for download".to_string(),
                )
            }
        };

        // Use DownloadWorker (pass false for is_move - we handle move ourselves)
        let mut worker = DownloadWorker::with_retry_policy(
            std::sync::Arc::clone(&self.vfs_manager),
            vec![remote_path.clone()],
            local_path,
            false, // is_move handled by CrossProtocolWorker
            self.retry_policy.clone(),
        );

        let result = worker.execute(control, progress_tx);

        // If move and successful, delete source
        if self.is_move && result.is_success() {
            if let Err(e) = self.delete_remote_source(&remote_path, control) {
                return OperationResult::Failed(format!(
                    "Copy succeeded but failed to delete source: {}",
                    e
                ));
            }
        }

        result
    }

    /// Execute upload (local → remote).
    fn execute_upload(
        &self,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
    ) -> OperationResult {
        let local_path = match &self.source {
            OperationPath::Local(p) => p.clone(),
            _ => return OperationResult::Failed("Source must be local for upload".to_string()),
        };

        let remote_path = match &self.destination {
            OperationPath::Remote(p) => p.clone(),
            _ => {
                return OperationResult::Failed("Destination must be remote for upload".to_string())
            }
        };

        // Use UploadWorker (pass false for is_move - we handle move ourselves)
        let mut worker = UploadWorker::with_retry_policy(
            std::sync::Arc::clone(&self.vfs_manager),
            vec![local_path.clone()],
            remote_path,
            false, // is_move handled by CrossProtocolWorker
            self.retry_policy.clone(),
        );

        let result = worker.execute(control, progress_tx);

        // If move and successful, delete local source
        if self.is_move && result.is_success() {
            if let Err(e) = self.delete_local_source(&local_path) {
                return OperationResult::Failed(format!(
                    "Copy succeeded but failed to delete source: {}",
                    e
                ));
            }
        }

        result
    }

    /// Execute remote-to-remote transfer via temp file.
    fn execute_remote_to_remote(
        &self,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
    ) -> OperationResult {
        let source_path = match &self.source {
            OperationPath::Remote(p) => p.clone(),
            _ => return OperationResult::Failed("Source must be remote".to_string()),
        };

        let dest_path = match &self.destination {
            OperationPath::Remote(p) => p.clone(),
            _ => return OperationResult::Failed("Destination must be remote".to_string()),
        };

        // Fast path: same connection move = server-side rename, no
        // bytes transferred. Renaming a 5 GB file shouldn't take any
        // longer than renaming a 5-byte one, so try this first whenever
        // it applies — fall back to download+upload only on error.
        if self.is_move
            && source_path.protocol == dest_path.protocol
            && source_path.host == dest_path.host
            && source_path.port == dest_path.port
            && source_path.username == dest_path.username
        {
            let _ = progress_tx.send(OperationProgress {
                phase: OperationPhase::Transferring,
                current_item: Some(format!(
                    "Renaming {}",
                    source_path
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                )),
                ..Default::default()
            });
            match self.vfs_manager.rename(&source_path, &dest_path).recv() {
                Ok(()) => return OperationResult::Success,
                Err(e) => {
                    log::warn!(
                        "Server-side rename failed ({}), falling back to copy+delete",
                        e
                    );
                    // fall through to download+upload+delete path
                }
            }
        }

        // Create temp directory for intermediate file.
        // Use a RAII guard to ensure cleanup on panic or early return.
        let temp_dir = std::env::temp_dir().join(format!("termide_xfer_{}", std::process::id()));
        if let Err(e) = std::fs::create_dir_all(&temp_dir) {
            return OperationResult::Failed(format!("Failed to create temp directory: {}", e));
        }
        struct TempDirGuard(PathBuf);
        impl Drop for TempDirGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _temp_guard = TempDirGuard(temp_dir.clone());

        let file_name = source_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("transfer_file");
        let temp_file = temp_dir.join(file_name);

        // Phase 1: Download to temp
        let _ = progress_tx.send(OperationProgress {
            phase: OperationPhase::Transferring,
            current_item: Some(format!("Downloading {} (1/2)", file_name)),
            ..Default::default()
        });

        let mut download_worker = DownloadWorker::with_retry_policy(
            std::sync::Arc::clone(&self.vfs_manager),
            vec![source_path.clone()],
            temp_file.clone(),
            false, // is_move handled by CrossProtocolWorker
            self.retry_policy.clone(),
        );

        let download_result = download_worker.execute(control, progress_tx);
        if !download_result.is_success() {
            return download_result; // _temp_guard cleans up on drop
        }

        if control.is_cancelled() {
            return OperationResult::Cancelled; // _temp_guard cleans up on drop
        }

        // Phase 2: Upload from temp
        let _ = progress_tx.send(OperationProgress {
            phase: OperationPhase::Transferring,
            current_item: Some(format!("Uploading {} (2/2)", file_name)),
            ..Default::default()
        });

        let mut upload_worker = UploadWorker::with_retry_policy(
            std::sync::Arc::clone(&self.vfs_manager),
            vec![temp_file],
            dest_path,
            false, // is_move handled by CrossProtocolWorker (temp files deleted separately)
            self.retry_policy.clone(),
        );

        let upload_result = upload_worker.execute(control, progress_tx);

        // _temp_guard will clean up temp dir on drop

        if !upload_result.is_success() {
            return upload_result;
        }

        // If move and successful, delete remote source
        if self.is_move {
            if let Err(e) = self.delete_remote_source(&source_path, control) {
                return OperationResult::Failed(format!(
                    "Copy succeeded but failed to delete source: {}",
                    e
                ));
            }
        }

        OperationResult::Success
    }

    /// Delete remote source file after successful move.
    fn delete_remote_source(
        &self,
        path: &VfsPath,
        control: &OperationControl,
    ) -> Result<(), OperationError> {
        super::poll_vfs_delete(self.vfs_manager.delete(path), control)
    }

    /// Delete local source file after successful move.
    fn delete_local_source(&self, path: &PathBuf) -> Result<(), OperationError> {
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}
