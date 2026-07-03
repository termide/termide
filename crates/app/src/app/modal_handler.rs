//! Modal window handling for the application.

use anyhow::Result;
use crossterm::event::KeyCode;

use super::App;
use crate::state::ActiveModal;
use crate::PanelExt;
use termide_modal::ModalResult;
use termide_ui::path_utils;

impl App {
    /// Handle keyboard event in modal window
    pub(super) fn handle_modal_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        // Indicator modals opened from menu: intercept Left/Right/Esc for menu navigation
        if self.state.is_menu_open() {
            let is_resource = self.state.is_resource_modal_open();
            let is_calendar = matches!(self.state.active_modal, Some(ActiveModal::Calendar(_)));

            if is_resource || is_calendar {
                match key.code {
                    KeyCode::Esc => {
                        self.state.close_indicator_modal();
                        self.state.close_menu();
                        return Ok(());
                    }
                    KeyCode::Left if is_resource => {
                        self.state.close_indicator_modal();
                        self.switch_to_prev_menu()?;
                        return Ok(());
                    }
                    KeyCode::Right if is_resource => {
                        self.state.close_indicator_modal();
                        self.switch_to_next_menu()?;
                        return Ok(());
                    }
                    KeyCode::Left if is_calendar => {
                        if let Some(ActiveModal::Calendar(cal)) = &self.state.active_modal {
                            if cal.at_left_edge() {
                                self.state.close_indicator_modal();
                                self.switch_to_prev_menu()?;
                                return Ok(());
                            }
                        }
                        // Not at edge — fall through to calendar's own handler
                    }
                    KeyCode::Right if is_calendar => {
                        if let Some(ActiveModal::Calendar(cal)) = &self.state.active_modal {
                            if cal.at_right_edge() {
                                self.state.close_indicator_modal();
                                self.switch_to_next_menu()?;
                                return Ok(());
                            }
                        }
                    }
                    _ => {} // Other keys — fall through to modal handler
                }
            }
        }

        // Build canonical+raw chord once for the modal handler.
        let chord = termide_core::KeyChord::new(key, &self.normalizer);

