use std::any::Any;
use std::path::PathBuf;

use super::App;
use crate::panels::file_manager::FileManager;
use crate::panels::welcome::Welcome;

impl App {
    /// Close panel by index and switch focus to next visible panel
    /// NOTE: panel_index parameter is now obsolete with LayoutManager, kept for compatibility
    pub(super) fn close_panel_at_index(&mut self, _panel_index: usize) {
        // Calculate available width for panel groups
        let terminal_width = self.state.terminal.width;
        let fm_width = if self.layout_manager.has_file_manager() {
            crate::constants::DEFAULT_FM_WIDTH
        } else {
            0
        };
        let available_width = terminal_width.saturating_sub(fm_width);

        // Close active panel (LayoutManager handles active panel tracking)
        let _ = self.layout_manager.close_active_panel(available_width);

        // Reload file manager to update git statuses
        // This is needed for example when closing .gitignore editor
        if let Some(fm_panel) = self.layout_manager.file_manager_mut() {
            let _ = fm_panel.reload();
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

    /// Find directory of another FM panel
    ///
    /// Note: This method is kept for backwards compatibility.
    /// With LayoutManager, there's only one FileManager.
    #[allow(dead_code)]
    pub(super) fn find_other_fm_directory(&self) -> Option<PathBuf> {
        // Get working directory from FileManager
        self.layout_manager
            .file_manager
            .as_ref()
            .and_then(|fm| fm.get_working_directory())
    }

    /// Find all panels that have working directories
    /// Returns deduplicated and sorted list of paths from all panel types (FM, Terminal, Editor)
    pub(super) fn find_all_other_panel_paths(&self) -> Vec<crate::ui::modal::SelectOption> {
        use std::collections::HashSet;

        let mut unique_paths: HashSet<PathBuf> = HashSet::new();

        // Collect all unique paths from FileManager
        if let Some(fm) = &self.layout_manager.file_manager {
            if let Some(dir) = fm.get_working_directory() {
                unique_paths.insert(dir);
            }
        }

        // Collect all unique paths from all panels in groups (Terminal, Editor, etc.)
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
        // With LayoutManager, there's only one FileManager
        if let Some(fm_panel) = self.layout_manager.file_manager_mut() {
            let panel_any: &mut dyn Any = &mut **fm_panel;
            if let Some(fm) = panel_any.downcast_mut::<FileManager>() {
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
