//! LSP integration methods for the Editor.
//!
//! This module contains LSP lifecycle plumbing, diagnostics, and server-status
//! tracking. Feature-specific handlers live in sibling submodules:
//! - Completion & auto-completion: `editor_lsp_completion`
//! - Code actions: `editor_lsp_code_action`
//! - Hover / definition / references / rename: `editor_lsp_nav`

use super::{Editor, LspManager, ServerStatus};

impl Editor {
    // =========================================================================
    // LSP Initialization & Lifecycle
    // =========================================================================

    /// Initialize LSP for this editor's file.
    ///
    /// Should be called after opening a file to enable LSP features.
    /// This detects the language, starts the appropriate server if configured,
    /// and sends the `didOpen` notification.
    pub fn init_lsp(&mut self, lsp_manager: &mut LspManager) {
        if let Some(path) = self.buffer.file_path() {
            self.lsp.init_for_file(path, lsp_manager);

            if self.lsp.enabled {
                let content = self.buffer.to_string();
                self.lsp.did_open(path, &content, lsp_manager);
                // `didOpen` carried the current text, so anything recorded
                // before it is already reflected there and must not be
                // replayed as a change on top.
                let _ = self.buffer.take_lsp_changes();
            }
        }
    }

    /// Open the document again if its server was restarted since it was
    /// opened: the new process knows nothing of it. The diagnostics of the
    /// old one were cleared when it went down. `true` if `didOpen` was sent.
    pub fn resync_lsp(&mut self, lsp_manager: &LspManager) -> bool {
        let Some(path) = self.buffer.file_path().map(|p| p.to_path_buf()) else {
            return false;
        };
        if !self.lsp.needs_reopen(&path, lsp_manager) {
            return false;
        }
        let content = self.buffer.to_string();
        self.lsp.reopen(&path, &content, lsp_manager);
        // The text sent carries every change recorded so far.
        let _ = self.buffer.take_lsp_changes();
        true
    }

    /// Notify LSP about buffer content change.
    ///
    /// Should be called after any text modification (insert, delete, etc.)
    /// to keep the language server in sync with the editor content.
    ///
    /// Sends only the ranges that changed when the server accepts them.
    /// Materialising the whole document instead walked the rope and allocated
    /// a copy of the file on every edit, which is paid per keystroke and grows
    /// with the file rather than with the change.
    pub fn notify_lsp_change(&mut self, lsp_manager: &LspManager) {
        let Some(path) = self.buffer.file_path().map(|p| p.to_path_buf()) else {
            // Nothing to sync: an unsaved scratch buffer was never opened.
            let _ = self.buffer.take_lsp_changes();
            return;
        };

        let recorded = self.buffer.take_lsp_changes();

        // `None` means the recorded ranges stopped describing the document, and
        // a server that only advertises Full sync cannot be sent ranges at all.
        if !self.lsp.supports_incremental_sync(&path, lsp_manager) {
            let content = self.buffer.to_string();
            self.lsp.did_change_full(&path, &content, lsp_manager);
            return;
        }

        let Some(changes) = recorded else {
            let content = self.buffer.to_string();
            self.lsp.did_change_full(&path, &content, lsp_manager);
            return;
        };

        if changes.is_empty() {
            return;
        }

        let changes = changes
            .into_iter()
            .map(|c| lsp_types::TextDocumentContentChangeEvent {
                range: Some(lsp_types::Range::new(
                    lsp_types::Position::new(c.start_line, c.start_character),
                    lsp_types::Position::new(c.end_line, c.end_character),
                )),
                range_length: None,
                text: c.text,
            })
            .collect();
        self.lsp.did_change_incremental(&path, changes, lsp_manager);
    }

    /// Cleanup LSP when editor is closed.
    ///
    /// Sends the `didClose` notification to the language server.
    pub fn cleanup_lsp(&self, lsp_manager: &LspManager) {
        if let Some(path) = self.buffer.file_path() {
            self.lsp.did_close(path, lsp_manager);
        }
    }

    /// Get the language ID for this editor's file.
    pub fn lsp_language(&self) -> Option<&str> {
        self.lsp.language_id.as_deref()
    }

    /// Mark that the buffer has changed (for LSP notification).
    pub fn mark_lsp_changed(&mut self) {
        self.lsp.mark_changed();
    }

    /// Send pending LSP change notification if needed.
    ///
    /// Returns true if a notification was sent.
    pub fn flush_lsp_changes(&mut self, lsp_manager: &LspManager) -> bool {
        if self.lsp.has_pending_change() {
            self.notify_lsp_change(lsp_manager);
            self.lsp.clear_pending_change();
            true
        } else {
            false
        }
    }

    // =========================================================================
    // LSP Diagnostics
    // =========================================================================

    /// Update diagnostics from LSP.
    pub fn update_diagnostics(&mut self, diagnostics: Vec<lsp_types::Diagnostic>) {
        self.lsp.update_diagnostics(diagnostics);
        self.render_cache.invalidate_diagnostic_cache();
    }

