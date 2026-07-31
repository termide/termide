//! Panel operations: movement, resize, and close handling.
//!
//! Handles panel manipulation including stacking, swapping, and resizing.

use anyhow::Result;

use super::App;
use crate::state::{ActiveModal, PendingAction};
use termide_core::{CommandResult, PanelCommand};
use termide_i18n as i18n;

impl App {
    /// Handle panel close request with confirmation if needed
    pub(crate) fn handle_close_panel_request(&mut self) -> Result<()> {
        // Check if confirmation is required before closing active panel
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(_message) = panel.needs_close_confirmation() {
                log::warn!("Close requested for panel requiring confirmation");
                // Check modification status via command (works for Editor panels)
                let mod_status = panel.handle_command(PanelCommand::GetModificationStatus);
                if let CommandResult::ModificationStatus {
                    is_modified,
                    has_external_change: has_external,
                } = mod_status
                {
                    use termide_modal::ChoiceModal;
                    let t = i18n::t();

                    if is_modified && has_external {
                        // Conflict: both local and external changes
                        let modal = ChoiceModal::new(
                            t.editor_close_conflict(),
                            Some(t.editor_close_conflict_question().to_string()),
                            vec![
                                t.editor_overwrite_disk().to_string(),
                                t.editor_reload_from_disk().to_string(),
                                t.editor_cancel().to_string(),
                            ],
                        );
                        let action = PendingAction::CloseEditorConflict;
                        self.state
                            .set_pending_action(action, ActiveModal::Choice(Box::new(modal)));
                        return Ok(());
                    } else if is_modified {
                        // Only local changes
                        let modal = ChoiceModal::new(
                            t.editor_close_unsaved(),
                            Some(t.editor_close_unsaved_question().to_string()),
                            vec![
                                t.editor_save_and_close().to_string(),
                                t.editor_close_without_saving().to_string(),
                                t.editor_cancel().to_string(),
                            ],
                        );
                        let action = PendingAction::CloseEditorWithSave;
                        self.state
                            .set_pending_action(action, ActiveModal::Choice(Box::new(modal)));
                        return Ok(());
                    } else if has_external {
                        // Only external changes
                        let modal = ChoiceModal::new(
                            t.editor_close_external(),
                            Some(t.editor_close_external_question().to_string()),
                            vec![
                                t.editor_overwrite_disk().to_string(),
                                t.editor_keep_disk_close().to_string(),
                                t.editor_reload_into_editor().to_string(),
                                t.editor_cancel().to_string(),
                            ],
                        );
                        let action = PendingAction::CloseEditorExternal;
                        self.state
                            .set_pending_action(action, ActiveModal::Choice(Box::new(modal)));
                        return Ok(());
                    }
                } else {
                    // For other panels show simple confirmation
                    let t = i18n::t();
                    let modal =
                        termide_modal::ConfirmModal::new(t.modal_confirm_title(), &_message);
                    let action = PendingAction::ClosePanel;
                    self.state
                        .set_pending_action(action, ActiveModal::Confirm(Box::new(modal)));
                    return Ok(());
                }
            }
        }

