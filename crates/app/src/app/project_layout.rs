//! Project layout persistence.
//!
//! Saves the panel layout of the current project and restores it.

use anyhow::Result;
use termide_layout::LayoutManager;
use termide_panel_editor::Editor;

use crate::LayoutPersistence;
use crate::PanelExt;

use super::App;

impl App {
    /// Save the current project's layout to its file
    pub(super) fn save_layout(&mut self) -> Result<()> {
        // In $EDITOR mode (launched with file arguments) the layout is not
        // persisted, so editing a commit message or crontab never clobbers the
        // project's real layout.
        if !self.persist_layout {
            return Ok(());
        }
        save_layout_of(&self.project_root, &mut self.layout_manager)
    }

    /// Load the current project's layout from its file and restore it
    pub fn load_layout(&mut self) -> Result<()> {
        // Load the layout of this project
        let layout = termide_project::ProjectLayout::load(&self.project_root)?;

        // Get the project directory for restoring temporary buffers
        let project_dir = termide_project::ProjectLayout::get_project_dir(&self.project_root)?;

        // Get terminal dimensions for creating Terminal panels
        // Height: subtract menu (1) + status bar (1) + panel border (1) = 3
        let term_height = self.state.terminal.height.saturating_sub(3);
        // Width: full terminal width (vertical layout doesn't reduce width)
        let term_width = self.state.terminal.width;

        // Restore the layout manager from the saved layout
        self.layout_manager = LayoutManager::from_layout(
            layout,
            &project_dir,
            term_height,
            term_width,
            self.state.editor_config(),
            self.state.config.ai.clone(),
        )?;

        // Adapt panel widths to current terminal size
        self.layout_manager
            .redistribute_widths_proportionally(term_width);

        log::info!("Project layout loaded");

        // Register watchers for the new panels
        self.state.needs_watcher_registration = true;

        // Initialize LSP for all restored editors
        if let Some(ref mut lsp_manager) = self.state.lsp_manager {
            for group in &mut self.layout_manager.panel_groups {
                for panel in group.panels_mut() {
                    if let Some(editor) = panel.as_editor_mut() {
                        editor.init_lsp(lsp_manager);
                    }
                }
            }
        }

        // Restore orphaned buffer files (not referenced in the layout anymore).
        // The recovery itself is automatic — orphans appear as new
        // editor panels so user data from a crashed run isn't lost
        // — but it used to be silent, so users seeing extra editor
        // tabs had no way to tell what they were. Surface a Journal
        // entry per restored buffer plus a single summary so the
        // information is one panel open away.
        match termide_project::restore_orphaned_buffers(&project_dir) {
            Ok(orphaned_files) => {
                let mut restored = 0usize;
                for buffer_file in orphaned_files {
                    if let Ok(content) =
                        termide_project::load_unsaved_buffer(&project_dir, &buffer_file)
                    {
                        let mut editor = Editor::with_config(self.state.editor_config());
                        if editor.insert_text(&content).is_ok() {
                            editor.set_unsaved_buffer_file(Some(buffer_file.clone()));
                            self.add_panel(Box::new(editor));
                            log::info!(
                                "Recovered unsaved buffer from a previous run: {}",
                                buffer_file
                            );
                            restored += 1;
                        }
                    }
                }
                if restored > 0 {
                    log::warn!(
                        "Restored {restored} unsaved buffer(s) from a previous run — \
                         they appear as new editor panels."
                    );
                }
            }
            Err(e) => log::warn!("Failed to restore orphaned buffers: {}", e),
        }

        Ok(())
    }

    /// Auto-save the project layout (ignores errors to not disrupt user experience)
    pub fn auto_save_layout(&mut self) {
        if let Err(e) = self.save_layout() {
            // Log error but don't interrupt user workflow
            log::error!("Failed to auto-save project layout: {}", e);
        }
    }
}

/// Save `layout` as the layout of the project at `project_root`.
pub(super) fn save_layout_of(
    project_root: &std::path::Path,
    layout_manager: &mut LayoutManager,
) -> Result<()> {
    // Get the storage directory of this project
    let project_dir = termide_project::ProjectLayout::get_project_dir(project_root)?;

    // Ensure all modified unnamed buffers have stable filenames
    for group in &mut layout_manager.panel_groups {
        for panel in group.panels_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                editor.ensure_unsaved_buffer_file();
            }
        }
    }

    // Serialize the layout (may save temporary buffers)
    let layout = layout_manager.to_state(&project_dir);

    // Save the layout to its file
    layout.save(project_root)?;

    // Remove stale unsaved buffer files not referenced by the current layout
    termide_project::cleanup_stale_buffers(&project_dir, &layout);

    log::info!("Project layout saved");
    Ok(())
}
