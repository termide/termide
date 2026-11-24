//! Moving and copying a project: its directory, through the file operations
//! (with their progress in the operations panel), then what termide keeps of
//! it apart from its files — saved layouts, unsaved buffers, agent sessions
//! with their checkpoints, breakpoints, and for a move what the user granted
//! it — carried along to the new path.
//!
//! An open project moving is closed first, so nothing writes into the
//! directory while it moves, and opened again at its new place, where it was
//! in the list. A copy is a project of its own: it opens in the background
//! beside the original, with its agent sessions under new ids and nothing
//! granted.
//!
//! A project whose directory went away outside termide is pointed at where
//! its files went: the same state follows them as after a move, with no
//! file moved.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use termide_file_ops::{OperationId, OperationPath, OperationRequest, OperationResult};
use termide_i18n as i18n;

use super::agent_panel::session_dir_of;
use super::parked_projects::{display_root, open_elsewhere_message, project_root_of};
use super::App;
use crate::state::{ActiveModal, OperationType, PendingAction, ProjectsOrigin};

/// A project moving or being copied, until its file operation ends.
pub(super) struct ProjectTransfer {
    op_id: OperationId,
    from: PathBuf,
    to: PathBuf,
    copy: bool,
    /// Whether it was the current project, emptied for the move.
    was_current: bool,
    /// The place in the list of open projects of a parked project moving,
    /// closed for the move.
    position: Option<usize>,
    /// What held the project and its new path against other instances
    /// while it moves.
    from_claim: Option<termide_project::ProjectClaim>,
    to_claim: Option<termide_project::ProjectClaim>,
    /// Where its agent sessions were filed, taken before it moves away.
    sessions_dir: Option<PathBuf>,
}

impl App {
    /// Ask where to move the project at `root` (or, with `copy`, where to
    /// copy it), then return `from` where it was asked if that is cancelled.
    pub(in crate::app) fn prompt_project_transfer(
        &mut self,
        root: PathBuf,
        copy: bool,
        from: ProjectsOrigin,
    ) {
        if self.in_transfer(&root) {
            self.state
                .set_error(i18n::t().projects_transfer_busy().to_string());
            return;
        }
        let t = i18n::t();
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let (title, prompt) = if copy {
            (t.modal_copy_single_title(&name), String::new())
        } else {
            (t.modal_move_single_title(&name), String::new())
        };
        let base = root.parent().unwrap_or(&root).to_path_buf();
        let default = format!("{}/", base.display().to_string().trim_end_matches('/'));
        let default = format!("{default}{name}");
        let modal = termide_modal::EditableSelectModal::new(
            title,
            prompt,
            default,
            self.destination_roots(),
        )
        .with_keybindings(&self.state.config.file_manager.keybindings)
        .with_base_dir(base)
        .keeping_file_name();
        self.state.set_pending_action(
            PendingAction::TransferProject { root, copy, from },
            ActiveModal::EditableSelect(Box::new(modal)),
        );
    }

    /// The destination prompt was answered with `input`: check the target,
    /// ask before a move closes running panels, and start.
    pub(in crate::app) fn handle_transfer_project_input(
        &mut self,
        root: PathBuf,
        copy: bool,
        input: &str,
        from: ProjectsOrigin,
    ) -> Result<()> {
        let input = input.trim();
        if input.is_empty() {
            return self.return_to_projects(from);
        }
        let base = root.parent().unwrap_or(&root).to_path_buf();
        let (dest, explicit_dir) =
            termide_ui::path_utils::resolve_local_destination_input(&base, input);
        let target = termide_ui::path_utils::resolve_batch_destination_path(
            &root,
            &dest,
            true,
            explicit_dir,
        );
        // Named through its parent with symlinks resolved, as the roots of
        // open projects are, so the state filed under it is found again.
        let target = match (target.parent(), target.file_name()) {
            (Some(parent), Some(name)) => project_root_of(parent.to_path_buf()).join(name),
            _ => target,
        };
        if target == root {
            return Ok(());
        }
        if let Err(message) = self.check_project_transfer(&root, &target, copy) {
            self.show_error_modal(message);
            return Ok(());
        }
        if !copy && self.project_requires_confirmation(&root) {
            let t = i18n::t();
            let message = format!("{}\n{}", display_root(&root), t.projects_close_warning());
            let modal = termide_modal::ConfirmModal::new(t.projects_move_title(), message)
                .defaulting_to_no();
            self.state.set_pending_action(
                PendingAction::ConfirmProjectMove { root, target, from },
                ActiveModal::Confirm(Box::new(modal)),
            );
            return Ok(());
        }
        self.start_project_transfer(root, target, copy)
    }

