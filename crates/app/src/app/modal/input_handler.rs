//! Input modal result handling.

// Note: PanelExt is used for FileManager file operations (create file/dir).

use anyhow::Result;
use std::path::PathBuf;

use super::super::App;
use crate::state::{ActiveModal, PendingAction};
use crate::PanelExt;
use termide_i18n as i18n;
use termide_modal::SaveAsResult;
use termide_panel_file_manager::{CreateOutcome, FileManager};

/// Whether `s` is a database connection URL the DB viewer handles.
fn is_database_url(s: &str) -> bool {
    s.starts_with("postgres://")
        || s.starts_with("postgresql://")
        || s.starts_with("mysql://")
        || s.starts_with("mariadb://")
        || s.starts_with("sqlite://")
}

/// The path a Save As answer names: `~` expanded, a relative name taken
/// against `directory`.
fn save_as_target(directory: &std::path::Path, input: &str) -> PathBuf {
    let input_path = termide_ui::expand_tilde(input);
    if input_path.is_absolute() {
        input_path
    } else {
        directory.join(input)
    }
}

/// Whether saving as `target` would replace something other than `current`,
/// the file being saved: anything there — a dangling link too — is asked
/// about first.
fn save_as_replaces(target: &std::path::Path, current: Option<&std::path::Path>) -> bool {
    if std::fs::symlink_metadata(target).is_err() {
        return false;
    }
    let Some(current) = current else {
        return true;
    };
    if target == current {
        return false;
    }
    match (dunce::canonicalize(target), dunce::canonicalize(current)) {
        (Ok(a), Ok(b)) => a != b,
        _ => true,
    }
}

impl App {
    /// Open a path typed in a viewer's "go to" prompt, routed to the
    /// appropriate viewer by type. Relative paths resolve against `base_dir`;
    /// a leading `~/` expands to the home directory.
    pub(in crate::app) fn handle_view_path(
        &mut self,
        base_dir: PathBuf,
        value: termide_modal::ModalOutput,
    ) -> Result<()> {
        let Some(input) = value.get::<String>() else {
            return Ok(());
        };
        let input = input.trim();
        if input.is_empty() {
            return Ok(());
        }

        // A database connection URL opens the read-only database viewer.
        if is_database_url(input) {
            self.open_database(input.to_string());
            return Ok(());
        }

        // An http(s) address is fetched over the network and opened by
        // content-type (the viewer-as-browser path).
        if input.starts_with("http://") || input.starts_with("https://") {
            self.start_url_fetch(input.to_string());
            return Ok(());
        }

        let path = super::path_suggestions::resolve_typed_path(input, &base_dir);

        if !path.exists() {
            self.show_error_modal(
                termide_i18n::t().nav_no_such_path_fmt(&path.display().to_string()),
            );
            return Ok(());
        }

        self.open_local_path(path)
    }