        // Close active panel without confirmation
        self.close_panel_at_index();
        Ok(())
    }

    /// Handle Escape-triggered close: always shows confirmation.
    /// Unlike F10/Alt+X which closes simple panels immediately,
    /// Escape always asks because it's too easy to press accidentally.
    pub(crate) fn handle_escape_close_request(&mut self) -> Result<()> {
        // Delegate to normal close request — it already handles:
        // - Editor with unsaved changes → save/discard/cancel dialog
        // - Terminal with running process → confirm dialog
        // For panels without needs_close_confirmation, show simple confirm
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if panel.needs_close_confirmation().is_some() {
                // Use existing confirmation logic (save dialog, etc.)
                // (panel borrow ends here, handle_close_panel_request will re-borrow)
            } else {
                // Simple confirmation for panels without special needs
                let panel_title = panel.title();
                let t = i18n::t();
                let message = format!("{} \"{}\"?", t.help_desc_close_panel(), panel_title);
                let modal = termide_modal::ConfirmModal::new(t.modal_confirm_title(), &message);
                self.state.set_pending_action(
                    PendingAction::ClosePanel,
                    ActiveModal::Confirm(Box::new(modal)),
                );
                return Ok(());
            }
        }

        // Panel has needs_close_confirmation — delegate to standard close flow
        self.handle_close_panel_request()
    }

    /// Close all Operations panels (called when no active operations remain)
    pub(super) fn close_operations_panel(&mut self) {
        let mut groups_to_remove = Vec::new();

        for group_idx in (0..self.layout_manager.panel_groups.len()).rev() {
            if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                let mut panels_to_remove = Vec::new();

                for panel_idx in (0..group.len()).rev() {
                    if let Some(panel) = group.panels().get(panel_idx) {
                        if panel.name() == "operations" {
                            panels_to_remove.push(panel_idx);
                        }
                    }
                }

                for panel_idx in panels_to_remove {
                    group.remove_panel(panel_idx);
                }

                if group.is_empty() {
                    groups_to_remove.push(group_idx);
                }
            }
        }

        let groups_were_removed = !groups_to_remove.is_empty();
        for group_idx in groups_to_remove {
            self.layout_manager.panel_groups.remove(group_idx);
        }

        if !self.layout_manager.panel_groups.is_empty()
            && self.layout_manager.focus >= self.layout_manager.panel_groups.len()
        {
            self.layout_manager.focus = self.layout_manager.panel_groups.len() - 1;
        }

        if groups_were_removed {
            let terminal_width = self.state.terminal.width;
            self.layout_manager
                .redistribute_widths_proportionally(terminal_width);
        }

        self.auto_save_session();
    }

    /// Close all Help panels (called before opening new panel)
    pub(super) fn close_help_panels(&mut self) {
        let mut groups_to_remove = Vec::new();

        for group_idx in (0..self.layout_manager.panel_groups.len()).rev() {
            if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                let mut panels_to_remove = Vec::new();

                for panel_idx in (0..group.len()).rev() {
                    if let Some(panel) = group.panels().get(panel_idx) {
                        if panel.is_help_panel() {
                            panels_to_remove.push(panel_idx);
                        }
                    }
                }

                for panel_idx in panels_to_remove {
                    group.remove_panel(panel_idx);
                }

                if group.is_empty() {
                    groups_to_remove.push(group_idx);
                }
            }
        }

        let groups_were_removed = !groups_to_remove.is_empty();
        for group_idx in groups_to_remove {
            self.layout_manager.panel_groups.remove(group_idx);
        }

        if !self.layout_manager.panel_groups.is_empty()
            && self.layout_manager.focus >= self.layout_manager.panel_groups.len()
        {
            self.layout_manager.focus = self.layout_manager.panel_groups.len() - 1;
        }

        if groups_were_removed {
            let terminal_width = self.state.terminal.width;
            self.layout_manager
                .redistribute_widths_proportionally(terminal_width);
        }
    }

    /// Alt+PageUp: move panel up in group, or move group left if at top
    pub(super) fn handle_swap_panel_left(&mut self) -> Result<()> {
        let terminal_width = self.state.terminal.width;
        let active_group_idx = self.layout_manager.focus;

        if let Some(group) = self.layout_manager.panel_groups.get(active_group_idx) {
            if group.len() == 1 {
                self.layout_manager
                    .move_panel_to_prev_group(terminal_width)?;
            } else {
                let expanded_idx = group.expanded_index();
                if expanded_idx == 0 {
                    self.layout_manager
                        .move_panel_to_prev_group(terminal_width)?;
                } else {
                    self.layout_manager.move_panel_up_in_group()?;
                }
            }
        }

        self.auto_save_session();
        Ok(())
    }

    /// Alt+PageDown: move panel down in group, or move group right if at bottom
    pub(super) fn handle_swap_panel_right(&mut self) -> Result<()> {
        let terminal_width = self.state.terminal.width;
        let active_group_idx = self.layout_manager.focus;

        if let Some(group) = self.layout_manager.panel_groups.get(active_group_idx) {
            if group.len() == 1 {
                self.layout_manager
                    .move_panel_to_next_group(terminal_width)?;
            } else {
                let expanded_idx = group.expanded_index();
                if expanded_idx >= group.len() - 1 {
                    self.layout_manager
                        .move_panel_to_next_group(terminal_width)?;
                } else {
                    self.layout_manager.move_panel_down_in_group()?;
                }
            }
        }

        self.auto_save_session();
        Ok(())
    }

    /// Change active group width
    pub(super) fn handle_resize_panel(&mut self, delta: i16) -> Result<()> {
        if let Some(group_idx) = self.layout_manager.active_group_index() {
            if self.layout_manager.panel_groups.len() <= 1 {
                return Ok(());
            }

            let terminal_width = self.state.terminal.width;
            let available_width = terminal_width;
            let min_width = self.state.config.general.min_panel_width as i16;

            // Freeze all auto-width groups before resize
            let actual_widths = self.layout_manager.calculate_actual_widths(available_width);
            for (idx, group) in self.layout_manager.panel_groups.iter_mut().enumerate() {
                if group.width.is_none() {
                    group.width = Some(actual_widths.get(idx).copied().unwrap_or(min_width as u16));
                }
            }

            let current_width = self.layout_manager.panel_groups[group_idx]
                .width
                .unwrap_or(min_width as u16);
            let desired_new_width = ((current_width as i16 + delta).clamp(min_width, 300)) as u16;
            let actual_delta = desired_new_width as i16 - current_width as i16;

            if actual_delta == 0 {
                return Ok(());
            }

            // Collect other groups with their widths
            let other_groups: Vec<(usize, u16)> = self
                .layout_manager
                .panel_groups
                .iter()
                .enumerate()
                .filter(|(idx, _)| *idx != group_idx)
                .map(|(idx, g)| (idx, g.width.unwrap_or(min_width as u16)))
                .collect();

            let total_other_width: u16 = other_groups.iter().map(|(_, w)| *w).sum();

            if total_other_width == 0 {
                return Ok(());
            }

            // Distribute delta proportionally across other groups
            let mut remaining_delta = -actual_delta;
            let mut new_widths: Vec<(usize, u16)> = Vec::new();

            for (i, &(idx, width)) in other_groups.iter().enumerate() {
                let is_last = i == other_groups.len() - 1;

                let delta_for_this = if is_last {
                    remaining_delta
                } else {
                    let proportion = width as f64 / total_other_width as f64;
                    ((-actual_delta as f64) * proportion).round() as i16
                };

                let new_width = ((width as i16 + delta_for_this).clamp(min_width, 300)) as u16;
                new_widths.push((idx, new_width));

                let actual_change = new_width as i16 - width as i16;
                remaining_delta -= actual_change;
            }

            // Apply new widths
            self.layout_manager.panel_groups[group_idx].width = Some(desired_new_width);

            for (idx, new_width) in new_widths {
                self.layout_manager.panel_groups[idx].width = Some(new_width);
            }

            // Correct balance if clamping broke zero-sum
            let total_new_width: u16 = self
                .layout_manager
                .panel_groups
                .iter()
                .map(|g| g.width.unwrap_or(min_width as u16))
                .sum();

            if total_new_width != available_width {
                let other_widths_sum: u16 = self
                    .layout_manager
                    .panel_groups
                    .iter()
                    .enumerate()
                    .filter(|(idx, _)| *idx != group_idx)
                    .map(|(_, g)| g.width.unwrap_or(min_width as u16))
                    .sum();

                let corrected_width = available_width.saturating_sub(other_widths_sum);
                self.layout_manager.panel_groups[group_idx].width =
                    Some(corrected_width.clamp(min_width as u16, 300));
            }
            self.auto_save_session();
        }
        Ok(())
    }

    /// Handle switch session modal result
    pub(super) fn handle_switch_session(&mut self, value: Box<dyn std::any::Any>) -> Result<()> {
        use termide_modal::{ConfirmModal, SessionAction};

        if let Some(action) = value.downcast_ref::<SessionAction>() {
            match action {
                SessionAction::Switch(path) => {
                    self.switch_to_session(path.clone())?;
                }
                SessionAction::Delete(path) => {
                    let display = path.display().to_string();
                    let modal = ConfirmModal::new("Delete session?", format!("Session: {display}"));
                    self.state.set_pending_action(
                        PendingAction::DeleteSession { path: path.clone() },
                        ActiveModal::Confirm(Box::new(modal)),
                    );
                }
            }
        }
        Ok(())
    }

    /// Switch to a different session
    fn switch_to_session(&mut self, new_project_root: std::path::PathBuf) -> Result<()> {
        // 1. Save current session
        self.auto_save_session();

        // 2. Change working directory
        std::env::set_current_dir(&new_project_root)?;
        log::info!("Changed working directory to: {:?}", new_project_root);

        // 3. Update project_root
        self.project_root = new_project_root;
        self.state.project_root = self.project_root.clone();
        self.state.project_bookmarks =
            termide_config::BookmarksConfig::load_from_project(&self.project_root);

        // Invalidate caches that depend on the project root: project-local
        // commands.toml lives under `<project_root>/.termide/`, so both the
        // commands registry and the global hotkey table (which folds
        // command hotkeys in) must rebuild for the new project.
        self.state.cache.commands_registry = None;
        self.state.cache.hotkey_table = None;

        // 4. Load new session
        self.load_session()?;

        // 5. Update terminal title to reflect new project root
        self.update_terminal_title();

        Ok(())
    }

    /// Handle confirmed session deletion
    pub(super) fn handle_delete_session(&mut self, path: &std::path::Path) -> Result<()> {
        if let Err(e) = termide_session::Session::delete_session(path) {
            log::error!("Failed to delete session for {:?}: {}", path, e);
            self.show_error_modal(format!("Failed to delete session: {e}"));
        } else {
            log::info!("Deleted session for {:?}", path);
        }
        // Reopen sessions modal with updated list
        self.handle_open_sessions_modal()?;
        Ok(())
    }

    /// Handle new session modal result - create/switch to session in selected directory
    pub(super) fn handle_new_session_result(
        &mut self,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(project_path) = value.downcast_ref::<std::path::PathBuf>() {
            self.create_new_session(project_path.clone())?;
        }
        Ok(())
    }

    /// Create a new session in the specified directory
    /// If a session already exists, it will be cleared (reset to default panels)
    fn create_new_session(&mut self, new_project_root: std::path::PathBuf) -> Result<()> {
        use termide_panel_file_manager::FileManager;
        use termide_session::Session;

        // 1. Save current session before switching
        self.auto_save_session();

        // 2. Clear any existing session in the target directory
        if let Ok(session_dir) = Session::get_session_dir(&new_project_root) {
            // Remove session file if it exists (this clears the session)
            let session_file = session_dir.join("session.toml");
            if session_file.exists() {
                if let Err(e) = std::fs::remove_file(&session_file) {
                    log::error!(
                        "Failed to remove session file {}: {}",
                        session_file.display(),
                        e
                    );
                } else {
                    log::info!("Cleared existing session in: {:?}", new_project_root);
                }
            }
        }

        // 3. Change working directory
        std::env::set_current_dir(&new_project_root)?;
        log::info!("Changed working directory to: {:?}", new_project_root);

        // 4. Update project_root
        self.project_root = new_project_root.clone();
        self.state.project_root = self.project_root.clone();
        self.state.project_bookmarks =
            termide_config::BookmarksConfig::load_from_project(&self.project_root);

        // Invalidate project-root-dependent caches (see switch_to_session).
        self.state.cache.commands_registry = None;
        self.state.cache.hotkey_table = None;

        // 5. Create fresh layout with default panels (2 FileManagers)
        self.layout_manager = termide_layout::LayoutManager::new();
        let fm1 = FileManager::new_with_path(new_project_root.clone());
        let fm2 = FileManager::new_with_path(new_project_root);
        self.add_panel(Box::new(fm1));
        self.add_panel(Box::new(fm2));

        // 6. Save the new session
        self.auto_save_session();

        // 7. Update terminal title to reflect new project root
        self.update_terminal_title();

        let t = termide_i18n::t();
        self.state.set_info(t.session_created().to_string());

        Ok(())
    }

    /// Update terminal window title to reflect current project root.
    fn update_terminal_title(&self) {
        let path = self.project_root.display().to_string();
        let title = format!("Termide: {}", termide_core::util::shorten_home_path(&path));
        if let Err(e) = crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle(title))
        {
            log::debug!("failed to set terminal title: {e}");
        }
    }

    /// Handle change root path modal result - move session to new directory
    pub(super) fn handle_change_root_path_result(
        &mut self,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(new_path) = value.downcast_ref::<std::path::PathBuf>() {
            self.move_session_to(new_path.clone())?;
        }
        Ok(())
    }

    /// Move current session to a new directory
    fn move_session_to(&mut self, new_project_root: std::path::PathBuf) -> Result<()> {
        use termide_session::Session;

        let old_project_root = self.project_root.clone();

        // Don't do anything if same directory
        if old_project_root == new_project_root {
            return Ok(());
        }

        // 1. Save current session
        self.auto_save_session();

        // 2. Copy all session data to new location (including unsaved buffers)
        if let Ok(old_session_dir) = Session::get_session_dir(&old_project_root) {
            if let Ok(new_session_dir) = Session::get_session_dir(&new_project_root) {
                // Create new session directory if needed
                if let Err(e) = std::fs::create_dir_all(&new_session_dir) {
                    log::error!(
                        "Failed to create session directory {}: {}",
                        new_session_dir.display(),
                        e
                    );
                }

                // Copy all files from old session directory
                if let Ok(entries) = std::fs::read_dir(&old_session_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_file() {
                            if let Some(filename) = path.file_name() {
                                let new_path = new_session_dir.join(filename);
                                if let Err(e) = std::fs::copy(&path, &new_path) {
                                    log::error!(
                                        "Failed to copy session file {} -> {}: {}",
                                        path.display(),
                                        new_path.display(),
                                        e
                                    );
                                }
                            }
                        }
                    }
                }

                // Remove old session directory
                if let Err(e) = std::fs::remove_dir_all(&old_session_dir) {
                    log::warn!(
                        "Failed to remove old session directory {}: {}",
                        old_session_dir.display(),
                        e
                    );
                }
            }
        }

        // 3. Change working directory
        std::env::set_current_dir(&new_project_root)?;
        log::info!(
            "Moved session from {:?} to {:?}",
            old_project_root,
            new_project_root
        );

        // 4. Update project_root
        self.project_root = new_project_root;
        self.state.project_root = self.project_root.clone();
        self.state.project_bookmarks =
            termide_config::BookmarksConfig::load_from_project(&self.project_root);

        // Invalidate project-root-dependent caches (see switch_to_session).
        self.state.cache.commands_registry = None;
        self.state.cache.hotkey_table = None;

        // 5. Save session in new location
        self.auto_save_session();

        let t = termide_i18n::t();
        self.state.set_info(t.session_moved().to_string());

        Ok(())
    }

    /// Open a file in a new editor panel with LSP initialization.
    ///
    /// This is the core helper for opening files. It handles:
    /// - Creating the editor with configuration
    /// - Initializing LSP for the editor
    /// - Adding the panel to layout
    /// - Auto-saving session
    ///
    /// Returns Ok(()) on success, or sets an error message and returns Err on failure.
    /// Use this instead of duplicating Editor::open_file_with_config patterns.
    pub(crate) fn open_editor_for_file(&mut self, file_path: std::path::PathBuf) -> Result<()> {
        use termide_panel_editor::Editor;

        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();

        match Editor::open_file_with_config(file_path, self.state.editor_config()) {
            Ok(mut editor_panel) => {
                // Initialize LSP for the editor
                if let Some(ref mut lsp_manager) = self.state.lsp_manager {
                    editor_panel.init_lsp(lsp_manager);
                }

                self.add_panel(Box::new(editor_panel));
                self.notify_outline_file_opened();
                self.auto_save_session();

                let t = i18n::t();
                self.state.set_info(t.editor_file_opened(&filename));
                Ok(())
            }
            Err(e) => {
                let t = i18n::t();
                let error_msg = t.status_error_open_file(&filename, &e.to_string());
                self.show_error_modal(error_msg.clone());
                anyhow::bail!(error_msg)
            }
        }
    }

    /// Open a file in read-only (view) mode.
    ///
    /// Similar to `open_editor_for_file` but uses `EditorConfig::view_only()`.
    pub(crate) fn open_editor_for_file_readonly(
        &mut self,
        file_path: std::path::PathBuf,
    ) -> Result<()> {
        use termide_panel_editor::{Editor, EditorConfig};

        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();

        match Editor::open_file_with_config(file_path, EditorConfig::view_only()) {
            Ok(mut editor_panel) => {
                // Initialize LSP for the editor
                if let Some(ref mut lsp_manager) = self.state.lsp_manager {
                    editor_panel.init_lsp(lsp_manager);
                }

                self.add_panel(Box::new(editor_panel));
                self.notify_outline_file_opened();
                self.auto_save_session();

                let t = i18n::t();
                self.state.set_info(t.editor_file_opened(&filename));
                Ok(())
            }
            Err(e) => {
                let t = i18n::t();
                let error_msg = t.status_error_open_file(&filename, &e.to_string());
                self.show_error_modal(error_msg.clone());
                anyhow::bail!(error_msg)
            }
        }
    }

    /// Handle switch directory modal result - change active panel's working directory
    pub(super) fn handle_switch_directory(&mut self, value: Box<dyn std::any::Any>) -> Result<()> {
        use crate::panel_ext::PanelExt;

        if let Some(path) = value.downcast_ref::<std::path::PathBuf>() {
            let t = i18n::t();

            // Get active panel and switch based on panel type
            if let Some(panel) = self.layout_manager.active_panel_mut() {
                // Try as FileManager
                if let Some(file_manager) = panel.as_file_manager_mut() {
                    let _ = file_manager.navigate_to(path.clone());
                    self.state.needs_watcher_registration = true;
                    self.state
                        .set_info(format!("Switched to: {}", path.display()));
                    return Ok(());
                }

                // Try as Terminal
                if let Some(terminal) = panel.as_terminal_mut() {
                    // Shell-escape the path for cd command
                    // Simple escaping: wrap in single quotes, escape existing single quotes
                    let path_str = path.to_string_lossy();
                    let escaped_path = format!("'{}'", path_str.replace('\'', "'\\''"));
                    let cd_command = format!("cd {}\n", escaped_path);
                    let _ = terminal.send_command(&cd_command);
                    self.state.set_info(format!("cd {}", path.display()));
                    return Ok(());
                }

                // Unsupported panel type (Editor, etc.)
                self.state
                    .set_info(t.directory_switcher_unsupported().to_string());
            }
        }
        Ok(())
    }

    /// Create a new terminal panel with the calculated dimensions.
    ///
    /// This is the core helper for creating terminal panels. It handles:
    /// - Calculating terminal dimensions from app state
    /// - Creating the terminal with PTY
    ///
    /// Returns Ok(terminal) on success, or sets an error message and returns Err on failure.
    /// The returned terminal can be used to send commands if needed.
    /// Caller is responsible for adding the panel to layout.
    pub(crate) fn create_terminal_panel(
        &mut self,
        cwd: Option<std::path::PathBuf>,
    ) -> Result<termide_panel_terminal::Terminal> {
        use termide_panel_terminal::Terminal;

        let width = self.state.terminal.width;
        let height = self.state.terminal.height;
        let term_height = height.saturating_sub(3);
        let term_width = width.saturating_sub(2);

        match Terminal::new_with_cwd(term_height, term_width, cwd) {
            Ok(terminal) => Ok(terminal),
            Err(e) => {
                let error_msg = format!("Failed to create terminal: {}", e);
                log::error!("{}", error_msg);
                self.show_error_modal(error_msg.clone());
                anyhow::bail!(error_msg)
            }
        }
    }
}
