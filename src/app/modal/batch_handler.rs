use anyhow::Result;
use std::path::PathBuf;

use super::super::App;
use crate::{
    i18n,
    panels::PanelExt,
    path_utils,
    state::{ActiveModal, BatchOperation, BatchOperationType, ConflictMode, PendingAction},
    ui::modal::ConflictModal,
};

impl App {
    /// Common method for handling file operations (Copy/Move)
    fn handle_file_operation(
        &mut self,
        operation_type: BatchOperationType,
        panel_index: usize,
        sources: Vec<PathBuf>,
        target_directory: Option<PathBuf>,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        // Determine absolute destination path
        let absolute_destination = if let Some(target_dir) = target_directory {
            // target_directory already set
            // First check bool (for Ctrl+V with ConfirmModal)
            if let Some(confirmed) = value.downcast_ref::<bool>() {
                if !confirmed {
                    return Ok(()); // Operation cancelled by user
                }
                target_dir
            // Then check String (for F5/C/M with InputModal)
            } else if let Some(destination_str) = value.downcast_ref::<String>() {
                let destination = PathBuf::from(destination_str);
                // If path is relative, use target_dir as base
                if destination.is_absolute() {
                    destination
                } else {
                    target_dir.join(&destination)
                }
            } else {
                return Ok(()); // Invalid response type
            }
        } else {
            // target_directory not set, get string from InputModal
            if let Some(destination_str) = value.downcast_ref::<String>() {
                // Get FileManager panel to determine base path
                if let Some(fm_panel) = self.get_first_file_manager_mut() {
                    if let Some(fm) = fm_panel.as_file_manager_mut() {
                        let destination = PathBuf::from(destination_str);

                        // If path is relative, make it absolute
                        if destination.is_absolute() {
                            destination
                        } else {
                            fm.get_current_directory().join(&destination)
                        }
                    } else {
                        crate::logger::error(format!("Panel {} is not FileManager", panel_index));
                        return Ok(());
                    }
                } else {
                    crate::logger::error(format!("Panel with index {} not found", panel_index));
                    return Ok(());
                }
            } else {
                return Ok(()); // Invalid response type
            }
        };

        // Create and start batch operation
        let batch_op = BatchOperation::new(operation_type, sources.clone(), absolute_destination);

        self.process_batch_operation(batch_op);
        Ok(())
    }

    /// Handle file copying
    pub(in crate::app) fn handle_copy_path(
        &mut self,
        panel_index: usize,
        sources: Vec<PathBuf>,
        target_directory: Option<PathBuf>,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        self.handle_file_operation(
            BatchOperationType::Copy,
            panel_index,
            sources,
            target_directory,
            value,
        )
    }

    /// Handle file moving
    pub(in crate::app) fn handle_move_path(
        &mut self,
        panel_index: usize,
        sources: Vec<PathBuf>,
        target_directory: Option<PathBuf>,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        self.handle_file_operation(
            BatchOperationType::Move,
            panel_index,
            sources,
            target_directory,
            value,
        )
    }

