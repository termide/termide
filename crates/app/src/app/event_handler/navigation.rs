//! Panel navigation, focus, paste, and cursor-position event handlers.

#![allow(deprecated)]

use anyhow::Result;
use std::path::PathBuf;

use crate::app::App;
use crate::PanelExt;
use termide_core::PanelCommand;

impl App {
    /// Handle RequestPaste event - paste clipboard to active panel
    pub(super) fn event_paste_to_active_panel(&mut self) -> Result<()> {
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            panel.handle_command(PanelCommand::Paste);
        }
        Ok(())
    }

    /// Handle bracketed paste event - paste text directly to active panel
    pub fn handle_paste_event(&mut self, text: String) -> Result<()> {
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            panel.handle_command(PanelCommand::PasteText { text });
        }
        Ok(())
    }

    /// Handle GotoLine event - move cursor to specific line in editor
    pub(in crate::app) fn event_goto_line(&mut self, line: usize) {
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                // Convert from 1-based (user-facing) to 0-based (internal)
                let line_0based = line.saturating_sub(1);
                editor.set_cursor_line(line_0based);
            }
        }
    }

    /// Move the editor cursor to a 0-based line/column (status-bar Pos modal).
    pub(in crate::app) fn event_goto_position(&mut self, line: usize, column: usize) {
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                editor.goto_position(line, column);
            }
        }
    }

    /// Handle NavigateTo event - navigate file manager to path
    pub(super) fn event_navigate_to(&mut self, path: PathBuf) -> Result<()> {
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(fm) = panel.as_file_manager_mut() {
                if let Err(e) = fm.navigate_to(path.clone()) {
                    log::error!("Navigation failed: {}", e);
                    self.show_error_modal(format!("Cannot navigate to: {}", path.display()));
                } else {
                    // Navigation resets watched_root; trigger watcher re-registration
                    self.state.needs_watcher_registration = true;
                }
            }
        }
        Ok(())
    }

    /// Handle OpenPath event - open path in new file manager panel
    pub(super) fn event_open_path(
        &mut self,
        path: PathBuf,
        select_file: Option<std::ffi::OsString>,
    ) -> Result<()> {
        use termide_panel_file_manager::FileManager;

        // Create new file manager panel at the given path
        let mut fm = FileManager::new_with_path(path.clone());

        // If a file should be selected, find and select it
        if let Some(file_name) = select_file {
            fm.select_by_name(&file_name);
        }

        // Add panel to layout
        self.add_panel(Box::new(fm));
        self.auto_save_session();

        Ok(())
    }

    /// Handle SplitPanel event - toggle panel stacking/splitting
    pub(super) fn event_split_panel(&mut self, direction: termide_core::SplitDirection) {
        let terminal_width = self.state.terminal.width;

        match direction {
            termide_core::SplitDirection::Horizontal => {
                // Horizontal split: create new column (unstack if multiple panels in group)
                let _ = self.layout_manager.toggle_panel_stacking(terminal_width);
            }
            termide_core::SplitDirection::Vertical => {
                // Vertical split: stack in same column (merge if single panel)
                let _ = self.layout_manager.toggle_panel_stacking(terminal_width);
            }
        }
    }

    /// Handle FocusPanel event - focus panel by name/title
    pub(super) fn event_focus_panel(&mut self, name: &str) {
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
        if let Some((group_idx, panel_idx, _title)) = found {
            if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                group.set_expanded(panel_idx);
            }
            self.layout_manager.focus = group_idx;
            self.notify_outline_file_opened();
        } else {
            let _ = name;
        }
    }
}
