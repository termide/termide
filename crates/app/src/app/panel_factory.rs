//! Panel creation: instantiate and add new panels (Terminal, Editor, FileManager, etc.)

#![allow(deprecated)]

use anyhow::Result;
use std::path::PathBuf;

use super::App;
use crate::PanelExt;

use termide_core::ReferenceLocation;
use termide_panel_editor::Editor;
use termide_panel_file_manager::FileManager;
use termide_panel_misc::{HelpPanel as Help, JournalPanel as Journal, ReferencesPanel};
use termide_panel_terminal::Terminal;

impl App {
    /// Create new terminal using the default shell (from config or auto-detect)
    pub(super) fn handle_new_terminal(&mut self) -> Result<()> {
        let shell = self.state.config.terminal.default_shell.clone();
        self.handle_new_terminal_with_shell(shell.as_deref())
    }

    /// Create new terminal with a specific shell (or auto-detect if None)
    pub(super) fn handle_new_terminal_with_shell(
        &mut self,
        shell_path: Option<&str>,
    ) -> Result<()> {
        self.close_help_panels();
        // Get working directory from current active panel
        let working_dir = self
            .layout_manager
            .active_panel_mut()
            .and_then(|p| p.get_working_directory());

        // Create new terminal
        let width = self.state.terminal.width;
        let height = self.state.terminal.height;
        let term_height = height.saturating_sub(3);
        let term_width = width.saturating_sub(2);

        let result = match shell_path {
            Some(path) => Terminal::new_with_shell(term_height, term_width, path, working_dir),
            None => Terminal::new_with_cwd(term_height, term_width, working_dir),
        };

        if let Ok(terminal_panel) = result {
            self.add_panel(Box::new(terminal_panel));
            self.auto_save_session();
        }
        Ok(())
    }

    /// Save shell preference to the active config target (project file
    /// when one exists, global otherwise). The in-memory `state.config`
    /// is updated so newly-spawned terminals see the new default shell
    /// without a restart.
    pub(super) fn save_shell_preference(&mut self, shell_path: &str) -> Result<()> {
        let mut config = (*self.state.config).clone();
        config.terminal.default_shell = Some(shell_path.to_string());
        self.save_config_to_active_target(config)
    }

