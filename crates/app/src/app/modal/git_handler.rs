//! Git-related modal result handlers (stash, commit, revert, push/pull, file actions).

use anyhow::Result;

use crate::app::App;
use crate::state::{ActiveModal, GitJob, GitJobKind, GitJobOutput};
use termide_core::PanelEvent;
use termide_modal::ModalResult;

/// The message for a git job's failure, git's own message `err` included.
fn git_job_failure(kind: &GitJobKind, err: &str) -> String {
    let t = termide_i18n::t();
    match kind {
        GitJobKind::Commit => t.git_commit_failed_fmt(err),
        GitJobKind::StashPush
        | GitJobKind::StashPop(_)
        | GitJobKind::StashApply(_)
        | GitJobKind::StashDrop(_)
        | GitJobKind::StashRename
        | GitJobKind::StashList { .. }
        | GitJobKind::StashInfo { .. } => t.git_stash_failed_fmt(err),
        GitJobKind::Stage => t.editor_stage_failed_fmt(err),
        GitJobKind::Unstage => t.editor_unstage_failed_fmt(err),
        GitJobKind::Revert | GitJobKind::RevertAll => t.git_revert_failed_fmt(err),
    }
}

/// How a finished git job is reported.
#[derive(Debug, PartialEq)]
struct GitJobReport {
    /// The notice on the status bar.
    info: Option<String>,
    /// The error window's message.
    error: Option<String>,
    /// Whether the git panels on its repository are refreshed.
    refresh: bool,
}

fn git_job_report(kind: &GitJobKind, result: &Result<GitJobOutput, String>) -> GitJobReport {
    let t = termide_i18n::t();
    let output = match result {
        Ok(output) => output,
        Err(error) => {
            return GitJobReport {
                info: None,
                error: Some(error.clone()),
                // A failed commit may still have changed the index (a hook
                // that formats and stages).
                refresh: *kind == GitJobKind::Commit,
            };
        }
    };
    let stash = |index: &usize| format!("stash@{{{index}}}");
    let info = match (kind, output) {
        (GitJobKind::Commit, GitJobOutput::Committed(hash)) => t.git_committed_fmt(hash),
        (GitJobKind::StashPush, _) => t.git_stash_created_msg().to_string(),
        (GitJobKind::StashPop(index), _) => t.git_stash_popped_fmt(&stash(index)),
        (GitJobKind::StashApply(index), _) => t.git_stash_applied_fmt(&stash(index)),
        (GitJobKind::StashDrop(index), _) => t.git_stash_dropped_fmt(&stash(index)),
        (GitJobKind::StashRename, _) => t.git_stash_renamed().to_string(),
        (GitJobKind::Stage, _) => t.git_file_staged().to_string(),
        (GitJobKind::Unstage, _) => t.git_file_unstaged().to_string(),
        (GitJobKind::Revert, _) => t.git_file_reverted().to_string(),
        (GitJobKind::RevertAll, _) => t.git_refreshed().to_string(),
        (GitJobKind::Commit | GitJobKind::StashList { .. } | GitJobKind::StashInfo { .. }, _) => {
            return GitJobReport {
                info: None,
                error: None,
                refresh: *kind == GitJobKind::Commit,
            }
        }
    };
    GitJobReport {
        info: Some(info),
        error: None,
        // Staging only writes the index, whose watcher refreshes the panels.
        refresh: !matches!(kind, GitJobKind::Stage | GitJobKind::Unstage),
    }
}

impl App {
    pub(in crate::app) fn handle_git_stash_drop(
        &mut self,
        repo_path: std::path::PathBuf,
        index: usize,
        value: termide_modal::ModalOutput,
    ) -> Result<()> {
        if let Some(&confirmed) = value.get::<bool>() {
            if confirmed {
                self.spawn_git_command(&repo_path, GitJobKind::StashDrop(index), move |repo| {
                    termide_git::stash_drop(repo, index)
                });
            }
        }
        Ok(())
    }

