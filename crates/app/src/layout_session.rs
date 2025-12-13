//! Layout manager session serialization.
//!
//! Provides session save/restore functionality for the layout manager.

use std::path::Path;

use anyhow::Result;

use termide_core::Panel;
use termide_layout::{LayoutManager, PanelGroup};
use termide_panel_editor::{Editor, EditorConfig};
use termide_panel_file_manager::FileManager;
use termide_panel_misc::LogViewerPanel;
use termide_panel_terminal::Terminal;
use termide_session::{
    cleanup_unsaved_buffer, load_unsaved_buffer, Session, SessionPanel, SessionPanelGroup,
};

/// Extension trait for session serialization.
pub trait LayoutManagerSession {
    /// Serialize current layout to Session.
    fn to_session(&mut self, session_dir: &Path) -> Session;

    /// Restore layout from Session.
    fn from_session(
        session: Session,
        session_dir: &Path,
        term_height: u16,
        term_width: u16,
        editor_config: EditorConfig,
    ) -> Result<LayoutManager>;
}

impl LayoutManagerSession for LayoutManager {
    fn to_session(&mut self, session_dir: &Path) -> Session {
        let panel_groups: Vec<SessionPanelGroup> = self
            .panel_groups
            .iter_mut()
            .map(|group| {
                let panels: Vec<_> = group
                    .panels_mut()
                    .iter_mut()
                    .filter_map(|panel| panel.to_session(session_dir))
                    .collect();

                SessionPanelGroup {
                    panels,
                    expanded_index: group.expanded_index(),
                    width: group.width,
                }
            })
            .collect();

        Session {
            panel_groups,
            focused_group: self.focus,
        }
    }

    fn from_session(
        session: Session,
        session_dir: &Path,
        term_height: u16,
        term_width: u16,
        editor_config: EditorConfig,
    ) -> Result<LayoutManager> {
        let mut layout = LayoutManager::new();

        for session_group in session.panel_groups {
            if session_group.panels.is_empty() {
                continue;
            }

            let mut panels: Vec<Box<dyn Panel>> = Vec::with_capacity(session_group.panels.len());

            for session_panel in session_group.panels {
                let panel: Option<Box<dyn Panel>> = match session_panel {
                    SessionPanel::FileManager { path } => {
                        Some(Box::new(FileManager::new_with_path(path)))
                    }
                    SessionPanel::Editor {
                        path,
                        unsaved_buffer_file,
                    } => {
                        if let Some(file_path) = path {
                            Editor::open_file_with_config(file_path, editor_config.clone())
                                .ok()
                                .map(|e| Box::new(e) as Box<dyn Panel>)
                        } else if let Some(ref buffer_file) = unsaved_buffer_file {
                            match load_unsaved_buffer(session_dir, buffer_file) {
                                Ok(content) => {
                                    if content.trim().is_empty() {
                                        let _ = cleanup_unsaved_buffer(session_dir, buffer_file);
                                        None
                                    } else {
                                        let mut editor = Editor::with_config(editor_config.clone());
                                        if let Err(e) = editor.insert_text(&content) {
                                            eprintln!(
                                                "Warning: Failed to restore unsaved buffer content: {}",
                                                e
                                            );
                                            None
                                        } else {
                                            editor
                                                .set_unsaved_buffer_file(Some(buffer_file.clone()));
                                            Some(Box::new(editor) as Box<dyn Panel>)
                                        }
                                    }
                                }
                                Err(e) => {
                                    eprintln!(
                                        "Warning: Failed to load unsaved buffer {}: {}",
                                        buffer_file, e
                                    );
                                    None
                                }
                            }
                        } else {
                            None
                        }
                    }
                    SessionPanel::Terminal { working_dir } => {
                        Terminal::new_with_cwd(term_height, term_width, Some(working_dir))
                            .ok()
                            .map(|t| Box::new(t) as Box<dyn Panel>)
                    }
                    SessionPanel::Debug => Some(Box::new(LogViewerPanel::default())),
                };

                if let Some(p) = panel {
                    panels.push(p);
                }
            }

            if panels.is_empty() {
                continue;
            }

            let mut group = PanelGroup::new(panels.remove(0));
            for panel in panels {
                group.add_panel(panel);
            }

            let expanded_idx = session_group
                .expanded_index
                .min(group.len().saturating_sub(1));
            group.set_expanded(expanded_idx);
            group.width = session_group.width;

            layout.panel_groups.push(group);
        }

        layout.focus = session
            .focused_group
            .min(layout.panel_groups.len().saturating_sub(1));

        Ok(layout)
    }
}
