//! Input modal result handling.

// Note: PanelExt is used for FileManager file operations (create file/dir).
#![allow(deprecated)]

use anyhow::Result;
use std::path::PathBuf;

use super::super::App;
use crate::PanelExt;
use termide_i18n as i18n;
use termide_modal::SaveAsResult;

impl App {
    /// Open a path typed in a viewer's "go to" prompt, routed to the
    /// appropriate viewer by type. Relative paths resolve against `base_dir`;
    /// a leading `~/` expands to the home directory.
    pub(in crate::app) fn handle_view_path(
        &mut self,
        base_dir: PathBuf,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        let Some(input) = value.downcast_ref::<String>() else {
            return Ok(());
        };
        let input = input.trim();
        if input.is_empty() {
            return Ok(());
        }

        // An http(s) address is fetched over the network and opened by
        // content-type (the viewer-as-browser path).
        if input.starts_with("http://") || input.starts_with("https://") {
            self.start_url_fetch(input.to_string());
            return Ok(());
        }

        // Resolve to an absolute path: `~/…` → home, relative → against base_dir.
        let path = if let Some(rest) = input.strip_prefix("~/") {
            match dirs::home_dir() {
                Some(home) => home.join(rest),
                None => PathBuf::from(input),
            }
        } else {
            let p = PathBuf::from(input);
            if p.is_absolute() {
                p
            } else {
                base_dir.join(p)
            }
        };

        if !path.exists() {
            self.show_error_modal(format!("No such path: {}", path.display()));
            return Ok(());
        }
        if path.is_dir() {
            self.show_error_modal(format!("Not a file: {}", path.display()));
            return Ok(());
        }

        // Route by type, mirroring the file manager's View-mode mapping.
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "html" | "htm" => self.event_view_html(path),
            "md" | "markdown" => self.event_view_markdown(path),
            "mmd" | "mermaid" => self.event_view_mermaid(path),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tiff" | "tif" => {
                self.event_preview_media(path)
            }
            _ => self.event_view_file(path),
        }
    }

    /// Handle file creation
    pub(in crate::app) fn handle_create_file(
        &mut self,
        _directory: PathBuf,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        self.create_entry_in_file_manager(value, false)
    }

    /// Apply an SSH key passphrase the user typed and retry the git network
    /// operation that prompted for it (via the askpass helper).
    pub(in crate::app) fn handle_git_ssh_passphrase_retry(
        &mut self,
        operation: String,
        repo_path: PathBuf,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        use termide_core::GitOperationType;
        let Some(passphrase) = value.downcast_ref::<String>() else {
            return Ok(());
        };
        if passphrase.is_empty() {
            return Ok(());
        }
        // Cache for the session so subsequent ops reuse it without re-prompting.
        self.state.git_ssh_passphrase = Some(passphrase.clone());
        let op = match operation.as_str() {
            "push" => GitOperationType::Push,
            "pull" => GitOperationType::Pull,
            _ => GitOperationType::Fetch,
        };
        self.event_git_operation(op, repo_path, Some(passphrase.clone()))
    }

    /// Handle directory creation
    pub(in crate::app) fn handle_create_directory(
        &mut self,
        _directory: PathBuf,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        self.create_entry_in_file_manager(value, true)
    }

    /// Shared helper for creating a file or directory in the active FileManager.
    fn create_entry_in_file_manager(
        &mut self,
        value: Box<dyn std::any::Any>,
        is_directory: bool,
    ) -> Result<()> {
        if let Some(name) = value.downcast_ref::<String>() {
            let t = i18n::t();
            let result = if let Some(panel) = self.layout_manager.active_panel_mut() {
                if let Some(fm) = panel.as_file_manager_mut() {
                    let result = if is_directory {
                        fm.create_directory(name.clone())
                    } else {
                        fm.create_file(name.clone())
                    };
                    if result.is_ok() {
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
                    Ok(_) => {
                        let msg = if is_directory {
                            t.status_dir_created(name)
                        } else {
                            t.status_file_created(name)
                        };
                        self.state.set_info(msg);
                    }
                    Err(e) => {
                        let kind = if is_directory { "directory" } else { "file" };
                        log::error!("{} creation error '{}': {}", kind, name, e);
                        let error_msg = format!("Failed to create {} '{}': {}", kind, name, e);
                        self.show_error_modal(error_msg);
                    }
                }
            }
        }
        Ok(())
    }

    /// Handle saving file with new name
    pub(in crate::app) fn handle_save_file_as(
        &mut self,
        directory: PathBuf,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        // Store info needed for LSP notification (before mutable borrow)
        let mut lsp_info: Option<(String, PathBuf)> = None;
        #[cfg(unix)]
        let mut saved_path: Option<PathBuf> = None;
        #[cfg(unix)]
        let mut make_executable = false;

        if let Some(result) = value.downcast_ref::<SaveAsResult>() {
            let t = i18n::t();
            #[cfg(unix)]
            {
                make_executable = result.executable;
            }

            // Get active Editor panel and save file
            if let Some(panel) = self.layout_manager.active_panel_mut() {
                if let Some(editor) = panel.as_editor_mut() {
                    // Resolve path: expand ~ and handle absolute/relative paths
                    let input_path = termide_ui::expand_tilde(&result.path);
                    let file_path = if input_path.is_absolute() {
                        input_path
                    } else {
                        directory.join(&result.path)
                    };
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

        Ok(())
    }

    /// Write in-memory `content` to a Save-As-chosen path (used by read-only
    /// viewers exporting their source). Relative names resolve against
    /// `directory`.
    pub(in crate::app) fn handle_save_content_as(
        &mut self,
        directory: PathBuf,
        content: String,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if let Some(result) = value.downcast_ref::<SaveAsResult>() {
            let t = i18n::t();
            let input_path = termide_ui::expand_tilde(&result.path);
            let file_path = if input_path.is_absolute() {
                input_path
            } else {
                directory.join(&result.path)
            };
            let display_path = file_path.display().to_string();
            match std::fs::write(&file_path, content.as_bytes()) {
                Ok(()) => self.state.set_info(t.status_file_saved(&display_path)),
                Err(e) => {
                    log::error!("Save error '{}': {}", display_path, e);
                    self.show_error_modal(t.status_error_save(&e.to_string()));
                }
            }
        }
        Ok(())
    }

    /// Handle git stash push: user provided stash message.
    pub(in crate::app) fn handle_git_stash_push(
        &mut self,
        repo_path: std::path::PathBuf,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        let message = match value.downcast_ref::<String>() {
            Some(s) => s.trim().to_string(),
            None => return Ok(()),
        };
        let include_untracked = self.state.stash.include_untracked;
        self.state.stash.include_untracked = false; // reset
        match termide_git::stash_push(&repo_path, &message, include_untracked) {
            Ok(()) => {
                self.state.set_info("Stash created".to_string());
                self.send_git_update(&repo_path);
            }
            Err(e) => {
                self.show_error_modal(format!("Stash push error: {}", e));
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
        value: Box<dyn std::any::Any>,
    ) -> anyhow::Result<()> {
        let new_name = match value.downcast_ref::<String>() {
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