    /// Why the project at `root` cannot go to `target`, if it cannot.
    fn check_project_transfer(&self, root: &Path, target: &Path, copy: bool) -> Result<(), String> {
        let t = i18n::t();
        if self.in_transfer(root) || self.in_transfer(target) {
            return Err(t.projects_transfer_busy().to_string());
        }
        if target.starts_with(root) {
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            return Err(t.fm_copy_into_itself(&name));
        }
        if target.symlink_metadata().is_ok() {
            return Err(t.projects_transfer_exists_fmt(&display_root(target)));
        }
        if let Some(owner) = termide_project::project_owner(target) {
            return Err(open_elsewhere_message(target, &owner));
        }
        if !copy {
            // The projects open inside it would lose their directories.
            if let Some(nested) = self
                .open_projects
                .roots()
                .find(|other| *other != root && other.starts_with(root))
            {
                return Err(t.projects_move_nested_open_fmt(&display_root(nested)));
            }
        }
        Ok(())
    }

    /// Whether the project at `root` is moving or being copied, or another
    /// is moving into or out of it.
    fn in_transfer(&self, root: &Path) -> bool {
        self.project_transfers.iter().any(|transfer| {
            [&transfer.from, &transfer.to]
                .iter()
                .any(|path| path.starts_with(root) || root.starts_with(path))
        })
    }

