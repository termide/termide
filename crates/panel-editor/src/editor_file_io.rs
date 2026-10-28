//! File I/O methods for the Editor.
//!
//! Save / force-save / save-as / reload-from-disk plus mtime and
//! upload-state helpers. Handles both local files and the
//! "queue async upload" path used for remote (VFS) files.

use anyhow::Result;
use std::path::PathBuf;

use termide_buffer::TextBuffer;
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
                        self.saved_paths.push(path.clone());
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
            self.saved_paths.push(path.to_path_buf());
            // Update file modification time after successful save
            self.file_state.mtime = file_io::get_file_mtime(path);
            self.file_state.external_change_detected = false;
        }

        // Update git diff after successful save
        self.update_git_diff();

        Ok(None) // Local file saved
    }

    /// Reload file from disk (discards local changes). The cursor and the
    /// scroll position stay where they were, clamped to the new content, so
    /// a file rewritten by a formatter or the agent does not throw the user
    /// back to the top.
    pub fn reload_from_disk(&mut self) -> Result<()> {
        if let Some(path) = self.buffer.file_path().map(|p| p.to_path_buf()) {
            // Re-read the file; the breakpoints come back from the store.
            self.sync_breakpoints();
            self.buffer = TextBuffer::from_file(&path)?;
            self.reload_breakpoints();
            // The server still holds the pre-reload text and the fresh buffer
            // has no ranges to describe the difference, so ask for a whole
            // document on the next sync and make sure one happens.
            self.buffer.request_full_lsp_sync();
            self.lsp.mark_changed();

            // Update modification time
            self.file_state.mtime = file_io::get_file_mtime(&path);
            self.file_state.external_change_detected = false;

            let last_line = self.buffer.line_count().saturating_sub(1);
            self.clamp_cursor();
            self.selection = None;
            if self.viewport.top_line > last_line {
                self.viewport.top_line = last_line;
                self.viewport.top_visual_row_offset = 0;
            }

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
    ///
    /// The target is always a local path, so afterwards the buffer is a plain
    /// local file: a remote origin is forgotten, otherwise the next `save()`
    /// would upload the old temp copy over the original remote file.
    pub fn save_file_as(&mut self, path: PathBuf) -> Result<()> {
        self.buffer.save_to(&path)?;
        log::info!("File saved as: {}", path.display());
        self.saved_paths.push(path.clone());

        self.file_state.remote_path = None;
        self.file_state.temp_local_path = None;
        self.file_state.remote_mtime = None;
        self.file_state.uploading = false;
        // Track the new file on disk, so later external changes are noticed.
        self.file_state.update_mtime(&path);
        self.file_state.title = file_io::path_to_title(&path);

        self.update_git_diff();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use termide_vfs::{VfsPath, VfsProtocol};

    use crate::state::FileState;
    use crate::Editor;

    #[test]
    fn save_as_turns_a_remote_buffer_into_a_local_file() {
        let dir = tempfile::tempdir().unwrap();
        let temp = dir.path().join("remote-copy.txt");
        std::fs::write(&temp, "remote\n").unwrap();
        let mut editor = Editor::open_file_with_config(temp.clone(), Default::default()).unwrap();
        editor.set_file_state(FileState::from_remote(
            VfsPath::remote(VfsProtocol::Sftp, "host", "/srv/file.txt"),
            temp.clone(),
            None,
            7,
        ));

        let target = dir.path().join("local.txt");
        editor.save_file_as(target.clone()).unwrap();

        assert!(!editor.file_state.is_remote());
        assert!(editor.file_state.temp_local_path().is_none());
        // The next save writes the new local file and asks for no upload.
        assert!(editor.save().unwrap().is_none());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "remote\n");
    }

    #[test]
    fn save_as_tracks_the_new_file_for_external_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut editor = Editor::new();
        editor.file_state.external_change_detected = true;
        let target = dir.path().join("new.txt");

        editor.save_file_as(target.clone()).unwrap();

        assert!(!editor.file_state.external_change_detected);
        assert!(editor.file_state.mtime.is_some());

        // Someone else rewrites the file later: the editor must notice.
        std::fs::write(&target, "changed elsewhere").unwrap();
        let later = SystemTime::now() + Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&target)
            .unwrap()
            .set_modified(later)
            .unwrap();
        editor.file_state.check_external_modification(&target);
        assert!(editor.file_state.external_change_detected);
    }
}
