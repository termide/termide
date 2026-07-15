//! Conflict-resolution continuation and rename-pattern flow for batch operations.

// Note: PanelExt is used for FileManager batch operations (copy/move/delete/rename).
#![allow(deprecated)]

use anyhow::Result;

use super::super::App;
use super::batch_handler::{make_copy_or_move_request, source_name};
use crate::state::{
    ActiveModal, BatchOperation, BatchOperationType, ConflictMode, PendingAction,
    PendingRemoteDelete,
};
use crate::PanelExt;
use termide_file_ops::{OperationPath, OperationRequest};
use termide_modal::ConflictModal;
use termide_ui::path_utils;

impl App {
    /// Handle continuation of batch operation after conflict resolution
    pub(in crate::app) fn handle_continue_batch_operation(
        &mut self,
        mut operation: BatchOperation,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        use termide_modal::ConflictResolution;

        if let Some(resolution) = value.downcast_ref::<ConflictResolution>() {
            match resolution {
                ConflictResolution::Overwrite => {
                    // Overwrite this file - execute operation directly
                    if let Some(source) = operation.current_source().cloned() {
                        let dest_str_ow = operation.destination.to_string_lossy().into_owned();
                        let is_remote_dest_ow = termide_vfs::is_vfs_url(&dest_str_ow);
                        let item_name_ow = path_utils::get_file_name_string(&source);

                        // Execute operation - only use remote path when source or dest is remote
                        let needs_remote_ow = is_remote_dest_ow || !source.exists();
                        if needs_remote_ow {
                            if let Some((vfs_manager, vfs_current_path)) =
                                self.find_remote_file_manager_info()
                            {
                                // Resolve final_dest using VFS stat to distinguish file vs directory
                                let final_dest = if is_remote_dest_ow {
                                    let base = termide_vfs::parse_vfs_url(&dest_str_ow)
                                        .map(|p| p.path)
                                        .unwrap_or_else(|e| {
                                            log::warn!("Failed to parse VFS URL: {}", e);
                                            operation.destination.clone()
                                        });
                                    // Stat remote path to check if it's a directory
                                    let base_vfs = Self::vfs_path_with_connection(
                                        &vfs_current_path,
                                        base.clone(),
                                    );
                                    let base_is_dir = vfs_manager
                                        .metadata(&base_vfs)
                                        .recv()
                                        .map(|m| m.file_type.is_dir())
                                        .unwrap_or(false);
                                    if base_is_dir {
                                        base.join(&item_name_ow)
                                    } else {
                                        base
                                    }
                                } else {
                                    path_utils::resolve_batch_destination_path(
                                        &source,
                                        &operation.destination,
                                        operation.sources.len() == 1,
                                        operation.destination_is_directory(),
                                    )
                                };
                                let src_name = source_name(&source);
                                let vfs_source = vfs_current_path.join(&src_name);

                                let is_move = operation.operation_type == BatchOperationType::Move;

                                let request = if is_remote_dest_ow && source.exists() {
                                    // Local source → remote destination: upload with overwrite
                                    let vfs_dest = Self::vfs_path_with_connection(
                                        &vfs_current_path,
                                        final_dest,
                                    );
                                    let mut r = OperationRequest::upload(source.clone(), vfs_dest);
                                    r.is_move = is_move;
                                    r
                                } else if is_remote_dest_ow {
                                    // Remote source → remote destination
                                    let vfs_dest = Self::vfs_path_with_connection(
                                        &vfs_current_path,
                                        final_dest,
                                    );
                                    make_copy_or_move_request(
                                        OperationPath::Remote(vfs_source.clone()),
                                        OperationPath::Remote(vfs_dest),
                                        is_move,
                                    )
                                } else {
                                    let r =
                                        OperationRequest::download(vfs_source.clone(), final_dest);
                                    if is_move {
                                        self.state.batch.pending_delete =
                                            Some(PendingRemoteDelete {
                                                vfs_source,
                                                vfs_manager: vfs_manager.clone(),
                                            });
                                    }
                                    r
                                };

                                self.start_batch_sub_operation(request, vfs_manager, operation);
                                return Ok(());
                            }
                        }

                        // Local file or directory - use OperationManager for async copy
                        if source.is_file() || source.is_dir() {
                            use termide_file_ops::ConflictMode as FileOpsConflictMode;

                            let final_dest = path_utils::resolve_batch_destination_path(
                                &source,
                                &operation.destination,
                                operation.sources.len() == 1,
                                operation.destination_is_directory(),
                            );
                            let is_move = operation.operation_type == BatchOperationType::Move;
                            let worker_dest = if source.is_dir() && final_dest.is_dir() {
                                final_dest
                                    .parent()
                                    .map(|p| p.to_path_buf())
                                    .unwrap_or_else(|| final_dest.clone())
                            } else {
                                final_dest.clone()
                            };
                            let request = make_copy_or_move_request(
                                OperationPath::Local(source.clone()),
                                OperationPath::Local(worker_dest),
                                is_move,
                            )
                            .with_conflict_mode(FileOpsConflictMode::OverwriteAll);

                            let vfs_manager = std::sync::Arc::new(termide_vfs::VfsManager::new());

                            self.start_batch_sub_operation(request, vfs_manager, operation);
                            return Ok(());
                        }

                        // Unknown source type - skip with error
                        log::error!("Unsupported source type: {}", source.display());
                        operation.increment_error();
                    }

                    // Move to next file
                    operation.advance();

                    // Store and return to allow UI update
                    self.state.pending_action =
                        Some(PendingAction::ContinueBatchOperation { operation });
                }
                ConflictResolution::Skip => {
                    // Skip this file
                    operation.increment_skipped();
                    operation.advance();

                    // Store and return to allow UI update
                    self.state.pending_action =
                        Some(PendingAction::ContinueBatchOperation { operation });
                }
                ConflictResolution::OverwriteAll => {
                    // Set "overwrite all" mode
                    operation.set_conflict_mode(ConflictMode::OverwriteAll);

                    // Store and return to allow UI update
                    self.state.pending_action =
                        Some(PendingAction::ContinueBatchOperation { operation });
                }
                ConflictResolution::SkipAll => {
                    // Set "skip all" mode
                    operation.set_conflict_mode(ConflictMode::SkipAll);
                    operation.increment_skipped();
                    operation.advance();

                    // Store and return to allow UI update
                    self.state.pending_action =
                        Some(PendingAction::ContinueBatchOperation { operation });
                }
                ConflictResolution::Rename => {
                    // Request rename pattern for single file
                    if let Some(source) = operation.current_source() {
                        let original_name = path_utils::get_file_name_string(source);

                        // Get file metadata for preview
                        let metadata = source.metadata().ok();
                        let created = metadata.as_ref().and_then(|m| m.created().ok());
                        let modified = metadata.as_ref().and_then(|m| m.modified().ok());

                        use termide_modal::RenamePatternModal;

                        let modal = RenamePatternModal::new(
                            &format!("Rename {}", original_name),
                            &original_name,
                            "$0", // Default pattern
                            created,
                            modified,
                        );

                        self.state.pending_action = Some(PendingAction::RenameWithPattern {
                            operation,
                            original_name,
                        });
                        self.state.active_modal = Some(ActiveModal::RenamePattern(Box::new(modal)));
                    }
                }
                ConflictResolution::RenameAll => {
                    // Request rename pattern for all files
                    if let Some(source) = operation.current_source() {
                        let original_name = path_utils::get_file_name_string(source);

                        // Get file metadata for preview
                        let metadata = source.metadata().ok();
                        let created = metadata.as_ref().and_then(|m| m.created().ok());
                        let modified = metadata.as_ref().and_then(|m| m.modified().ok());

                        use termide_modal::RenamePatternModal;

                        let modal = RenamePatternModal::new(
                            &format!("Rename all ({})", original_name),
                            &original_name,
                            "$0", // Default pattern
                            created,
                            modified,
                        );

                        // Set flag that this is RenameAll
                        operation.set_conflict_mode(ConflictMode::Ask); // Reset to Ask to apply pattern

                        self.state.pending_action = Some(PendingAction::RenameWithPattern {
                            operation,
                            original_name,
                        });
                        self.state.active_modal = Some(ActiveModal::RenamePattern(Box::new(modal)));
                    }
                }
                ConflictResolution::Cancel => {
                    // Cancel the entire batch operation
                }
            }
        }
        Ok(())
    }

