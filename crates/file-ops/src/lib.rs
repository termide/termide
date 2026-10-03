//! Unified file operations system for Termide.
//!
//! This crate provides a centralized system for managing file operations
//! (copy, move, delete, upload, download) with:
//!
//! - Unified progress reporting
//! - Pause/cancel support for all operations
//! - Priority-based operation queue
//! - Automatic retry for network operations
//! - Thread pool management (not thread-per-operation)
//!
//! # Architecture
//!
//! ```text
//!                       ┌─────────────────────┐
//!                       │    UI / Modal       │
//!                       │  (ProgressModal)    │
//!                       └──────────┬──────────┘
//!                                  │ OperationEvent
//!                                  ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    OperationManager                              │
//! │  ┌─────────┐  ┌─────────────┐  ┌─────────────────────────────┐ │
//! │  │  Queue  │  │   Active    │  │    Event Channel            │ │
//! │  │ (Deque) │──│  Operations │──│ (mpsc::Sender<Event>)       │ │
//! │  └─────────┘  └──────┬──────┘  └─────────────────────────────┘ │
//! │                      │ Workers                                  │
//! └──────────────────────┼──────────────────────────────────────────┘
//!                        │
//!         ┌──────────────┼──────────────┐
//!         │              │              │
//!         ▼              ▼              ▼
//! ┌───────────────┐ ┌───────────────┐ ┌───────────────┐
//! │ LocalWorker   │ │DownloadWorker│ │ UploadWorker  │
//! │ (Copy/Move)   │ │(VFS→Local)   │ │(Local→VFS)    │
//! └───────────────┘ └───────────────┘ └───────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use termide_file_ops::{OperationManager, OperationRequest, OperationPath};
//!
//! // Create manager
//! let vfs_manager = Arc::new(VfsManager::new());
//! let mut ops = OperationManager::new(vfs_manager);
//!
//! // Queue a copy operation
//! let request = OperationRequest::copy(
//!     vec![OperationPath::local("/source/file.txt")],
//!     OperationPath::local("/dest/"),
//! );
//! let id = ops.queue_operation(request)?;
//!
//! // Start the operation
//! ops.start(id)?;
//!
//! // Poll for events
//! loop {
//!     for event in ops.poll() {
//!         match event {
//!             OperationEvent::Progress(id, progress) => {
//!                 println!("{}% complete", progress.percentage() * 100.0);
//!             }
//!             OperationEvent::Completed(id, result) => {
//!                 println!("Operation completed: {:?}", result);
//!                 break;
//!             }
//!             _ => {}
//!         }
//!     }
//!     std::thread::sleep(std::time::Duration::from_millis(100));
//! }
//! ```

pub mod manager;
pub mod queue;
pub mod retry;
pub mod types;
pub mod worker;

// Re-export main types for convenience
pub use manager::{OperationManager, OperationManagerConfig};
pub use queue::{OperationQueue, QueuedOperation};
pub use retry::{is_retryable_error, RetryPolicy, RetryState};
pub use types::{
    BackgroundOperationSummary, ConflictInfo, ConflictMode, ConflictResolution, OperationControl,
    OperationError, OperationEvent, OperationId, OperationInfo, OperationPath, OperationPhase,
    OperationPriority, OperationProgress, OperationRequest, OperationResult, OperationType,
};
pub use worker::{
    destination_overlap, ConflictContext, DestinationOverlap, DownloadWorker, LocalCopyWorker,
    LocalDeleteWorker, OperationWorker, SpeedTracker, UploadWorker,
};
