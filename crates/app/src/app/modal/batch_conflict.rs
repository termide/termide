//! Conflict-resolution continuation and rename-pattern flow for batch operations.

// Note: PanelExt is used for FileManager batch operations (copy/move/delete/rename).

use anyhow::Result;

use super::super::App;
use super::batch_handler::{is_copyable_local_source, make_copy_or_move_request};
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
        value: termide_modal::ModalOutput,
    ) -> Result<()> {
        use termide_modal::ConflictResolution;

        if let Some(resolution) = value.get::<ConflictResolution>() {
            match resolution {
                ConflictResolution::Overwrite => {
                    // Overwrite this file - execute operation directly
                    if let Some(source) = operation.current_source().cloned() {
                        let dest_str_ow = operation.destination.to_string_lossy().into_owned();
                        let is_move = operation.operation_type == BatchOperationType::Move;
                        // Under Rename All the taken name is the pattern's.
                        let renamed = renamed_item_name(&operation, &source);

                        if termide_vfs::is_vfs_url(&dest_str_ow) {
                            let name = renamed
                                .clone()
                                .unwrap_or_else(|| path_utils::get_file_name_string(&source));
                            if let Some(target) = self.remote_batch_target(&operation, &source) {
                                let (dest, _) = self.remote_item_destination(
                                    &target,
                                    &operation,
                                    &name,
                                    renamed.is_some(),
                                );
                                let request = target.request(dest, is_move);
                                self.start_batch_sub_operation(request, target.manager, operation);
                                return Ok(());
                            }
                            log::error!(
                                "No connection to the remote destination of {}",
                                source.display()
                            );
                            operation.increment_error();
                            operation.advance();
                            self.state.pending_action =
                                Some(PendingAction::ContinueBatchOperation { operation });
                            return Ok(());
                        }

                        // A remote source is downloaded to the local destination.
                        if operation.source_is_remote(&source) {
                            if let Some((vfs_manager, _, vfs_source)) =
                                self.remote_batch_source(&operation, &source)
                            {
                                let final_dest =
                                    item_destination(&operation, &source, renamed.as_deref());
                                operation.note_item_destination(&final_dest);
                                let request =
                                    OperationRequest::download(vfs_source.clone(), final_dest);
                                if is_move {
                                    self.state.batch.pending_delete = Some(PendingRemoteDelete {
                                        vfs_source,
                                        vfs_manager: vfs_manager.clone(),
                                    });
                                }
                                self.start_batch_sub_operation(request, vfs_manager, operation);
                                return Ok(());
                            }
                        }

                        // Local file or directory - use OperationManager for async copy
                        if is_copyable_local_source(&source) {
                            use termide_file_ops::ConflictMode as FileOpsConflictMode;

                            let final_dest =
                                item_destination(&operation, &source, renamed.as_deref());
                            let is_move = operation.operation_type == BatchOperationType::Move;
                            operation.note_item_destination(&final_dest);
                            let worker_dest = if source.is_dir() && final_dest.is_dir() {
                                final_dest
                                    .parent()
                                    .map(|p| p.to_path_buf())
                                    .unwrap_or_else(|| final_dest.clone())
                            } else {
                                final_dest
                            };
                            let request = make_copy_or_move_request(
                                OperationPath::Local(source),
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
                            all: false,
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
                            &format!(
                                "{} ({})",
                                termide_i18n::t().conflict_rename_all(),
                                original_name
                            ),
                            &original_name,
                            "$0", // Default pattern
                            created,
                            modified,
                        );

                        // A name the pattern gives that is taken too is asked about.
                        operation.set_conflict_mode(ConflictMode::Ask);

                        self.state.pending_action = Some(PendingAction::RenameWithPattern {
                            operation,
                            original_name,
                            all: true,
                        });
                        self.state.active_modal = Some(ActiveModal::RenamePattern(Box::new(modal)));
                    }
                }
                ConflictResolution::Cancel => self.cancel_batch_at_conflict(&operation),
            }
        }
        Ok(())
    }

    /// Cancel a batch stopped at a conflict: nothing is in flight, so its
    /// card leaves the Operations panel, and panels show what the items
    /// before it already did.
    pub(in crate::app) fn cancel_batch_at_conflict(&mut self, operation: &BatchOperation) {
        self.state.finish_batch_tracking();
        if operation.success_count > 0 {
            for panel in self.layout_manager.iter_all_panels_mut() {
                if let Some(fm) = panel.as_file_manager_mut() {
                    fm.clear_selection();
                    let _ = fm.force_reload_directory();
                }
            }
        }
        self.state.needs_redraw = true;
    }

    /// Handle rename pattern input result
    pub(in crate::app) fn handle_rename_with_pattern(
        &mut self,
        mut operation: BatchOperation,
        original_name: String,
        all: bool,
        value: termide_modal::ModalOutput,
    ) -> Result<()> {
        if let Some(pattern_str) = value.get::<String>() {
            use termide_state::RenamePattern;

            let pattern = RenamePattern::new(pattern_str.clone());

            // Rename (this item only): take the counter and apply the
            // pattern once.
            if !all {
                // This is Rename (single rename)
                if let Some(source) = operation.current_source().cloned() {
                    let counter = operation.get_and_increment_rename_counter();
                    let metadata = source.metadata().ok();
                    let created = metadata.as_ref().and_then(|m| m.created().ok());
                    let modified = metadata.as_ref().and_then(|m| m.modified().ok());

                    let new_name = pattern.apply(&original_name, counter, created, modified);

                    let dest_str = operation.destination.to_string_lossy();
                    if termide_vfs::is_vfs_url(&dest_str) {
                        self.process_remote_batch_item(operation, source, &new_name, true);
                        return Ok(());
                    }

                    let new_dest = path_utils::resolve_rename_destination_path(
                        &operation.destination,
                        &new_name,
                        operation.destination_is_directory(),
                    );

                    if std::fs::symlink_metadata(&new_dest).is_ok() {
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
                    // The renamed path is what this item writes, and what
                    // a cancel may clean up.
                    operation.note_item_destination(&new_dest);
                    if operation.source_is_remote(&source) {
                        if let Some((vfs_manager, _, vfs_source)) =
                            self.remote_batch_source(&operation, &source)
                        {
                            let is_move = operation.operation_type == BatchOperationType::Move;
                            let request = OperationRequest::download(vfs_source.clone(), new_dest);
                            if is_move {
                                self.state.batch.pending_delete = Some(PendingRemoteDelete {
                                    vfs_source,
                                    vfs_manager: vfs_manager.clone(),
                                });
                            }
                            self.start_batch_sub_operation(request, vfs_manager, operation);
                        }
                    } else if let Some(panel) = self.layout_manager.active_panel_mut() {
                        if let Some(_fm) = panel.as_file_manager_mut() {
                            let is_move = operation.operation_type == BatchOperationType::Move;
                            let request = make_copy_or_move_request(
                                OperationPath::Local(source),
                                OperationPath::Local(new_dest),
                                is_move,
                            );

                            let vfs_manager = std::sync::Arc::new(termide_vfs::VfsManager::new());

                            self.start_batch_sub_operation(request, vfs_manager, operation);
                        }
                    }
                }
            } else {
                // Rename All: the batch names this item and the ones after
                // it with the pattern (`process_batch_operation`).
                operation.set_rename_pattern(pattern);
                self.process_batch_operation(operation);
            }
        }
        Ok(())
    }
}

/// The name a Rename All pattern gave the current item, which
/// `process_batch_operation` computed with the counter it then advanced;
/// `None` without a pattern.
fn renamed_item_name(operation: &BatchOperation, source: &std::path::Path) -> Option<String> {
    let pattern = operation.rename_pattern.as_ref()?;
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let metadata = source.metadata().ok();
    let created = metadata.as_ref().and_then(|m| m.created().ok());
    let modified = metadata.as_ref().and_then(|m| m.modified().ok());
    let counter = operation.rename_counter.saturating_sub(1);
    Some(pattern.apply(&name, counter, created, modified))
}

/// Where the current item lands locally: under its pattern name when it has
/// one, under its own name otherwise.
fn item_destination(
    operation: &BatchOperation,
    source: &std::path::Path,
    renamed: Option<&str>,
) -> std::path::PathBuf {
    match renamed {
        Some(name) => path_utils::resolve_rename_destination_path(
            &operation.destination,
            name,
            operation.destination_is_directory(),
        ),
        None => path_utils::resolve_batch_destination_path(
            source,
            &operation.destination,
            operation.sources.len() == 1,
            operation.destination_is_directory(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use termide_state::RenamePattern;

    /// Overwriting under Rename All replaces the pattern's name — the one
    /// the conflict was about — not the item's own name.
    #[test]
    fn an_overwrite_under_rename_all_targets_the_pattern_name() {
        let mut operation = BatchOperation::new(
            BatchOperationType::Copy,
            vec![PathBuf::from("/nonexistent/src/a.txt")],
            PathBuf::from("/nonexistent/dst"),
        )
        .with_destination_directory(true);
        let source = Path::new("/nonexistent/src/a.txt");
        assert_eq!(renamed_item_name(&operation, source), None);
        assert_eq!(
            item_destination(&operation, source, None),
            PathBuf::from("/nonexistent/dst/a.txt")
        );

        operation.set_rename_pattern(RenamePattern::new("$1_$I.$-1".to_string()));
        // `process_batch_operation` took counter 1 for this item.
        assert_eq!(operation.get_and_increment_rename_counter(), 1);
        let renamed = renamed_item_name(&operation, source);
        assert_eq!(renamed.as_deref(), Some("a_1.txt"));
        assert_eq!(
            item_destination(&operation, source, renamed.as_deref()),
            PathBuf::from("/nonexistent/dst/a_1.txt")
        );
    }
}