    /// Open an existing local `path` where it belongs: a directory in the
    /// file manager, a file by its type — the rendered viewers, the image
    /// preview, the database viewer, else the read-only editor. Open… and a
    /// followed link share it.
    pub(in crate::app) fn open_local_path(&mut self, path: PathBuf) -> Result<()> {
        // A directory opens in the file manager.
        if path.is_dir() {
            self.add_panel(Box::new(FileManager::new_with_path(path)));
            self.state.needs_watcher_registration = true;
            self.auto_save_layout();
            return Ok(());
        }

        // Route a file by type, mirroring the file manager's View-mode mapping.
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "html" | "htm" => self.event_view_html(path),
            "md" | "markdown" => self.event_view_markdown(path),
            "mmd" | "mermaid" => self.event_view_mermaid(path),
            "csv" | "tsv" => self.event_view_csv(path),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tiff" | "tif" => {
                self.event_preview_media(path)
            }
            "db" | "sqlite" | "sqlite3" | "db3" => self.event_view_database(path),
            _ => self.event_view_file(path),
        }
    }

    /// Handle file creation
    pub(in crate::app) fn handle_create_file(
        &mut self,
        _directory: PathBuf,
        value: termide_modal::ModalOutput,
    ) -> Result<()> {
        self.create_entry_in_file_manager(value, false)
    }

    /// Handle directory creation
    pub(in crate::app) fn handle_create_directory(
        &mut self,
        _directory: PathBuf,
        value: termide_modal::ModalOutput,
    ) -> Result<()> {
        self.create_entry_in_file_manager(value, true)
    }

    /// Shared helper for creating a file or directory in the active FileManager.
    fn create_entry_in_file_manager(
        &mut self,
        value: termide_modal::ModalOutput,
        is_directory: bool,
    ) -> Result<()> {
        if let Some(name) = value.get::<String>() {
            let t = i18n::t();
            let result = if let Some(panel) = self.layout_manager.active_panel_mut() {
                if let Some(fm) = panel.as_file_manager_mut() {
                    let result = if is_directory {
                        fm.create_directory(name.clone())
                    } else {
                        fm.create_file(name.clone())
                    };
                    // A remote create only starts here; reloading now would
                    // replace the pending operation with a listing that cannot
                    // yet show the new entry. The panel's own tick reloads and
                    // reports it when the round-trip lands.
                    if matches!(result, Ok(CreateOutcome::Done)) {
                        let _ = fm.load_directory();
                    }
                    Some(result)
                } else {
                    log::error!("FileManager panel could not be accessed");
                    None
                }
            } else {
                log::error!("FileManager not found");
                None
            };

            if let Some(result) = result {
                match result {
                    Ok(CreateOutcome::Done) => {
                        let msg = if is_directory {
                            t.status_dir_created(name)
                        } else {
                            t.status_file_created(name)
                        };
                        self.state.set_info(msg);
                    }
                    // The FileManager reports the outcome once the remote
                    // answers — saying "created" here would be a guess.
                    Ok(CreateOutcome::Pending) => {}
                    Err(e) => {
                        let kind = if is_directory { "directory" } else { "file" };
                        log::error!("{} creation error '{}': {}", kind, name, e);
                        let error = e.to_string();
                        let error_msg = if is_directory {
                            t.create_dir_failed_fmt(name, &error)
                        } else {
                            t.create_file_failed_fmt(name, &error)
                        };
                        self.show_error_modal(error_msg);
                    }
                }
            }
        }
        Ok(())
    }

    /// Handle saving file with new name. A name that is taken by another
    /// file is replaced only once the user confirms it.
    pub(in crate::app) fn handle_save_file_as(
        &mut self,
        directory: PathBuf,
        value: termide_modal::ModalOutput,
    ) -> Result<()> {
        let Some(result) = value.get::<SaveAsResult>() else {
            return Ok(());
        };
        let file_path = save_as_target(&directory, &result.path);
        let executable = result.executable;
        let current = self
            .layout_manager
            .active_panel_mut()
            .and_then(|panel| panel.as_editor_mut())
            .and_then(|editor| editor.file_path().map(PathBuf::from));
        if save_as_replaces(&file_path, current.as_deref()) {
            self.confirm_save_as_replace(
                &file_path,
                PendingAction::ConfirmSaveFileAs {
                    path: file_path.clone(),
                    executable,
                },
            );
            return Ok(());
        }
        self.save_editor_as(file_path, executable);
        Ok(())
    }

    /// Save As onto a taken name, answered.
    pub(in crate::app) fn handle_confirm_save_file_as(
        &mut self,
        path: PathBuf,
        executable: bool,
        value: termide_modal::ModalOutput,
    ) -> Result<()> {
        if value.get::<bool>().copied().unwrap_or(false) {
            self.save_editor_as(path, executable);
        }
        Ok(())
    }

    /// Ask whether to replace the existing `path`; `action` carries on.
    fn confirm_save_as_replace(&mut self, path: &std::path::Path, action: PendingAction) {
        let t = i18n::t();
        let modal = termide_modal::ConfirmModal::new(
            t.modal_confirm_title(),
            // The name alone: the confirm box does not wrap a long path.
            t.save_as_replace_fmt(&path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into(),
            )),
        );
        self.state
            .set_pending_action(action, ActiveModal::Confirm(Box::new(modal)));
    }

    /// Save the active editor as `file_path`, set its executable bit when
    /// asked, and tell its language server.
    fn save_editor_as(&mut self, file_path: PathBuf, executable: bool) {
        // Store info needed for LSP notification (before mutable borrow)
        let mut lsp_info: Option<(String, PathBuf)> = None;
        #[cfg(unix)]
        let mut saved_path: Option<PathBuf> = None;
        #[cfg(unix)]
        let make_executable = executable;
        #[cfg(not(unix))]
        let _ = executable;

        {
            let t = i18n::t();

            // Get active Editor panel and save file
            if let Some(panel) = self.layout_manager.active_panel_mut() {
                if let Some(editor) = panel.as_editor_mut() {
                    let display_path = file_path.display().to_string();

                    match editor.save_file_as(file_path.clone()) {
                        Ok(_) => {
                            self.state.set_info(t.status_file_saved(&display_path));
                            #[cfg(unix)]
                            {
                                saved_path = Some(file_path.clone());
                            }

                            // Collect LSP info for didSave notification
                            if let Some(lang) = editor.lsp_language() {
                                lsp_info = Some((lang.to_string(), file_path));
                            }
                        }
                        Err(e) => {
                            log::error!("Save error '{}': {}", display_path, e);
                            self.show_error_modal(t.status_error_save(&e.to_string()));
                        }
                    }
                }
            }
        }

        // Set executable permission if requested (Unix only — Windows has no executable bit)
        #[cfg(unix)]
        if make_executable {
            if let Some(ref path) = saved_path {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(metadata) = std::fs::metadata(path) {
                    let mut perms = metadata.permissions();
                    let mode = perms.mode();
                    // Add execute bits for user, group, and others (where read is set)
                    let new_mode = mode | ((mode & 0o444) >> 2);
                    perms.set_mode(new_mode);
                    if let Err(e) = std::fs::set_permissions(path, perms) {
                        log::warn!("Failed to set executable permission: {}", e);
                    }
                }
            }
        }

        // Send LSP didSave notification (triggers full analysis for semantic errors)
        if let Some((lang, file_path)) = lsp_info {
            if let Some(ref lsp_manager) = self.state.lsp_manager {
                lsp_manager.did_save(&lang, &file_path, None);
            }
        }
    }

    /// Write in-memory `content` to a Save-As-chosen path (used by read-only
    /// viewers exporting their source). Relative names resolve against
    /// `directory`.
    pub(in crate::app) fn handle_save_content_as(
        &mut self,
        directory: PathBuf,
        content: String,
        value: termide_modal::ModalOutput,
    ) -> Result<()> {
        let Some(result) = value.get::<SaveAsResult>() else {
            return Ok(());
        };
        let file_path = save_as_target(&directory, &result.path);
        if save_as_replaces(&file_path, None) {
            self.confirm_save_as_replace(
                &file_path,
                PendingAction::ConfirmSaveContentAs {
                    path: file_path.clone(),
                    content,
                },
            );
            return Ok(());
        }
        self.write_content_as(&file_path, &content);
        Ok(())
    }

    /// Exporting a viewer's content onto a taken name, answered.
    pub(in crate::app) fn handle_confirm_save_content_as(
        &mut self,
        path: PathBuf,
        content: String,
        value: termide_modal::ModalOutput,
    ) -> Result<()> {
        if value.get::<bool>().copied().unwrap_or(false) {
            self.write_content_as(&path, &content);
        }
        Ok(())
    }

    fn write_content_as(&mut self, file_path: &std::path::Path, content: &str) {
        let t = i18n::t();
        let display_path = file_path.display().to_string();
        match termide_fsutil::write_atomic(file_path, content.as_bytes()) {
            Ok(()) => self.state.set_info(t.status_file_saved(&display_path)),
            Err(e) => {
                log::error!("Save error '{}': {}", display_path, e);
                self.show_error_modal(t.status_error_save(&e.to_string()));
            }
        }
    }

    /// Handle git stash push: user provided stash message.
    pub(in crate::app) fn handle_git_stash_push(
        &mut self,
        repo_path: std::path::PathBuf,
        value: termide_modal::ModalOutput,
    ) -> Result<()> {
        let message = match value.get::<String>() {
            Some(s) => s.trim().to_string(),
            None => return Ok(()),
        };
        let include_untracked = self.state.stash.include_untracked;
        self.state.stash.include_untracked = false; // reset
        match termide_git::stash_push(&repo_path, &message, include_untracked) {
            Ok(()) => {
                self.state
                    .set_info(termide_i18n::t().git_stash_created_msg().to_string());
                self.send_git_update(&repo_path);
            }
            Err(e) => {
                self.show_error_modal(termide_i18n::t().git_stash_failed_fmt(&e.to_string()));
            }
        }
        Ok(())
    }

    /// Handle LSP rename symbol: user confirmed new name, send request to LSP.
    pub(in crate::app) fn handle_lsp_rename_symbol(
        &mut self,
        _file_path: std::path::PathBuf,
        line: usize,
        column: usize,
        value: termide_modal::ModalOutput,
    ) -> anyhow::Result<()> {
        let new_name = match value.get::<String>() {
            Some(s) => s.trim().to_string(),
            None => return Ok(()),
        };
        if new_name.is_empty() {
            return Ok(());
        }

        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                if let Some(ref lsp_manager) = self.state.lsp_manager {
                    editor.request_rename(line, column, new_name, lsp_manager);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Save As asks before replacing anything but the file being saved.
    #[test]
    fn save_as_asks_before_replacing_another_file() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("current.txt");
        let other = dir.path().join("other.txt");
        std::fs::write(&current, "a").unwrap();
        std::fs::write(&other, "b").unwrap();

        assert!(!save_as_replaces(
            &dir.path().join("new.txt"),
            Some(&current)
        ));
        assert!(!save_as_replaces(&current, Some(&current)));
        assert!(save_as_replaces(&other, Some(&current)));
        // An unnamed buffer or a viewer export has no file of its own.
        assert!(save_as_replaces(&other, None));
        #[cfg(unix)]
        {
            let dangling = dir.path().join("dangling");
            std::os::unix::fs::symlink(dir.path().join("gone"), &dangling).unwrap();
            assert!(save_as_replaces(&dangling, None));
        }

        assert_eq!(
            save_as_target(dir.path(), "x.txt"),
            dir.path().join("x.txt")
        );
        assert_eq!(
            save_as_target(dir.path(), "/abs/y"),
            PathBuf::from("/abs/y")
        );
    }
}
