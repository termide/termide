//! Central operation manager for coordinating file operations.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use termide_vfs::VfsManager;

use crate::queue::{OperationQueue, QueuedOperation};
use crate::retry::RetryPolicy;
use crate::types::{
    BackgroundOperationSummary, ConflictMode, ConflictResolution, OperationControl, OperationError,
    OperationEvent, OperationId, OperationInfo, OperationPath, OperationPriority,
    OperationProgress, OperationRequest, OperationResult, OperationType,
};
use crate::worker::{
    ConflictContext, CrossProtocolWorker, DownloadWorker, LocalCopyWorker, LocalDeleteWorker,
    OperationWorker, PackWorker, RemoteDeleteWorker, UnpackWorker, UploadWorker,
};

/// Configuration for the operation manager.
#[derive(Debug, Clone)]
pub struct OperationManagerConfig {
    /// Maximum concurrent operations.
    pub max_concurrent: usize,
    /// Maximum queue size (0 = unlimited).
    pub max_queue_size: usize,
    /// Default retry policy for network operations.
    pub retry_policy: RetryPolicy,
}

impl Default for OperationManagerConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 2,
            max_queue_size: 100,
            retry_policy: RetryPolicy::network(),
        }
    }
}

/// Active operation state.
struct ActiveOperation {
    /// Operation information.
    info: OperationInfo,
    /// Worker thread handle.
    thread_handle: Option<JoinHandle<OperationResult>>,
    /// Progress receiver.
    progress_rx: mpsc::Receiver<OperationProgress>,
    /// Conflict mode for this operation.
    conflict_mode: ConflictMode,
    /// Pending conflict resolution (if operation is waiting for user decision).
    pending_conflict_resolution: Option<mpsc::Sender<ConflictResolution>>,
}

/// Central manager for all file operations.
pub struct OperationManager {
    /// Configuration.
    config: OperationManagerConfig,
    /// VFS manager for remote operations.
    vfs_manager: Arc<VfsManager>,
    /// Operation queue.
    queue: OperationQueue,
    /// Active operations.
    active: HashMap<OperationId, ActiveOperation>,
    /// Event channel sender.
    event_tx: mpsc::Sender<OperationEvent>,
    /// Event channel receiver.
    event_rx: mpsc::Receiver<OperationEvent>,
    /// Next operation ID.
    next_id: AtomicU64,
}

impl OperationManager {
    /// Create a new operation manager.
    pub fn new(vfs_manager: Arc<VfsManager>) -> Self {
        Self::with_config(vfs_manager, OperationManagerConfig::default())
    }

    /// Update the VFS manager used for remote operations.
    pub fn set_vfs_manager(&mut self, vfs_manager: Arc<VfsManager>) {
        self.vfs_manager = vfs_manager;
    }

