//! Bookmarks menu actions — bookmarks dropdown, add/edit/delete bookmarks.

use anyhow::Result;
use std::path::PathBuf;

use super::super::App;
use crate::state::{ActiveModal, PendingAction};
use crate::PanelExt;
use termide_panel_file_manager::FileManager;
use termide_panel_terminal::Terminal;

impl App {
    /// Rename selected bookmark (F2) — edit description via InputModal
    fn rename_selected_bookmark(&mut self) -> Result<()> {
        let items = termide_ui_render::get_bookmarks_items(
            &self.state.bookmarks,
            self.state.project_bookmarks.as_ref(),
        );
        let sel = self.state.ui.bookmarks_submenu.selected;
        if let Some(item) = items.get(sel) {
            if item.is_separator || item.has_submenu {
                return Ok(());
            }
            // Find bookmark — clone data before mutating state
            let key = item.key.clone();
            if let Some(bm) = self.state.bookmarks.find(&key) {
                let current_name = bm.display_name().to_string();
                let bm_path = bm.path.clone();
                let bm_group = bm.group.clone();
                let bm_is_project = bm.is_project;
                self.state.close_menu();
                let t = termide_i18n::t();
                let modal = termide_modal::InputModal::with_default(
                    t.help_desc_rename(),
                    t.help_desc_rename(),
                    &current_name,
                );
                self.state.set_pending_action(
                    termide_state::PendingAction::RenameBookmark {
                        path: bm_path,
                        group: bm_group,
                        is_project: bm_is_project,
                        selected: sel,
                    },
                    crate::state::ActiveModal::Input(Box::new(modal)),
                );
            }
        }
        Ok(())
    }

    /// Rename selected bookmark in nested submenu (F2)
    fn rename_selected_nested_bookmark(&mut self) -> Result<()> {
        let group_name = match &self.state.ui.current_bookmarks_group {
            Some(name) => name.clone(),
            None => return Ok(()),
        };
        let is_project = self.state.ui.current_bookmarks_group_is_project;
        let items = termide_ui_render::get_bookmarks_group_items(
            &self.state.bookmarks,
            self.state.project_bookmarks.as_ref(),
            &group_name,
            is_project,
        );
        let sel = self.state.ui.bookmarks_nested.selected;
        if let Some(item) = items.get(sel) {
            if item.is_separator {
                return Ok(());
            }
            let key = item.key.clone();
            if let Some(bm) = self.state.bookmarks.find(&key) {
                let current_name = bm.display_name().to_string();
                let bm_path = bm.path.clone();
                let bm_group = bm.group.clone();
                let bm_is_project = bm.is_project;
                self.state.close_menu();
                let t = termide_i18n::t();
                let modal = termide_modal::InputModal::with_default(
                    t.help_desc_rename(),
                    t.help_desc_rename(),
                    &current_name,
                );
                self.state.set_pending_action(
                    termide_state::PendingAction::RenameBookmark {
                        path: bm_path,
                        group: bm_group,
                        is_project: bm_is_project,
                        selected: sel,
                    },
                    crate::state::ActiveModal::Input(Box::new(modal)),
                );
            }
        }
        Ok(())
    }

    // =========================================================================
    // Bookmarks submenu handling
    // =========================================================================

    /// Handle keyboard event in Bookmarks submenu
    pub(in crate::app) fn handle_bookmarks_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        use super::{navigate_submenu, SubmenuNavAction};

        // If nested submenu is open, delegate to nested handler
        if self.state.ui.bookmarks_nested.open {
            return self.handle_bookmarks_nested_submenu_key(key);
        }