    /// Handle rename pattern input result
    pub(in crate::app) fn handle_rename_with_pattern(
        &mut self,
        mut operation: BatchOperation,
        original_name: String,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(pattern_str) = value.downcast_ref::<String>() {
            use termide_state::RenamePattern;

            let pattern = RenamePattern::new(pattern_str.clone());

            // Check that for single file (Rename)
            // need to get counter and apply pattern once
            if operation.rename_pattern.is_none() {
                // This is Rename (single rename)
                if let Some(source) = operation.current_source().cloned() {
                    let counter = operation.get_and_increment_rename_counter();
                    let metadata = source.metadata().ok();
                    let created = metadata.as_ref().and_then(|m| m.created().ok());
                    let modified = metadata.as_ref().and_then(|m| m.modified().ok());

                    let new_name = pattern.apply(&original_name, counter, created, modified);

                    let dest_str = operation.destination.to_string_lossy();
                    let is_remote_dest = termide_vfs::is_vfs_url(&dest_str);

                    // Create new destination path with new name
                    let new_dest = if is_remote_dest {
                        // For remote URLs, is_dir() always returns false, so
                        // resolve_rename_destination_path would incorrectly
                        // use with_file_name(). Instead, parse URL and join.
                        let base = termide_vfs::parse_vfs_url(&dest_str)
                            .map(|p| p.path)
                            .unwrap_or_else(|e| {
                                log::warn!("Failed to parse VFS URL: {}", e);
                                operation.destination.clone()
                            });
                        base.join(&new_name)
                    } else {
                        path_utils::resolve_rename_destination_path(
                            &operation.destination,
                            &new_name,
                            operation.destination_is_directory(),
                        )
                    };

                    // Check that new path doesn't conflict (local only;
                    // remote conflicts were already detected via VFS stat)
                    if !is_remote_dest && new_dest.exists() {
                        // Show ConflictModal again
                        let remaining_items = operation
                            .sources
                            .len()
                            .saturating_sub(operation.current_index + 1);
                        let modal = ConflictModal::new(&source, &new_dest, remaining_items);
                        self.state.pending_action =
                            Some(PendingAction::ContinueBatchOperation { operation });
                        self.state.active_modal = Some(ActiveModal::Conflict(Box::new(modal)));
                        return Ok(());
                    }

                    // Execute operation directly with the renamed destination.
                    // Do NOT modify operation.destination — it must remain the original
                    // directory for subsequent files in the batch. Instead, run the
                    // operation for this single file using new_dest directly
                    // (same approach as the Overwrite handler).
                    let needs_remote_rn = is_remote_dest || !source.exists();
                    if needs_remote_rn {
                        if let Some((vfs_manager, vfs_current_path)) =
                            self.find_remote_file_manager_info()
                        {
                            let source_name = source
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            let vfs_source = vfs_current_path.join(&source_name);

                            let is_move = operation.operation_type == BatchOperationType::Move;

                            let request = if is_remote_dest {
                                let vfs_dest =
                                    Self::vfs_path_with_connection(&vfs_current_path, new_dest);
                                make_copy_or_move_request(
                                    OperationPath::Remote(vfs_source.clone()),
                                    OperationPath::Remote(vfs_dest),
                                    is_move,
                                )
                            } else {
                                let r = OperationRequest::download(vfs_source.clone(), new_dest);
                                if is_move {
                                    self.state.batch.pending_delete = Some(PendingRemoteDelete {
                                        vfs_source,
                                        vfs_manager: vfs_manager.clone(),
                                    });
                                }
                                r
                            };

                            self.start_batch_sub_operation(request, vfs_manager, operation);
                        }
                    } else if let Some(panel) = self.layout_manager.active_panel_mut() {
                        if let Some(_fm) = panel.as_file_manager_mut() {
                            let is_move = operation.operation_type == BatchOperationType::Move;
                            let request = make_copy_or_move_request(
                                OperationPath::Local(source.clone()),
                                OperationPath::Local(new_dest),
                                is_move,
                            );

                            let vfs_manager = std::sync::Arc::new(termide_vfs::VfsManager::new());

                            self.start_batch_sub_operation(request, vfs_manager, operation);
                        }
                    }
                }
            } else {
                // This is RenameAll - pattern already set in operation,
                // just continue processing
                operation.set_rename_pattern(pattern);
                // Continue processing the batch operation
                self.process_batch_operation(operation);
            }
        }
        Ok(())
    }
}
