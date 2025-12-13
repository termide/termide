//! Panel event processing for the application.
//!
//! Processes `PanelEvent`s emitted by panels and translates them
//! into application state changes.

// Note: PanelExt is used for panel-specific operations (mouse clicks, resize)
// that require concrete type access. Common operations use Panel::handle_command().
#![allow(deprecated)]

use anyhow::Result;
use std::path::PathBuf;

use super::App;
use crate::PanelExt;
use termide_core::PanelEvent;
use termide_i18n as i18n;
use termide_logger as logger;
use termide_panel_editor::Editor;

impl App {
    /// Process events emitted by a panel.
    ///
    /// This method handles all `PanelEvent` variants and translates them
    /// into appropriate application state changes.
    pub(super) fn process_panel_events(&mut self, events: Vec<PanelEvent>) -> Result<()> {
        for event in events {
            self.process_single_event(event)?;
        }
        Ok(())
    }

    /// Process a single panel event.
    fn process_single_event(&mut self, event: PanelEvent) -> Result<()> {
        match event {
            // === File operations ===
            PanelEvent::OpenFile(path) => {
                self.event_open_file(path)?;
            }

            PanelEvent::ClosePanel => {
                // Request close of current panel (with confirmation if needed)
                self.handle_close_panel_request(0)?;
            }

            // === Status messages ===
            PanelEvent::ShowMessage(message) => {
                self.state.set_info(message);
            }

            PanelEvent::ShowError(message) => {
                self.state.set_error(message);
            }

            PanelEvent::SetStatusMessage { message, is_error } => {
                if is_error {
                    self.state.set_error(message);
                } else {
                    self.state.set_info(message);
                }
            }

            PanelEvent::ClearStatus => {
                self.state.clear_status();
            }

            // === Panel navigation ===
            PanelEvent::NextPanel => {
                self.layout_manager.next_group();
            }

            PanelEvent::PrevPanel => {
                self.layout_manager.prev_group();
            }

            // === Clipboard ===
            PanelEvent::CopyToClipboard(text) => {
                if let Err(e) = termide_clipboard::copy(&text) {
                    logger::error(format!("Failed to copy to clipboard: {}", e));
                }
            }

            // === Events not yet implemented ===
            PanelEvent::NeedsRedraw => {
                // UI will redraw on next frame anyway
            }

            PanelEvent::Quit => {
                logger::debug("Quit event received");
                self.handle_quit_request()?;
            }

            PanelEvent::SaveFile(path) => {
                self.event_save_file(path)?;
            }

            PanelEvent::CloseFile => {
                // Same as ClosePanel for now
                self.handle_close_panel_request(0)?;
            }

            PanelEvent::NavigateTo(path) => {
                self.event_navigate_to(path)?;
            }

            PanelEvent::GotoLine(line) => {
                self.event_goto_line(line);
            }

            PanelEvent::ShowConfirm {
                message,
                on_confirm,
            } => {
                self.event_show_confirm(message, on_confirm);
            }

            PanelEvent::ShowInput {
                prompt,
                initial_value,
                on_submit,
            } => {
                self.event_show_input(prompt, initial_value, on_submit);
            }

            PanelEvent::ShowSelect {
                title,
                options,
                on_select,
            } => {
                self.event_show_select(title, options, on_select);
            }

            PanelEvent::ShowSearch { initial_query } => {
                self.event_show_search(initial_query);
            }

            PanelEvent::ShowReplace { find, replace } => {
                self.event_show_replace(find, replace);
            }

            PanelEvent::ShowConflict {
                source,
                destination,
                remaining,
            } => {
                self.event_show_conflict(source, destination, remaining);
            }

            PanelEvent::WatchPath(path) => {
                self.event_watch_path(path);
            }

            PanelEvent::UnwatchPath(path) => {
                self.event_unwatch_path(path);
            }

            PanelEvent::RefreshGitStatus(path) => {
                self.event_refresh_git_status(path);
            }

            PanelEvent::RequestPaste => {
                self.event_paste_to_active_panel()?;
            }

            PanelEvent::FocusPanel(name) => {
                self.event_focus_panel(&name);
            }

            PanelEvent::SplitPanel { direction, .. } => {
                self.event_split_panel(direction);
            }
        }
        Ok(())
    }