    /// Move (or copy) the project at `root` to `target`, checked already:
    /// close it for a move, then hand the directory to the file operations.
    pub(in crate::app) fn start_project_transfer(
        &mut self,
        root: PathBuf,
        target: PathBuf,
        copy: bool,
    ) -> Result<()> {
        // Checked again: a confirmation may have stood meanwhile.
        if let Err(message) = self.check_project_transfer(&root, &target, copy) {
            self.show_error_modal(message);
            return Ok(());
        }
        let to_claim = match termide_project::claim_project(&target, None) {
            Ok(claim) => claim,
            Err(owner) => {
                self.show_error_modal(open_elsewhere_message(&target, &owner));
                return Ok(());
            }
        };
        let sessions_dir = session_dir_of(&root);
        let is_open = self.open_projects.is_open(&root);
        let was_current = !copy && root == self.project_root;
        let (position, from_claim) = if copy {
            if is_open {
                self.save_open_project_layout(&root);
            }
            (None, None)
        } else if is_open {
            self.vacate_project(&root)
        } else {
            (None, None)
        };

        let request = if copy {
            OperationRequest::copy(
                vec![OperationPath::Local(root.clone())],
                OperationPath::Local(target.clone()),
            )
        } else {
            OperationRequest::r#move(
                vec![OperationPath::Local(root.clone())],
                OperationPath::Local(target.clone()),
            )
        };
        let started = self.start_tracked_operation(
            request,
            Arc::new(termide_vfs::VfsManager::new()),
            if copy {
                OperationType::Copy
            } else {
                OperationType::Move
            },
            root.display().to_string(),
            target.display().to_string(),
            1,
            0,
        );
        match started {
            Ok(op_id) => self.project_transfers.push(ProjectTransfer {
                op_id,
                from: root,
                to: target,
                copy,
                was_current,
                position,
                from_claim,
                to_claim,
                sessions_dir,
            }),
            Err(e) => {
                let path = display_root(&target);
                let t = i18n::t();
                drop(to_claim);
                if !copy {
                    self.reopen_moved_project(&root, &root, was_current, position, from_claim);
                }
                self.show_error_modal(if copy {
                    t.projects_copy_failed_fmt(&path, &e.to_string())
                } else {
                    t.projects_move_failed_fmt(&path, &e.to_string())
                });
            }
        }
        Ok(())
    }

    /// Ask where the project at `root`, whose directory is gone, is now:
    /// its files moved outside termide. The picker starts at the nearest
    /// directory of the old path still there; cancelling returns `from`.
    pub(in crate::app) fn prompt_project_relink(&mut self, root: PathBuf, from: ProjectsOrigin) {
        if self.in_transfer(&root) {
            self.state
                .set_error(i18n::t().projects_transfer_busy().to_string());
            return;
        }
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let start = root
            .ancestors()
            .find(|dir| dir.is_dir())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.project_root.clone());
        let t = i18n::t();
        let modal = termide_modal::DirectoryPickerModal::new(
            start,
            t.projects_relink_title_fmt(&name),
            t.directory_picker_select().to_string(),
        )
        .with_keybindings(&self.state.config.file_manager.keybindings);
        self.state.set_pending_action(
            PendingAction::RelinkProject { root, from },
            ActiveModal::DirectoryPicker(Box::new(modal)),
        );
    }

    /// The project at `root` is at `target` now, its files moved there
    /// outside termide: carry what termide keeps of it along, as after a
    /// move. Picking the old path again returns to `from`.
    pub(in crate::app) fn relink_project(
        &mut self,
        root: PathBuf,
        target: PathBuf,
        from: ProjectsOrigin,
    ) -> Result<()> {
        let target = project_root_of(target);
        if target == root {
            return self.return_to_projects(from);
        }
        if let Err(message) = self.check_project_relink(&root, &target) {
            self.show_error_modal(message);
            return Ok(());
        }
        // Held while the state moves, so no instance opens it meanwhile.
        let claim = match termide_project::claim_project(&target, None) {
            Ok(claim) => claim,
            Err(owner) => {
                self.show_error_modal(open_elsewhere_message(&target, &owner));
                return Ok(());
            }
        };
        let sessions_dir = session_dir_of(&root);
        if let Err(e) = relocate_project_data(&root, &target, false, sessions_dir.as_deref()) {
            log::error!("Could not carry the state of {root:?} to {target:?}: {e:#}");
            drop(claim);
            self.show_error_modal(format!("{e:#}"));
            return Ok(());
        }
        relocate_breakpoints(&root, &target);
        repair_git_worktrees(&target);
        drop(claim);
        self.state
            .set_info(i18n::t().project_relinked_fmt(&display_root(&target)));
        Ok(())
    }

    /// Why the project at `root` cannot be pointed at `target`, if it
    /// cannot: a directory with a project of its own would get two.
    fn check_project_relink(&self, root: &Path, target: &Path) -> Result<(), String> {
        let t = i18n::t();
        if self.in_transfer(root) || self.in_transfer(target) {
            return Err(t.projects_transfer_busy().to_string());
        }
        if let Some(owner) = termide_project::project_owner(root) {
            return Err(open_elsewhere_message(root, &owner));
        }
        // Asked before its owner: one open here is held by this instance.
        let has_project = self.open_projects.is_open(target)
            || termide_project::ProjectLayout::get_project_path(target)
                .is_ok_and(|path| path.exists());
        if has_project {
            return Err(t.projects_relink_has_state_fmt(&display_root(target)));
        }
        if let Some(owner) = termide_project::project_owner(target) {
            return Err(open_elsewhere_message(target, &owner));
        }
        Ok(())
    }

    /// The file operation `id` ended with `result`. Returns whether it was a
    /// project's, now handled.
    pub(super) fn complete_project_transfer(
        &mut self,
        id: OperationId,
        result: &OperationResult,
    ) -> bool {
        let Some(index) = self
            .project_transfers
            .iter()
            .position(|transfer| transfer.op_id == id)
        else {
            return false;
        };
        let transfer = self.project_transfers.remove(index);
        self.state.untrack_operation(id);
        let outcome = match result {
            OperationResult::Success | OperationResult::SuccessWithPath(_) => Ok(()),
            // A move whose files all made it, whatever it says.
            _ if !transfer.copy && !transfer.from.exists() && transfer.to.is_dir() => Ok(()),
            OperationResult::Failed(error) => Err(error.clone()),
            OperationResult::PartialSuccess { failed_files, .. } => Err(failed_files.join("; ")),
            OperationResult::Cancelled => Err(i18n::t().agent_run_cancelled().to_string()),
        };
        self.finish_project_transfer(transfer, outcome);
        true
    }

    /// Carry the project's state along after its directory moved or was
    /// copied, and open it where it is now; open it where it was after a
    /// failure, leaving whatever reached the new place there.
    fn finish_project_transfer(&mut self, transfer: ProjectTransfer, outcome: Result<(), String>) {
        let ProjectTransfer {
            from,
            to,
            copy,
            was_current,
            position,
            from_claim,
            to_claim,
            sessions_dir,
            ..
        } = transfer;
        let t = i18n::t();
        if let Err(error) = outcome {
            drop(to_claim);
            if !copy {
                self.reopen_moved_project(&from, &from, was_current, position, from_claim);
            }
            let path = display_root(&to);
            self.show_error_modal(if copy {
                t.projects_copy_failed_fmt(&path, &error)
            } else {
                t.projects_move_failed_fmt(&path, &error)
            });
            return;
        }

        let to = project_root_of(to);
        if !copy {
            // This run's log may be filed with the project.
            if let (Ok(old), Ok(new)) = (
                termide_project::ProjectLayout::get_project_dir(&from),
                termide_project::ProjectLayout::get_project_dir(&to),
            ) {
                termide_logger::relocate_file(&old, &new);
            }
        }
        if let Err(e) = relocate_project_data(&from, &to, copy, sessions_dir.as_deref()) {
            log::error!("Could not carry the state of {from:?} to {to:?}: {e:#}");
            self.state.set_error(format!("{e:#}"));
        }
        if copy {
            // Beside the original, to be entered when wanted.
            if self.open_projects.open_pending(to.clone()) {
                if let Some(place) = self.open_projects.position(&from) {
                    self.open_projects.move_to(&to, place + 1);
                }
            }
            drop(to_claim);
            self.sync_open_projects();
            self.state
                .set_info(t.project_copied_to_fmt(&display_root(&to)));
        } else {
            relocate_breakpoints(&from, &to);
            repair_git_worktrees(&to);
            drop(from_claim);
            self.reopen_moved_project(&from, &to, was_current, position, to_claim);
            self.state
                .set_info(t.project_moved_to_fmt(&display_root(&to)));
        }
        self.state.needs_redraw = true;
    }

    /// Open the project that was at `from` again, at `root` — where it moved,
    /// or `from` itself when the move failed — as it was open before: current
    /// when it was and still is, emptied for the move; otherwise in the
    /// background at its place in the list. One the user closed meanwhile
    /// stays closed.
    fn reopen_moved_project(
        &mut self,
        from: &Path,
        root: &Path,
        was_current: bool,
        position: Option<usize>,
        claim: Option<termide_project::ProjectClaim>,
    ) {
        let mut position = position;
        if was_current {
            if self.project_root == from {
                let operations_panel = self.close_operations_panel();
                self.drop_current_layout();
                self.open_projects.move_current(root.to_path_buf());
                if let Err(e) = std::env::set_current_dir(root) {
                    log::warn!("Could not enter {root:?}: {e}");
                }
                self.set_project_root(root.to_path_buf());
                // Saved from here on, at its new place.
                self.vacated_root = None;
                self.keep_project_claim(root.to_path_buf(), claim);
                self.load_layout_or_default();
                self.carry_operations_panel(operations_panel);
                self.update_terminal_title();
                self.sync_open_projects();
                return;
            }
            // Left for another project meanwhile, it was parked empty.
            position = self.open_projects.position(from);
            self.drop_parked_project(from);
            self.vacated_root = None;
        }
        if let Some(position) = position {
            if self.open_projects.open_pending(root.to_path_buf()) {
                self.open_projects.move_to(root, position);
            }
            self.keep_project_claim(root.to_path_buf(), claim);
        }
        self.sync_open_projects();
    }
}

