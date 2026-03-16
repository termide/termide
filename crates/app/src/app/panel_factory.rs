//! Panel creation: instantiate and add new panels (Terminal, Editor, FileManager, etc.)

#![allow(deprecated)]

use anyhow::Result;
use std::path::PathBuf;

use super::App;
use crate::PanelExt;

use termide_panel_editor::Editor;
use termide_panel_file_manager::FileManager;
use termide_panel_misc::{HelpPanel as Help, JournalPanel as Journal};
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
        log::debug!("Opening new Terminal panel with shell: {:?}", shell_path);
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

    /// Save shell preference to config file
    pub(super) fn save_shell_preference(&self, shell_path: &str) -> Result<()> {
        let mut config = termide_config::Config::load()?;
        config.terminal.default_shell = Some(shell_path.to_string());
        config.save()?;
        Ok(())
    }

    /// Create new file manager
    pub(super) fn handle_new_file_manager(&mut self) -> Result<()> {
        log::debug!("Opening new FileManager panel");
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
        log::debug!("Opening new Editor panel");
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
            log::debug!("Switching focus to existing Journal panel");
            return Ok(());
        }

        // No existing Journal panel found, create new one
        log::debug!("Opening new Journal panel");
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
        log::debug!("Opening Help panel");
        let help = Help::new(&self.state.config);
        self.add_panel(Box::new(help));
        self.auto_save_session();
        Ok(())
    }

    /// Open scripts folder in file manager
    pub(super) fn handle_manage_scripts(&mut self) -> Result<()> {
        use termide_config::get_data_dir;

        log::debug!("Opening scripts folder in File Manager");
        self.close_help_panels();

        // Get the scripts directory path
        let scripts_dir = match get_data_dir() {
            Ok(data_dir) => {
                let scripts_path = data_dir.join("scripts");
                // Create the directory if it doesn't exist
                if !scripts_path.exists() {
                    if let Err(e) = std::fs::create_dir_all(&scripts_path) {
                        log::warn!("Failed to create scripts directory: {}", e);
                    }
                }
                scripts_path
            }
            Err(e) => {
                log::warn!("Failed to get data dir: {}", e);
                self.state
                    .set_error(format!("Failed to get scripts directory: {}", e));
                return Ok(());
            }
        };

        let fm_panel = FileManager::new_with_path(scripts_dir);
        self.add_panel(Box::new(fm_panel));
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
                self.state
                    .set_error(format!("Failed to get config path: {}", e));
                return Ok(());
            }
        };

        self.close_help_panels();

        let _ = self.open_editor_for_file(config_path);
        Ok(())
    }

    /// Open or focus the Outline panel (singleton).
    pub(super) fn handle_open_outline(&mut self) -> Result<()> {
        log::debug!("Opening Outline panel");
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
        log::debug!("Opening Diagnostics panel");
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

    /// Open Git Status panel
    pub(super) fn handle_open_git_status(&mut self) -> Result<()> {
        log::debug!("Opening Git Status panel");
        self.close_help_panels();

        if !self.find_and_focus_panel_by_name("git_status") {
            let paths = self.collect_panel_paths();
            let git_status_panel = termide_panel_git_status::GitStatusPanel::new(&paths);
            self.add_panel(Box::new(git_status_panel));
        }
        self.auto_save_session();
        Ok(())
    }

    /// Open Git Log panel
    pub(super) fn handle_open_git_log(&mut self) -> Result<()> {
        log::debug!("Opening Git Log panel");
        self.close_help_panels();

        let paths = self.collect_panel_paths();
        let git_log_panel = termide_panel_git_log::GitLogPanel::new(&paths);
        self.add_panel(Box::new(git_log_panel));
        self.auto_save_session();
        Ok(())
    }
}
