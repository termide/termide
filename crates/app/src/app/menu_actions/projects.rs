//! Projects menu actions — project switching, directory switcher.

use anyhow::Result;
use std::path::PathBuf;

use super::super::App;
use crate::projects_menu::{ProjectRow, ProjectsTarget};
use crate::state::{ActiveModal, PendingAction};
use crate::PanelExt;
use termide_app_core::Panel;
use termide_i18n as i18n;
use termide_ui_render::{
    PROJECTS_SUBMENU_CHANGE_ROOT, PROJECTS_SUBMENU_NEW, PROJECTS_SUBMENU_SWITCH,
};

impl App {
    /// Open the projects modal to switch between projects. It lists the
    /// projects as the Projects menu does, with the cursor on the one left
    /// last: `Enter` goes back to it.
    pub(in crate::app) fn handle_open_projects_modal(&mut self) -> Result<()> {
        use crate::projects_menu::listed_projects;
        use termide_modal::{ProjectItem, ProjectsModal};
        use termide_project::format_local_minute;

        let t = i18n::t();

        let known = crate::projects_menu::known_projects();
        let items: Vec<ProjectItem> = listed_projects(&self.state.open_projects, &known)
            .into_iter()
            .map(|project| ProjectItem {
                display_path: termide_core::util::shorten_home_path(
                    &project.root.display().to_string(),
                ),
                modified: project
                    .modified
                    .map(format_local_minute)
                    .unwrap_or_default(),
                is_current: project.root == self.project_root,
                is_open: project.open,
                attention: project.attention,
                project_path: project.root,
            })
            .collect();

        // Only show modal if there are other projects
        if items.iter().any(|item| !item.is_current) {
            let previous = self.open_projects.by_recent_use().get(1).copied();
            let cursor = previous
                .and_then(|root| items.iter().position(|item| item.project_path == root))
                .or_else(|| items.iter().position(|item| !item.is_current))
                .unwrap_or(0);
            let modal = ProjectsModal::new(t.projects_title(), items).with_cursor(cursor);
            self.state.set_pending_action(
                PendingAction::SwitchProject,
                ActiveModal::Projects(Box::new(modal)),
            );
        }

        Ok(())
    }

    /// Open directory switcher modal
    pub(in crate::app) fn handle_open_directory_switcher(&mut self) -> Result<()> {
        use termide_modal::{DirectoryItem, DirectorySwitcherModal};

        let t = i18n::t();

        // Check if active panel supports directory switching (Terminal or FileManager)
        let panel_supported = self
            .layout_manager
            .active_panel_mut()
            .map(|p| p.as_terminal_mut().is_some() || p.as_file_manager_mut().is_some())
            .unwrap_or(false);

        if !panel_supported {
            self.state
                .set_info(t.directory_switcher_unsupported().to_string());
            return Ok(());
        }

        // For terminal panels, check if there's a running process (cd won't work)
        let has_running_process = self
            .layout_manager
            .active_panel_mut()
            .and_then(|p| p.as_terminal_mut())
            .map(|t| t.has_running_processes())
            .unwrap_or(false);

        if has_running_process {
            self.state
                .set_info(t.directory_switcher_process_running().to_string());
            return Ok(());
        }

        // Get current panel's working directory
        let current_dir = self
            .layout_manager
            .active_panel_mut()
            .and_then(|p| p.get_working_directory());

        // Get all unique paths from all panels
        let panel_paths = self.collect_panel_paths();

        // Get bookmarked directories
        let bookmark_dirs = self.state.bookmarks.directories();

        // Build combined items list
        let mut items: Vec<DirectoryItem> = Vec::new();
        let mut seen_paths = std::collections::HashSet::new();

        // Add panel paths first
        for path in panel_paths {
            let is_current = current_dir.as_ref() == Some(&path);
            let display = termide_core::util::shorten_home_path(&path.display().to_string());
            seen_paths.insert(path.clone());
            items.push(DirectoryItem {
                path,
                display,
                is_current,
                is_bookmark: false,
            });
        }

        // Add bookmarked directories (if not already in list)
        for bookmark in bookmark_dirs {
            let path = PathBuf::from(&bookmark.path);
            if !seen_paths.contains(&path) {
                // Show path instead of display name for consistency
                let display = termide_core::util::shorten_home_path(&bookmark.path);
                let is_current = current_dir.as_ref() == Some(&path);
                items.push(DirectoryItem {
                    path,
                    display,
                    is_current,
                    is_bookmark: true,
                });
            }
        }

        // Drive roots (Windows only): the one way to another drive without
        // typing its path, and what ".." at a drive root opens.
        for path in termide_vfs::drive_roots() {
            if items.iter().any(|item| item.path == path) {
                continue;
            }
            items.push(DirectoryItem {
                display: path.display().to_string(),
                is_current: current_dir.as_ref() == Some(&path),
                path,
                is_bookmark: false,
            });
        }

        // Sort items alphabetically by display path
        items.sort_by(|a, b| a.display.cmp(&b.display));

        // If no paths available, show info message
        if items.is_empty() {
            self.state
                .set_info(t.directory_switcher_no_paths().to_string());
            return Ok(());
        }

        // Find index of current directory to position cursor there
        let current_idx = items.iter().position(|item| item.is_current).unwrap_or(0);
        let modal = DirectorySwitcherModal::new(t.directory_switcher_title(), items)
            .with_cursor(current_idx);
        self.state.set_pending_action(
            PendingAction::SwitchDirectory,
            ActiveModal::DirectorySwitcher(Box::new(modal)),
        );

        Ok(())
    }