    /// Create with custom configuration.
    pub fn with_config(vfs_manager: Arc<VfsManager>, config: OperationManagerConfig) -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            queue: OperationQueue::with_max_size(config.max_queue_size),
            config,
            vfs_manager,
            active: HashMap::new(),
            event_tx,
            event_rx,
            next_id: AtomicU64::new(1),
        }
    }

    /// Generate a new unique operation ID.
    fn generate_id(&self) -> OperationId {
        OperationId::new(self.next_id.fetch_add(1, Ordering::SeqCst))
    }

    /// Queue a new operation.
    ///
    /// Returns the operation ID if queued successfully.
    pub fn queue_operation(
        &mut self,
        request: OperationRequest,
    ) -> Result<OperationId, OperationError> {
        let id = self.generate_id();
        let queued = QueuedOperation::new(id, request);

        if !self.queue.enqueue(queued) {
            return Err(OperationError::QueueFull);
        }

        Ok(id)
    }

    /// Start a queued operation immediately (bypassing the queue).
    pub fn start_now(&mut self, request: OperationRequest) -> Result<OperationId, OperationError> {
        let id = self.generate_id();
        self.start_operation(id, request)?;
        Ok(id)
    }

    /// Internal: start an operation.
    fn start_operation(
        &mut self,
        id: OperationId,
        request: OperationRequest,
    ) -> Result<(), OperationError> {
        // Check concurrent limit
        if self.active.len() >= self.config.max_concurrent {
            // Re-queue with high priority
            let mut queued = QueuedOperation::new(id, request);
            queued.priority = OperationPriority::High;
            self.queue.enqueue(queued);
            return Ok(());
        }

        let control = OperationControl::new();
        let (progress_tx, progress_rx) = mpsc::channel();

        // Create conflict resolution channel
        let (resolution_tx, resolution_rx) = mpsc::channel();
        let conflict_mode = request.conflict_mode;

        // Create worker based on operation type
        let worker: Box<dyn OperationWorker> = self.create_worker(&request)?;

        // Clone what we need for the thread
        let control_clone = control.clone();
        let event_tx_for_conflict = self.event_tx.clone();

        // Start worker thread with conflict handling
        let thread_handle = thread::spawn(move || {
            let mut worker = worker;

            // Create conflict context
            let mut conflict_ctx = ConflictContext {
                operation_id: id,
                conflict_mode,
                event_tx: event_tx_for_conflict,
                resolution_rx,
            };

            // Use execute_with_conflicts if available
            worker.execute_with_conflicts(&control_clone, &progress_tx, Some(&mut conflict_ctx))
            // Note: Completed event is NOT sent here. poll() generates it
            // from thread_handle.join() to avoid duplicate Completed events.
        });

        // Send started event
        let _ = self.event_tx.send(OperationEvent::Started(id));

        // Store active operation
        let info = OperationInfo {
            id,
            op_type: request.op_type,
            sources: request.sources,
            destination: request.destination,
            progress: OperationProgress::default(),
            control,
            is_active: true,
        };

        self.active.insert(
            id,
            ActiveOperation {
                info,
                thread_handle: Some(thread_handle),
                progress_rx,
                conflict_mode,
                pending_conflict_resolution: Some(resolution_tx),
            },
        );

        Ok(())
    }

    /// Create a worker for the given request.
    fn create_worker(
        &self,
        request: &OperationRequest,
    ) -> Result<Box<dyn OperationWorker>, OperationError> {
        match request.op_type {
            OperationType::Copy | OperationType::Move => {
                // Determine if local or remote
                let all_local = request.sources.iter().all(|p| p.is_local())
                    && request
                        .destination
                        .as_ref()
                        .map(|d| d.is_local())
                        .unwrap_or(true);

                if all_local {
                    let sources: Vec<PathBuf> = request
                        .sources
                        .iter()
                        .filter_map(|p| match p {
                            OperationPath::Local(path) => Some(path.clone()),
                            _ => None,
                        })
                        .collect();

                    let destination = match &request.destination {
                        Some(OperationPath::Local(path)) => path.clone(),
                        _ => {
                            return Err(OperationError::Invalid(
                                "Copy/Move requires destination".to_string(),
                            ))
                        }
                    };

                    Ok(Box::new(LocalCopyWorker::new(
                        sources,
                        destination,
                        request.is_move,
                    )))
                } else {
                    // Cross-protocol copy/move
                    // Currently supports single source
                    let source = request.sources.first().ok_or_else(|| {
                        OperationError::Invalid(
                            "Copy/Move requires at least one source".to_string(),
                        )
                    })?;

                    let destination = request.destination.as_ref().ok_or_else(|| {
                        OperationError::Invalid("Copy/Move requires destination".to_string())
                    })?;

                    let worker = CrossProtocolWorker::new(
                        Arc::clone(&self.vfs_manager),
                        source.clone(),
                        destination.clone(),
                        request.is_move,
                    )?;

                    Ok(Box::new(worker))
                }
            }

            OperationType::Delete => {
                let first_is_local = request.sources.first().is_none_or(|p| p.is_local());
                let all_same = request
                    .sources
                    .iter()
                    .all(|p| p.is_local() == first_is_local);

                if first_is_local && all_same {
                    let paths: Vec<PathBuf> = request
                        .sources
                        .iter()
                        .filter_map(|p| match p {
                            OperationPath::Local(path) => Some(path.clone()),
                            _ => None,
                        })
                        .collect();

                    Ok(Box::new(LocalDeleteWorker::new(paths)))
                } else if !first_is_local && all_same {
                    let paths: Vec<termide_vfs::VfsPath> = request
                        .sources
                        .iter()
                        .filter_map(|p| match p {
                            OperationPath::Remote(path) => Some(path.clone()),
                            _ => None,
                        })
                        .collect();

                    Ok(Box::new(RemoteDeleteWorker::new(
                        Arc::clone(&self.vfs_manager),
                        paths,
                    )))
                } else {
                    Err(OperationError::Invalid(
                        "Mixed local/remote delete not supported".to_string(),
                    ))
                }
            }

            OperationType::Download => {
                // Get all remote sources
                let sources: Vec<termide_vfs::VfsPath> = request
                    .sources
                    .iter()
                    .filter_map(|p| match p {
                        OperationPath::Remote(path) => Some(path.clone()),
                        _ => None,
                    })
                    .collect();

                if sources.is_empty() {
                    return Err(OperationError::Invalid(
                        "Download requires remote source(s)".to_string(),
                    ));
                }

                let dest_dir = match &request.destination {
                    Some(OperationPath::Local(path)) => path.clone(),
                    _ => {
                        return Err(OperationError::Invalid(
                            "Download requires local destination".to_string(),
                        ))
                    }
                };

                Ok(Box::new(DownloadWorker::new(
                    Arc::clone(&self.vfs_manager),
                    sources,
                    dest_dir,
                    request.is_move,
                )))
            }

            OperationType::Upload => {
                // Get all local sources
                let sources: Vec<PathBuf> = request
                    .sources
                    .iter()
                    .filter_map(|p| match p {
                        OperationPath::Local(path) => Some(path.clone()),
                        _ => None,
                    })
                    .collect();

                if sources.is_empty() {
                    return Err(OperationError::Invalid(
                        "Upload requires local source(s)".to_string(),
                    ));
                }

                let dest_base = match &request.destination {
                    Some(OperationPath::Remote(path)) => path.clone(),
                    _ => {
                        return Err(OperationError::Invalid(
                            "Upload requires remote destination".to_string(),
                        ))
                    }
                };

                Ok(Box::new(UploadWorker::new(
                    Arc::clone(&self.vfs_manager),
                    sources,
                    dest_base,
                    request.is_move,
                )))
            }

            OperationType::Pack => {
                let sources: Vec<PathBuf> = request
                    .sources
                    .iter()
                    .map(|p| match p {
                        OperationPath::Local(path) => Ok(path.clone()),
                        OperationPath::Remote(_) => Err(OperationError::Invalid(
                            "Pack requires local sources".to_string(),
                        )),
                    })
                    .collect::<Result<_, _>>()?;
                let archive = match &request.destination {
                    Some(OperationPath::Local(path)) => path.clone(),
                    _ => {
                        return Err(OperationError::Invalid(
                            "Pack requires a local archive path".to_string(),
                        ))
                    }
                };
                Ok(Box::new(PackWorker::new(
                    sources,
                    archive,
                    request.compression,
                )))
            }

            OperationType::Unpack => {
                let archives: Vec<PathBuf> = request
                    .sources
                    .iter()
                    .map(|p| match p {
                        OperationPath::Local(path) => Ok(path.clone()),
                        OperationPath::Remote(_) => Err(OperationError::Invalid(
                            "Unpack requires local archives".to_string(),
                        )),
                    })
                    .collect::<Result<_, _>>()?;
                let destination = match &request.destination {
                    Some(OperationPath::Local(path)) => path.clone(),
                    _ => {
                        return Err(OperationError::Invalid(
                            "Unpack requires a local destination".to_string(),
                        ))
                    }
                };
                Ok(Box::new(UnpackWorker::new(
                    archives,
                    destination,
                    request.archive_passwords.clone(),
                )))
            }
        }
    }

    /// Pause an active operation.
    pub fn pause(&self, id: OperationId) {
        if let Some(op) = self.active.get(&id) {
            op.info.control.set_paused(true);
            let _ = self.event_tx.send(OperationEvent::Paused(id));
        }
    }

    /// Resume a paused operation.
    pub fn resume(&self, id: OperationId) {
        if let Some(op) = self.active.get(&id) {
            op.info.control.set_paused(false);
            let _ = self.event_tx.send(OperationEvent::Resumed(id));
        }
    }

    /// Cancel an operation (active or queued).
    pub fn cancel(&mut self, id: OperationId) {
        // Try to cancel active operation
        if let Some(op) = self.active.get(&id) {
            op.info.control.cancel();
            // The thread will send the completion event
        }

        // Try to remove from queue
        self.queue.remove(id);
    }

    /// Poll for events and update state.
    ///
    /// Returns all pending events.
    pub fn poll(&mut self) -> Vec<OperationEvent> {
        let mut events = Vec::new();

        // Collect events from the channel
        while let Ok(event) = self.event_rx.try_recv() {
            events.push(event);
        }

        // Update progress for active operations
        let mut completed = Vec::new();
        for (id, op) in &mut self.active {
            // Drain progress updates
            while let Ok(progress) = op.progress_rx.try_recv() {
                events.push(OperationEvent::Progress(*id, progress.clone()));
                op.info.progress = progress;
            }

            // Check if thread completed
            if let Some(handle) = op.thread_handle.take() {
                if handle.is_finished() {
                    // Thread finished, get result
                    match handle.join() {
                        Ok(result) => {
                            events.push(OperationEvent::Completed(*id, result));
                            completed.push(*id);
                        }
                        Err(_) => {
                            events.push(OperationEvent::Completed(
                                *id,
                                OperationResult::Failed("Thread panicked".to_string()),
                            ));
                            completed.push(*id);
                        }
                    }
                } else {
                    // Put handle back
                    op.thread_handle = Some(handle);
                }
            }
        }

        // Remove completed operations
        for id in completed {
            self.active.remove(&id);
        }

        // Start queued operations if we have capacity
        while self.active.len() < self.config.max_concurrent {
            if let Some(queued) = self.queue.dequeue() {
                let _ = self.start_operation(queued.id, queued.request);
            } else {
                break;
            }
        }

        events
    }

    /// Check if any operations are running or queued.
    pub fn has_operations(&self) -> bool {
        !self.active.is_empty() || !self.queue.is_empty()
    }

    /// Cancel all operations and clear the queue.
    pub fn cancel_all(&mut self) {
        // Cancel active operations
        for op in self.active.values() {
            op.info.control.cancel();
        }

        // Clear queue
        self.queue.clear();
    }

    /// Resolve a conflict for an operation waiting for user decision.
    ///
    /// Returns `true` if the resolution was sent successfully.
    pub fn resolve_conflict(&mut self, id: OperationId, resolution: ConflictResolution) -> bool {
        if let Some(op) = self.active.get_mut(&id) {
            // Update conflict mode for "All" resolutions
            match &resolution {
                ConflictResolution::OverwriteAll => {
                    op.conflict_mode = ConflictMode::OverwriteAll;
                }
                ConflictResolution::SkipAll => {
                    op.conflict_mode = ConflictMode::SkipAll;
                }
                ConflictResolution::RenameAll => {
                    op.conflict_mode = ConflictMode::RenameAll;
                }
                _ => {}
            }

            // Send resolution to waiting worker.
            // Keep the sender alive for subsequent conflicts in the same operation.
            if let Some(ref tx) = op.pending_conflict_resolution {
                if tx.send(resolution).is_ok() {
                    return true;
                }
            }
        }
        false
    }

    /// Get a summary of all background operations for status bar display.
    pub fn background_summary(&self) -> BackgroundOperationSummary {
        let mut summary = BackgroundOperationSummary {
            active_count: self.active.len(),
            queued_count: self.queue.len(),
            ..Default::default()
        };

        // Aggregate progress from all active operations
        for op in self.active.values() {
            let progress = &op.info.progress;
            summary.total_bytes_transferred += progress.bytes_transferred;
            summary.total_bytes += progress.total_bytes;
            summary.files_completed += progress.files_completed;
            summary.total_files += progress.total_files;
            summary.speed_bps += progress.speed_bps;

            if op.info.control.is_paused() {
                summary.any_paused = true;
            }

            // Use the first active operation's current item as activity description
            if summary.current_activity.is_none() {
                summary.current_activity = match op.info.op_type {
                    OperationType::Copy => Some("Copying".to_string()),
                    OperationType::Move => Some("Moving".to_string()),
                    OperationType::Delete => Some("Deleting".to_string()),
                    OperationType::Download => Some("Downloading".to_string()),
                    OperationType::Upload => Some("Uploading".to_string()),
                    OperationType::Pack => Some("Packing".to_string()),
                    OperationType::Unpack => Some("Unpacking".to_string()),
                };
            }
        }

        summary
    }
}

impl std::fmt::Debug for OperationManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OperationManager")
            .field("active_count", &self.active.len())
            .field("queued_count", &self.queue.len())
            .field("max_concurrent", &self.config.max_concurrent)
            .finish_non_exhaustive()
    }
}