    /// Handle RequestPaste event - paste clipboard to active panel
    fn event_paste_to_active_panel(&mut self) -> Result<()> {
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                if let Err(e) = editor.paste_from_clipboard() {
                    logger::error(format!("Paste to editor failed: {}", e));
                }
            } else if let Some(terminal) = panel.as_terminal_mut() {
                if let Err(e) = terminal.paste_from_clipboard() {
                    logger::error(format!("Paste to terminal failed: {}", e));
                }
            }
        }
        Ok(())
    }

    /// Handle OpenFile event - open file in editor
    fn event_open_file(&mut self, file_path: PathBuf) -> Result<()> {
        self.close_welcome_panels();
        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?");
        let t = i18n::t();
        logger::info(format!("Opening file via event: {}", filename));

        match Editor::open_file_with_config(file_path.clone(), self.state.editor_config()) {
            Ok(editor_panel) => {
                self.add_panel(Box::new(editor_panel));
                self.auto_save_session();
                logger::info(format!("File '{}' opened in editor", filename));
                self.state.set_info(t.editor_file_opened(filename));
            }
            Err(e) => {
                let error_msg = t.status_error_open_file(filename, &e.to_string());
                logger::error(format!("Error opening '{}': {}", filename, e));
                self.state.set_error(error_msg);
            }
        }
        Ok(())
    }

    /// Handle GotoLine event - move cursor to specific line in editor
    fn event_goto_line(&mut self, line: usize) {
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                // Convert from 1-based (user-facing) to 0-based (internal)
                let line_0based = line.saturating_sub(1);
                editor.set_cursor_line(line_0based);
                logger::debug(format!("Moved to line {}", line));
            }
        }
    }

    /// Handle NavigateTo event - navigate file manager to path
    fn event_navigate_to(&mut self, path: PathBuf) -> Result<()> {
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(fm) = panel.as_file_manager_mut() {
                if let Err(e) = fm.navigate_to(path.clone()) {
                    logger::error(format!("Navigation failed: {}", e));
                    self.state
                        .set_error(format!("Cannot navigate to: {}", path.display()));
                }
            }
        }
        Ok(())
    }

    /// Handle SaveFile event - save file at given path
    fn event_save_file(&mut self, path: PathBuf) -> Result<()> {
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                match editor.save_file_as(path.clone()) {
                    Ok(()) => {
                        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                        self.state.set_info(format!("Saved: {}", filename));
                    }
                    Err(e) => {
                        logger::error(format!("Save failed: {}", e));
                        self.state.set_error(format!("Save failed: {}", e));
                    }
                }
            }
        }
        Ok(())
    }

    /// Handle WatchPath event - register path with file watcher
    fn event_watch_path(&mut self, path: PathBuf) {
        if let Some(watcher) = &mut self.state.fs_watcher {
            if path.is_dir() {
                // Check if it's a git repo
                if termide_git::find_repo_root(&path).is_some() {
                    if let Err(e) = watcher.watch_repository(path.clone()) {
                        logger::error(format!(
                            "Failed to watch repository {}: {}",
                            path.display(),
                            e
                        ));
                    }
                } else if let Err(e) = watcher.watch_directory(path.clone()) {
                    logger::error(format!(
                        "Failed to watch directory {}: {}",
                        path.display(),
                        e
                    ));
                }
            }
        }
    }

    /// Handle RefreshGitStatus event - refresh git status for panels in path
    fn event_refresh_git_status(&mut self, path: PathBuf) {
        // Reload FileManagers whose current path starts with the given path
        for panel in self.layout_manager.iter_all_panels_mut() {
            if let Some(fm) = panel.as_file_manager_mut() {
                if fm.current_path().starts_with(&path) || path.starts_with(fm.current_path()) {
                    let _ = fm.reload_directory();
                }
            }
        }
    }

    /// Handle UnwatchPath event - unregister path from file watcher
    fn event_unwatch_path(&mut self, path: PathBuf) {
        if let Some(watcher) = &mut self.state.fs_watcher {
            if termide_git::find_repo_root(&path).is_some() {
                watcher.unwatch_repository(&path);
            } else {
                watcher.unwatch_directory(&path);
            }
        }
    }

    /// Handle ShowConflict event - show file conflict resolution modal
    fn event_show_conflict(&mut self, source: PathBuf, destination: PathBuf, remaining: usize) {
        use crate::state::{ActiveModal, BatchOperation, BatchOperationType, PendingAction};
        use termide_modal::ConflictModal;

        // Create a minimal batch operation for conflict resolution
        let operation = BatchOperation::new(
            BatchOperationType::Copy, // Default to copy, actual type determined by context
            vec![source.clone()],
            destination.parent().unwrap_or(&destination).to_path_buf(),
        );

        let modal = ConflictModal::new(&source, &destination, remaining);
        self.state.set_pending_action(
            PendingAction::ContinueBatchOperation { operation },
            ActiveModal::Conflict(Box::new(modal)),
        );
    }

    /// Handle ShowSelect event - show selection modal
    fn event_show_select(
        &mut self,
        title: String,
        options: Vec<String>,
        on_select: termide_core::SelectAction,
    ) {
        use crate::state::{ActiveModal, PendingAction};
        use termide_modal::SelectModal;

        // Map SelectAction to PendingAction
        let pending_action = match on_select {
            termide_core::SelectAction::SelectTheme => {
                // Theme selection is handled differently
                return;
            }
            termide_core::SelectAction::SelectLanguage => {
                // Language selection is handled differently
                return;
            }
            termide_core::SelectAction::SelectEncoding => {
                // Encoding selection is handled differently
                return;
            }
            termide_core::SelectAction::CloseEditorChoice => {
                PendingAction::CloseEditorWithSave { panel_index: 0 }
            }
            termide_core::SelectAction::Custom(_) => {
                // Custom actions not yet supported
                return;
            }
        };

        let modal = SelectModal::single(title, "", options);
        self.state
            .set_pending_action(pending_action, ActiveModal::Select(Box::new(modal)));
    }

    /// Handle ShowSearch event - show search modal
    fn event_show_search(&mut self, _initial_query: Option<String>) {
        use crate::state::{ActiveModal, PendingAction};
        use termide_modal::SearchModal;

        // Note: SearchModal doesn't support initial query yet
        let modal = SearchModal::new("");

        self.state
            .set_pending_action(PendingAction::Search, ActiveModal::Search(Box::new(modal)));
    }

    /// Handle ShowReplace event - show replace modal
    fn event_show_replace(&mut self, _find: Option<String>, _replace: Option<String>) {
        use crate::state::{ActiveModal, PendingAction};
        use termide_modal::ReplaceModal;

        // Note: ReplaceModal doesn't support initial values yet
        let modal = ReplaceModal::new();

        self.state.set_pending_action(
            PendingAction::Replace,
            ActiveModal::Replace(Box::new(modal)),
        );
    }

    /// Handle ShowInput event - show input modal
    fn event_show_input(
        &mut self,
        prompt: String,
        initial_value: String,
        on_submit: termide_core::InputAction,
    ) {
        use crate::state::{ActiveModal, PendingAction};
        use termide_modal::InputModal;

        // Map InputAction to PendingAction
        let pending_action = match &on_submit {
            termide_core::InputAction::RenameFile { from } => PendingAction::MovePath {
                panel_index: 0,
                sources: vec![from.clone()],
                target_directory: from.parent().map(|p| p.to_path_buf()),
            },
            termide_core::InputAction::CreateFile { in_dir } => PendingAction::CreateFile {
                panel_index: 0,
                directory: in_dir.clone(),
            },
            termide_core::InputAction::CreateDirectory { in_dir } => {
                PendingAction::CreateDirectory {
                    panel_index: 0,
                    directory: in_dir.clone(),
                }
            }
            termide_core::InputAction::SearchInFile => PendingAction::Search,
            termide_core::InputAction::SearchReplace => PendingAction::Replace,
            termide_core::InputAction::GotoLine => {
                // GotoLine is handled directly, not through modal
                return;
            }
            termide_core::InputAction::SaveFileAs { directory } => PendingAction::SaveFileAs {
                panel_index: 0,
                directory: directory.clone(),
            },
            termide_core::InputAction::CopyTo { sources } => PendingAction::CopyPath {
                panel_index: 0,
                sources: sources.clone(),
                target_directory: None,
            },
            termide_core::InputAction::MoveTo { sources } => PendingAction::MovePath {
                panel_index: 0,
                sources: sources.clone(),
                target_directory: None,
            },
        };

        // Create input modal
        let modal = InputModal::with_default("Input", prompt, &initial_value);
        self.state
            .set_pending_action(pending_action, ActiveModal::Input(Box::new(modal)));
    }

    /// Handle ShowConfirm event - show confirmation modal
    fn event_show_confirm(&mut self, message: String, on_confirm: termide_core::ConfirmAction) {
        use crate::state::{ActiveModal, PendingAction};
        use termide_modal::ConfirmModal;

        // Map ConfirmAction to PendingAction
        let pending_action = match on_confirm {
            termide_core::ConfirmAction::DeleteFile(path) => PendingAction::DeletePath {
                panel_index: 0,
                paths: vec![path],
            },
            termide_core::ConfirmAction::DeletePaths(paths) => PendingAction::DeletePath {
                panel_index: 0,
                paths,
            },
            termide_core::ConfirmAction::DeleteDirectory(path) => PendingAction::DeletePath {
                panel_index: 0,
                paths: vec![path],
            },
            termide_core::ConfirmAction::DiscardChanges(_path) => {
                PendingAction::ClosePanel { panel_index: 0 }
            }
            termide_core::ConfirmAction::CloseWithoutSaving => {
                PendingAction::CloseEditorWithSave { panel_index: 0 }
            }
            termide_core::ConfirmAction::QuitApplication => PendingAction::QuitApplication,
            termide_core::ConfirmAction::OverwriteFile { .. } => {
                // This case is handled by the conflict modal, not confirm
                return;
            }
        };

        // Create confirmation modal
        let t = i18n::t();
        let modal = ConfirmModal::new(t.modal_yes(), message);
        self.state
            .set_pending_action(pending_action, ActiveModal::Confirm(Box::new(modal)));
    }

    /// Handle SplitPanel event - toggle panel stacking/splitting
    fn event_split_panel(&mut self, direction: termide_core::SplitDirection) {
        let terminal_width = self.state.terminal.width;

        match direction {
            termide_core::SplitDirection::Horizontal => {
                // Horizontal split: create new column (unstack if multiple panels in group)
                if let Err(e) = self.layout_manager.toggle_panel_stacking(terminal_width) {
                    logger::debug(format!("Split failed: {}", e));
                }
            }
            termide_core::SplitDirection::Vertical => {
                // Vertical split: stack in same column (merge if single panel)
                if let Err(e) = self.layout_manager.toggle_panel_stacking(terminal_width) {
                    logger::debug(format!("Stack failed: {}", e));
                }
            }
        }
    }

    /// Handle FocusPanel event - focus panel by name/title
    fn event_focus_panel(&mut self, name: &str) {
        // First, find the matching panel indices
        let mut found: Option<(usize, usize, String)> = None;
        for (group_idx, group) in self.layout_manager.panel_groups.iter().enumerate() {
            for (panel_idx, panel) in group.panels().iter().enumerate() {
                if panel.title().contains(name) {
                    found = Some((group_idx, panel_idx, panel.title().to_string()));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }

        // Then, apply the focus change
        if let Some((group_idx, panel_idx, title)) = found {
            if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                group.set_expanded(panel_idx);
            }
            self.layout_manager.focus = group_idx;
            logger::debug(format!("Focused panel: {}", title));
        } else {
            logger::debug(format!("Panel not found: {}", name));
        }
    }
}
