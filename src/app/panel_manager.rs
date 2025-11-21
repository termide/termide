use std::any::Any;
use std::path::PathBuf;

use crate::panels::file_manager::FileManager;
use crate::panels::welcome::Welcome;
use super::App;

impl App {
    /// Close panel by index and switch focus to next visible panel
    pub(super) fn close_panel_at_index(&mut self, panel_index: usize) {
        // Close panel
        self.panels.close_panel(panel_index);

        // Reload file manager (panel 0) to update git statuses
        // This is needed for example when closing .gitignore editor
        if let Some(fm_panel) = self.panels.get_mut(0) {
            let _ = fm_panel.reload();
        }

        // Add Welcome panel if needed
        // - In Single mode: when panels.count() == 0
        // - In MultiPanel mode: when only FM remains (panels.count() == 1)
        let should_add_welcome = match self.state.layout_mode {
            crate::state::LayoutMode::Single => self.panels.count() == 0,
            crate::state::LayoutMode::MultiPanel => {
                // Check that only one panel (FM) remains without Welcome
                if self.panels.count() == 1 {
                    // Make sure remaining panel is not Welcome
                    self.panels.get(0).map(|p| !p.is_welcome_panel()).unwrap_or(true)
                } else {
                    false
                }
            }
        };

        if should_add_welcome {
            let welcome = Welcome::new();
            self.panels.add_panel(Box::new(welcome));
            // Focus returns to file manager (panel 0) when welcome panel is added
            self.state.active_panel = 0;
            return;
        }

        // Find next visible panel
        if self.panels.count() > 0 {
            // Try next visible panel
            let visible = self.panels.visible_indices();
            if !visible.is_empty() {
                // If closed panel was last, select previous visible
                if panel_index >= self.panels.count() {
                    self.state.active_panel = *visible.last().unwrap();
                } else {
                    // Select closest visible panel
                    self.state.active_panel = visible
                        .iter()
                        .find(|&&i| i >= panel_index)
                        .or_else(|| visible.last())
                        .copied()
                        .unwrap_or(0);
                }
            }
        }
    }

    /// Find directory of another FM panel (not current_panel_index)
    pub(super) fn find_other_fm_directory(&self, current_panel_index: usize) -> Option<PathBuf> {
        // Search for another FM panel
        for i in 0..self.panels.count() {
            if i != current_panel_index {
                if let Some(panel) = self.panels.get(i) {
                    // Check if panel is FileManager
                    let panel_any: &dyn Any = &**panel;
                    if panel_any.is::<FileManager>() {
                        // Get working directory
                        if let Some(dir) = panel.get_working_directory() {
                            return Some(dir);
                        }
                    }
                }
            }
        }

        None
    }

    /// Refresh all FM panels that show specified directory
    pub(super) fn refresh_fm_panels(&mut self, directory: &std::path::Path) {
        for i in 0..self.panels.count() {
            if let Some(panel) = self.panels.get_mut(i) {
                let panel_any: &mut dyn Any = &mut **panel;
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
}