        use termide_ui_render::get_bookmarks_items;
        let items =
            get_bookmarks_items(&self.state.bookmarks, self.state.project_bookmarks.as_ref());
        let item_count = items.len();
        let separators: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.is_separator)
            .map(|(idx, _)| idx)
            .collect();

        match navigate_submenu(
            &key,
            &mut self.state.ui.bookmarks_submenu,
            item_count,
            &separators,
        ) {
            SubmenuNavAction::Close => self.state.close_menu(),
            SubmenuNavAction::Execute => self.execute_bookmarks_submenu_action()?,
            SubmenuNavAction::Right => {
                use termide_ui_render::get_bookmarks_items;
                let items = get_bookmarks_items(
                    &self.state.bookmarks,
                    self.state.project_bookmarks.as_ref(),
                );
                let sel = self.state.ui.bookmarks_submenu.selected;
                if items.get(sel).is_some_and(|i| i.has_submenu) {
                    self.execute_bookmarks_submenu_action()?;
                } else {
                    self.switch_to_next_menu()?;
                }
            }
            SubmenuNavAction::Left => self.switch_to_prev_menu()?,
            SubmenuNavAction::Rename => self.rename_selected_bookmark()?,
            SubmenuNavAction::Edit => self.edit_selected_bookmark()?,
            SubmenuNavAction::None => {}
            SubmenuNavAction::Delete => self.delete_selected_bookmark()?,
        }
        Ok(())
    }

    /// Execute action for selected Bookmarks submenu item
    pub(in crate::app) fn execute_bookmarks_submenu_action(&mut self) -> Result<()> {
        let selected = self.state.ui.bookmarks_submenu.selected;

        if selected == 0 {
            // Add current - open add bookmark modal
            self.state.close_menu();
            self.handle_add_bookmark()?;
            return Ok(());
        }

        // Index 1: separator (should not be reachable)
        // Indices 2+: actual bookmarks

        // Build the same item list as the dropdown to match indices
        use termide_ui_render::get_bookmarks_items;
        let items =
            get_bookmarks_items(&self.state.bookmarks, self.state.project_bookmarks.as_ref());

        if let Some(item) = items.get(selected) {
            if item.is_separator || item.key.is_empty() {
                return Ok(());
            }
            if item.has_submenu {
                // Toggle: if this nested submenu is already open for this group, close it.
                if self.state.ui.bookmarks_nested.open
                    && self.state.ui.current_bookmarks_group.as_deref() == Some(item.key.as_str())
                {
                    self.state.close_bookmarks_nested_submenu();
                } else {
                    // Group — open nested submenu
                    self.state
                        .open_bookmarks_nested_submenu(item.key.clone(), item.is_project);
                }
            } else {
                // Direct bookmark — open it
                let path = item.key.clone();
                let bookmark_type = self
                    .find_bookmark(&path)
                    .map(|b| b.bookmark_type())
                    .unwrap_or(termide_config::BookmarkType::Unknown);
                self.state.close_menu();
                self.open_bookmark(&path, bookmark_type)?;
            }
        }

        Ok(())
    }

    /// Handle keyboard event in Bookmarks nested submenu (group items)
    fn handle_bookmarks_nested_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        use super::{navigate_submenu, SubmenuNavAction};
        use termide_ui_render::get_bookmarks_group_items;

        let group_name = self.state.ui.current_bookmarks_group.clone();

        let is_project = self.state.ui.current_bookmarks_group_is_project;
        let item_count = group_name
            .as_ref()
            .map(|name| {
                get_bookmarks_group_items(
                    &self.state.bookmarks,
                    self.state.project_bookmarks.as_ref(),
                    name,
                    is_project,
                )
                .len()
            })
            .unwrap_or(0);

        match navigate_submenu(&key, &mut self.state.ui.bookmarks_nested, item_count, &[]) {
            SubmenuNavAction::Close | SubmenuNavAction::Left => {
                self.state.close_bookmarks_nested_submenu();
            }
            SubmenuNavAction::Execute => self.execute_bookmarks_nested_action()?,
            SubmenuNavAction::Right => self.switch_to_next_menu()?,
            SubmenuNavAction::Rename => self.rename_selected_nested_bookmark()?,
            SubmenuNavAction::Edit => self.edit_selected_nested_bookmark()?,
            SubmenuNavAction::None => {}
            SubmenuNavAction::Delete => self.delete_selected_nested_bookmark()?,
        }
        Ok(())
    }

    /// Execute action for selected item in Bookmarks nested submenu
    pub(in crate::app) fn execute_bookmarks_nested_action(&mut self) -> Result<()> {
        use termide_ui_render::get_bookmarks_group_items;

        let group_name = match &self.state.ui.current_bookmarks_group {
            Some(name) => name.clone(),
            None => return Ok(()),
        };

        let is_project = self.state.ui.current_bookmarks_group_is_project;
        let items = get_bookmarks_group_items(
            &self.state.bookmarks,
            self.state.project_bookmarks.as_ref(),
            &group_name,
            is_project,
        );

        if let Some(item) = items.get(self.state.ui.bookmarks_nested.selected) {
            let path = item.key.clone();
            let bookmark_type = self
                .find_bookmark(&path)
                .map(|b| b.bookmark_type())
                .unwrap_or(termide_config::BookmarkType::Unknown);
            self.state.close_menu();
            self.open_bookmark(&path, bookmark_type)?;
        }

        Ok(())
    }

    /// Edit selected bookmark from main bookmarks submenu (F4)
    fn edit_selected_bookmark(&mut self) -> Result<()> {
        use termide_ui_render::get_bookmarks_items;
        let items =
            get_bookmarks_items(&self.state.bookmarks, self.state.project_bookmarks.as_ref());
        let sel = self.state.ui.bookmarks_submenu.selected;
        self.open_edit_bookmark_modal(items.get(sel))
    }

    /// Edit selected bookmark from nested bookmarks submenu (F4)
    fn edit_selected_nested_bookmark(&mut self) -> Result<()> {
        use termide_ui_render::get_bookmarks_group_items;
        let group_name = match &self.state.ui.current_bookmarks_group {
            Some(name) => name.clone(),
            None => return Ok(()),
        };
        let is_project = self.state.ui.current_bookmarks_group_is_project;
        let items = get_bookmarks_group_items(
            &self.state.bookmarks,
            self.state.project_bookmarks.as_ref(),
            &group_name,
            is_project,
        );
        let sel = self.state.ui.bookmarks_nested.selected;
        self.open_edit_bookmark_modal(items.get(sel))
    }

    /// Open the bookmark add modal pre-filled with existing bookmark data for editing
    fn open_edit_bookmark_modal(
        &mut self,
        item: Option<&termide_ui_render::DropdownItem>,
    ) -> Result<()> {
        use termide_modal::BookmarkAddModal;
        let Some(item) = item else { return Ok(()) };
        if item.is_separator || item.has_submenu || item.key.is_empty() {
            return Ok(());
        }

        let path = &item.key;
        // Use the current nested group context to find the correct bookmark
        // (same path can exist in different groups)
        let current_group = self.state.ui.current_bookmarks_group.as_deref();
        let bookmark = self.find_bookmark_in_group(path, current_group, item.is_project);
        let description = bookmark.and_then(|b| b.description.as_deref());
        let group = bookmark.and_then(|b| b.group.as_deref());

        let all_groups = self.all_bookmark_group_names();

        let modal = BookmarkAddModal::new(None, all_groups).with_values(
            path,
            description,
            group,
            item.is_project,
        );

        let selected = self.state.ui.bookmarks_submenu.selected;
        let group = self.state.ui.current_bookmarks_group.clone();

        let original_group = bookmark.and_then(|b| b.group.clone());
        self.state.close_menu();
        self.state.set_pending_action(
            PendingAction::EditBookmark {
                original_path: path.clone(),
                original_group,
                was_project: item.is_project,
                group,
                is_project: item.is_project,
                selected,
            },
            ActiveModal::BookmarkAdd(Box::new(modal)),
        );
        Ok(())
    }

    /// Delete selected bookmark from main bookmarks submenu
    fn delete_selected_bookmark(&mut self) -> Result<()> {
        use termide_ui_render::get_bookmarks_items;
        let items =
            get_bookmarks_items(&self.state.bookmarks, self.state.project_bookmarks.as_ref());
        let sel = self.state.ui.bookmarks_submenu.selected;
        self.confirm_delete_bookmark_item(items.get(sel), None, sel)
    }

    /// Delete selected bookmark from nested bookmarks submenu (group items)
    fn delete_selected_nested_bookmark(&mut self) -> Result<()> {
        use termide_ui_render::get_bookmarks_group_items;
        let group_name = match &self.state.ui.current_bookmarks_group {
            Some(name) => name.clone(),
            None => return Ok(()),
        };
        let is_project = self.state.ui.current_bookmarks_group_is_project;
        let items = get_bookmarks_group_items(
            &self.state.bookmarks,
            self.state.project_bookmarks.as_ref(),
            &group_name,
            is_project,
        );
        let parent_sel = self.state.ui.bookmarks_submenu.selected;
        let sel = self.state.ui.bookmarks_nested.selected;
        self.confirm_delete_bookmark_item(items.get(sel), Some(group_name), parent_sel)
    }

    /// Open a confirmation modal to delete a bookmark item or group
    fn confirm_delete_bookmark_item(
        &mut self,
        item: Option<&termide_ui_render::DropdownItem>,
        group: Option<String>,
        selected: usize,
    ) -> Result<()> {
        use termide_modal::ConfirmModal;
        let Some(item) = item else { return Ok(()) };
        if item.is_separator || item.key.is_empty() {
            return Ok(());
        }

        let (action, modal) = if item.has_submenu {
            let group_name = &item.key;
            (
                PendingAction::DeleteBookmarkGroup {
                    group: group_name.clone(),
                    is_project: item.is_project,
                    selected,
                },
                ConfirmModal::new("Delete bookmark group?", group_name.to_string()),
            )
        } else {
            (
                PendingAction::DeleteBookmark {
                    path: item.key.clone(),
                    is_project: item.is_project,
                    group,
                    selected,
                },
                ConfirmModal::new("Delete bookmark?", item.label.to_string()),
            )
        };

        self.state.close_menu();
        self.state
            .set_pending_action(action, ActiveModal::Confirm(Box::new(modal)));
        Ok(())
    }

    /// Handle adding a bookmark
    pub(in crate::app) fn handle_add_bookmark(&mut self) -> Result<()> {
        use termide_modal::BookmarkAddModal;

        let selected = self.state.ui.bookmarks_submenu.selected;
        let group = self.state.ui.current_bookmarks_group.clone();
        let is_project = self.state.ui.current_bookmarks_group_is_project;

        let current_path = self.get_current_bookmark_path();
        let existing_groups = self.all_bookmark_group_names();

        let modal = BookmarkAddModal::new(current_path, existing_groups);
        self.state.set_pending_action(
            PendingAction::AddBookmark {
                group,
                is_project,
                selected,
            },
            ActiveModal::BookmarkAdd(Box::new(modal)),
        );

        Ok(())
    }

    /// Get current path from active panel for bookmarking
    fn get_current_bookmark_path(&self) -> Option<String> {
        if let Some(panel) = self.layout_manager.active_panel() {
            // Try to get file path from editor
            if let Some(editor) = panel.as_editor() {
                if let Some(path) = editor.file_path() {
                    return Some(path.display().to_string());
                }
            }
            // Fall back to working directory
            if let Some(cwd) = panel.get_working_directory() {
                return Some(cwd.display().to_string());
            }
        }
        None
    }

    /// Reopen bookmarks menu after modal (delete confirmation/cancel).
    /// If `group` is Some, also opens the nested submenu for that group
    /// and sets the parent cursor to the group's position.
    pub(in crate::app) fn reopen_bookmarks_menu(
        &mut self,
        group: Option<String>,
        is_project: bool,
        fallback_selected: usize,
    ) {
        use termide_ui_render::{get_bookmarks_items, menu::BOOKMARKS_MENU_INDEX};
        self.state.ui.menu_open = true;
        self.state.ui.selected_menu_item = Some(BOOKMARKS_MENU_INDEX);
        self.state.open_bookmarks_submenu();

        if let Some(group_name) = group {
            // Find the group's index in the current items list
            let items =
                get_bookmarks_items(&self.state.bookmarks, self.state.project_bookmarks.as_ref());
            let group_idx = items
                .iter()
                .position(|i| i.has_submenu && i.key == group_name && i.is_project == is_project)
                .unwrap_or(fallback_selected);
            self.state.ui.bookmarks_submenu.selected = group_idx;
            self.state
                .open_bookmarks_nested_submenu(group_name, is_project);
        } else {
            self.state.ui.bookmarks_submenu.selected = fallback_selected;
        }
    }

    /// Get deduplicated, sorted list of all group names from global and project bookmarks.
    fn all_bookmark_group_names(&self) -> Vec<String> {
        let mut groups = self.state.bookmarks.group_names();
        if let Some(ref proj) = self.state.project_bookmarks {
            for g in proj.group_names() {
                if !groups.contains(&g) {
                    groups.push(g);
                }
            }
        }
        groups.sort();
        groups
    }

    /// Find a bookmark by path in project or global bookmarks.
    fn find_bookmark(&self, path: &str) -> Option<&termide_config::Bookmark> {
        self.state
            .project_bookmarks
            .as_ref()
            .and_then(|p| p.find(path))
            .or_else(|| self.state.bookmarks.find(path))
    }

    /// Find a bookmark by path and group in project or global bookmarks.
    fn find_bookmark_in_group(
        &self,
        path: &str,
        group: Option<&str>,
        is_project: bool,
    ) -> Option<&termide_config::Bookmark> {
        if is_project {
            self.state
                .project_bookmarks
                .as_ref()
                .and_then(|p| p.find_in_group(path, group))
        } else {
            self.state.bookmarks.find_in_group(path, group)
        }
    }

    /// Open a bookmark based on its type
    /// Open a local path in the rendered viewer when its type is one the
    /// viewers handle (HTML / Markdown / Mermaid / image). Returns `false` for
    /// other types so the caller can fall back (editor, external opener).
    fn open_in_viewer_by_ext(&mut self, path: &str) -> bool {
        let p = PathBuf::from(path);
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "html" | "htm" => self.event_view_html(p).is_ok(),
            "md" | "markdown" => self.event_view_markdown(p).is_ok(),
            "mmd" | "mermaid" => self.event_view_mermaid(p).is_ok(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tiff" | "tif" => {
                self.event_preview_media(p).is_ok()
            }
            _ => false,
        }
    }

    /// Open the read-only database viewer for a connection URL
    /// (`sqlite://`, `postgres://`, `mysql://`).
    pub(in crate::app) fn open_database(&mut self, url: String) {
        self.close_help_panels();
        let panel = termide_panel_db::DbPanel::new(url, String::new());
        self.add_panel(Box::new(panel));
        self.auto_save_session();
    }

    /// Open a local SQLite file in the database viewer, deriving a
    /// `sqlite:///<abs-path>` URL from the file path.
    pub(in crate::app) fn event_view_database(&mut self, path: PathBuf) -> Result<()> {
        let abs = std::fs::canonicalize(&path).unwrap_or(path);
        self.open_database(format!("sqlite://{}", abs.display()));
        Ok(())
    }

    /// Hand a path/URL to the system's default application.
    fn open_path_external(&self, path: &str) {
        let _ = std::process::Command::new("xdg-open")
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    fn open_bookmark(
        &mut self,
        path: &str,
        bookmark_type: termide_config::BookmarkType,
    ) -> Result<()> {
        use termide_config::BookmarkType;

        match bookmark_type {
            BookmarkType::Directory => {
                // Check if active panel is a file manager - reuse it
                if let Some(panel) = self.layout_manager.active_panel_mut() {
                    if let Some(fm) = panel.as_file_manager_mut() {
                        let _ = fm.navigate_to(PathBuf::from(path));
                        self.state.needs_watcher_registration = true;
                        return Ok(());
                    }
                }
                // No active file manager - create new panel
                self.close_help_panels();
                let fm_panel = FileManager::new_with_path(PathBuf::from(path));
                self.add_panel(Box::new(fm_panel));
                self.auto_save_session();
            }
            BookmarkType::TextFile => {
                // Rendered types open in the viewer; everything else in the editor.
                self.close_help_panels();
                if !self.open_in_viewer_by_ext(path) {
                    let _ = self.open_editor_for_file(PathBuf::from(path));
                }
            }
            BookmarkType::ViewerFile => {
                // Images open in the image preview; other viewer files externally.
                self.close_help_panels();
                if !self.open_in_viewer_by_ext(path) {
                    self.open_path_external(path);
                }
            }
            BookmarkType::HttpLink => {
                // Open in the built-in viewer (text-mode browse) unless the user
                // configured links to open in the system browser.
                if self.state.config.viewer.open_links == termide_core::LinkOpen::External {
                    self.open_path_external(path);
                } else {
                    self.close_help_panels();
                    self.start_url_fetch(path.to_string());
                }
            }
            BookmarkType::SftpPath
            | BookmarkType::FtpPath
            | BookmarkType::SmbPath
            | BookmarkType::NfsPath => {
                // Navigate to remote path using VFS
                if let Some(panel) = self.layout_manager.active_panel_mut() {
                    if let Some(fm) = panel.as_file_manager_mut() {
                        let _ = fm.navigate_to_url(path);
                        self.state.needs_watcher_registration = true;
                        return Ok(());
                    }
                }
                // No active file manager - create new panel and navigate
                self.close_help_panels();
                let mut fm_panel = FileManager::new();
                let _ = fm_panel.navigate_to_url(path);
                self.add_panel(Box::new(fm_panel));
                self.auto_save_session();
            }
            BookmarkType::SshConnection => {
                // Open SSH connection in terminal
                // Parse ssh://[user@]host[:port] format into proper ssh command
                let ssh_cmd = {
                    let url_part = path.strip_prefix("ssh://").unwrap_or(path);
                    let mut cmd_parts = vec!["ssh".to_string()];

                    // Split off any path component (ignore it for SSH)
                    let authority = url_part.split('/').next().unwrap_or(url_part);

                    // Parse user@host:port format
                    let (user_host, port) = if let Some(colon_pos) = authority.rfind(':') {
                        // Check if what's after colon looks like a port number
                        let after_colon = &authority[colon_pos + 1..];
                        if after_colon.chars().all(|c| c.is_ascii_digit())
                            && !after_colon.is_empty()
                            && after_colon.parse::<u16>().is_ok_and(|p| p > 0)
                        {
                            (&authority[..colon_pos], Some(after_colon))
                        } else {
                            (authority, None)
                        }
                    } else {
                        (authority, None)
                    };

                    // Add port if specified
                    if let Some(port) = port {
                        cmd_parts.push("-p".to_string());
                        cmd_parts.push(port.to_string());
                    }

                    cmd_parts.push(user_host.to_string());
                    cmd_parts.join(" ")
                };

                self.close_help_panels();

                let width = self.state.terminal.width;
                let height = self.state.terminal.height;
                let term_height = height.saturating_sub(3);
                let term_width = width.saturating_sub(2);

                match Terminal::new_with_command(term_height, term_width, &ssh_cmd) {
                    Ok(terminal) => {
                        self.add_panel(Box::new(terminal));
                        self.auto_save_session();
                    }
                    Err(e) => {
                        log::error!("Failed to create SSH terminal: {}", e);
                        self.show_error_modal(format!("Failed to open SSH connection: {}", e));
                    }
                }
            }
            BookmarkType::Database => {
                // Open the read-only database viewer for this connection URL.
                self.open_database(path.to_string());
            }
            BookmarkType::Unknown => {
                // Try to determine type and handle
                let p = PathBuf::from(path);
                if p.is_dir() {
                    self.close_help_panels();
                    let fm_panel = FileManager::new_with_path(p);
                    self.add_panel(Box::new(fm_panel));
                    self.auto_save_session();
                } else if p.is_file() {
                    let _ = self.open_editor_for_file(p);
                }
            }
        }

        Ok(())
    }
}