    /// Create new file manager
    pub(super) fn handle_new_file_manager(&mut self) -> Result<()> {
        self.close_help_panels();

        // Check if active panel is a remote FileManager and clone it
        let remote_info = self
            .layout_manager
            .active_panel_mut()
            .and_then(|p| p.as_file_manager_mut())
            .filter(|fm| fm.is_remote())
            .map(|fm| (fm.display_path(), fm.vfs_manager_arc()));

        let fm_panel = if let Some((vfs_url, vfs_manager)) = remote_info {
            // Clone remote panel with same VFS URL
            FileManager::new_with_vfs_url(&vfs_url, vfs_manager)?
        } else {
            // Fallback to local filesystem
            let working_dir = self
                .layout_manager
                .active_panel_mut()
                .and_then(|p| p.get_working_directory())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));
            FileManager::new_with_path(working_dir)
        };

        self.add_panel(Box::new(fm_panel));
        self.auto_save_session();
        Ok(())
    }

    /// Create new editor
    pub(super) fn handle_new_editor(&mut self) -> Result<()> {
        self.close_help_panels();

        // Get working directory from current active panel (e.g., FileManager)
        let initial_directory = self
            .layout_manager
            .active_panel_mut()
            .and_then(|p| p.get_working_directory());

        let mut config = self.state.editor_config();
        config.initial_directory = initial_directory;

        let editor_panel = Editor::with_config(config);
        self.add_panel(Box::new(editor_panel));
        self.auto_save_session();
        Ok(())
    }

    /// Create new journal panel (singleton - only one instance allowed)
    pub(super) fn handle_new_journal(&mut self) -> Result<()> {
        // Check if Journal panel already exists and focus it
        if self.focus_existing_journal_panel() {
            return Ok(());
        }

        // No existing Journal panel found, create new one
        self.close_help_panels();
        let journal_panel = Journal::new(self.state.theme);
        self.add_panel(Box::new(journal_panel));
        self.auto_save_session();
        Ok(())
    }

    /// Find and focus existing Journal panel if it exists
    /// Returns true if Journal panel was found and focused
    fn focus_existing_journal_panel(&mut self) -> bool {
        // Iterate through all panel groups
        for (group_idx, group) in self.layout_manager.panel_groups.iter_mut().enumerate() {
            // Check each panel in the group
            for (panel_idx, panel) in group.panels().iter().enumerate() {
                if panel.is_journal() {
                    // Found Journal panel - set it as expanded and focus the group
                    group.set_expanded(panel_idx);
                    self.layout_manager.focus = group_idx;
                    return true;
                }
            }
        }

        false
    }

    /// Open or switch to help panel
    pub(super) fn handle_new_help(&mut self) -> Result<()> {
        let help = Help::new(&self.state.config);
        self.add_panel(Box::new(help));
        self.auto_save_session();
        Ok(())
    }

    /// Open config file in editor
    pub(super) fn open_config_in_editor(&mut self) -> Result<()> {
        use termide_config::Config;

        let config_path = match Config::config_file_path() {
            Ok(path) => path,
            Err(e) => {
                log::warn!("Failed to get config path: {}", e);
                self.show_error_modal(format!("Failed to get config path: {}", e));
                return Ok(());
            }
        };

        self.close_help_panels();

        let _ = self.open_editor_for_file(config_path);
        Ok(())
    }

    /// Open Settings modal with tabbed interface.
    ///
    /// The modal is seeded from `state.config` (the in-memory effective
    /// config — defaults + global + project) so any uncommitted edits
    /// from the current session are visible in the form. The third
    /// footer button label is derived from the existence of the
    /// per-project override file at modal-open time.
    pub(super) fn open_settings_modal(&mut self) {
        use termide_modal::{ActiveModal, SettingsModal};

        let config = (*self.state.config).clone();
        let project_override_active =
            termide_config::project_config_path(&self.project_root).exists();
        let modal = SettingsModal::new(config, project_override_active);
        self.state.set_pending_action(
            crate::state::PendingAction::Settings,
            ActiveModal::Settings(Box::new(modal)),
        );
    }

    /// Open or refresh the References panel with LSP find-references results.
    ///
    /// If the panel is already open, updates its contents and focuses it.
    /// Otherwise creates a new panel.
    pub(super) fn handle_open_references_panel(
        &mut self,
        locations: Vec<ReferenceLocation>,
        symbol_name: Option<String>,
    ) -> Result<()> {
        // Find existing panel (immutable check)
        let panel_exists = self
            .layout_manager
            .iter_all_panels_mut()
            .any(|p| p.name() == "references");

        if panel_exists {
            // Update existing panel with new results, then focus it
            let mut update_data = Some((locations, symbol_name));
            for panel in self.layout_manager.iter_all_panels_mut() {
                if let Some(refs_panel) = panel.as_any_mut().downcast_mut::<ReferencesPanel>() {
                    if let Some((locs, sym)) = update_data.take() {
                        refs_panel.update(locs, sym);
                    }
                    break;
                }
            }
            self.find_and_focus_panel_by_name("references");
        } else {
            let panel = ReferencesPanel::new(locations, symbol_name, self.state.theme);
            self.add_panel(Box::new(panel));
        }
        Ok(())
    }

    /// Open or focus the Outline panel (singleton).
    pub(super) fn handle_open_outline(&mut self) -> Result<()> {
        self.close_help_panels();

        if !self.find_and_focus_panel_by_name("outline") {
            let outline = termide_panel_outline::OutlinePanel::new(*self.state.theme);
            self.add_panel(Box::new(outline));
        }
        // On first open: populate from any available editor
        self.populate_outline_from_any_editor();
        self.auto_save_session();
        Ok(())
    }

    /// Open Diagnostics panel
    pub(super) fn handle_open_diagnostics(&mut self) -> Result<()> {
        self.close_help_panels();

        if !self.find_and_focus_panel_by_name("diagnostics") {
            let mut diagnostics_panel =
                termide_panel_diagnostics::DiagnosticsPanel::new(self.state.theme);

            // Initialize with existing diagnostics from all files
            for (path, diags) in &self.state.all_diagnostics {
                diagnostics_panel.update_diagnostics(path.clone(), diags);
            }

            self.add_panel(Box::new(diagnostics_panel));
        }
        self.auto_save_session();
        Ok(())
    }

    /// Open a Git Status panel.
    ///
    /// Always creates a NEW panel (like editors/terminals) rather than focusing
    /// an existing one, so several repositories can be watched side by side —
    /// each instance tracks its own repo via the repo dropdown.
    pub(super) fn handle_open_git_status(&mut self) -> Result<()> {
        self.close_help_panels();

        let paths = self.collect_repo_search_paths();
        let git_status_panel = termide_panel_git_status::GitStatusPanel::new(&paths);
        self.add_panel(Box::new(git_status_panel));
        self.auto_save_session();
        Ok(())
    }

    /// Open Git Log panel (singleton)
    pub(super) fn handle_open_git_log(&mut self) -> Result<()> {
        self.close_help_panels();

        if !self.find_and_focus_panel_by_name("git_log") {
            let paths = self.collect_repo_search_paths();
            let git_log_panel = termide_panel_git_log::GitLogPanel::new(&paths);
            self.add_panel(Box::new(git_log_panel));
        }
        self.auto_save_session();
        Ok(())
    }
}
