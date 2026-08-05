//! File I/O methods for the Editor.
//!
//! Save / force-save / save-as / reload-from-disk plus mtime and
//! upload-state helpers. Handles both local files and the
//! "queue async upload" path used for remote (VFS) files.

use anyhow::Result;
use std::path::PathBuf;

use termide_buffer::{Cursor, TextBuffer};
use termide_config::Config;

use crate::file_io;

use super::Editor;

impl Editor {
    /// Save file
    /// Returns error if file was modified externally (use force_save() to override)
    /// Returns Some((temp_path, remote_path, vfs_manager)) for remote files (async upload via OperationManager), None for local files
    pub fn save(
        &mut self,
    ) -> Result<
        Option<(
            PathBuf,
            termide_vfs::VfsPath,
            std::sync::Arc<termide_vfs::VfsManager>,
        )>,
    > {
        // Check for external modification conflict. There is no in-place
        // force-save shortcut (Ctrl+Shift+S is Save As), so point the user at
        // the actions that actually exist: reload to take the disk version, or
        // Save As to write elsewhere. Overwriting in place is offered by the
        // conflict dialog when closing the editor.
        if self.file_state.external_change_detected {
            return Err(anyhow::anyhow!(
                "File was modified on disk. Reload (Ctrl+Shift+R) to load the disk version and discard your changes, or Save As (Ctrl+Shift+S) to write to a different file."
            ));
        }

        // Handle remote file saves
        if self.file_state.is_remote() {
            let vfs_manager = self
                .vfs_manager
                .clone()
                .ok_or_else(|| anyhow::anyhow!("No VFS manager for remote file"))?;

            // Save to local temp file first
            self.buffer.save()?;

            // Get remote path and temp path
            let remote_path = self
                .file_state
                .remote_path()
                .ok_or_else(|| anyhow::anyhow!("No remote path"))?
                .clone();
            let temp_path = self
                .file_state
                .temp_local_path()
                .ok_or_else(|| anyhow::anyhow!("No temp path"))?
                .to_path_buf();

            log::info!(
                "Remote file save requested: {}",
                remote_path.to_url_string()
            );

            // Return info for async upload via OperationManager
            // Note: mtime and external_change_detected will be updated when upload completes
            return Ok(Some((temp_path, remote_path, vfs_manager)));
        }

        // Check if this is a config file
        if let Some(path) = self.buffer.file_path().map(|p| p.to_path_buf()) {
            if Config::is_config_file(&path) {
                let path_str = path.display().to_string();
                // Validate config before saving
                let content = self.buffer.to_string();
                match Config::validate_content(&content) {
                    Ok(new_config) => {
                        // Save and set config update flag
                        self.buffer.save()?;
                        log::info!("Config file saved: {}", path_str);
                        self.config_update = Some(new_config);
                        // Update file modification time after successful save
                        self.file_state.mtime = file_io::get_file_mtime(&path);
                        self.file_state.external_change_detected = false;
                    }
                    Err(e) => {
                        log::error!("Save failed - config validation error: {}", e);
                        return Err(anyhow::anyhow!("Invalid config: {}", e));
                    }
                }
                return Ok(None); // Config file saved locally
            }
        }

        self.buffer.save()?;

        if let Some(path) = self.buffer.file_path() {
            log::info!("File saved: {}", path.display());
            // Update file modification time after successful save
            self.file_state.mtime = file_io::get_file_mtime(path);
            self.file_state.external_change_detected = false;
        }

        // Update git diff after successful save
        self.update_git_diff();

        Ok(None) // Local file saved
    }

    /// Reload file from disk (discards local changes)
    pub fn reload_from_disk(&mut self) -> Result<()> {
        if let Some(path) = self.buffer.file_path().map(|p| p.to_path_buf()) {
            // Re-read the file
            self.buffer = TextBuffer::from_file(&path)?;

            // Update modification time
            self.file_state.mtime = file_io::get_file_mtime(&path);
            self.file_state.external_change_detected = false;

            // Reset cursor and selection
            self.cursor = Cursor::new();
            self.selection = None;

            // Update git diff
            self.update_git_diff();

            // Invalidate rendering cache so new content is displayed immediately
            self.render_cache.invalidate_wrap_cache();
            self.render_cache
                .highlight
                .invalidate_range(0, self.buffer.line_count());

            log::info!("File reloaded from disk: {}", path.display());
        }
        Ok(())
    }

    /// Force save (ignore external changes)
    /// Returns Some((temp_path, remote_path, vfs_manager)) for remote files (async upload), None for local files
    pub fn force_save(
        &mut self,
    ) -> Result<
        Option<(
            PathBuf,
            termide_vfs::VfsPath,
            std::sync::Arc<termide_vfs::VfsManager>,
        )>,
    > {
        self.file_state.external_change_detected = false;
        self.save()
    }

    /// Update file modification time (for remote file uploads)
    pub fn update_file_mtime(&mut self, mtime: Option<std::time::SystemTime>) {
        self.file_state.mtime = mtime;
        if self.file_state.is_remote() {
            self.file_state.update_remote_mtime(mtime);
        }
    }

    /// Clear external change detected flag (after successful remote upload)
    pub fn clear_external_change_detected(&mut self) {
        self.file_state.external_change_detected = false;
    }

    /// Set upload state for remote files
    pub fn set_uploading(&mut self, uploading: bool) {
        self.file_state.set_uploading(uploading);
    }

    /// Save file as (Save As)
    pub fn save_file_as(&mut self, path: PathBuf) -> Result<()> {
        self.buffer.save_to(&path)?;
        log::info!("File saved as: {}", path.display());

        // Update title
        self.file_state.title = file_io::path_to_title(&path);

        Ok(())
    }
}
