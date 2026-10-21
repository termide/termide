//! Git-related modal result handlers (stash, commit, revert, push/pull, file actions).

use anyhow::Result;

use crate::app::App;
use crate::state::ActiveModal;
use termide_core::PanelEvent;
use termide_modal::ModalResult;

impl App {
    pub(in crate::app) fn handle_git_stash_drop(
        &mut self,
        repo_path: std::path::PathBuf,
        index: usize,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(&confirmed) = value.downcast_ref::<bool>() {
            if confirmed {
                match termide_git::stash_drop(&repo_path, index) {
                    Ok(()) => {
                        self.state.set_info(format!("Dropped stash@{{{}}}", index));
                        self.send_git_update(&repo_path);
                    }
                    Err(e) => {
                        self.show_error_modal(format!("Stash drop error: {}", e));
                    }
                }
            }
        }
        Ok(())
    }

    /// Handle git stash action from InfoActionModal.
    /// Actions: "pop", "apply", "drop", "diff"
    pub(in crate::app) fn handle_git_stash_action(
        &mut self,
        repo_path: std::path::PathBuf,
        index: usize,
        ref_str: String,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        use termide_modal::InfoActionResult;
        let Some(result) = value.downcast_ref::<InfoActionResult>() else {
            return Ok(());
        };
        let action_id = match result {
            InfoActionResult::Action(id) => id.as_str(),
            _ => return Ok(()),
        };

        match action_id {
            "pop" => match termide_git::stash_pop(&repo_path, index) {
                Ok(()) => {
                    self.state.set_info(format!("Popped stash@{{{}}}", index));
                    self.send_git_update(&repo_path);
                }
                Err(e) => {
                    self.show_error_modal(format!("Stash pop error: {}", e));
                }
            },
            "apply" => match termide_git::stash_apply(&repo_path, index) {
                Ok(()) => {
                    self.state.set_info(format!("Applied stash@{{{}}}", index));
                    self.send_git_update(&repo_path);
                }
                Err(e) => {
                    self.show_error_modal(format!("Stash apply error: {}", e));
                }
            },
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
        value: Box<dyn std::any::Any>,
        file_path: &std::path::Path,
        repo_path: &std::path::Path,
        is_staged: bool,
    ) -> Result<()> {
        use termide_modal::InfoActionResult;

        if let Some(result) = value.downcast_ref::<InfoActionResult>() {
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
                        if let Err(e) = termide_git::stage_file(repo_path, file_path) {
                            self.show_error_modal(format!("Stage error: {}", e));
                        } else {
                            self.state.set_info("File staged".to_string());
                        }
                    }
                    "unstage" => {
                        if let Err(e) = termide_git::unstage_file(repo_path, file_path) {
                            self.show_error_modal(format!("Unstage error: {}", e));
                        } else {
                            self.state.set_info("File unstaged".to_string());
                        }
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
        value: Box<dyn std::any::Any>,
        repo_path: &std::path::Path,
    ) -> Result<()> {
        // value is the commit message (String)
        if let Some(message) = value.downcast_ref::<String>() {
            match termide_git::commit(repo_path, message) {
                Ok(commit_id) => {
                    self.state.set_info(format!(
                        "Committed: {}",
                        &commit_id[..8.min(commit_id.len())]
                    ));
                    // Trigger git update event to refresh panels
                    self.send_git_update(repo_path);
                }
                Err(e) => {
                    self.show_error_modal(format!("Commit failed: {}", e));
                }
            }
        }
        Ok(())
    }

    /// Handle git revert file action (after confirmation)
    pub(in crate::app) fn handle_git_revert_file(
        &mut self,
        value: Box<dyn std::any::Any>,
        file_path: &std::path::Path,
        repo_path: &std::path::Path,
        is_staged: bool,
    ) -> Result<()> {
        // value is bool from ConfirmModal
        if let Some(&confirmed) = value.downcast_ref::<bool>() {
            if confirmed {
                // If file is staged, unstage it first
                if is_staged {
                    if let Err(e) = termide_git::unstage_file(repo_path, file_path) {
                        self.show_error_modal(format!("Unstage error: {}", e));
                        return Ok(());
                    }
                }
                // Now revert the file
                if let Err(e) = termide_git::revert_file(repo_path, file_path) {
                    self.show_error_modal(format!("Revert error: {}", e));
                } else {
                    self.state.set_info("File reverted".to_string());
                    // Trigger git update event to refresh panels
                    self.send_git_update(repo_path);
                }
            }
        }
        Ok(())
    }

    /// Handle git revert ALL changes action (after confirmation)
    pub(in crate::app) fn handle_git_revert_all(
        &mut self,
        value: Box<dyn std::any::Any>,
        repo_path: &std::path::Path,
    ) -> Result<()> {
        if let Some(&confirmed) = value.downcast_ref::<bool>() {
            if confirmed {
                // Unstage everything first
                if let Err(e) = termide_git::unstage_all(repo_path) {
                    self.show_error_modal(format!("Unstage error: {}", e));
                    return Ok(());
                }
                // Revert all unstaged files (those that were originally modified, not untracked)
                // git checkout -- . reverts tracked files; clean -fd removes untracked
                if let Err(e) = termide_git::revert_all(repo_path) {
                    self.show_error_modal(format!("Revert error: {}", e));
                } else {
                    let t = termide_i18n::t();
                    self.state.set_info(t.git_refreshed().to_string());
                    self.send_git_update(repo_path);
                }
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
        result: &ModalResult<Box<dyn std::any::Any>>,
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
            if let Some(action_result) = value.downcast_ref::<InfoActionResult>() {
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