    /// Run a git command that answers with nothing but its failure on a
    /// worker thread, its failure worded for `kind`.
    pub(in crate::app) fn spawn_git_command(
        &mut self,
        repo_path: &std::path::Path,
        kind: GitJobKind,
        command: impl FnOnce(&std::path::Path) -> Result<(), String> + Send + 'static,
    ) {
        let failure_kind = kind.clone();
        self.spawn_git_job(repo_path, kind, move |repo| {
            command(repo)
                .map(|()| GitJobOutput::Done)
                .map_err(|e| git_job_failure(&failure_kind, &e))
        });
    }

    /// Run a git command on a worker thread, reported by `check_git_jobs`
    /// when it ends. `work` answers with the message for its failure, worded.
    pub(in crate::app) fn spawn_git_job(
        &mut self,
        repo_path: &std::path::Path,
        kind: GitJobKind,
        work: impl FnOnce(&std::path::Path) -> Result<GitJobOutput, String> + Send + 'static,
    ) {
        let (tx, receiver) = std::sync::mpsc::channel();
        let repo = repo_path.to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(work(&repo));
        });
        self.state.git_jobs.push(GitJob {
            repo_path: repo_path.to_path_buf(),
            kind,
            receiver,
        });
    }

    /// Handle git stash action from InfoActionModal.
    /// Actions: "pop", "apply", "drop", "diff"
    pub(in crate::app) fn handle_git_stash_action(
        &mut self,
        repo_path: std::path::PathBuf,
        index: usize,
        ref_str: String,
        value: termide_modal::ModalOutput,
    ) -> Result<()> {
        use termide_modal::InfoActionResult;
        let Some(result) = value.get::<InfoActionResult>() else {
            return Ok(());
        };
        let action_id = match result {
            InfoActionResult::Action(id) => id.as_str(),
            _ => return Ok(()),
        };

        match action_id {
            "pop" => {
                self.spawn_git_command(&repo_path, GitJobKind::StashPop(index), move |repo| {
                    termide_git::stash_pop(repo, index)
                });
            }
            "apply" => {
                self.spawn_git_command(&repo_path, GitJobKind::StashApply(index), move |repo| {
                    termide_git::stash_apply(repo, index)
                });
            }
            "drop" => {
                let message = self
                    .state
                    .stash
                    .entries
                    .get(index)
                    .map(|e| e.message.as_str())
                    .unwrap_or("?");
                let t = termide_i18n::t();
                let modal = termide_modal::ConfirmModal::new(t.stash_drop(), message);
                self.state.set_pending_action(
                    termide_state::PendingAction::GitStashDrop { repo_path, index },
                    termide_modal::ActiveModal::Confirm(Box::new(modal)),
                );
            }
            "diff" => {
                use termide_panel_git_diff::GitDiffPanel;
                let message = self
                    .state
                    .stash
                    .entries
                    .get(index)
                    .map(|e| e.message.clone())
                    .unwrap_or_default();
                let panel = GitDiffPanel::new_for_stash(repo_path, ref_str, message);
                self.add_panel(Box::new(panel));
            }
            _ => {}
        }

        Ok(())
    }

    pub(in crate::app) fn handle_git_file_action(
        &mut self,
        value: termide_modal::ModalOutput,
        file_path: &std::path::Path,
        repo_path: &std::path::Path,
        is_staged: bool,
    ) -> Result<()> {
        use termide_modal::InfoActionResult;

        if let Some(result) = value.get::<InfoActionResult>() {
            match result {
                InfoActionResult::Action(action) => match action.as_str() {
                    "git_status" => {
                        // Open a Git Status panel for THIS repository. Always
                        // create a new one rather than focusing an existing
                        // panel (which may track a different repo).
                        let git_status_panel =
                            termide_panel_git_status::GitStatusPanel::new_for_repo(
                                repo_path.to_path_buf(),
                                Some(&self.project_root),
                            );
                        self.add_panel(Box::new(git_status_panel));
                        self.auto_save_layout();
                    }
                    "stage" => {
                        let file = file_path.to_path_buf();
                        self.spawn_git_command(repo_path, GitJobKind::Stage, move |repo| {
                            termide_git::stage_file(repo, &file)
                        });
                    }
                    "unstage" => {
                        let file = file_path.to_path_buf();
                        self.spawn_git_command(repo_path, GitJobKind::Unstage, move |repo| {
                            termide_git::unstage_file(repo, &file)
                        });
                    }
                    "edit" => {
                        // Open file in editor (editor shows git diff markers automatically)
                        let full_path = repo_path.join(file_path);
                        let _ = self.open_editor_for_file(full_path);
                    }
                    "diff" => {
                        // Open git diff panel filtered to this file
                        self.process_single_event(PanelEvent::OpenGitDiff {
                            repo_path: repo_path.to_path_buf(),
                            commit_hash: None,
                            file_path: Some(file_path.to_path_buf()),
                        })?;
                    }
                    "revert" => {
                        // Open confirmation modal before reverting
                        let t = termide_i18n::t();
                        let confirm_msg =
                            format!("{}\n\n{}", file_path.display(), t.git_revert_confirm());
                        let modal =
                            termide_modal::ConfirmModal::new(t.git_action_revert(), &confirm_msg);
                        self.state.set_pending_action(
                            termide_state::PendingAction::GitRevertFile {
                                file_path: file_path.to_path_buf(),
                                repo_path: repo_path.to_path_buf(),
                                is_staged,
                            },
                            ActiveModal::Confirm(Box::new(modal)),
                        );
                    }
                    _ => {
                        // Close the modal for "close" or any unknown action
                    }
                },
                InfoActionResult::Closed => {
                    // Just close the modal
                }
                InfoActionResult::CancelOperation => {
                    // This is handled in handle_git_push_pull_from_modal, should not reach here
                }
            }
        }
        Ok(())
    }

    /// Handle git commit action from CommitModal
    pub(in crate::app) fn handle_git_commit(
        &mut self,
        value: termide_modal::ModalOutput,
        repo_path: &std::path::Path,
    ) -> Result<()> {
        // value is the commit message (String)
        if let Some(message) = value.get::<String>() {
            // Hooks may run for minutes.
            let message = message.clone();
            self.spawn_git_job(repo_path, GitJobKind::Commit, move |repo| {
                termide_git::commit(repo, &message)
                    .map(GitJobOutput::Committed)
                    .map_err(|e| git_job_failure(&GitJobKind::Commit, &e))
            });
            self.state
                .set_info(termide_i18n::t().git_commit_in_progress());
        }
        Ok(())
    }

    /// Report the git jobs that finished and refresh the panels on their
    /// repositories.
    pub(in crate::app) fn check_git_jobs(&mut self) {
        use std::sync::mpsc::TryRecvError;
        if self.state.git_jobs.is_empty() {
            return;
        }
        let mut finished = Vec::new();
        self.state
            .git_jobs
            .retain(|job| match job.receiver.try_recv() {
                Ok(result) => {
                    finished.push((job.repo_path.clone(), job.kind.clone(), result));
                    false
                }
                Err(TryRecvError::Empty) => true,
                Err(TryRecvError::Disconnected) => {
                    let failure = git_job_failure(&job.kind, "");
                    finished.push((job.repo_path.clone(), job.kind.clone(), Err(failure)));
                    false
                }
            });
        for (repo_path, kind, result) in finished {
            self.state.needs_redraw = true;
            if let GitJobKind::StashList {
                button_area,
                has_changes,
                project_root,
            } = kind
            {
                if let Ok(GitJobOutput::Stashes(entries)) = result {
                    self.open_stash_dropdown(
                        repo_path,
                        entries,
                        button_area,
                        has_changes,
                        &project_root,
                    );
                }
                continue;
            }
            if let GitJobKind::StashInfo {
                index,
                project_root,
            } = kind
            {
                if let Ok(GitJobOutput::StashDetails(Some(info))) = result {
                    if project_root == self.project_root
                        && self.state.active_modal.is_none()
                        && self.state.ui.stash_submenu.open
                    {
                        self.show_stash_info_modal(repo_path, index, info);
                    }
                }
                continue;
            }
            let report = git_job_report(&kind, &result);
            if kind == GitJobKind::Commit && report.error.is_some() {
                // Drop the "committing" notice.
                self.state.clear_status();
            }
            if let Some(info) = report.info {
                self.state.set_info(info);
            }
            if let Some(error) = report.error {
                self.show_error_modal(error);
            }
            if report.refresh {
                self.send_git_update(&repo_path);
            }
        }
    }

    /// Open the stash dropdown on the stashes listed for it, unless the
    /// screen moved on while they were listed: another project shown, a
    /// window or another dropdown opened.
    fn open_stash_dropdown(
        &mut self,
        repo_path: std::path::PathBuf,
        entries: Vec<termide_git::StashEntry>,
        button_area: ratatui::layout::Rect,
        has_changes: bool,
        project_root: &std::path::Path,
    ) {
        if project_root != self.project_root
            || self.state.active_modal.is_some()
            || self.state.ui.stash_submenu.open
        {
            return;
        }
        self.state.stash.entries = entries;
        self.state.ui.stash_button_area = Some(button_area);
        self.state.stash.repo_path = Some(repo_path);
        self.state.stash.has_changes = has_changes;
        self.state.ui.stash_submenu.open_unchosen();
    }

    /// The stash button pressed: list the stashes in the background and open
    /// the dropdown when they come; pressed again before that, or with the
    /// dropdown open, it closes instead.
    pub(in crate::app) fn toggle_stash_dropdown(
        &mut self,
        repo_path: std::path::PathBuf,
        button_area: ratatui::layout::Rect,
        has_changes: bool,
    ) {
        self.state.needs_redraw = true;
        if self.state.ui.stash_submenu.open {
            self.state.ui.stash_submenu.close();
            return;
        }
        let listing = self
            .state
            .git_jobs
            .iter()
            .position(|job| matches!(job.kind, GitJobKind::StashList { .. }));
        if let Some(listing) = listing {
            // Its answer is dropped with the receiver.
            self.state.git_jobs.remove(listing);
            return;
        }
        let kind = GitJobKind::StashList {
            button_area,
            has_changes,
            project_root: self.project_root.clone(),
        };
        self.spawn_git_job(&repo_path, kind, |repo| {
            Ok(GitJobOutput::Stashes(termide_git::stash_list(repo)))
        });
    }

    /// Handle git revert file action (after confirmation)
    pub(in crate::app) fn handle_git_revert_file(
        &mut self,
        value: termide_modal::ModalOutput,
        file_path: &std::path::Path,
        repo_path: &std::path::Path,
        is_staged: bool,
    ) -> Result<()> {
        // value is bool from ConfirmModal
        if let Some(&confirmed) = value.get::<bool>() {
            if confirmed {
                let file = file_path.to_path_buf();
                self.spawn_git_command(repo_path, GitJobKind::Revert, move |repo| {
                    // A staged file is unstaged first (a staged rename undone
                    // whole), then restored.
                    if is_staged {
                        termide_git::revert_staged_file(repo, &file)
                    } else {
                        termide_git::revert_file(repo, &file)
                    }
                });
            }
        }
        Ok(())
    }

    /// Handle git revert ALL changes action (after confirmation)
    pub(in crate::app) fn handle_git_revert_all(
        &mut self,
        value: termide_modal::ModalOutput,
        repo_path: &std::path::Path,
    ) -> Result<()> {
        if let Some(&confirmed) = value.get::<bool>() {
            if confirmed {
                self.spawn_git_job(repo_path, GitJobKind::RevertAll, |repo| {
                    // Unstage everything first
                    termide_git::unstage_all(repo)
                        .map_err(|e| termide_i18n::t().editor_unstage_failed_fmt(&e))?;
                    // git checkout -- . reverts tracked files; clean -fd
                    // removes untracked
                    termide_git::revert_all(repo)
                        .map(|()| GitJobOutput::Done)
                        .map_err(|e| git_job_failure(&GitJobKind::RevertAll, &e))
                });
            }
        }
        Ok(())
    }

    /// Send git update event to refresh git panels.
    /// Panels showing content get `OnGitUpdate`, panels collapsed to their
    /// title bar get `MarkStale` (consistent with `poll_watcher_events()`).
    pub(in crate::app) fn send_git_update(&mut self, repo_path: &std::path::Path) {
        use termide_core::PanelCommand;
        let repo_paths: Vec<&std::path::Path> = vec![repo_path];
        let area_height = self.panel_area_height();
        for (panel, is_visible) in self
            .layout_manager
            .iter_all_panels_with_visibility_mut(area_height)
        {
            let result = if is_visible {
                panel.handle_command(PanelCommand::OnGitUpdate {
                    repo_paths: &repo_paths,
                })
            } else {
                panel.handle_command(PanelCommand::MarkStale)
            };
            if result.needs_redraw() {
                self.state.needs_redraw = true;
            }
        }
    }

    /// Handle git push/pull actions from InfoActionModal
    /// Returns true if the action was handled (and modal should stay open or closed by us)
    pub(in crate::app) fn handle_git_push_pull_from_modal(
        &mut self,
        result: &ModalResult<termide_modal::ModalOutput>,
    ) -> Result<bool> {
        use termide_core::GitOperationType;
        use termide_modal::InfoActionResult;
        use termide_state::PendingAction;

        // Check if the pending action is a git file action
        let is_git_file_action = matches!(
            &self.state.pending_action,
            Some(PendingAction::GitFileAction { .. })
        );

        if !is_git_file_action {
            return Ok(false);
        }

        // Check if result is push, pull, or cancel operation
        if let ModalResult::Confirmed(value) = result {
            if let Some(action_result) = value.get::<InfoActionResult>() {
                match action_result {
                    InfoActionResult::Action(action) => {
                        let operation = match action.as_str() {
                            "push" => GitOperationType::Push,
                            "pull" => GitOperationType::Pull,
                            _ => return Ok(false),
                        };

                        // Get repo_path from pending action
                        let repo_path = match &self.state.pending_action {
                            Some(PendingAction::GitFileAction { repo_path, .. }) => {
                                repo_path.clone()
                            }
                            _ => return Ok(false),
                        };

                        // Set operation in progress on the modal
                        if let Some(ActiveModal::InfoAction(modal)) = &mut self.state.active_modal {
                            modal.set_operation_in_progress(Some(action.clone()));
                        }

                        // Start background git operation
                        self.event_git_operation(operation, repo_path, None)?;

                        return Ok(true);
                    }
                    InfoActionResult::CancelOperation => {
                        // Cancel the running git operation
                        self.event_cancel_git_operation();

                        // Clear operation state on modal but keep it open
                        if let Some(ActiveModal::InfoAction(modal)) = &mut self.state.active_modal {
                            modal.set_operation_in_progress(None);
                        }

                        return Ok(true);
                    }
                    _ => return Ok(false),
                }
            }
        }

        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A git job reports as it did when it ran on the interface thread:
    /// its notice on success, git's message in an error window on failure,
    /// and the panels refreshed after what changes the working tree.
    #[test]
    fn git_job_reports_keep_their_notices_and_refreshes() {
        let t = termide_i18n::t();
        let done = Ok(GitJobOutput::Done);

        let popped = git_job_report(&GitJobKind::StashPop(2), &done);
        assert_eq!(popped.info, Some(t.git_stash_popped_fmt("stash@{2}")));
        assert_eq!(popped.error, None);
        assert!(popped.refresh);

        let committed = git_job_report(
            &GitJobKind::Commit,
            &Ok(GitJobOutput::Committed("abc123".into())),
        );
        assert_eq!(committed.info, Some(t.git_committed_fmt("abc123")));
        assert!(committed.refresh);

        // Staging is left to the watcher on the index.
        for kind in [GitJobKind::Stage, GitJobKind::Unstage] {
            let report = git_job_report(&kind, &done);
            assert!(report.info.is_some());
            assert!(!report.refresh);
        }

        // A failure shows the worded message and refreshes nothing, except
        // a commit's: its hooks may have staged.
        let failure = git_job_failure(&GitJobKind::StashDrop(0), "conflict");
        assert_eq!(failure, t.git_stash_failed_fmt("conflict"));
        let failed = git_job_report(&GitJobKind::StashDrop(0), &Err(failure.clone()));
        assert_eq!(
            failed,
            GitJobReport {
                info: None,
                error: Some(failure),
                refresh: false,
            }
        );
        let commit_failed = git_job_report(&GitJobKind::Commit, &Err("hook".into()));
        assert!(commit_failed.refresh);
    }
}
