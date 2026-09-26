//! Path-related modal result handlers: goto-path, follow-symlink, chmod/permissions,
//! and batch-copy symlink flag live-sync.

use std::path::Path;

use anyhow::Result;

use crate::app::App;
use crate::panel_ext::PanelExt;
use crate::state::ActiveModal;

impl App {
    /// Sync copy modal checkbox state into `PendingAction::CopyPath`.
    pub(in crate::app) fn sync_copy_symlink_flag(&mut self) {
        if !matches!(
            self.state.pending_action,
            Some(termide_state::PendingAction::CopyPath { .. })
        ) {
            return;
        }
        let (create_symlink_checked, create_relative_checked) = match &self.state.active_modal {
            Some(ActiveModal::Input(m)) => {
                (m.is_checkbox_checked(), m.is_secondary_checkbox_checked())
            }
            Some(ActiveModal::EditableSelect(m)) => {
                (m.is_checkbox_checked(), m.is_secondary_checkbox_checked())
            }
            _ => (false, false),
        };
        if let Some(termide_state::PendingAction::CopyPath {
            create_symlink,
            create_relative_symlink,
            ..
        }) = &mut self.state.pending_action
        {
            *create_symlink = create_symlink_checked;
            *create_relative_symlink = create_symlink_checked && create_relative_checked;
        }
    }

    /// Apply permissions immediately if a toggle just happened in the modal
    pub(in crate::app) fn try_apply_permissions_live(&mut self) {
        let mode = if let Some(ActiveModal::InfoAction(modal)) = &mut self.state.active_modal {
            modal.take_pending_permission_change()
        } else {
            return;
        };
        let Some(mode) = mode else { return };

        use termide_state::PendingAction;
        let file_path = match &self.state.pending_action {
            Some(PendingAction::ChangePermissions { file_path }) => file_path.clone(),
            Some(PendingAction::GitFileAction {
                file_path,
                repo_path,
                ..
            }) => repo_path.join(file_path),
            Some(PendingAction::FollowSymlink { target_path }) => target_path.clone(),
            _ => return,
        };

        Self::apply_permissions(&file_path, mode);
    }

    /// Handle conflict resolution from ConflictModal for OperationManager operations.
    pub(in crate::app) fn handle_resolve_operation_conflict(
        &mut self,
        operation_id: termide_file_ops::OperationId,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        use termide_file_ops::ConflictResolution as FileOpsResolution;
        use termide_modal::ConflictResolution as ModalResolution;

        if let Some(modal_resolution) = value.downcast_ref::<ModalResolution>() {
            // Convert modal resolution to file-ops resolution
            let file_ops_resolution = match modal_resolution {
                ModalResolution::Overwrite => FileOpsResolution::Overwrite,
                ModalResolution::Skip => FileOpsResolution::Skip,
                ModalResolution::OverwriteAll => FileOpsResolution::OverwriteAll,
                ModalResolution::SkipAll => FileOpsResolution::SkipAll,
                ModalResolution::Rename | ModalResolution::RenameAll => {
                    // Rename is not supported by OperationManager yet, treat as Skip
                    log::warn!("Rename resolution not yet supported, skipping file");
                    FileOpsResolution::Skip
                }
                ModalResolution::Cancel => FileOpsResolution::Cancel,
            };

            // Send resolution to the operation
            if !self
                .state
                .resolve_operation_conflict(operation_id, file_ops_resolution)
            {
                log::error!(
                    "Failed to send conflict resolution for operation {}",
                    operation_id
                );
            }
        }
        Ok(())
    }

    /// Handle the file manager's "go to path" result. A URL (`sftp://…`)
    /// goes to the VFS as typed; a local path is taken from the panel's
    /// directory, and a file is shown selected in its directory.
    pub(in crate::app) fn handle_goto_path(
        &mut self,
        current_directory: &Path,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        let Some(input) = value.downcast_ref::<String>() else {
            return Ok(());
        };
        let input = input.trim();
        if input.is_empty() {
            return Ok(());
        }
        let Some(fm) = self
            .layout_manager
            .active_panel_mut()
            .and_then(|panel| panel.as_file_manager_mut())
        else {
            return Ok(());
        };
        // Navigation errors are not reported; the panel stays where it was.
        if input.contains("://") {
            let _ = fm.navigate_to_url(input);
        } else {
            let path = super::path_suggestions::resolve_typed_path(input, current_directory);
            if path.is_file() {
                fm.navigate_to_file(&path);
            } else {
                let _ = fm.navigate_to(path);
            }
        }
        self.state.needs_watcher_registration = true;
        Ok(())
    }

    /// Apply Unix permissions to a file
    pub(in crate::app) fn apply_permissions(file_path: &std::path::Path, mode: u32) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = std::fs::metadata(file_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(mode);
                if let Err(e) = std::fs::set_permissions(file_path, perms) {
                    log::error!("Failed to set permissions on {:?}: {}", file_path, e);
                } else {
                    log::info!("Set permissions {:04o} on {:?}", mode, file_path);
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (file_path, mode);
        }
    }

    /// Handle follow symlink action from InfoActionModal
    pub(in crate::app) fn handle_follow_symlink(
        &mut self,
        value: Box<dyn std::any::Any>,
        target_path: &std::path::Path,
    ) -> Result<()> {
        use termide_modal::InfoActionResult;

        if let Some(InfoActionResult::Action(action)) = value.downcast_ref::<InfoActionResult>() {
            if action == "follow" {
                if target_path.is_dir() {
                    if let Some(panel) = self.layout_manager.active_panel_mut() {
                        if let Some(fm) = panel.as_file_manager_mut() {
                            let _ = fm.navigate_to_url(&target_path.display().to_string());
                            self.state.needs_watcher_registration = true;
                        }
                    }
                } else if target_path.is_file() {
                    let _ = self.open_editor_for_file(target_path.to_path_buf());
                }
            }
        }
        Ok(())
    }
}