        // Get mutable reference to active modal window
        if let Some(modal) = self.state.get_active_modal_mut() {
            // Handle event in corresponding modal window
            let modal_result = modal.handle_key_erased(chord)?;

            // If modal window returned result, handle it
            if let Some(result) = modal_result {
                // Check modal type before taking state references
                let is_rename_pattern = matches!(modal, ActiveModal::RenamePattern(_));

                // Handle cancellation from RenamePattern - return to ConflictModal
                if is_rename_pattern && matches!(result, ModalResult::Cancelled) {
                    // Take operation from pending action and return to ConflictModal
                    #[allow(clippy::collapsible_match)]
                    if let Some(action) = self.state.take_pending_action() {
                        if let termide_state::PendingAction::RenameWithPattern {
                            operation, ..
                        } = action
                        {
                            use termide_modal::ConflictModal;

                            if let Some(source) = operation.current_source() {
                                let final_dest = path_utils::resolve_batch_destination_path(
                                    source,
                                    &operation.destination,
                                    operation.sources.len() == 1,
                                    operation.destination_is_directory(),
                                );

                                let remaining_items = operation
                                    .sources
                                    .len()
                                    .saturating_sub(operation.current_index + 1);
                                let modal =
                                    ConflictModal::new(source, &final_dest, remaining_items);
                                self.state.pending_action =
                                    Some(termide_state::PendingAction::ContinueBatchOperation {
                                        operation,
                                    });
                                self.state.active_modal =
                                    Some(ActiveModal::Conflict(Box::new(modal)));
                                return Ok(());
                            }
                        }
                    }
                }

                // Return to bookmarks menu on cancel of bookmark deletion
                if matches!(result, ModalResult::Cancelled) {
                    use termide_state::PendingAction;
                    if let Some(action) = self.state.pending_action.take() {
                        match action {
                            PendingAction::DeleteBookmark {
                                group,
                                is_project,
                                selected,
                                ..
                            }
                            | PendingAction::AddBookmark {
                                group,
                                is_project,
                                selected,
                            }
                            | PendingAction::EditBookmark {
                                group,
                                is_project,
                                selected,
                                ..
                            } => {
                                self.state.close_modal();
                                self.reopen_bookmarks_menu(group, is_project, selected);
                                return Ok(());
                            }
                            PendingAction::DeleteBookmarkGroup {
                                selected,
                                is_project,
                                ..
                            } => {
                                self.state.close_modal();
                                self.reopen_bookmarks_menu(None, is_project, selected);
                                return Ok(());
                            }
                            other => self.state.pending_action = Some(other),
                        }
                    }
                }

                self.sync_copy_symlink_flag();
                // Read InputModal checkbox before closing (used by stash push)
                if let Some(termide_modal::ActiveModal::Input(ref modal)) = self.state.active_modal
                {
                    self.state.stash.include_untracked = modal.is_checkbox_checked();
                }
                self.state.close_modal();
                if let ModalResult::Confirmed(value) = result {
                    self.handle_modal_result(value)?;
                }
            }
        }
        self.try_apply_permissions_live();
        Ok(())
    }

    /// Handle paste event in modal window
    /// Returns true if modal handled the paste, false to pass to panel
    pub(super) fn handle_modal_paste(&mut self, text: &str) -> bool {
        if let Some(modal) = self.state.get_active_modal_mut() {
            modal.handle_paste(text)
        } else {
            false
        }
    }

    /// Handle mouse event in modal window
    pub(super) fn handle_modal_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        modal_area: ratatui::layout::Rect,
    ) -> Result<()> {
        // Get mutable reference to active modal window
        if let Some(modal) = self.state.get_active_modal_mut() {
            // Handle event in corresponding modal window
            let modal_result = modal.handle_mouse_erased(mouse, modal_area)?;

            // If modal window returned result, handle it
            if let Some(result) = modal_result {
                // Check if this is a git push/pull action that should keep modal open
                if self.handle_git_push_pull_from_modal(&result)? {
                    return Ok(());
                }

                self.sync_copy_symlink_flag();
                // Read InputModal checkbox before closing (used by stash push)
                if let Some(termide_modal::ActiveModal::Input(ref modal)) = self.state.active_modal
                {
                    self.state.stash.include_untracked = modal.is_checkbox_checked();
                }
                self.state.close_modal();
                if let ModalResult::Confirmed(value) = result {
                    self.handle_modal_result(value)?;
                }
            }
        }
        self.try_apply_permissions_live();
        Ok(())
    }

    /// Route the dead-remote-session recovery dialog's button to the active
    /// file manager (reconnect / open local) or close the panel. Plain VFS
    /// "OK" modals return id "ok" and fall through to the no-op arm.
    fn handle_vfs_message_action(&mut self, value: Box<dyn std::any::Any>) {
        use termide_modal::InfoActionResult;
        let Some(InfoActionResult::Action(id)) = value.downcast_ref::<InfoActionResult>() else {
            return;
        };
        match id.as_str() {
            "reconnect" => {
                if let Some(fm) = self.active_file_manager_mut() {
                    fm.reconnect_remote();
                }
            }
            "go_local" => {
                if let Some(fm) = self.active_file_manager_mut() {
                    fm.switch_to_local_home();
                }
            }
            "close" => self.close_panel_at_index(),
            _ => {}
        }
    }

    /// Handle modal window result
    pub(super) fn handle_modal_result(&mut self, value: Box<dyn std::any::Any>) -> Result<()> {
        use termide_state::PendingAction;

        if let Some(action) = self.state.take_pending_action() {
            match action {
                PendingAction::CreateFile { directory } => {
                    self.handle_create_file(directory, value)?;
                }
                PendingAction::ViewPath { base_dir } => {
                    self.handle_view_path(base_dir, value)?;
                }
                PendingAction::GitSshPassphraseRetry {
                    operation,
                    repo_path,
                } => {
                    self.handle_git_ssh_passphrase_retry(operation, repo_path, value)?;
                }
                PendingAction::CreateDirectory { directory } => {
                    self.handle_create_directory(directory, value)?;
                }
                PendingAction::DeletePath { paths } => {
                    self.handle_delete_path(paths, value)?;
                }
                PendingAction::DeleteRemotePath { paths, vfs_manager } => {
                    self.handle_delete_remote_path(paths, vfs_manager, value)?;
                }
                PendingAction::CleanupPartialRemote { path, vfs_manager } => {
                    // Confirmed → run the delete synchronously so the
                    // panel listing afterwards reflects the actual
                    // server state. Errors (typically "not found"
                    // when cancel arrived before any byte hit the
                    // server) go to the log instead of a modal so
                    // the user isn't bothered with an expected outcome.
                    if value.downcast_ref::<bool>().copied().unwrap_or(false) {
                        let op = vfs_manager.delete(&path);
                        match op.recv() {
                            Ok(()) => log::info!(
                                "Cleaned up cancelled-upload partial: {}",
                                path.log_safe_key()
                            ),
                            Err(e) => log::info!(
                                "Partial cleanup for {} skipped: {}",
                                path.log_safe_key(),
                                e
                            ),
                        }
                        // Re-list any file manager that might be
                        // showing the parent directory — otherwise
                        // the just-deleted partial would still
                        // appear in the panel until manual Ctrl+R.
                        for panel in self.layout_manager.iter_all_panels_mut() {
                            if let Some(fm) = panel.as_file_manager_mut() {
                                let _ = fm.load_directory();
                            }
                        }
                    }
                }
                PendingAction::SaveFileAs { directory } => {
                    self.handle_save_file_as(directory, value)?;
                }
                PendingAction::SaveContentAs { directory, content } => {
                    self.handle_save_content_as(directory, content, value)?;
                }
                PendingAction::SaveBinary => {
                    if value.downcast_ref::<bool>().copied().unwrap_or(false) {
                        if let Some(panel) = self.layout_manager.active_panel_mut() {
                            if let Some(bin) = panel
                                .as_any_mut()
                                .downcast_mut::<termide_panel_binary::BinaryPanel>()
                            {
                                match bin.save() {
                                    Ok(()) => {
                                        self.state.set_info("Saved (backup: .bak)".to_string())
                                    }
                                    Err(e) => self.show_error_modal(format!("Save failed: {e}")),
                                }
                            }
                        }
                    }
                }
                PendingAction::ClosePanel => {
                    self.handle_close_panel(value)?;
                }
                PendingAction::CloseEditorWithSave => {
                    self.handle_close_editor_with_save(value)?;
                }
                PendingAction::CloseEditorExternal => {
                    self.handle_close_editor_external(value)?;
                }
                PendingAction::CloseEditorConflict => {
                    self.handle_close_editor_conflict(value)?;
                }
                PendingAction::CopyPath {
                    sources,
                    target_directory,
                    create_symlink,
                    create_relative_symlink,
                } => {
                    self.handle_copy_path(
                        sources,
                        target_directory,
                        create_symlink,
                        create_relative_symlink,
                        value,
                    )?;
                }
                PendingAction::MovePath {
                    sources,
                    target_directory,
                } => {
                    self.handle_move_path(sources, target_directory, value)?;
                }
                PendingAction::BatchFileOperation { operation } => {
                    self.process_batch_operation(operation);
                }
                PendingAction::ContinueBatchOperation { operation } => {
                    self.handle_continue_batch_operation(operation, value)?;
                }
                PendingAction::RenameWithPattern {
                    operation,
                    original_name,
                } => {
                    self.handle_rename_with_pattern(operation, original_name, value)?;
                }
                PendingAction::ChangeEditorTabSize => {
                    if let Some(text) = value.downcast_ref::<String>() {
                        if let Ok(n) = text.trim().parse::<usize>() {
                            let n = n.clamp(1, 16);
                            if let Some(panel) = self.layout_manager.active_panel_mut() {
                                if let Some(editor) = panel.as_editor_mut() {
                                    editor.set_tab_size_override(Some(n));
                                }
                            }
                        }
                    }
                }
                PendingAction::GotoLine => {
                    if let Some(text) = value.downcast_ref::<String>() {
                        // Accept "line" or "line:column" (1-based, matching the
                        // status-bar Pos display).
                        let raw = text.trim();
                        let mut parts = raw.splitn(2, ':');
                        if let Some(Ok(line)) = parts.next().map(|s| s.trim().parse::<usize>()) {
                            if line > 0 {
                                let column = parts
                                    .next()
                                    .and_then(|s| s.trim().parse::<usize>().ok())
                                    .filter(|&c| c > 0)
                                    .map(|c| c - 1)
                                    .unwrap_or(0);
                                self.event_goto_position(line - 1, column);
                            }
                        }
                    }
                }
                PendingAction::QuitApplication => {
                    // User confirmed quit - exit application
                    self.state.quit();
                }
                PendingAction::CancelOperation(op_id) => {
                    // User confirmed cancelling the background operation.
                    if value.downcast_ref::<bool>().copied().unwrap_or(false) {
                        self.event_cancel_operation(op_id);
                    }
                }
                PendingAction::ReplaceInContent { replace_with } => {
                    // User confirmed replacing all content-search matches.
                    if value.downcast_ref::<bool>().copied().unwrap_or(false) {
                        if let Some(fm) = self.active_file_manager_mut() {
                            let (files, count) = fm.replace_all_in_content_results(&replace_with);
                            fm.close_file_search();
                            let t = termide_i18n::t();
                            let lines = vec![(String::new(), t.replace_done_fmt(count, files))];
                            let modal =
                                termide_modal::InfoModal::new(t.replace_done_title(), lines);
                            self.state.active_modal =
                                Some(termide_modal::ActiveModal::Info(Box::new(modal)));
                        }
                    }
                }
                PendingAction::SwitchSession => {
                    self.handle_switch_session(value)?;
                }
                PendingAction::NewSession => {
                    self.handle_new_session_result(value)?;
                }
                PendingAction::DeleteSession { path } => {
                    if value.downcast_ref::<bool>().copied().unwrap_or(false) {
                        self.handle_delete_session(&path)?;
                    }
                }
                PendingAction::DeleteBookmark {
                    path,
                    is_project,
                    group,
                    selected,
                } => {
                    if value.downcast_ref::<bool>().copied().unwrap_or(false) {
                        if is_project {
                            if let Some(ref mut proj) = self.state.project_bookmarks {
                                proj.remove_in_group(&path, group.as_deref());
                                let proj_dir = self.state.project_root.join(".termide");
                                if let Err(e) = proj.save_to_dir(&proj_dir) {
                                    log::error!(
                                        "Failed to save project bookmarks to {}: {}",
                                        proj_dir.display(),
                                        e
                                    );
                                }
                            }
                        } else {
                            self.state
                                .bookmarks
                                .remove_in_group(&path, group.as_deref());
                            self.state.save_bookmarks();
                        }
                    }
                    self.reopen_bookmarks_menu(group, is_project, selected);
                }
                PendingAction::DeleteBookmarkGroup {
                    group,
                    is_project,
                    selected,
                } => {
                    if value.downcast_ref::<bool>().copied().unwrap_or(false) {
                        if is_project {
                            if let Some(ref mut proj) = self.state.project_bookmarks {
                                proj.remove_group(&group);
                                let proj_dir = self.state.project_root.join(".termide");
                                if let Err(e) = proj.save_to_dir(&proj_dir) {
                                    log::error!(
                                        "Failed to save project bookmarks to {}: {}",
                                        proj_dir.display(),
                                        e
                                    );
                                }
                            }
                        } else {
                            self.state.bookmarks.remove_group(&group);
                            self.state.save_bookmarks();
                        }
                    }
                    self.reopen_bookmarks_menu(None, false, selected);
                }
                PendingAction::EditBookmark {
                    original_path,
                    original_group,
                    was_project,
                    selected,
                    ..
                } => {
                    use termide_modal::BookmarkAddResult;
                    let result_group = value
                        .downcast_ref::<BookmarkAddResult>()
                        .and_then(|r| r.group.clone());
                    let result_is_project = value
                        .downcast_ref::<BookmarkAddResult>()
                        .is_some_and(|r| r.is_project);
                    if let Some(err) = self.handle_edit_bookmark_result(
                        value,
                        &original_path,
                        original_group.as_deref(),
                        was_project,
                    )? {
                        self.show_bookmark_error(&err);
                    }
                    self.reopen_bookmarks_menu(result_group, result_is_project, selected);
                }
                PendingAction::ChangeRootPath => {
                    self.handle_change_root_path_result(value)?;
                }
                // Navigation actions are handled in key_handler, should not get here
                PendingAction::NextPanel | PendingAction::PrevPanel => {}
                // Git actions will open panels directly, should not get here
                PendingAction::OpenGitStatus | PendingAction::OpenGitLog => {}
                // Git file action from File Info modal
                PendingAction::GitFileAction {
                    file_path,
                    repo_path,
                    is_staged,
                } => {
                    self.handle_git_file_action(value, &file_path, &repo_path, is_staged)?;
                }
                // Git commit action
                PendingAction::GitCommit { repo_path } => {
                    self.handle_git_commit(value, &repo_path)?;
                }
                // Git revert file action (with confirmation)
                PendingAction::GitRevertFile {
                    file_path,
                    repo_path,
                    is_staged,
                } => {
                    self.handle_git_revert_file(value, &file_path, &repo_path, is_staged)?;
                }
                // Git revert all changes action (with confirmation)
                PendingAction::GitRevertAll { repo_path } => {
                    self.handle_git_revert_all(value, &repo_path)?;
                }
                // Switch active panel's working directory
                PendingAction::SwitchDirectory => {
                    self.handle_switch_directory(value)?;
                }
                // Add bookmark
                PendingAction::AddBookmark { selected, .. } => {
                    use termide_modal::BookmarkAddResult;
                    let result_group = value
                        .downcast_ref::<BookmarkAddResult>()
                        .and_then(|r| r.group.clone());
                    let result_is_project = value
                        .downcast_ref::<BookmarkAddResult>()
                        .is_some_and(|r| r.is_project);
                    if let Some(err) = self.handle_add_bookmark_result(value)? {
                        self.show_bookmark_error(&err);
                    }
                    self.reopen_bookmarks_menu(result_group, result_is_project, selected);
                }
                // Go to path/URL
                PendingAction::GoToPath {
                    current_directory: _,
                } => {
                    self.handle_goto_path(value)?;
                }
                // VFS message / connection-error recovery dialog. Plain "OK"
                // modals return id "ok" (ignored); the dead-session dialog
                // returns reconnect / go_local / close.
                PendingAction::VfsMessage => {
                    self.handle_vfs_message_action(value);
                }
                // Handle cancelled copy/move operation cleanup
                PendingAction::CancelCopyCleanup {
                    partial_path,
                    all_dest_paths,
                    is_directory,
                    batch_operation,
                } => {
                    self.handle_cancel_copy_cleanup(
                        partial_path,
                        all_dest_paths,
                        is_directory,
                        batch_operation,
                        value,
                    )?;
                }
                // Change file permissions (applied live via try_apply_permissions_live)
                PendingAction::ChangePermissions { .. } => {}
                // Follow symlink — navigate to target
                PendingAction::FollowSymlink { target_path } => {
                    self.handle_follow_symlink(value, &target_path)?;
                }
                // Handle conflict resolution for OperationManager operations
                PendingAction::ResolveOperationConflict { operation_id } => {
                    self.handle_resolve_operation_conflict(operation_id, value)?;
                }
                // LSP rename symbol: user confirmed new name in the input modal
                PendingAction::LspRenameSymbol {
                    file_path,
                    line,
                    column,
                } => {
                    self.handle_lsp_rename_symbol(file_path, line, column, value)?;
                }
                // Command palette: user chose a command by index
                PendingAction::CommandPalette => {
                    self.handle_command_palette_result(value)?;
                }
                // Git stash push: create new stash with user message
                PendingAction::GitStashPush { repo_path } => {
                    self.handle_git_stash_push(repo_path, value)?;
                }
                // Git stash drop: drop stash after confirmation
                PendingAction::GitStashDrop { repo_path, index } => {
                    self.handle_git_stash_drop(repo_path, index, value)?;
                }
                // Git stash rename: change message
                PendingAction::GitStashRename { repo_path, index } => {
                    if let Some(new_message) = value.downcast_ref::<String>() {
                        let new_message = new_message.trim();
                        if !new_message.is_empty() {
                            match termide_git::stash_rename(&repo_path, index, new_message) {
                                Ok(()) => {
                                    self.state.set_info("Stash renamed".to_string());
                                    self.send_git_update(&repo_path);
                                }
                                Err(e) => {
                                    self.show_error_modal(format!("Stash rename error: {}", e));
                                }
                            }
                        }
                    }
                }
                // Git stash action: user chose from context menu (Pop/Apply/Drop/Diff)
                PendingAction::GitStashAction {
                    repo_path,
                    index,
                    ref_str,
                } => {
                    self.handle_git_stash_action(repo_path, index, ref_str, value)?;
                }
                PendingAction::CreateCommand => {
                    use termide_modal::CommandConfigResult;
                    if let Some(result) = value.downcast_ref::<CommandConfigResult>() {
                        self.handle_command_config_result(result)?;
                    }
                    self.state.cache.commands_registry = None;
                    self.state.cache.hotkey_table = None;
                }
                PendingAction::EditCommand {
                    command_name,
                    is_project,
                    ..
                } => {
                    use termide_modal::CommandConfigResult;
                    if let Some(result) = value.downcast_ref::<CommandConfigResult>() {
                        self.handle_edit_command_config_result(command_name, is_project, result)?;
                    }
                }
                PendingAction::RunCommandWithParams { command } => {
                    use termide_modal::CommandParamsResult;
                    if let Some(result) = value.downcast_ref::<CommandParamsResult>() {
                        self.run_command_with_params(&command, &result.values)?;
                    }
                }
                PendingAction::DeleteCommand {
                    command_name,
                    is_project,
                    ..
                } => {
                    if value.downcast_ref::<bool>().copied().unwrap_or(false) {
                        let config_dir = if is_project {
                            self.project_root.join(".termide")
                        } else {
                            termide_config::get_config_dir()
                                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                        };
                        let mut metadata =
                            termide_config::commands::CommandsMetadata::load(&config_dir);
                        metadata.entries.remove(&command_name);
                        if let Err(e) = metadata.save(&config_dir) {
                            log::error!("Failed to save commands.toml: {}", e);
                        }
                        self.state.cache.commands_registry = None;
                        self.state.cache.hotkey_table = None;
                        self.state.needs_redraw = true;
                    }
                }
                PendingAction::RenameCommand {
                    command_name,
                    is_project,
                    group,
                    selected,
                } => {
                    if let Some(new_name) = value.downcast_ref::<String>() {
                        let sanitized = termide_modal::sanitize_filename(new_name.trim());
                        if !sanitized.is_empty() && sanitized != command_name {
                            let config_dir = if is_project {
                                self.project_root.join(".termide")
                            } else {
                                termide_config::get_config_dir()
                                    .unwrap_or_else(|_| std::path::PathBuf::from("."))
                            };
                            let mut metadata =
                                termide_config::commands::CommandsMetadata::load(&config_dir);
                            if let Some(entry) = metadata.entries.remove(&command_name) {
                                metadata.entries.insert(sanitized, entry);
                                if let Err(e) = metadata.save(&config_dir) {
                                    log::error!("Failed to save commands.toml: {}", e);
                                }
                            }
                            self.state.cache.commands_registry = None;
                            self.state.cache.hotkey_table = None;
                        }
                    }
                    self.reopen_commands_menu(group, selected);
                }
                PendingAction::RenameBookmark {
                    path,
                    group,
                    is_project,
                    selected,
                } => {
                    if let Some(new_name) = value.downcast_ref::<String>() {
                        if !new_name.is_empty() {
                            let config = if is_project {
                                self.state.project_bookmarks.as_mut()
                            } else {
                                Some(&mut self.state.bookmarks)
                            };
                            if let Some(config) = config {
                                if let Some(bm) = config.find_mut(&path) {
                                    bm.description = Some(new_name.clone());
                                }
                                if let Err(e) = config.save() {
                                    log::error!(
                                        "Failed to save bookmark config after rename: {}",
                                        e
                                    );
                                }
                            }
                        }
                    }
                    self.reopen_bookmarks_menu(group, is_project, selected);
                }
                PendingAction::Settings => {
                    use termide_modal::SettingsResult;
                    if let Some(result) = value.downcast_ref::<SettingsResult>() {
                        match result {
                            SettingsResult::Apply(config) => {
                                if let Err(e) =
                                    self.save_config_to_active_target((**config).clone())
                                {
                                    log::error!("Failed to save settings: {}", e);
                                    self.show_error_modal(format!(
                                        "Failed to save settings: {}",
                                        e
                                    ));
                                } else {
                                    // Refresh theme and language
                                    let theme_name = config.general.theme.clone();
                                    self.apply_theme(&theme_name)?;
                                    let lang_code = config.general.language.clone();
                                    let languages = termide_i18n::get_language_list();
                                    if let Some((_, name)) =
                                        languages.iter().find(|(c, _)| c == &lang_code)
                                    {
                                        self.apply_language(&lang_code, name)?;
                                    }
                                    self.state.set_info("Settings saved".to_string());
                                }
                            }
                            SettingsResult::CreateProjectOverride(config) => {
                                let cfg = (**config).clone();
                                if let Err(e) = cfg
                                    .save_project(&self.project_root, &self.state.global_baseline)
                                {
                                    log::error!("Failed to create project override: {}", e);
                                    self.show_error_modal(format!(
                                        "Failed to create project override: {}",
                                        e
                                    ));
                                } else {
                                    self.state.config = std::sync::Arc::new(cfg);
                                    log::info!(
                                        "Created project override at {}",
                                        termide_config::project_config_path(&self.project_root)
                                            .display()
                                    );
                                }
                            }
                            SettingsResult::RemoveProjectOverride => {
                                let t = termide_i18n::t();
                                let modal = termide_modal::ConfirmModal::new(
                                    t.settings_remove_project_override_title(),
                                    t.settings_remove_project_override_message(),
                                );
                                self.state.set_pending_action(
                                    crate::state::PendingAction::RemoveProjectOverride,
                                    crate::state::ActiveModal::Confirm(Box::new(modal)),
                                );
                            }
                            SettingsResult::Cancel => {}
                        }
                    }
                }
                PendingAction::RemoveProjectOverride => {
                    if value.downcast_ref::<bool>().copied().unwrap_or(false) {
                        let path = termide_config::project_config_path(&self.project_root);
                        match std::fs::remove_file(&path) {
                            Ok(()) => {
                                log::info!("Removed project override at {}", path.display());
                                // Effective config falls back to defaults+global.
                                self.state.config =
                                    std::sync::Arc::clone(&self.state.global_baseline);
                            }
                            Err(e) => {
                                log::error!("Failed to remove project override: {}", e);
                                self.show_error_modal(format!(
                                    "Failed to remove project override: {}",
                                    e
                                ));
                            }
                        }
                    }
                }
                PendingAction::DbFilter => {
                    use termide_modal::DbFilterResult;
                    if let Some(result) = value.downcast_ref::<DbFilterResult>() {
                        if let Some(panel) = self.layout_manager.active_panel_mut() {
                            if let Some(db) = panel
                                .as_any_mut()
                                .downcast_mut::<termide_panel_db::DbPanel>()
                            {
                                db.apply_filter_result(result.clone());
                            }
                        }
                    }
                }
                PendingAction::DbConnectionError => {
                    use termide_modal::InfoActionResult;
                    if let Some(InfoActionResult::Action(id)) =
                        value.downcast_ref::<InfoActionResult>()
                    {
                        match id.as_str() {
                            "reconnect" => {
                                if let Some(panel) = self.layout_manager.active_panel_mut() {
                                    if let Some(db) = panel
                                        .as_any_mut()
                                        .downcast_mut::<termide_panel_db::DbPanel>()
                                    {
                                        db.reconnect();
                                    }
                                }
                            }
                            "close" => self.close_panel_at_index(),
                            _ => {}
                        }
                    }
                }
                PendingAction::DbRowDetail { tsv, json, insert } => {
                    use termide_modal::InfoActionResult;
                    if let Some(InfoActionResult::Action(id)) =
                        value.downcast_ref::<InfoActionResult>()
                    {
                        let text = match id.as_str() {
                            "copy_json" => Some(json),
                            "copy_insert" => Some(insert),
                            "copy_tsv" => Some(tsv),
                            _ => None,
                        };
                        if let Some(text) = text {
                            if let Err(e) = termide_clipboard::copy(&text) {
                                log::error!("Failed to copy to clipboard: {}", e);
                            } else {
                                self.state
                                    .set_info(termide_i18n::t().db_copied().to_string());
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