    // =========================================================================
    // LSP Server Status
    // =========================================================================

    /// Check if LSP server is loading (for spinner display).
    pub fn is_lsp_loading(&self) -> bool {
        self.lsp.server_loading
    }

    /// Update server loading status from actual LSP server status.
    ///
    /// Returns true if status changed (needs redraw).
    pub fn update_lsp_loading_status(&mut self, lsp_manager: &LspManager) -> bool {
        // Clone paths to avoid borrow issues
        let file_path = match self.file_path() {
            Some(p) => p.to_path_buf(),
            None => return false,
        };
        let lang = match &self.lsp.language_id {
            Some(l) => l.clone(),
            None => return false,
        };

        // Get current server status and update status text
        let status = lsp_manager.server_status(&lang, &file_path);
        let new_status_text = match status {
            Some(ServerStatus::Starting) => Some("starting".to_string()),
            Some(ServerStatus::Indexing) => Some("indexing".to_string()),
            Some(ServerStatus::Crashed) => Some("crashed".to_string()),
            Some(ServerStatus::Restarting) => Some("restarting".to_string()),
            _ => None,
        };
        let status_text_changed = self.lsp.server_status_text != new_status_text;
        self.lsp.server_status_text = new_status_text;

        // A crashed server will not become ready, nor will one whose start
        // failed again (it reports no status): stop the spinner.
        if self.lsp.server_loading && matches!(status, Some(ServerStatus::Crashed) | None) {
            self.lsp.server_loading = false;
            return true;
        }

        // Check if server went back to indexing (e.g., after file changes),
        // or is being restarted
        if !self.lsp.server_loading
            && (status == Some(ServerStatus::Restarting)
                || lsp_manager.server_is_indexing(&lang, &file_path))
        {
            self.lsp.server_loading = true;
            return true;
        }

        // Check if server became ready
        if self.lsp.server_loading && lsp_manager.server_is_ready(&lang, &file_path) {
            self.lsp.server_loading = false;
            return true;
        }

        // Return true if status text changed (for redraw)
        status_text_changed
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::{Duration, Instant};

    use termide_lsp::{LspConfig, LspServerConfig};

    use super::*;
    use crate::EditorConfig;

    /// The `[≡]` menu offers a restart of the file's language server only
    /// when the file uses one, and asks the app for it by file.
    #[test]
    fn the_panel_menu_restarts_the_language_server_of_the_file() {
        use termide_core::{Panel, PanelEvent};

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let mut editor =
            Editor::open_file_with_config(file.clone(), EditorConfig::default()).unwrap();
        let offered = |e: &Editor| {
            Panel::context_menu_items(e)
                .iter()
                .any(|(_, action)| action == crate::RESTART_LANGUAGE_SERVER_ACTION)
        };
        assert!(!offered(&editor), "no server for the file");

        editor.lsp.enabled = true;
        assert!(offered(&editor));
        let events =
            Panel::handle_status_action(&mut editor, crate::RESTART_LANGUAGE_SERVER_ACTION);
        assert!(matches!(
            events.as_slice(),
            [PanelEvent::RestartLanguageServer { file: f }] if *f == file
        ));
    }

    /// When the file's server cannot start, the menu still offers its
    /// restart; once the server is installed, the restart starts it and the
    /// editor opens its document in it, with the text it holds now.
    #[test]
    fn a_server_that_failed_to_start_attaches_the_editor_once_restarted() {
        use std::os::unix::fs::PermissionsExt;
        use termide_core::Panel;

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let binary = dir.path().join("server");
        let mut config = LspConfig::default();
        config.servers.insert(
            "rust".into(),
            LspServerConfig {
                command: binary.to_string_lossy().into_owned(),
                args: Vec::new(),
                root_markers: Vec::new(),
            },
        );
        let mut manager = LspManager::new(config);
        let mut editor = Editor::open_file_with_config(file, EditorConfig::default()).unwrap();
        editor.init_lsp(&mut manager);
        assert!(!editor.lsp.enabled);
        assert!(!editor.is_lsp_loading());
        assert!(Panel::context_menu_items(&editor)
            .iter()
            .any(|(_, action)| action == crate::RESTART_LANGUAGE_SERVER_ACTION));
        editor.insert_text("// edited\n").unwrap();

        // A failed retry stops the spinner it started.
        assert!(manager.restart_server("rust", editor.file_path().unwrap()));
        editor.update_lsp_loading_status(&manager);
        assert!(editor.is_lsp_loading());
        let deadline = Instant::now() + Duration::from_secs(10);
        while editor.is_lsp_loading() {
            assert!(Instant::now() < deadline, "the spinner never stopped");
            manager.poll_restarts();
            editor.update_lsp_loading_status(&manager);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!editor.resync_lsp(&manager));

        let reply = r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#;
        // Written aside and renamed in, so no thread forking meanwhile
        // holds it open for writing (`ETXTBSY`).
        let staged = dir.path().join("server.new");
        std::fs::write(
            &staged,
            format!(
                "#!/bin/sh\nprintf 'Content-Length: {}\\r\\n\\r\\n%s' '{reply}'; cat > received\n",
                reply.len()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(staged, &binary).unwrap();
        assert!(manager.restart_server("rust", editor.file_path().unwrap()));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !editor.resync_lsp(&manager) {
            assert!(Instant::now() < deadline, "never started");
            manager.poll_restarts();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(editor.lsp.enabled);
        assert!(!editor.lsp.server_start_failed);
        assert!(!editor.resync_lsp(&manager), "sent once");

        let received = dir.path().join("received");
        let deadline = Instant::now() + Duration::from_secs(10);
        let opened = loop {
            let received = std::fs::read_to_string(&received).unwrap_or_default();
            if let Some(message) = received
                .split("Content-Length:")
                .find(|m| m.contains(r#""method":"textDocument/didOpen""#))
            {
                break message.to_string();
            }
            assert!(Instant::now() < deadline, "no didOpen in {received:?}");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(
            opened.contains(r#""text":"// edited\nfn main() {}\n""#),
            "{opened}"
        );
        assert!(opened.contains(r#""version":1"#), "{opened}");
    }

    /// A server that dies shows as crashed in the title and stops the
    /// spinner, instead of spinning on "starting" for good.
    #[test]
    fn a_crashed_server_shows_in_the_title_and_stops_the_spinner() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let mut config = LspConfig::default();
        config.servers.insert(
            "rust".into(),
            LspServerConfig {
                command: "sh".into(),
                args: vec!["-c".into(), "exit 0".into()],
                root_markers: Vec::new(),
            },
        );
        let mut manager = LspManager::new(config);
        let mut editor = Editor::open_file_with_config(file, EditorConfig::default()).unwrap();
        editor.init_lsp(&mut manager);
        assert!(editor.is_lsp_loading());

        // It may look ready for a moment before the crash is noticed.
        let deadline = Instant::now() + Duration::from_secs(5);
        while editor.lsp.server_status_text.as_deref() != Some("crashed") {
            assert!(Instant::now() < deadline, "the crash never showed");
            editor.update_lsp_loading_status(&manager);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!editor.is_lsp_loading());
    }

    /// A crashed server is restarted, and the editor opens its document in
    /// the new process again: the current text, at the current version.
    #[test]
    fn a_restarted_server_is_sent_the_open_document_again() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let reply = r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#;
        // The first run dies once the document is open; the second answers
        // `initialize` and records what it is sent.
        let script = format!(
            "if [ ! -e crashed ]; then : > crashed; head -c 1 > /dev/null; exit 0; fi; \
             printf 'Content-Length: {}\\r\\n\\r\\n%s' '{reply}'; cat > received",
            reply.len()
        );
        let mut config = LspConfig::default();
        config.servers.insert(
            "rust".into(),
            LspServerConfig {
                command: "sh".into(),
                args: vec!["-c".into(), script],
                root_markers: Vec::new(),
            },
        );
        let mut manager = LspManager::new(config);
        let mut editor = Editor::open_file_with_config(file, EditorConfig::default()).unwrap();
        editor.init_lsp(&mut manager);
        assert!(!editor.resync_lsp(&manager), "opened in the live server");

        // Edits made while the server is down still reach the new one.
        editor.insert_text("// edited\n").unwrap();
        editor.flush_lsp_changes(&manager);
        let version = editor.lsp.document_version;
        assert!(version > 1);

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut saw_restarting = false;
        while !editor.resync_lsp(&manager) {
            assert!(Instant::now() < deadline, "never restarted");
            manager.poll_restarts();
            editor.update_lsp_loading_status(&manager);
            saw_restarting |= editor.lsp.server_status_text.as_deref() == Some("restarting");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(saw_restarting, "the title never showed the restart");
        assert!(!editor.resync_lsp(&manager), "sent once");

        let text = editor.buffer.to_string();
        let received = dir.path().join("received");
        let deadline = Instant::now() + Duration::from_secs(10);
        let opened = loop {
            let received = std::fs::read_to_string(&received).unwrap_or_default();
            if let Some(message) = received
                .split("Content-Length:")
                .find(|m| m.contains(r#""method":"textDocument/didOpen""#))
            {
                break message.to_string();
            }
            assert!(Instant::now() < deadline, "no didOpen in {received:?}");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(text.starts_with("// edited\n"));
        let escaped = text.replace('\n', "\\n");
        assert!(
            opened.contains(&format!(r#""text":"{escaped}""#)),
            "{opened}"
        );
        assert!(
            opened.contains(&format!(r#""version":{version}"#)),
            "{opened}"
        );
    }
}