    // =========================================================================
    // Projects submenu handling
    // =========================================================================

    /// Handle keyboard event in the Projects menu. Delete/F8 closes an open
    /// project, or deletes the saved layout of one that is not open; the
    /// current project stays.
    pub(in crate::app) fn handle_projects_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        use super::navigate_submenu;
        use super::SubmenuNavAction;

        let menu = self.state.projects_menu();
        let target = ProjectsTarget::of(menu.selected_row());
        let removable = match menu.selected_row() {
            Some(ProjectRow::Project(project)) if project.root != self.project_root => {
                Some(project.clone())
            }
            _ => None,
        };
        let mut cursor = termide_state::SubmenuState {
            open: true,
            selected: menu.selected,
        };
        let action = navigate_submenu(&key, &mut cursor, menu.items.len(), &menu.separators());
        drop(menu);
        self.state.ui.projects_submenu.selected = cursor.selected;

        match action {
            SubmenuNavAction::Close => self.state.close_menu(),
            SubmenuNavAction::Left => self.switch_to_prev_menu()?,
            SubmenuNavAction::Right => self.switch_to_next_menu()?,
            SubmenuNavAction::Execute => self.activate_projects_target(target)?,
            SubmenuNavAction::Delete => {
                if let Some(project) = removable {
                    let selection = self.state.ui.projects_submenu.selected;
                    self.state.close_menu();
                    if project.open {
                        self.confirm_close_project(project.root, Some(selection));
                    } else {
                        self.confirm_delete_project(project.root, Some(selection));
                    }
                }
            }
            SubmenuNavAction::Rename | SubmenuNavAction::Edit | SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// Open the Projects menu at row `selection`, as close as the reloaded
    /// list still allows.
    pub(in crate::app) fn reopen_projects_menu(&mut self, selection: usize) {
        self.state.ui.menu_open = true;
        self.state.ui.selected_menu_item = Some(termide_ui_render::PROJECTS_MENU_INDEX);
        self.state.open_projects_submenu();
        self.state.restore_projects_selection(selection);
    }

    /// Carry out the selected row.
    pub(in crate::app) fn activate_projects_target(
        &mut self,
        target: ProjectsTarget,
    ) -> Result<()> {
        match target {
            ProjectsTarget::None => {}
            ProjectsTarget::Project(path) => {
                self.state.close_menu();
                if path != self.project_root {
                    self.switch_to_project(path)?;
                }
            }
            ProjectsTarget::Reopen => {
                self.state.close_menu();
                self.reopen_previous_projects();
            }
            ProjectsTarget::Action(PROJECTS_SUBMENU_NEW) => {
                self.state.close_menu();
                self.handle_new_project()?;
            }
            ProjectsTarget::Action(PROJECTS_SUBMENU_SWITCH) => {
                self.state.close_menu();
                self.handle_open_projects_modal()?;
            }
            ProjectsTarget::Action(PROJECTS_SUBMENU_CHANGE_ROOT) => {
                self.state.close_menu();
                self.handle_change_root_path()?;
            }
            ProjectsTarget::Action(_) => {}
        }
        Ok(())
    }

    /// Open directory picker for creating a new project
    pub(in crate::app) fn handle_new_project(&mut self) -> Result<()> {
        use termide_modal::DirectoryPickerModal;

        let t = i18n::t();
        // Get current project root as starting directory
        let initial_dir = self.project_root.clone();

        let modal = DirectoryPickerModal::new(
            initial_dir,
            t.projects_new().to_string(),
            t.directory_picker_create().to_string(),
        );
        self.state.set_pending_action(
            PendingAction::NewProject,
            ActiveModal::DirectoryPicker(Box::new(modal)),
        );

        Ok(())
    }

    /// Open directory picker for changing root path of the current project
    pub(in crate::app) fn handle_change_root_path(&mut self) -> Result<()> {
        use termide_modal::DirectoryPickerModal;

        let t = i18n::t();
        // Get current project root as starting directory
        let initial_dir = self.project_root.clone();

        let modal = DirectoryPickerModal::new(
            initial_dir,
            t.projects_change_root().to_string(),
            t.directory_picker_move().to_string(),
        );
        self.state.set_pending_action(
            PendingAction::ChangeRootPath,
            ActiveModal::DirectoryPicker(Box::new(modal)),
        );

        Ok(())
    }
}
