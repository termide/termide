//! Conflict handling for file operations.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::UNIX_EPOCH;

use crate::types::{
    ConflictInfo, ConflictMode, ConflictResolution, OperationError, OperationEvent, OperationId,
    OperationPath,
};
/// Result of conflict check - determines how to handle the file.
#[derive(Debug)]
pub enum ConflictAction {
    /// Proceed with copy/move to original destination.
    Proceed,
    /// Skip this file.
    Skip,
    /// Rename: copy/move to new destination path.
    RenameAs(PathBuf),
}

/// Context for conflict handling in workers.
pub struct ConflictContext {
    /// Operation ID for events.
    pub operation_id: OperationId,
    /// Current conflict handling mode.
    pub conflict_mode: ConflictMode,
    /// Channel to send events (including ConflictDetected).
    pub event_tx: mpsc::Sender<OperationEvent>,
    /// Channel to receive conflict resolutions.
    pub resolution_rx: mpsc::Receiver<ConflictResolution>,
}

impl ConflictContext {
    /// Check for conflict and handle according to mode.
    /// Returns: ConflictAction indicating how to proceed.
    pub fn check_conflict(
        &mut self,
        source: &Path,
        dest: &Path,
        remaining_items: usize,
    ) -> Result<ConflictAction, OperationError> {
        // Check if destination exists
        if !dest.exists() {
            return Ok(ConflictAction::Proceed); // No conflict
        }

        // Handle based on current mode
        match self.conflict_mode {
            ConflictMode::OverwriteAll => Ok(ConflictAction::Proceed),
            ConflictMode::SkipAll => Ok(ConflictAction::Skip),
            ConflictMode::RenameAll => {
                // Auto-generate a unique name
                let new_dest = generate_unique_path(dest);
                Ok(ConflictAction::RenameAs(new_dest))
            }
            ConflictMode::Ask => {
                // Gather file info for the conflict
                let source_meta = fs::metadata(source).ok();
                let dest_meta = fs::metadata(dest).ok();

                let conflict_info = ConflictInfo {
                    source: OperationPath::Local(source.to_path_buf()),
                    destination: OperationPath::Local(dest.to_path_buf()),
                    source_size: source_meta.as_ref().map(|m| m.len()).unwrap_or(0),
                    dest_size: dest_meta.as_ref().map(|m| m.len()).unwrap_or(0),
                    source_modified: source_meta.as_ref().and_then(|m| {
                        m.modified()
                            .ok()
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs()))
                    }),
                    dest_modified: dest_meta.as_ref().and_then(|m| {
                        m.modified()
                            .ok()
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs()))
                    }),
                    remaining_items,
                };

                // Send conflict event
                let _ = self.event_tx.send(OperationEvent::ConflictDetected(
                    self.operation_id,
                    conflict_info,
                ));

                // Wait for resolution (blocking)
                match self.resolution_rx.recv() {
                    Ok(resolution) => match resolution {
                        ConflictResolution::Overwrite => Ok(ConflictAction::Proceed),
                        ConflictResolution::Skip => Ok(ConflictAction::Skip),
                        ConflictResolution::Rename(new_name) => {
                            // Use the user-provided new name
                            let new_dest = dest.parent().unwrap_or(Path::new("")).join(&new_name);
                            Ok(ConflictAction::RenameAs(new_dest))
                        }
                        ConflictResolution::OverwriteAll => {
                            self.conflict_mode = ConflictMode::OverwriteAll;
                            Ok(ConflictAction::Proceed)
                        }
                        ConflictResolution::SkipAll => {
                            self.conflict_mode = ConflictMode::SkipAll;
                            Ok(ConflictAction::Skip)
                        }
                        ConflictResolution::RenameAll => {
                            self.conflict_mode = ConflictMode::RenameAll;
                            let new_dest = generate_unique_path(dest);
                            Ok(ConflictAction::RenameAs(new_dest))
                        }
                        ConflictResolution::Cancel => Err(OperationError::Cancelled),
                    },
                    Err(_) => {
                        // Channel closed - operation cancelled
                        Err(OperationError::Cancelled)
                    }
                }
            }
        }
    }
}

/// Generate a unique path by appending a number suffix.
/// Example: "file.txt" -> "file (1).txt", "file (1).txt" -> "file (2).txt"
pub(crate) fn generate_unique_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new(""));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let extension = path.extension().and_then(|e| e.to_str());

    for i in 1..1000 {
        let new_name = if let Some(ext) = extension {
            format!("{} ({}).{}", stem, i, ext)
        } else {
            format!("{} ({})", stem, i)
        };
        let new_path = parent.join(&new_name);
        if !new_path.exists() {
            return new_path;
        }
    }

    // Fallback: use timestamp
    let timestamp = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let new_name = if let Some(ext) = extension {
        format!("{}_{}.{}", stem, timestamp, ext)
    } else {
        format!("{}_{}", stem, timestamp)
    };
    parent.join(&new_name)
}