    /// Handle continuation of batch operation after conflict resolution
    pub(in crate::app) fn handle_continue_batch_operation(
        &mut self,
        mut operation: BatchOperation,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        use crate::ui::modal::ConflictResolution;

        if let Some(resolution) = value.downcast_ref::<ConflictResolution>() {
            match resolution {
                ConflictResolution::Overwrite => {
                    // Overwrite this file - execute operation directly
                    if let Some(source) = operation.current_source().cloned() {
                        let item_name = source
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("?")
                            .to_string();

                        let final_dest = path_utils::resolve_batch_destination_path(
                            &source,
                            &operation.destination,
                            operation.sources.len() == 1,
                        );

                        // Execute operation
                        if let Some(fm_panel) = self.get_first_file_manager_mut() {
                            if let Some(fm) = fm_panel.as_file_manager_mut() {
                                let result = match operation.operation_type {
                                    BatchOperationType::Copy => {
                                        fm.copy_path(source.clone(), final_dest.clone())
                                    }
                                    BatchOperationType::Move => {
                                        fm.move_path(source.clone(), final_dest.clone())
                                    }
                                };

                                match result {
                                    Ok(_) => {
                                        let action_name = match operation.operation_type {
                                            BatchOperationType::Copy => "copied",
                                            BatchOperationType::Move => "moved",
                                        };
                                        crate::logger::info(format!(
                                            "File {}: {}",
                                            action_name, item_name
                                        ));
                                        operation.increment_success();
                                    }
                                    Err(e) => {
                                        let action_name = match operation.operation_type {
                                            BatchOperationType::Copy => "copy",
                                            BatchOperationType::Move => "move",
                                        };
                                        crate::logger::error(format!(
                                            "Failed to {} '{}': {}",
                                            action_name, item_name, e
                                        ));
                                        operation.increment_error();
                                    }
                                }
                            }
                        }
                    }
                    // Move to next file
                    operation.advance();
                    self.process_batch_operation(operation.clone());
                }
                ConflictResolution::Skip => {
                    // Skip this file
                    operation.increment_skipped();
                    operation.advance();
                    self.process_batch_operation(operation.clone());
                }
                ConflictResolution::OverwriteAll => {
                    // Set "overwrite all" mode
                    operation.set_conflict_mode(ConflictMode::OverwriteAll);
                    self.process_batch_operation(operation.clone());
                }
                ConflictResolution::SkipAll => {
                    // Set "skip all" mode
                    operation.set_conflict_mode(ConflictMode::SkipAll);
                    operation.increment_skipped();
                    operation.advance();
                    self.process_batch_operation(operation.clone());
                }
                ConflictResolution::Rename => {
                    // Request rename pattern for single file
                    if let Some(source) = operation.current_source() {
                        let original_name = source
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("?")
                            .to_string();

                        // Get file metadata for preview
                        let metadata = source.metadata().ok();
                        let created = metadata.as_ref().and_then(|m| m.created().ok());
                        let modified = metadata.as_ref().and_then(|m| m.modified().ok());

                        use crate::state::{ActiveModal, PendingAction};
                        use crate::ui::modal::RenamePatternModal;

                        let modal = RenamePatternModal::new(
                            "Rename file",
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
                        let original_name = source
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("?")
                            .to_string();

                        // Get file metadata for preview
                        let metadata = source.metadata().ok();
                        let created = metadata.as_ref().and_then(|m| m.created().ok());
                        let modified = metadata.as_ref().and_then(|m| m.modified().ok());

                        use crate::state::{ActiveModal, PendingAction};
                        use crate::ui::modal::RenamePatternModal;

                        let modal = RenamePatternModal::new(
                            "Rename all conflicting files",
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
            }
        }
        Ok(())
    }

    /// Handle batch file operation (copy/move)
    pub(in crate::app) fn process_batch_operation(&mut self, mut operation: BatchOperation) {
        // Check if operation is complete
        if operation.is_complete() {
            // Show final results
            self.show_batch_results(&operation);

            // Clear selection and refresh panel
            if let Some(fm_panel) = self.get_first_file_manager_mut() {
                if let Some(fm) = fm_panel.as_file_manager_mut() {
                    if operation.success_count > 0 {
                        fm.clear_selection();
                    }
                    let _ = fm.load_directory();

                    // Refresh all FM panels showing target directory
                    self.refresh_fm_panels(&operation.destination);

                    // For move - refresh source directories
                    if operation.operation_type == BatchOperationType::Move
                        && !operation.sources.is_empty()
                    {
                        if let Some(parent) = operation.sources[0].parent() {
                            self.refresh_fm_panels(parent);
                        }
                    }
                }
            }
            return;
        }

        // Get current file
        let Some(source) = operation.current_source().cloned() else {
            return;
        };

        let item_name = source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();

        // Determine target path (considering rename pattern if set)
        let final_dest = if operation.rename_pattern.is_some() {
            // Apply rename pattern
            let counter = operation.get_and_increment_rename_counter();
            let metadata = source.metadata().ok();
            let created = metadata.as_ref().and_then(|m| m.created().ok());
            let modified = metadata.as_ref().and_then(|m| m.modified().ok());

            let pattern = operation.rename_pattern.as_ref().unwrap();
            let new_name = pattern.apply(&item_name, counter, created, modified);

            path_utils::resolve_rename_destination_path(&operation.destination, &new_name)
        } else {
            // Standard logic without renaming
            path_utils::resolve_batch_destination_path(
                &source,
                &operation.destination,
                operation.sources.len() == 1,
            )
        };

        // Check conflict
        if final_dest.exists() {
            match operation.conflict_mode {
                ConflictMode::Ask => {
                    // Show conflict resolution modal window
                    let remaining_items = operation
                        .sources
                        .len()
                        .saturating_sub(operation.current_index + 1);
                    let modal = ConflictModal::new(&source, &final_dest, remaining_items);
                    self.state.pending_action =
                        Some(PendingAction::ContinueBatchOperation { operation });
                    self.state.active_modal = Some(ActiveModal::Conflict(Box::new(modal)));
                    return;
                }
                ConflictMode::SkipAll => {
                    // Skip file
                    crate::logger::info(format!("'{}' пропущен (файл существует)", item_name));
                    operation.increment_skipped();
                    operation.advance();
                    self.process_batch_operation(operation);
                    return;
                }
                ConflictMode::OverwriteAll => {
                    // Continue with overwrite (processing below)
                }
            }
        }

        // Execute operation
        if let Some(fm_panel) = self.get_first_file_manager_mut() {
            if let Some(fm) = fm_panel.as_file_manager_mut() {
                let result = match operation.operation_type {
                    BatchOperationType::Copy => fm.copy_path(source.clone(), final_dest.clone()),
                    BatchOperationType::Move => fm.move_path(source.clone(), final_dest.clone()),
                };

                match result {
                    Ok(_) => {
                        let t = i18n::t();
                        let action_name = match operation.operation_type {
                            BatchOperationType::Copy => t.action_copied(),
                            BatchOperationType::Move => t.action_moved(),
                        };
                        crate::logger::info(format!("'{}' {}", item_name, action_name));
                        operation.increment_success();
                    }
                    Err(e) => {
                        let t = i18n::t();
                        let action_name = match operation.operation_type {
                            BatchOperationType::Copy => t.action_copying(),
                            BatchOperationType::Move => t.action_moving(),
                        };
                        crate::logger::error(format!(
                            "Ошибка {} '{}': {}",
                            action_name, item_name, e
                        ));
                        operation.increment_error();
                    }
                }
            }
        }

        // Move to next file
        operation.advance();
        self.process_batch_operation(operation);
    }

    /// Show batch operation final results
    pub(in crate::app) fn show_batch_results(&mut self, operation: &BatchOperation) {
        let total = operation.total_count();
        let success = operation.success_count;
        let errors = operation.error_count;
        let skipped = operation.skipped_count;
        let t = i18n::t();

        let action_name = match operation.operation_type {
            BatchOperationType::Copy => (t.batch_result_file_copied(), t.batch_result_copied()),
            BatchOperationType::Move => (t.batch_result_file_moved(), t.batch_result_moved()),
        };

        if total == 1 {
            if success == 1 {
                self.state.set_info(format!("Файл {}", action_name.0));
            } else {
                let error_msg = match operation.operation_type {
                    BatchOperationType::Copy => t.batch_result_error_copy(),
                    BatchOperationType::Move => t.batch_result_error_move(),
                };
                self.state.set_error(error_msg.to_string());
            }
        } else {
            let mut parts = vec![];
            if success > 0 {
                parts.push(format!("{}: {}", action_name.1, success));
            }
            if skipped > 0 {
                parts.push(t.batch_result_skipped_fmt(skipped));
            }
            if errors > 0 {
                parts.push(t.batch_result_errors_fmt(errors));
            }

            self.state.set_info(parts.join(", "));
        }
    }

    /// Handle rename pattern input result
    pub(in crate::app) fn handle_rename_with_pattern(
        &mut self,
        mut operation: BatchOperation,
        original_name: String,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(pattern_str) = value.downcast_ref::<String>() {
            use crate::rename_pattern::RenamePattern;

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

                    // Create new destination path with new name
                    let new_dest = path_utils::resolve_rename_destination_path(
                        &operation.destination,
                        &new_name,
                    );

                    // Check that new path doesn't conflict
                    if new_dest.exists() {
                        // Show ConflictModal again
                        use crate::state::{ActiveModal, PendingAction};
                        use crate::ui::modal::ConflictModal;

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

                    // Execute operation with new name
                    operation.destination = new_dest;
                    self.process_batch_operation(operation);
                }
            } else {
                // This is RenameAll - pattern already set in operation,
                // just continue processing
                operation.set_rename_pattern(pattern);
                self.process_batch_operation(operation);
            }
        }
        Ok(())
    }
}
