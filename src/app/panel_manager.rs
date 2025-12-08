use std::path::PathBuf;

use super::App;
use crate::panels::{welcome::Welcome, Panel, PanelExt};

impl App {
    /// Close panel by index and switch focus to next visible panel
    /// NOTE: panel_index parameter is now obsolete with LayoutManager, kept for compatibility
    pub(super) fn close_panel_at_index(&mut self, _panel_index: usize) {
        // Before closing, cleanup temporary files if this is an unsaved editor
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                // Check if editor has a temporary unsaved buffer file
                if let Some(filename) = editor.unsaved_buffer_file() {
                    // Get session directory and delete the temporary file
                    if let Ok(session_dir) =
                        crate::session::Session::get_session_dir(&self.project_root)
                    {
                        if let Err(e) =
                            crate::session::delete_unsaved_buffer(&session_dir, filename)
                        {
                            crate::logger::warn(format!(
                                "Failed to delete unsaved buffer file: {}",
                                e
                            ));
                        }
                    }
                }
            }
        }

        // Before closing, unwatch filesystem if this is a FileManager panel
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(fm) = panel.as_file_manager_mut() {
                // Unwatch the filesystem root for this FileManager
                if let Some(watched_root) = fm.take_watched_root() {
                    if let Some(watcher) = &mut self.state.fs_watcher {
                        if crate::git::find_repo_root(&watched_root).is_some() {
                            watcher.unwatch_repository(&watched_root);
                        } else {
                            watcher.unwatch_directory(&watched_root);
                        }
                    }
                }
            }
        }

        // Calculate available width for panel groups
        let terminal_width = self.state.terminal.width;

        // Close active panel (LayoutManager handles active panel tracking)
        let _ = self.layout_manager.close_active_panel(terminal_width);
        self.auto_save_session();

        // Reload all FileManager panels to update git statuses
        // This is needed for example when closing .gitignore editor
        for group in &mut self.layout_manager.panel_groups {
            for panel in group.panels_mut() {
                if let Some(fm) = panel.as_file_manager_mut() {
                    let _ = fm.reload();
                }
            }
        }

        // Add Welcome panel if needed
        // Check if no panel groups remain (all panels closed)
        let should_add_welcome = self.layout_manager.panel_groups.is_empty();

        if should_add_welcome {
            let welcome = Welcome::new();
            self.add_panel(Box::new(welcome));
        }

        // Active panel tracking is handled by LayoutManager
        // No need to manually update active_panel index
    }

    /// Find all panels that have working directories
    /// Returns deduplicated and sorted list of paths from all panel types (FM, Terminal, Editor)
    pub(super) fn find_all_other_panel_paths(&self) -> Vec<crate::ui::modal::SelectOption> {
        use std::collections::HashSet;

        let mut unique_paths: HashSet<PathBuf> = HashSet::new();

        // Collect all unique paths from all panels in groups
        for group in &self.layout_manager.panel_groups {
            for panel in group.panels() {
                // Get working directory from any panel type
                if let Some(dir) = panel.get_working_directory() {
                    unique_paths.insert(dir);
                }
            }
        }

        // Convert to Vec and sort by path
        let mut options: Vec<_> = unique_paths
            .into_iter()
            .map(|path| {
                let path_str = path.display().to_string();
                crate::ui::modal::SelectOption {
                    panel_index: 0,          // Not used with LayoutManager
                    value: path_str.clone(), // Value is the path string
                    display: path_str,       // Display is also the path string
                }
            })
            .collect();

        // Sort by value for consistent ordering
        options.sort_by(|a, b| a.value.cmp(&b.value));

        options
    }

    /// Refresh all FM panels that show specified directory
    pub(super) fn refresh_fm_panels(&mut self, directory: &std::path::Path) {
        // Refresh all FileManager panels showing this directory
        for group in &mut self.layout_manager.panel_groups {
            for panel in group.panels_mut() {
                if let Some(fm) = panel.as_file_manager_mut() {
                    // Check if FM working directory matches target
                    let fm_dir = fm.get_current_directory();
                    if fm_dir == directory {
                        // Refresh directory contents
                        let _ = fm.load_directory();
                    }
                }
            }
        }
    }
}
