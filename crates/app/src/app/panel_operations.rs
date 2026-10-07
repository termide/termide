//! Panel operations: movement, resize, and close handling.
//!
//! Handles panel manipulation including stacking, swapping, and resizing.

use anyhow::Result;

use super::App;
use crate::state::{ActiveModal, PendingAction, ProjectsOrigin};
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
    /// and hand back the one closed, for a project switch to carry it over.
    pub(super) fn close_operations_panel(&mut self) -> Option<Box<dyn termide_core::Panel>> {
        let mut closed = None;
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
                    closed = group.remove_panel(panel_idx).or(closed);
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

        if closed.is_some() {
            self.auto_save_layout();
        }
        closed
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

        self.auto_save_layout();
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

        self.auto_save_layout();
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
            self.auto_save_layout();
        }
        Ok(())
    }

    /// Handle the projects modal result
    pub(super) fn handle_switch_project(&mut self, value: Box<dyn std::any::Any>) -> Result<()> {
        use termide_modal::ProjectAction;

        if let Some(action) = value.downcast_ref::<ProjectAction>() {
            match action {
                ProjectAction::Switch(path) => {
                    self.switch_to_project(path.clone())?;
                }
                ProjectAction::Close(path) => {
                    self.confirm_close_project(path.clone(), ProjectsOrigin::Switcher)
                }
                ProjectAction::Delete(path) => {
                    self.confirm_delete_project(path.clone(), ProjectsOrigin::Switcher)
                }
            }
        }
        Ok(())
    }

    /// Switch to a different project. The project left stays open in the
    /// background: its panels are parked, not dropped.
    pub(super) fn switch_to_project(&mut self, new_project_root: std::path::PathBuf) -> Result<()> {
        let new_project_root = super::parked_projects::project_root_of(new_project_root);
        if new_project_root == self.project_root {
            return Ok(());
        }
        let entered = self.enter_project(new_project_root)?;
        match entered.restored {
            Some(parked) => self.restore_parked_project(parked),
            None => {
                if let Err(e) = self.load_layout() {
                    log::warn!(
                        "Could not load the project layout ({e}); starting with the default layout."
                    );
                    self.setup_default_layout();
                }
            }
        }
        self.carry_operations_panel(entered.operations_panel);
        self.update_terminal_title();
        self.sync_open_projects();
        Ok(())
    }

    /// Ask before deleting the saved layout of the project at `path`, then
    /// return `from` where it was asked.
    pub(super) fn confirm_delete_project(
        &mut self,
        path: std::path::PathBuf,
        from: ProjectsOrigin,
    ) {
        let t = termide_i18n::t();
        let message = t.projects_delete_fmt(&termide_core::util::shorten_home_path(
            &path.display().to_string(),
        ));
        let modal = termide_modal::ConfirmModal::new(t.projects_delete_title(), message);
        self.state.set_pending_action(
            PendingAction::DeleteProject { path, from },
            ActiveModal::Confirm(Box::new(modal)),
        );
    }

    pub(super) fn handle_delete_project(
        &mut self,
        path: &std::path::Path,
        from: ProjectsOrigin,
    ) -> Result<()> {
        if let Err(e) = termide_project::ProjectLayout::delete_layout(path) {
            log::error!("Failed to delete project layout for {:?}: {}", path, e);
            // Keep the error on screen instead of reopening over it.
            self.show_error_modal(i18n::t().projects_delete_failed_fmt(&e.to_string()));
            return Ok(());
        }
        log::info!("Deleted project layout for {:?}", path);
        self.return_to_projects(from)
    }

    /// Handle new project modal result - create/switch to a project in selected directory
    pub(super) fn handle_new_project_result(
        &mut self,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(project_path) = value.downcast_ref::<std::path::PathBuf>() {
            self.create_new_project(project_path.clone())?;
        }
        Ok(())
    }

    /// Create a new project in the specified directory
    /// If it already has a saved layout, that is cleared (reset to default panels).
    /// A directory that is already open as a project is switched to instead:
    /// resetting it would stop what runs in its panels.
    fn create_new_project(&mut self, new_project_root: std::path::PathBuf) -> Result<()> {
        use termide_panel_file_manager::FileManager;
        use termide_project::ProjectLayout;

        let new_project_root = super::parked_projects::project_root_of(new_project_root);
        if new_project_root == self.project_root {
            // Resetting it would stop what runs in its panels.
            self.state
                .set_info(termide_i18n::t().projects_already_current().to_string());
            return Ok(());
        }
        if self.open_projects.is_open(&new_project_root) {
            return self.switch_to_project(new_project_root);
        }

        // 1. Clear any existing layout of the target directory
        if let Ok(project_dir) = ProjectLayout::get_project_dir(&new_project_root) {
            // Remove the layout file if it exists (this clears the layout)
            let layout_file = project_dir.join("session.toml");
            if layout_file.exists() {
                if let Err(e) = std::fs::remove_file(&layout_file) {
                    log::error!(
                        "Failed to remove layout file {}: {}",
                        layout_file.display(),
                        e
                    );
                } else {
                    log::info!("Cleared existing layout in: {:?}", new_project_root);
                }
            }
        }

        // 2. Park the current project and enter the new one
        let entered = self.enter_project(new_project_root.clone())?;

        // 3. Create fresh layout with default panels (2 FileManagers)
        let fm1 = FileManager::new_with_path(new_project_root.clone());
        let fm2 = FileManager::new_with_path(new_project_root);
        self.add_panel(Box::new(fm1));
        self.add_panel(Box::new(fm2));
        self.carry_operations_panel(entered.operations_panel);

        // 4. Save the new layout
        self.auto_save_layout();

        // 5. Update terminal title to reflect new project root
        self.update_terminal_title();
        self.sync_open_projects();

        let t = termide_i18n::t();
        self.state.set_info(t.project_created().to_string());

        Ok(())
    }

    /// Update terminal window title to reflect current project root.
    pub(super) fn update_terminal_title(&self) {
        let path = self.project_root.display().to_string();
        let title = format!("Termide: {}", termide_core::util::shorten_home_path(&path));
        if let Err(e) = crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle(title))
        {
            log::debug!("failed to set terminal title: {e}");
        }
    }

    /// Handle change root path modal result - move the project to a new directory
    pub(super) fn handle_change_root_path_result(
        &mut self,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(new_path) = value.downcast_ref::<std::path::PathBuf>() {
            self.move_project_to(new_path.clone())?;
        }
        Ok(())
    }

    /// Move the current project's stored state to a new directory
    fn move_project_to(&mut self, new_project_root: std::path::PathBuf) -> Result<()> {
        use termide_project::ProjectLayout;

        let new_project_root = super::parked_projects::project_root_of(new_project_root);

        let old_project_root = self.project_root.clone();

        // Don't do anything if same directory
        if old_project_root == new_project_root {
            return Ok(());
        }
        // Another open project lives there: two would share one root.
        if self.open_projects.is_open(&new_project_root) {
            self.show_error_modal(i18n::t().projects_already_open().to_string());
            return Ok(());
        }

        // 1. Save the current layout
        self.auto_save_layout();

        // 2. Copy all project data to new location (including unsaved buffers)
        if let Ok(old_project_dir) = ProjectLayout::get_project_dir(&old_project_root) {
            if let Ok(new_project_dir) = ProjectLayout::get_project_dir(&new_project_root) {
                // Create the new project directory if needed
                if let Err(e) = std::fs::create_dir_all(&new_project_dir) {
                    log::error!(
                        "Failed to create project directory {}: {}",
                        new_project_dir.display(),
                        e
                    );
                }

                // Copy all files from the old project directory
                if let Ok(entries) = std::fs::read_dir(&old_project_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_file() {
                            if let Some(filename) = path.file_name() {
                                let new_path = new_project_dir.join(filename);
                                if let Err(e) = std::fs::copy(&path, &new_path) {
                                    log::error!(
                                        "Failed to copy project file {} -> {}: {}",
                                        path.display(),
                                        new_path.display(),
                                        e
                                    );
                                }
                            }
                        }
                    }
                }

                // Remove the old project directory
                if let Err(e) = std::fs::remove_dir_all(&old_project_dir) {
                    log::warn!(
                        "Failed to remove old project directory {}: {}",
                        old_project_dir.display(),
                        e
                    );
                }
            }
        }

        // 3. Change working directory
        std::env::set_current_dir(&new_project_root)?;
        log::info!(
            "Moved project from {:?} to {:?}",
            old_project_root,
            new_project_root
        );

        // 4. Update project_root
        self.open_projects.move_current(new_project_root.clone());
        self.set_project_root(new_project_root);
        self.sync_open_projects();

        // 5. Save the layout in the new location
        self.auto_save_layout();

        let t = termide_i18n::t();
        self.state.set_info(t.project_moved().to_string());

        Ok(())
    }

    /// Open a file in a new editor panel with LSP initialization.
    ///
    /// This is the core helper for opening files. It handles:
    /// - Creating the editor with configuration
    /// - Initializing LSP for the editor
    /// - Adding the panel to layout
    /// - Auto-saving the layout
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
                self.auto_save_layout();

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
                self.auto_save_layout();

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
                    let _ = terminal.send_cd(path);
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
