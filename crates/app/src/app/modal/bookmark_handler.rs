//! Bookmark-related modal result handlers.

use anyhow::Result;

use crate::app::App;
use crate::state::ActiveModal;

impl App {
    /// Handle bookmark add result
    /// Returns an error message if the bookmark could not be saved.
    pub(in crate::app) fn handle_add_bookmark_result(
        &mut self,
        value: termide_modal::ModalOutput,
    ) -> Result<Option<String>> {
        use std::path::Path;
        use termide_config::Bookmark;
        use termide_modal::BookmarkAddResult;

        let Some(result) = value.get::<BookmarkAddResult>() else {
            return Ok(None);
        };

        if result.path.is_empty() {
            return Ok(Some("Bookmark path cannot be empty".to_string()));
        }

        let mut bookmark = Bookmark::new(result.path.clone());

        let description = match &result.description {
            Some(desc) => desc.clone(),
            None => Path::new(&result.path)
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| result.path.clone()),
        };
        bookmark = bookmark.with_description(description);

        if let Some(group) = &result.group {
            bookmark = bookmark.with_group(group.clone());
        }

        if result.is_project {
            let proj = self
                .state
                .project_bookmarks
                .get_or_insert_with(Default::default);
            proj.bookmarks.push(bookmark);
            let proj_dir = self.state.project_root.join(".termide");
            if let Err(e) = std::fs::create_dir_all(&proj_dir) {
                return Ok(Some(format!("Failed to create .termide/: {e}")));
            }
            if let Err(e) = proj.save_to_dir(&proj_dir) {
                return Ok(Some(format!("Failed to save project bookmarks: {e}")));
            }
        } else {
            self.state.bookmarks.bookmarks.push(bookmark);
            if let Err(e) = self.state.bookmarks.save() {
                return Ok(Some(format!("Failed to save bookmarks: {e}")));
            }
        }
        Ok(None)
    }

    /// Handle bookmark edit result — remove old bookmark, add updated one.
    /// Returns an error message if saving failed.
    pub(in crate::app) fn handle_edit_bookmark_result(
        &mut self,
        value: termide_modal::ModalOutput,
        original_path: &str,
        original_group: Option<&str>,
        was_project: bool,
    ) -> Result<Option<String>> {
        use std::path::Path;
        use termide_config::Bookmark;
        use termide_modal::BookmarkAddResult;

        let Some(result) = value.get::<BookmarkAddResult>() else {
            return Ok(None);
        };

        if result.path.is_empty() {
            return Ok(Some("Bookmark path cannot be empty".to_string()));
        }

        // Remove old bookmark from its original location (by path + group)
        if was_project {
            if let Some(ref mut proj) = self.state.project_bookmarks {
                proj.remove_in_group(original_path, original_group);
            }
        } else {
            self.state
                .bookmarks
                .remove_in_group(original_path, original_group);
        }

        // Build new bookmark
        let mut bookmark = Bookmark::new(result.path.clone());
        let description = match &result.description {
            Some(desc) => desc.clone(),
            None => Path::new(&result.path)
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| result.path.clone()),
        };
        bookmark = bookmark.with_description(description);
        if let Some(group) = &result.group {
            bookmark = bookmark.with_group(group.clone());
        }

        // Add to target location
        if result.is_project {
            let proj = self
                .state
                .project_bookmarks
                .get_or_insert_with(Default::default);
            proj.bookmarks.push(bookmark);
            let proj_dir = self.state.project_root.join(".termide");
            if let Err(e) = std::fs::create_dir_all(&proj_dir) {
                return Ok(Some(format!("Failed to create .termide/: {e}")));
            }
            if let Err(e) = proj.save_to_dir(&proj_dir) {
                return Ok(Some(format!("Failed to save project bookmarks: {e}")));
            }
        } else {
            self.state.bookmarks.bookmarks.push(bookmark);
            if let Err(e) = self.state.bookmarks.save() {
                return Ok(Some(format!("Failed to save bookmarks: {e}")));
            }
        }

        // Save the source too if it changed location (project ↔ global)
        if was_project && !result.is_project {
            if let Some(ref proj) = self.state.project_bookmarks {
                let proj_dir = self.state.project_root.join(".termide");
                if let Err(e) = proj.save_to_dir(&proj_dir) {
                    log::error!(
                        "Failed to save project bookmarks to {}: {}",
                        proj_dir.display(),
                        e
                    );
                }
            }
        } else if !was_project && result.is_project {
            self.state.save_bookmarks();
        }

        Ok(None)
    }

    /// Show an error about a bookmark operation via InfoModal
    pub(in crate::app) fn show_bookmark_error(&mut self, message: &str) {
        use termide_modal::InfoModal;
        let modal = InfoModal::new(
            "Bookmark error",
            vec![("".to_string(), message.to_string())],
        );
        self.state.active_modal = Some(ActiveModal::Info(Box::new(modal)));
    }
}
