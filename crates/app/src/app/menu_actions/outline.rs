//! Outline panel synchronization — editor ↔ outline sync.

use super::super::App;
use crate::PanelExt;

impl App {
    /// Notify outline panel that a file was opened/switched.
    pub(crate) fn notify_outline_file_opened(&mut self) {
        let editor_info = self.collect_editor_info_for_outline();
        if let Some((path, content, language, cursor_line)) = editor_info {
            self.push_to_outline(path, &content, language.as_deref(), Some(cursor_line));
        }
    }

    /// Re-sync the outline with the layout as it now stands — after a panel
    /// close or a restored layout: bind it to the active editor, else to any
    /// editor, else clear it.
    pub(in crate::app) fn resync_outline(&mut self) {
        // 1. Try the now-active panel (may be the next editor in stack)
        if self.collect_editor_info_for_outline().is_some() {
            self.notify_outline_file_opened();
            return;
        }
        // 2. Try any editor remaining in layout
        let has_editor = self
            .layout_manager
            .iter_all_panels_mut()
            .any(|p| p.as_editor().is_some());
        if has_editor {
            self.populate_outline_from_any_editor();
            return;
        }
        // 3. No editors — clear outline
        for group in &mut self.layout_manager.panel_groups {
            for panel in group.panels_mut() {
                if let Some(outline) = panel
                    .as_any_mut()
                    .downcast_mut::<termide_panel_outline::OutlinePanel>()
                {
                    outline.clear();
                    return;
                }
            }
        }
    }

    /// Collect editor data for outline (extracted for reuse).
    ///
    /// Only returns data when the active panel is an editor.
    /// Switching to non-editor panels keeps the outline bound to the last editor.
    fn collect_editor_info_for_outline(
        &mut self,
    ) -> Option<(Option<std::path::PathBuf>, String, Option<String>, usize)> {
        let panel = self.layout_manager.active_panel_mut()?;
        let editor = panel.as_editor_mut()?;
        let path = editor.file_path().map(|p| p.to_path_buf());
        let content = editor.content_string();
        let cursor_line = editor.cursor_line();
        let language = path
            .as_ref()
            .and_then(|p| termide_highlight::detect_language(p))
            .map(|s| s.to_string());
        Some((path, content, language, cursor_line))
    }

    /// Lightweight check for live editing — only compare edit_version, debounced 1s.
    pub(in crate::app) fn check_outline_live_edit(&mut self) {
        let needs_repopulate = self
            .layout_manager
            .panel_groups
            .iter_mut()
            .flat_map(|g| g.panels_mut())
            .find_map(|p| {
                p.as_any_mut()
                    .downcast_mut::<termide_panel_outline::OutlinePanel>()
            })
            .is_some_and(|outline| outline.needs_repopulate());
        if needs_repopulate {
            self.populate_outline_from_any_editor();
            return;
        }

        let Some(panel) = self.layout_manager.active_panel_mut() else {
            return;
        };
        let Some(editor) = panel.as_editor_mut() else {
            return;
        };

        let version = editor.edit_version();
        if version == self.outline_last_version {
            // No edits — also sync cursor cheaply
            let cursor = editor.cursor_line();
            if cursor != self.outline_last_cursor {
                self.outline_last_cursor = cursor;
                self.sync_outline_cursor(cursor);
            }
            return;
        }

        // Version changed — check debounce (1 second since last update)
        let now = std::time::Instant::now();
        if let Some(last) = self.outline_last_edit_time {
            if now.duration_since(last) < std::time::Duration::from_secs(1) {
                return; // Too soon, wait
            }
        }

        self.outline_last_version = version;
        self.outline_last_cursor = editor.cursor_line();
        self.outline_last_edit_time = Some(now);

        // Only now clone content
        let content = editor.content_string();
        let path = editor.file_path().map(|p| p.to_path_buf());
        let language = path
            .as_ref()
            .and_then(|p| termide_highlight::detect_language(p))
            .map(|s| s.to_string());
        self.push_to_outline(
            path,
            &content,
            language.as_deref(),
            Some(self.outline_last_cursor),
        );
    }

    /// Sync only cursor position to outline (no content extraction).
    fn sync_outline_cursor(&mut self, cursor_line: usize) {
        for group in &mut self.layout_manager.panel_groups {
            for panel in group.panels_mut() {
                if let Some(outline) = panel
                    .as_any_mut()
                    .downcast_mut::<termide_panel_outline::OutlinePanel>()
                {
                    outline.sync_cursor_line(cursor_line);
                    return;
                }
            }
        }
    }