/// Carry what termide keeps of the project at `from` to `to`: its agent
/// sessions, filed in `sessions_dir`, then the saved layouts naming them.
fn relocate_project_data(
    from: &Path,
    to: &Path,
    copy: bool,
    sessions_dir: Option<&Path>,
) -> Result<()> {
    let mut sessions = HashMap::new();
    if let (Some(from_dir), Some(to_dir)) = (sessions_dir, session_dir_of(to)) {
        if from_dir.exists() {
            sessions = termide_agent_core::relocate::relocate_sessions(
                termide_agent_core::relocate::SessionsRelocation {
                    from_dir,
                    to_dir: &to_dir,
                    from_root: from,
                    to_root: to,
                    copy,
                },
            )?;
        }
    }
    termide_project::relocate_project_state(from, to, copy, &sessions)
}

/// Point the breakpoints set in the files of the project that moved from
/// `from` at their new paths under `to`.
fn relocate_breakpoints(from: &Path, to: &Path) {
    let mut store = termide_debug::breakpoints::store();
    let mut moved: HashMap<PathBuf, Vec<usize>> = HashMap::new();
    for breakpoint in store.all() {
        if breakpoint.path.starts_with(from) {
            moved
                .entry(breakpoint.path)
                .or_default()
                .push(breakpoint.line - 1);
        }
    }
    if moved.is_empty() {
        return;
    }
    for (path, lines) in moved {
        if let Ok(rel) = path.strip_prefix(from) {
            store.set_lines(&to.join(rel), lines);
            store.set_lines(&path, std::iter::empty());
        }
    }
    drop(store);
    termide_debug::breakpoints::flush();
}

/// Mend the links between a repository moved to `root` and its worktrees,
/// which name each other by absolute path. Runs on a worker thread: it
/// touches only git's files, not the state the project claim guards, and its
/// outcome is only logged.
fn repair_git_worktrees(root: &Path) {
    if !root.join(".git").exists() {
        return;
    }
    let root = root.to_path_buf();
    std::thread::spawn(move || run_git_worktree_repair(&root));
}

fn run_git_worktree_repair(root: &Path) {
    match std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["worktree", "repair"])
        .env("PATH", termide_fsutil::absolute_path_var())
        .stdin(std::process::Stdio::null())
        .output()
    {
        Ok(output) if !output.status.success() => log::warn!(
            "git worktree repair in {root:?}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(e) => log::warn!("Could not run git worktree repair in {root:?}: {e}"),
        Ok(_) => {}
    }
}
