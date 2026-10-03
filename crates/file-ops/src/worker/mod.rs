//! Worker trait and implementations for file operations.

mod conflict;
mod cross_protocol;
mod download;
mod local;
mod pack;
mod remote_delete;
mod upload;

use std::sync::mpsc;

use crate::types::{OperationControl, OperationProgress, OperationResult};

pub use conflict::{ConflictAction, ConflictContext};
pub use cross_protocol::{CrossProtocolDirection, CrossProtocolWorker};
pub use download::DownloadWorker;
pub use local::{destination_overlap, DestinationOverlap, LocalCopyWorker, LocalDeleteWorker};
pub use pack::PackWorker;
pub use remote_delete::RemoteDeleteWorker;
pub use upload::UploadWorker;

use std::time::Instant;

use crate::types::OperationError;

/// Chunk size for file operations (1MB).
pub(crate) const CHUNK_SIZE: usize = 1024 * 1024;

/// Poll a VFS delete operation until completion, checking for cancellation.
pub(super) fn poll_vfs_delete(
    operation: termide_vfs::VfsOperation<()>,
    control: &OperationControl,
) -> Result<(), OperationError> {
    loop {
        if control.is_cancelled() {
            return Err(OperationError::Cancelled);
        }
        if let Some(result) = operation.try_recv() {
            return match result {
                Ok(()) => Ok(()),
                Err(e) => Err(OperationError::Vfs(e)),
            };
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Smoothing factor for exponential moving average (alpha = 0.3).
const EMA_ALPHA: f64 = 0.3;

/// Minimum interval (seconds) between speed recalculations.
const SPEED_UPDATE_INTERVAL: f64 = 0.2;

/// Tracks transfer speed using exponential moving average.
#[derive(Debug)]
pub struct SpeedTracker {
    last_bytes: u64,
    last_time: Instant,
    current_speed: f64,
}

impl Default for SpeedTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl SpeedTracker {
    pub fn new() -> Self {
        Self {
            last_bytes: 0,
            last_time: Instant::now(),
            current_speed: 0.0,
        }
    }

    /// Update speed estimate. Returns current speed in bytes/sec.
    pub fn update(&mut self, bytes_transferred: u64) -> f64 {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_time).as_secs_f64();
        if elapsed >= SPEED_UPDATE_INTERVAL {
            let delta = bytes_transferred.saturating_sub(self.last_bytes);
            if elapsed > 0.0 {
                let instant_speed = delta as f64 / elapsed;
                self.current_speed = if self.current_speed > 0.0 {
                    (1.0 - EMA_ALPHA) * self.current_speed + EMA_ALPHA * instant_speed
                } else {
                    instant_speed
                };
            }
            self.last_bytes = bytes_transferred;
            self.last_time = now;
        }
        self.current_speed
    }

    /// Get current speed in bytes per second.
    pub fn speed(&self) -> f64 {
        self.current_speed
    }

    /// Reset speed tracker (e.g., when operation is paused).
    pub fn reset(&mut self) {
        self.current_speed = 0.0;
        self.last_bytes = 0;
        self.last_time = Instant::now();
    }

    /// Calculate ETA in seconds based on remaining bytes.
    pub fn eta(&self, bytes_transferred: u64, total_bytes: u64) -> Option<u64> {
        if self.current_speed > 0.0 && total_bytes > bytes_transferred {
            Some(((total_bytes - bytes_transferred) as f64 / self.current_speed) as u64)
        } else {
            None
        }
    }
}

/// Trait for operation workers.
pub trait OperationWorker: Send {
    /// Execute the operation.
    fn execute(
        &mut self,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
    ) -> OperationResult;

    /// Execute with conflict handling support.
    fn execute_with_conflicts(
        &mut self,
        control: &OperationControl,
        progress_tx: &mpsc::Sender<OperationProgress>,
        _conflict_ctx: Option<&mut ConflictContext>,
    ) -> OperationResult {
        // Default implementation ignores conflict context
        self.execute(control, progress_tx)
    }
}