    /// Re-extract outline symbols when the tracked file changed on disk.
    pub(in crate::app) fn notify_outline_on_fs_change(
        &mut self,
        changed_paths: &std::collections::HashSet<std::path::PathBuf>,
    ) {
        if changed_paths.is_empty() {
            return;
        }
        // Check if outline tracks one of the changed files
        let tracked: Option<std::path::PathBuf> = self.find_outline_tracked_file();
        let Some(tracked_path) = tracked else {
            return;
        };
        if !changed_paths.contains(&tracked_path) {
            return;
        }
        // File changed on disk — re-extract from editor's current content
        self.notify_outline_file_opened();
    }

    /// Find the file path currently tracked by the outline panel.
    fn find_outline_tracked_file(&self) -> Option<std::path::PathBuf> {
        for group in &self.layout_manager.panel_groups {
            for panel in group.panels() {
                if let Some(outline) = panel
                    .as_any()
                    .downcast_ref::<termide_panel_outline::OutlinePanel>()
                {
                    return outline.tracked_file().map(|p| p.to_path_buf());
                }
            }
        }
        None
    }

    /// Populate the outline panel from any editor found in the layout.
    /// Used on first open when the outline itself may already be focused.
    pub(in crate::app) fn populate_outline_from_any_editor(&mut self) {
        // The active editor first: the first one in the layout may be
        // another file.
        if let Some((path, content, language, cursor_line)) = self.collect_editor_info_for_outline()
        {
            self.push_to_outline(path, &content, language.as_deref(), Some(cursor_line));
            return;
        }
        let editor_info: Option<(Option<std::path::PathBuf>, String, Option<String>)> = {
            let mut info = None;
            for panel in self.layout_manager.iter_all_panels_mut() {
                if let Some(editor) = panel.as_editor_mut() {
                    let path = editor.file_path().map(|p| p.to_path_buf());
                    let content = editor.content_string();
                    let language = path
                        .as_ref()
                        .and_then(|p| termide_highlight::detect_language(p))
                        .map(|s| s.to_string());
                    info = Some((path, content, language));
                    break;
                }
            }
            info
        };

        if let Some((path, content, language)) = editor_info {
            self.push_to_outline(path, &content, language.as_deref(), None);
        }
    }

    /// Apply pending outline navigation to the editor (called from tick).
    pub(in crate::app) fn apply_outline_navigation(&mut self) {
        // Collect pending navigation from outline panel
        let nav: Option<termide_panel_outline::OutlineNavigation> = {
            let mut result = None;
            for group in &mut self.layout_manager.panel_groups {
                for panel in group.panels_mut() {
                    if let Some(outline) = panel
                        .as_any_mut()
                        .downcast_mut::<termide_panel_outline::OutlinePanel>()
                    {
                        result = outline.take_pending_navigation();
                        break;
                    }
                }
                if result.is_some() {
                    break;
                }
            }
            result
        };

        // Find the matching editor, expand it if collapsed, and navigate
        if let Some(nav) = nav {
            let mut target: Option<(usize, usize)> = None;
            for (gi, group) in self.layout_manager.panel_groups.iter().enumerate() {
                for (pi, panel) in group.panels().iter().enumerate() {
                    if let Some(editor) = panel.as_editor() {
                        if editor.file_path() == Some(&nav.path) {
                            target = Some((gi, pi));
                            break;
                        }
                    }
                }
                if target.is_some() {
                    break;
                }
            }

            if let Some((gi, pi)) = target {
                // Expand the editor panel if it's collapsed
                if let Some(group) = self.layout_manager.panel_groups.get_mut(gi) {
                    group.set_expanded(pi);
                }
                // Now navigate
                if let Some(group) = self.layout_manager.panel_groups.get_mut(gi) {
                    if let Some(panel) = group.panels_mut().get_mut(pi) {
                        if let Some(editor) = panel.as_editor_mut() {
                            editor.goto_position(nav.line, nav.column);
                        }
                    }
                }
            }
        }
    }

    /// Push collected editor data into the outline panel (if it exists).
    fn push_to_outline(
        &mut self,
        path: Option<std::path::PathBuf>,
        content: &str,
        language: Option<&str>,
        cursor_line: Option<usize>,
    ) {
        let mut symbol_lines_for_editor = Vec::new();
        'outer: for group in &mut self.layout_manager.panel_groups {
            for panel in group.panels_mut() {
                if let Some(outline) = panel
                    .as_any_mut()
                    .downcast_mut::<termide_panel_outline::OutlinePanel>()
                {
                    outline.update_content(path, content, language);
                    if let Some(line) = cursor_line {
                        outline.sync_cursor_line(line);
                    }
                    symbol_lines_for_editor = outline.symbol_lines();
                    break 'outer;
                }
            }
        }
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                editor.set_symbol_lines(symbol_lines_for_editor);
            }
        }
    }
}
