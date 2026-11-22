//! Path-related modal result handlers: goto-path, follow-symlink, chmod/permissions,
//! batch-copy symlink flag live-sync, and conflicts a running copy worker raises.

use std::path::{Path, PathBuf};

use anyhow::Result;
use termide_state::{PendingAction, WorkerConflict};
use termide_ui::path_utils;

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

    /// Ask about a conflict a running worker raised: about the worker's
    /// destination, or about the name picked for the item when `renamed`
    /// holds one that is taken too.
    pub(in crate::app) fn ask_worker_conflict(
        &mut self,
        conflict: WorkerConflict,
        renamed: Option<String>,
    ) {
        let shown = match &renamed {
            Some(name) => renamed_destination(&conflict.destination, name),
            None => conflict.destination.clone(),
        };
        let modal =
            termide_modal::ConflictModal::new(&conflict.source, &shown, conflict.remaining_items);
        self.state.set_pending_action(
            PendingAction::ResolveOperationConflict { conflict, renamed },
            ActiveModal::Conflict(Box::new(modal)),
        );
        self.state.needs_redraw = true;
    }

    /// Handle the conflict dialog's answer for a running worker. Rename asks
    /// for the name first; every other answer goes to the worker now.
    pub(in crate::app) fn handle_resolve_operation_conflict(
        &mut self,
        conflict: WorkerConflict,
        renamed: Option<String>,
        value: termide_modal::ModalOutput,
    ) {
        // The worker waits for some answer: one the dialog did not give
        // ends the operation.
        let resolution = value
            .get::<termide_modal::ConflictResolution>()
            .copied()
            .unwrap_or(termide_modal::ConflictResolution::Cancel);
        match worker_resolution(&resolution, renamed.as_deref()) {
            Some(answer) => self.answer_worker_conflict(&conflict, answer),
            None => self.ask_worker_rename(conflict),
        }
    }

    /// Ask for the name of a worker conflict's item, the way the batch flow
    /// asks before a copy starts: the rename pattern prompt, on `$0`.
    fn ask_worker_rename(&mut self, conflict: WorkerConflict) {
        let original_name = path_utils::get_file_name_string(&conflict.source);
        let (created, modified) = item_times(&conflict.source);
        let modal = termide_modal::RenamePatternModal::new(
            &format!("Rename {}", original_name),
            &original_name,
            "$0",
            created,
            modified,
        );
        self.state.set_pending_action(
            PendingAction::RenameOperationConflict { conflict },
            ActiveModal::RenamePattern(Box::new(modal)),
        );
        self.state.needs_redraw = true;
    }

    /// Handle the name typed for a worker conflict's item: a free name goes
    /// to the worker, a taken one is asked about, as the batch flow does.
    /// Esc on the prompt goes back to the conflict dialog (`handle_modal_key`).
    pub(in crate::app) fn handle_rename_operation_conflict(
        &mut self,
        conflict: WorkerConflict,
        value: termide_modal::ModalOutput,
    ) {
        let Some(pattern) = value.get::<String>() else {
            self.ask_worker_conflict(conflict, None);
            return;
        };
        let name = picked_name(pattern, &conflict.source);
        match rename_target(&conflict.destination, &name) {
            RenameTarget::Free => {
                self.answer_worker_conflict(
                    &conflict,
                    termide_file_ops::ConflictResolution::Rename(name),
                );
            }
            RenameTarget::Taken => self.ask_worker_conflict(conflict, Some(name)),
            RenameTarget::Invalid => self.ask_worker_rename(conflict),
        }
    }

    fn answer_worker_conflict(
        &mut self,
        conflict: &WorkerConflict,
        answer: termide_file_ops::ConflictResolution,
    ) {
        if !self
            .state
            .resolve_operation_conflict(conflict.operation_id, answer)
        {
            log::error!(
                "Failed to send conflict resolution for operation {}",
                conflict.operation_id
            );
        }
    }

    /// Handle the file manager's "go to path" result. A URL (`sftp://…`)
    /// goes to the VFS as typed; a local path is taken from the panel's
    /// directory, and a file is shown selected in its directory.
    pub(in crate::app) fn handle_goto_path(
        &mut self,
        current_directory: &Path,
        value: termide_modal::ModalOutput,
    ) -> Result<()> {
        let Some(input) = value.get::<String>() else {
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
        value: termide_modal::ModalOutput,
        target_path: &std::path::Path,
    ) -> Result<()> {
        use termide_modal::InfoActionResult;

        if let Some(InfoActionResult::Action(action)) = value.get::<InfoActionResult>() {
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

/// The answer to a conflict a copy worker raised, as the worker takes it;
/// `None` for Rename, whose name is asked for first. With `renamed`, the
/// dialog was about that name, picked for the item and taken too:
/// overwriting replaces the file under it.
fn worker_resolution(
    resolution: &termide_modal::ConflictResolution,
    renamed: Option<&str>,
) -> Option<termide_file_ops::ConflictResolution> {
    use termide_file_ops::ConflictResolution as Worker;
    use termide_modal::ConflictResolution as Modal;
    Some(match (resolution, renamed) {
        // The worker has no "overwrite all" for a name of the user's: this
        // item goes under it, later conflicts are still asked about.
        (Modal::Overwrite | Modal::OverwriteAll, Some(name)) => Worker::Rename(name.to_string()),
        (Modal::Overwrite, None) => Worker::Overwrite,
        (Modal::OverwriteAll, None) => Worker::OverwriteAll,
        (Modal::Skip, _) => Worker::Skip,
        (Modal::SkipAll, _) => Worker::SkipAll,
        // The worker names this and every later conflict "name (N)".
        (Modal::RenameAll, _) => Worker::RenameAll,
        (Modal::Rename, _) => return None,
        (Modal::Cancel, _) => Worker::Cancel,
    })
}

/// The name a rename pattern gives the item at `source`, counted as the
/// first, as the prompt's preview shows it.
fn picked_name(pattern: &str, source: &Path) -> String {
    let original_name = path_utils::get_file_name_string(source);
    let (created, modified) = item_times(source);
    termide_state::RenamePattern::new(pattern.to_string()).apply(
        &original_name,
        1,
        created,
        modified,
    )
}

fn item_times(path: &Path) -> (Option<std::time::SystemTime>, Option<std::time::SystemTime>) {
    let metadata = path.metadata().ok();
    (
        metadata.as_ref().and_then(|m| m.created().ok()),
        metadata.as_ref().and_then(|m| m.modified().ok()),
    )
}

/// Where the item of a worker conflict lands under `name`: beside the taken
/// destination, as the worker puts it.
fn renamed_destination(destination: &Path, name: &str) -> PathBuf {
    destination.parent().unwrap_or(Path::new("")).join(name)
}

/// What a name picked for a worker conflict's item amounts to.
#[derive(Debug, PartialEq, Eq)]
enum RenameTarget {
    /// Nothing is there: the worker can write it.
    Free,
    /// Something is there, a dangling link too: asked about first, since
    /// the worker would replace it unasked.
    Taken,
    /// Not a file name: asked for again.
    Invalid,
}

fn rename_target(destination: &Path, name: &str) -> RenameTarget {
    let pattern = termide_state::RenamePattern::new(String::new());
    if !pattern.is_valid_result(name) || name == "." || name == ".." {
        return RenameTarget::Invalid;
    }
    if std::fs::symlink_metadata(renamed_destination(destination, name)).is_ok() {
        RenameTarget::Taken
    } else {
        RenameTarget::Free
    }
}

#[cfg(test)]
mod tests {
    use super::{picked_name, rename_target, worker_resolution, RenameTarget};
    use termide_file_ops::ConflictResolution as Worker;
    use termide_modal::ConflictResolution as Modal;

    /// Rename All reaches the worker as Rename All, not as Skip.
    #[test]
    fn rename_all_reaches_the_worker() {
        assert!(matches!(
            worker_resolution(&Modal::RenameAll, None),
            Some(Worker::RenameAll)
        ));
        assert!(matches!(
            worker_resolution(&Modal::Cancel, None),
            Some(Worker::Cancel)
        ));
    }

    /// Plain Rename is not answered as Skip: the name is asked for first.
    #[test]
    fn rename_asks_for_a_name_instead_of_skipping() {
        assert!(worker_resolution(&Modal::Rename, None).is_none());
        assert!(worker_resolution(&Modal::Rename, Some("b.txt")).is_none());
    }

    /// Overwriting a picked name that is taken writes the item under that
    /// name, not over the file the worker first asked about.
    #[test]
    fn overwriting_a_taken_picked_name_renames_onto_it() {
        for answer in [Modal::Overwrite, Modal::OverwriteAll] {
            assert!(matches!(
                worker_resolution(&answer, Some("b.txt")),
                Some(Worker::Rename(name)) if name == "b.txt"
            ));
        }
        assert!(matches!(
            worker_resolution(&Modal::Overwrite, None),
            Some(Worker::Overwrite)
        ));
        assert!(matches!(
            worker_resolution(&Modal::Skip, Some("b.txt")),
            Some(Worker::Skip)
        ));
    }

    /// The pattern names the item as the prompt's preview does.
    #[test]
    fn the_pattern_names_the_item() {
        let source = std::path::Path::new("/nonexistent/src/a.txt");
        assert_eq!(picked_name("$0", source), "a.txt");
        assert_eq!(picked_name("$1_new.$-1", source), "a_new.txt");
    }

    /// A free name goes to the worker; a taken one — the item's own name
    /// as the prompt starts with, or a dangling link — is asked about; a
    /// name with a separator is asked for again.
    #[test]
    fn a_picked_name_is_checked_beside_the_destination() {
        let tmp = tempfile::TempDir::new().unwrap();
        let destination = tmp.path().join("a.txt");
        std::fs::write(&destination, "old").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path().join("gone"), tmp.path().join("link")).unwrap();

        assert_eq!(rename_target(&destination, "c.txt"), RenameTarget::Free);
        assert_eq!(rename_target(&destination, "a.txt"), RenameTarget::Taken);
        #[cfg(unix)]
        assert_eq!(rename_target(&destination, "link"), RenameTarget::Taken);
        assert_eq!(rename_target(&destination, "x/y"), RenameTarget::Invalid);
        assert_eq!(rename_target(&destination, ""), RenameTarget::Invalid);
        assert_eq!(rename_target(&destination, ".."), RenameTarget::Invalid);
    }
}
