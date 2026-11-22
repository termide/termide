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
        self.save_checked(true)
    }

    /// Save, first looking at the file on disk when `check_disk` is set:
    /// the watcher reports a change only after its debounce, and drops
    /// events it cannot keep up with, so a save right after another program
    /// wrote the file would otherwise overwrite that write unseen.
    fn save_checked(
        &mut self,
        check_disk: bool,
    ) -> Result<
        Option<(
            PathBuf,
            termide_vfs::VfsPath,
            std::sync::Arc<termide_vfs::VfsManager>,
        )>,
    > {
        if check_disk && !self.file_state.is_remote() {
            if let Some(path) = self.buffer.file_path() {
                if self.file_state.changed_on_disk(path) {
                    self.file_state.external_change_detected = true;
                }
            }
        }

        // Check for external modification conflict. There is no in-place
        // force-save shortcut, so point the user at the actions that exist,
        // on the keys they are bound to: reload to take the disk version, or
        // Save As to write elsewhere. Overwriting in place is offered by the
        // conflict dialog when closing the editor.
        if self.file_state.external_change_detected {
            let key = |action: &str| match self.hotkeys.display(action) {
                keys if keys.is_empty() => "—".to_string(),
                keys => keys,
            };
            return Err(anyhow::anyhow!(
                termide_i18n::t().editor_modified_on_disk_fmt(&key("reload"), &key("save_as"))
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
                        // Record the file as written, for later external changes
                        self.file_state.update_mtime(&path);
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
            // Record the file as written, for later external changes
            self.file_state.update_mtime(path);
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
            // The size limit of opening holds here too: a file grown past
            // it stays out, and the buffer keeps what it had.
            file_io::check_file_metadata(&path)?;
            self.sync_breakpoints();
            self.buffer = TextBuffer::from_file(&path)?;
            self.reload_breakpoints();
            // The new buffer has no folds; ranges asked of the old one are stale.
            self.folding = Default::default();
            // The server still holds the pre-reload text and the fresh buffer
            // has no ranges to describe the difference, so ask for a whole
            // document on the next sync and make sure one happens.
            self.buffer.request_full_lsp_sync();
            self.lsp.mark_changed();

            // Record the file as loaded, for later external changes
            self.file_state.update_mtime(&path);

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
        self.save_checked(false)
    }

    /// The server's modification time of a remote file when it was
    /// downloaded or last uploaded; `None` for a local file or when the
    /// server did not tell.
    pub fn remote_mtime(&self) -> Option<std::time::SystemTime> {
        self.file_state.remote_mtime
    }

    /// Record the server's modification time of a remote file, after an
    /// upload or once the user chose to overwrite a newer version.
    pub fn set_remote_mtime(&mut self, mtime: Option<std::time::SystemTime>) {
        if self.file_state.is_remote() {
            self.file_state.update_remote_mtime(mtime);
        }
    }

    /// The server's modification time of the remote file now, asked of the
    /// server (blocking); `None` for a local file or when it cannot tell.
    fn server_mtime(&self) -> Option<std::time::SystemTime> {
        let vfs = self.vfs_manager.as_ref()?;
        let path = self.file_state.remote_path()?;
        match vfs.metadata(path).recv() {
            Ok(metadata) => metadata.modified,
            Err(error) => {
                log::warn!("cannot stat {} on the server: {error}", path.log_safe_key());
                None
            }
        }
    }

    /// Before an upload: the server's modification time when the remote
    /// file changed there since it was downloaded or last uploaded, to ask
    /// the user before overwriting it. `None` when it did not, or when
    /// either time is unknown — a file gone from the server is written anew.
    pub fn remote_changed_on_server(&self) -> Option<std::time::SystemTime> {
        let recorded = self.file_state.remote_mtime?;
        self.server_mtime().filter(|now| *now != recorded)
    }

    /// After a successful upload: the server's new modification time is
    /// the base of the next save's check.
    pub fn refresh_remote_mtime(&mut self) {
        if self.file_state.is_remote() {
            let mtime = self.server_mtime();
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

    /// The upload of a remote save failed or was cancelled: the edits are
    /// only in the local copy, so the buffer is unsaved again and closing
    /// it asks first.
    pub fn upload_failed(&mut self) {
        self.file_state.set_uploading(false);
        self.buffer.mark_unsaved();
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
            None,
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

    fn set_mtime(path: &std::path::Path, mtime: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }

    #[test]
    fn save_refuses_a_file_changed_on_disk_before_the_watcher_reports_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, "one\n").unwrap();
        let mut editor = Editor::open_file_with_config(path.clone(), Default::default()).unwrap();
        editor.insert_char('x').unwrap();

        // Another program writes the file; no watcher event has arrived.
        std::fs::write(&path, "written elsewhere\n").unwrap();
        // Its clock may be behind ours: an older time is a change as well.
        set_mtime(&path, SystemTime::now() - Duration::from_secs(3600));

        assert!(editor.save().is_err());
        assert!(editor.file_state.external_change_detected);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "written elsewhere\n"
        );

        // Overwriting on purpose still works, and is then the new base.
        assert!(editor.force_save().unwrap().is_none());
        assert_ne!(
            std::fs::read_to_string(&path).unwrap(),
            "written elsewhere\n"
        );
        assert!(!editor.file_state.external_change_detected);
        editor.insert_char('y').unwrap();
        assert!(editor.save().is_ok());
    }

    /// A "remote" file served by the local file system through the VFS,
    /// which answers `metadata` as a server would.
    fn remote_editor(dir: &std::path::Path) -> (Editor, std::path::PathBuf) {
        let server = dir.join("server.txt");
        std::fs::write(&server, "on the server\n").unwrap();
        let temp = dir.join("copy.txt");
        std::fs::copy(&server, &temp).unwrap();
        let mut editor = Editor::open_file_with_config(temp.clone(), Default::default()).unwrap();
        let downloaded = std::fs::metadata(&server).unwrap().modified().ok();
        editor.set_file_state(FileState::from_remote(
            VfsPath::local(&server),
            temp,
            None,
            14,
            downloaded,
        ));
        editor.set_vfs_manager(std::sync::Arc::new(termide_vfs::VfsManager::new()));
        (editor, server)
    }

    #[test]
    fn a_remote_file_changed_on_the_server_is_noticed_before_upload() {
        let dir = tempfile::tempdir().unwrap();
        let (mut editor, server) = remote_editor(dir.path());
        assert_eq!(editor.remote_changed_on_server(), None);

        std::fs::write(&server, "someone else\n").unwrap();
        set_mtime(&server, SystemTime::now() - Duration::from_secs(3600));
        let changed = editor.remote_changed_on_server();
        assert!(changed.is_some());

        // Overwrite chosen: the server's time is the new base.
        editor.set_remote_mtime(changed);
        assert_eq!(editor.remote_changed_on_server(), None);

        // After an upload the server's new time is taken.
        set_mtime(&server, SystemTime::now());
        editor.refresh_remote_mtime();
        assert_eq!(editor.remote_changed_on_server(), None);
    }

    #[test]
    fn reload_leaves_out_a_file_grown_past_the_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.txt");
        std::fs::write(&path, "small\n").unwrap();
        let mut editor = Editor::open_file_with_config(path.clone(), Default::default()).unwrap();

        // Sparse, so the test writes nothing near the limit.
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(crate::constants::MAX_EDITOR_FILE_SIZE + 1)
            .unwrap();

        assert!(editor.reload_from_disk().is_err());
        assert_eq!(editor.buffer.to_string(), "small\n");

        // The watcher's reload of an unmodified buffer keeps out of it too.
        editor.check_external_modification();
        assert_eq!(editor.buffer.to_string(), "small\n");
        assert!(editor.file_state.external_change_detected);
    }
}
