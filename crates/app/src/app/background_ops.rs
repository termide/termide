//! Background operation handlers.
//!
//! Contains tick handlers for various background operations:
//! - Directory size calculation
//! - Git operations (push/pull, status, diff)
//! - Command execution
//! - System resource monitoring
//! - Modal spinners
//! - LSP completion polling

use std::sync::mpsc::TryRecvError;

use termide_core::PanelCommand;
use termide_modal::InfoModal;

use crate::state::ActiveModal;
use crate::PanelExt;

use super::App;

impl App {
    /// Start a background fetch of `url` from a viewer's `Ctrl+G`, opening the
    /// result in a *new* viewer.
    pub(super) fn start_url_fetch(&mut self, url: String) {
        self.start_fetch(url, false);
    }

    /// Start a background fetch that replaces the *active* viewer in place
    /// (a followed link or a history step inside a fetched page).
    pub(super) fn start_url_fetch_in_place(&mut self, url: String) {
        self.start_fetch(url, true);
    }

    /// Spawn the blocking GET on a worker thread; the result is delivered over
    /// a channel and picked up by [`check_view_fetch`](App::check_view_fetch).
    fn start_fetch(&mut self, url: String, in_place: bool) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.state.view_fetch_receiver = Some(rx);
        self.state.view_fetch_in_place = in_place;
        self.state.set_info(format!("Fetching {url}…"));
        std::thread::spawn(move || {
            let _ = tx.send(termide_fetch::fetch(&url));
        });
        self.state.needs_redraw = true;
    }

    /// Poll the in-flight URL fetch, if any, and open the result on completion.
    pub(super) fn check_view_fetch(&mut self) {
        let Some(rx) = self.state.view_fetch_receiver.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.state.view_fetch_receiver = None;
                let in_place = self.state.view_fetch_in_place;
                match result {
                    Ok(fetched) if in_place => self.apply_fetched_in_place(fetched),
                    Ok(fetched) => self.open_fetched(fetched),
                    Err(e) => self.show_error_modal(format!("Fetch failed: {e}")),
                }
                self.state.needs_redraw = true;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.state.view_fetch_receiver = None;
            }
        }
    }

    /// Open a fetched document in a new viewer, routed by Content-Type.
    fn open_fetched(&mut self, fetched: termide_fetch::Fetched) {
        let title = fetch_title(&fetched.final_url);
        let url = fetched.final_url.clone();
        match classify(&fetched) {
            Some(ViewKind::Html(src)) => {
                self.add_panel(Box::new(termide_panel_html::HtmlPanel::from_source(
                    title,
                    src,
                    Some(url),
                )));
            }
            Some(ViewKind::Markdown(src)) => {
                self.add_panel(Box::new(
                    termide_panel_markdown::MarkdownPanel::from_source(title, src, Some(url)),
                ));
            }
            Some(ViewKind::Image(bytes, ext)) => {
                self.open_fetched_image(&bytes, &ext, &url);
                return;
            }
            None => {
                self.show_error_modal(format!(
                    "Unsupported content type: {}",
                    fetched.content_type
                ));
                return;
            }
        }
        self.auto_save_session();
    }

    /// Cache fetched image bytes to a temp file and open them in the image
    /// preview (which handles graphics-protocol display or an external fallback).
    fn open_fetched_image(&mut self, bytes: &[u8], ext: &str, url: &str) {
        let title = fetch_title(url);
        let raw_stem = title.rsplit_once('.').map_or(title.as_str(), |(s, _)| s);
        let mut stem = sanitize_filename(raw_stem);
        if stem.is_empty() {
            stem = "image".to_string();
        }
        let path = std::env::temp_dir().join(format!("termide-web-{stem}.{ext}"));
        match std::fs::write(&path, bytes) {
            Ok(()) => {
                self.close_help_panels();
                let _ = self.event_preview_media(path);
            }
            Err(e) => self.show_error_modal(format!("Failed to cache image: {e}")),
        }
    }

    /// Apply a navigation result to the active viewer in place when its type
    /// matches the content; otherwise fall back to opening a new viewer (so the
    /// browsing panel's history isn't clobbered by a type switch).
    fn apply_fetched_in_place(&mut self, fetched: termide_fetch::Fetched) {
        let title = fetch_title(&fetched.final_url);
        let url = fetched.final_url.clone();
        match classify(&fetched) {
            Some(ViewKind::Html(src)) => {
                if let Some(p) = self.layout_manager.active_panel_mut().and_then(|p| {
                    p.as_any_mut()
                        .downcast_mut::<termide_panel_html::HtmlPanel>()
                }) {
                    p.apply_fetched(title, src, url);
                    return;
                }
                self.add_panel(Box::new(termide_panel_html::HtmlPanel::from_source(
                    title,
                    src,
                    Some(url),
                )));
            }
            Some(ViewKind::Markdown(src)) => {
                if let Some(p) = self.layout_manager.active_panel_mut().and_then(|p| {
                    p.as_any_mut()
                        .downcast_mut::<termide_panel_markdown::MarkdownPanel>()
                }) {
                    p.apply_fetched(title, src, url);
                    return;
                }
                self.add_panel(Box::new(
                    termide_panel_markdown::MarkdownPanel::from_source(title, src, Some(url)),
                ));
            }
            Some(ViewKind::Image(bytes, ext)) => self.open_fetched_image(&bytes, &ext, &url),
            None => self.show_error_modal(format!(
                "Unsupported content type: {}",
                fetched.content_type
            )),
        }
    }
}

/// Which viewer a fetched document maps to, with its content prepared.
enum ViewKind {
    Html(String),
    Markdown(String),
    /// Raw image bytes plus a file extension for the image preview.
    Image(Vec<u8>, String),
}

/// Classify a fetched document by Content-Type. `None` for unsupported types.
fn classify(fetched: &termide_fetch::Fetched) -> Option<ViewKind> {
    let ct = fetched.content_type.as_str();
    if let Some(ext) = image_ext(ct) {
        return Some(ViewKind::Image(fetched.body.clone(), ext.to_string()));
    }
    match ct {
        "text/html" | "application/xhtml+xml" => Some(ViewKind::Html(fetched.text())),
        "text/markdown" | "text/x-markdown" => Some(ViewKind::Markdown(fetched.text())),
        ct if ct.starts_with("text/") || ct == "application/json" || ct == "application/xml" => {
            // Plain text → shown verbatim in the HTML viewer via <pre>.
            Some(ViewKind::Html(format!(
                "<pre>{}</pre>",
                escape_html(&fetched.text())
            )))
        }
        _ => None,
    }
}

/// File extension for an image Content-Type the image preview can show.
fn image_ext(content_type: &str) -> Option<&'static str> {
    match content_type {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/bmp" => Some("bmp"),
        "image/tiff" => Some("tiff"),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some("ico"),
        _ => None,
    }
}

/// A short display title from a URL: its last path segment, else the host.
fn fetch_title(url: &str) -> String {
    let no_scheme = url.split("://").nth(1).unwrap_or(url);
    no_scheme
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(no_scheme)
        .to_string()
}

/// Keep a filename stem to a safe, bounded set of characters for a temp path.
fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

/// Minimal HTML text escaping for wrapping plain text in `<pre>`.
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

impl App {
    /// Check channel for directory size calculation results
    pub(super) fn check_dir_size_update(&mut self) {
        use termide_panel_file_manager::FileManager;

        if let Some(rx) = &self.state.dir_size_receiver {
            // Try to receive result without blocking
            if let Ok(result) = rx.try_recv() {
                let t = termide_i18n::t();
                let formatted_size = FileManager::format_size_static(result.size);
                // Keep the immediate child count in parentheses on the size
                // line, matching what was shown while calculating.
                let formatted_size = match result.item_count {
                    Some(n) => format!("{} ({})", formatted_size, t.file_info_items(n)),
                    None => formatted_size,
                };

                // Update Info or InfoAction modal if open
                match &mut self.state.active_modal {
                    Some(ActiveModal::Info(ref mut modal)) => {
                        modal.update_value(t.file_info_size(), formatted_size);
                        self.state.needs_redraw = true;
                    }
                    Some(ActiveModal::InfoAction(ref mut modal)) => {
                        modal.update_value(t.file_info_size(), formatted_size);
                        self.state.needs_redraw = true;
                    }
                    _ => {}
                }

                // Clear channel
                self.state.dir_size_receiver = None;
            }
        }
    }

    /// Single-pass check of all background panel updates:
    /// - async git status results (FileManager panels)
    /// - async directory reloads (FileManager panels, watcher-triggered)
    /// - pending git diff updates and async git diff results (all panels)
    ///
    /// Consolidated into one panel iteration to avoid 3 separate `iter_all_panels_mut()` passes.
    pub(super) fn check_background_panel_updates(&mut self) {
        for panel in self.layout_manager.iter_all_panels_mut() {
            // FileManager: drain async git status receiver
            if let Some(fm) = panel.as_file_manager_mut() {
                if fm.check_git_status_async() {
                    self.state.needs_redraw = true;
                }
            }
            // FileManager: drain async directory reload receiver
            if let Some(fm) = panel.as_file_manager_mut() {
                if fm.check_async_reload() {
                    self.state.needs_redraw = true;
                }
            }
            // All panels: check debounced git diff buffer updates
            panel.handle_command(PanelCommand::CheckPendingGitDiff);
            // All panels: drain async git diff result receiver
            if panel
                .handle_command(PanelCommand::CheckGitDiffReceiver)
                .needs_redraw()
            {
                self.state.needs_redraw = true;
            }
        }
    }

    /// If `stderr` from a failed git network op looks like an SSH key
    /// authentication failure, handle it: either prompt for the key passphrase
    /// (first failure) and let the retry run, or — if a cached passphrase was
    /// already tried — clear it and report a clean error (no prompt loop).
    /// Returns `true` when the failure was handled here.
    pub(super) fn maybe_prompt_ssh_passphrase(
        &mut self,
        operation: &str,
        repo_path: std::path::PathBuf,
        stderr: &str,
    ) -> bool {
        let s = stderr.to_ascii_lowercase();
        let is_auth_failure = (s.contains("permission denied") && s.contains("publickey"))
            || s.contains("authentication failed")
            || s.contains("passphrase");
        if !is_auth_failure {
            return false;
        }

        if self.state.git_ssh_passphrase.is_some() {
            // A cached passphrase was already tried and still failed — it's
            // wrong, or the key isn't authorized. Don't loop: clear and report.
            self.state.git_ssh_passphrase = None;
            self.state
                .set_error(format!("git {operation}: SSH authentication failed"));
            return true;
        }

        self.event_show_input(
            "Enter passphrase for your SSH key:".to_string(),
            String::new(),
            termide_core::InputAction::GitSshPassphrase {
                operation: operation.to_string(),
                repo_path,
            },
        );
        true
    }

    /// Check for background git operation result (push/pull/fetch)
    pub(super) fn check_git_operation_result(&mut self) {
        let handle = match self.state.git_operation_handle.take() {
            Some(h) => h,
            None => return,
        };

        match handle.receiver.try_recv() {
            Ok(result) => {
                self.state.ui.git_operation_in_progress = false;
                self.state.clear_status();
                // Notify all panels about git operation completed (shows Push/Pull buttons)
                self.notify_git_operation_state(false, None, 0);

                // Fetch is silent - no modal, just refresh. On failure (e.g. an
                // SSH key not loaded in the agent) surface a status-line message
                // rather than a modal, so an auto-fetch on startup never nags.
                if result.operation == "fetch" {
                    if !result.success {
                        let repo = handle.repo_path.clone();
                        if !self.maybe_prompt_ssh_passphrase("fetch", repo, &result.stderr) {
                            let msg = format!(
                                "git fetch failed: {}",
                                result.stderr.lines().next().unwrap_or("unknown error")
                            );
                            self.state.set_error(msg);
                        }
                    }
                    // Refresh all git panels silently
                    for panel in self.layout_manager.iter_all_panels_mut() {
                        panel.handle_command(PanelCommand::Reload);
                    }
                    self.state.needs_redraw = true;
                    return;
                }

                // On an SSH auth failure, prompt for the key passphrase and
                // retry instead of showing a failure modal.
                if !result.success {
                    let repo = handle.repo_path.clone();
                    if self.maybe_prompt_ssh_passphrase(&result.operation, repo, &result.stderr) {
                        for panel in self.layout_manager.iter_all_panels_mut() {
                            panel.handle_command(PanelCommand::Reload);
                        }
                        self.state.needs_redraw = true;
                        return;
                    }
                }

                // Show result modal for push/pull
                self.state.bell();
                let t = termide_i18n::t();
                let title = if result.success {
                    if result.operation == "push" {
                        t.git_push_success()
                    } else {
                        t.git_pull_success()
                    }
                } else if result.operation == "push" {
                    t.git_push_failed()
                } else {
                    t.git_pull_failed()
                };

                // Collect output lines (no labels, just plain text)
                let mut lines = vec![];

                // Add stdout lines
                for line in result.stdout.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        lines.push((String::new(), trimmed.to_string()));
                    }
                }

                // Add stderr lines
                for line in result.stderr.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        lines.push((String::new(), trimmed.to_string()));
                    }
                }

                // Fallback if no output
                if lines.is_empty() {
                    lines.push((String::new(), t.git_completed().to_string()));
                }

                let modal = InfoModal::new(title, lines);
                self.state.active_modal = Some(ActiveModal::Info(Box::new(modal)));
                self.state.needs_redraw = true;

                // Refresh all git panels
                for panel in self.layout_manager.iter_all_panels_mut() {
                    panel.handle_command(PanelCommand::Reload);
                }
            }
            Err(TryRecvError::Empty) => {
                // Check timeout (30 seconds)
                const GIT_OPERATION_TIMEOUT: std::time::Duration =
                    std::time::Duration::from_secs(30);
                if handle.started_at.elapsed() >= GIT_OPERATION_TIMEOUT {
                    log::warn!(
                        "Git {} timed out after {}s (PID: {})",
                        handle.operation,
                        GIT_OPERATION_TIMEOUT.as_secs(),
                        handle.pid
                    );

                    // Kill the process
                    #[cfg(unix)]
                    {
                        let _ = std::process::Command::new("kill")
                            .arg("-KILL")
                            .arg(handle.pid.to_string())
                            .status();
                    }
                    #[cfg(windows)]
                    {
                        let _ = std::process::Command::new("taskkill")
                            .args(["/PID", &handle.pid.to_string(), "/F"])
                            .status();
                    }

                    self.state.ui.git_operation_in_progress = false;
                    self.state.clear_status();
                    self.notify_git_operation_state(false, None, 0);

                    let t = termide_i18n::t();
                    self.show_error_modal(format!(
                        "git {} {}",
                        handle.operation,
                        t.git_operation_timed_out()
                    ));
                    return;
                }

                // Operation still in progress
                // Throttle spinner animation to 125ms (8 FPS) to reduce CPU usage
                const GIT_SPINNER_INTERVAL: std::time::Duration =
                    std::time::Duration::from_millis(125);
                let should_advance = self
                    .state
                    .last_git_spinner_update
                    .is_none_or(|t| t.elapsed() >= GIT_SPINNER_INTERVAL);

                if should_advance {
                    // Advance spinner frame for animation
                    self.state.ui.spinner_frame = self.state.ui.spinner_frame.wrapping_add(1);
                    self.state.last_git_spinner_update = Some(std::time::Instant::now());

                    // Notify all panels with updated spinner frame
                    let operation = Some(handle.operation.clone());
                    let spinner_frame = self.state.ui.spinner_frame;
                    for panel in self.layout_manager.iter_all_panels_mut() {
                        panel.handle_command(PanelCommand::SetGitOperationInProgress {
                            in_progress: true,
                            operation: operation.clone(),
                            spinner_frame,
                        });
                    }

                    // InfoActionModal spinner updated by update_modal_spinners()
                    self.state.needs_redraw = true;
                }

                // Put handle back
                self.state.git_operation_handle = Some(handle);
            }
            Err(TryRecvError::Disconnected) => {
                // Thread finished without sending (shouldn't happen)
                self.state.ui.git_operation_in_progress = false;
                self.state.clear_status();
                // Notify all panels about git operation completed (shows Push/Pull buttons)
                self.notify_git_operation_state(false, None, 0);
            }
        }
    }

    /// Check for background command operation results (.report. commands)
    pub(super) fn check_command_operation_result(&mut self) {
        if self.state.command_operation_handles.is_empty() {
            return;
        }

        let mut last_result_modal = None;

        self.state.command_operation_handles.retain(|handle| {
            match handle.receiver.try_recv() {
                Ok(result) => {
                    // Remove from Operations panel
                    if let Some(op_id) = handle.operation_id {
                        self.state.active_operations.remove(&op_id);
                    }

                    // Build modal (last completed command wins if multiple finish same tick)
                    let title = if result.success {
                        format!("{} \u{2713}", result.command_name)
                    } else {
                        format!("{} \u{2717}", result.command_name)
                    };

                    let mut lines = vec![];
                    for line in result.stdout.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            lines.push((String::new(), trimmed.to_string()));
                        }
                    }
                    for line in result.stderr.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            lines.push((String::new(), trimmed.to_string()));
                        }
                    }
                    if lines.is_empty() {
                        lines.push((String::new(), "(no output)".to_string()));
                    }

                    last_result_modal = Some((title, lines));
                    false // remove from list
                }
                Err(TryRecvError::Empty) => true, // keep polling
                Err(TryRecvError::Disconnected) => {
                    if let Some(op_id) = handle.operation_id {
                        self.state.active_operations.remove(&op_id);
                    }
                    false // remove
                }
            }
        });

        // Show modal for the last completed command
        if let Some((title, lines)) = last_result_modal {
            let modal = InfoModal::new(&title, lines);
            self.state.active_modal = Some(ActiveModal::Info(Box::new(modal)));
            self.state.needs_redraw = true;
        }
    }

    /// Check for completed background commands (.bg.) and remove from Operations panel.
    pub(super) fn check_bg_command_completion(&mut self) {
        self.state.bg_command_handles.retain(|(op_id, rx, _pid)| {
            match rx.try_recv() {
                Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.state.active_operations.remove(op_id);
                    self.state.needs_redraw = true;
                    false // remove from list
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true, // keep polling
            }
        });
    }

    /// Poll LSP status for expanded editors and completion for active editor
    pub(super) fn poll_lsp_completion(&mut self) {
        // Update LSP loading status for expanded editors only
        // Collapsed editors will catch up when they are expanded again
        let mut any_loading = false;
        for panel in self.layout_manager.iter_expanded_panels_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                // Check if server loading status changed
                if let Some(ref lsp_manager) = self.state.lsp_manager {
                    if editor.update_lsp_loading_status(lsp_manager) {
                        // Server is now ready, trigger redraw to remove spinner
                        self.state.needs_redraw = true;
                    }
                }

                // Track if any editor is still loading (for spinner animation)
                if editor.is_lsp_loading() {
                    any_loading = true;
                }
            }
        }

        // Request periodic redraw for spinner animation while any editor is loading
        // Throttle to 125ms (8 FPS) to reduce CPU usage
        if any_loading {
            const LSP_SPINNER_INTERVAL: std::time::Duration = std::time::Duration::from_millis(125);
            let should_redraw = self
                .state
                .last_lsp_loading_redraw
                .is_none_or(|t| t.elapsed() >= LSP_SPINNER_INTERVAL);
            if should_redraw {
                self.state.last_lsp_loading_redraw = Some(std::time::Instant::now());
                self.state.needs_redraw = true;
            }
        }

        // Poll for diagnostics from LSP and dispatch to editors and diagnostics panel
        if let Some(ref lsp_manager) = self.state.lsp_manager {
            while let Some(params) = lsp_manager.poll_diagnostics() {
                // Convert URI to path - parse as URL then extract file path
                let uri_str = params.uri.as_str();
                if let Some(path_str) = uri_str.strip_prefix("file://") {
                    // On Unix paths start with /, on Windows with drive letter
                    #[cfg(unix)]
                    let path = std::path::PathBuf::from(path_str);
                    #[cfg(windows)]
                    let path = std::path::PathBuf::from(path_str.trim_start_matches('/'));

                    let diagnostics = params.diagnostics;

                    // Single pass over panels: the matching editor takes an
                    // owned copy via clone(); the diagnostics panel borrows.
                    let mut editor_updated = false;
                    for panel in self.layout_manager.iter_all_panels_mut() {
                        if !editor_updated {
                            if let Some(editor) = panel.as_editor_mut() {
                                if editor.file_path() == Some(&path) {
                                    editor.update_diagnostics(diagnostics.clone());
                                    self.state.needs_redraw = true;
                                    editor_updated = true;
                                    continue;
                                }
                            }
                        }
                        if let Some(diag_panel) = panel.as_diagnostics_panel_mut() {
                            diag_panel.update_diagnostics(path.clone(), &diagnostics);
                            self.state.needs_redraw = true;
                        }
                    }

                    // Move the diagnostics into app state (no extra clone).
                    self.state.all_diagnostics.insert(path, diagnostics);
                }
            }
        }

        // Apply edits the server pushed via `workspace/applyEdit` — the path
        // command-based quick-fixes (e.g. phpactor "Import class") use to
        // deliver their changes after `workspace/executeCommand`. Collected
        // first, then applied outside the manager borrow.
        let mut server_edits: Vec<lsp_types::WorkspaceEdit> = Vec::new();
        if let Some(ref lsp_manager) = self.state.lsp_manager {
            while let Some(edit) = lsp_manager.poll_apply_edit() {
                server_edits.push(edit);
            }
        }
        for edit in server_edits {
            match self.apply_workspace_edit(edit) {
                Ok(0) => {}
                Ok(count) => self
                    .state
                    .set_info(format!("Code action applied to {count} file(s)")),
                Err(e) => self.state.set_error(format!("Code action failed: {e}")),
            }
            self.state.needs_redraw = true;
        }

        // Now handle completion and hover for the active editor only
        let mut pending_definition_event = None;
        let mut pending_references_event: Option<Vec<termide_core::ReferenceLocation>> = None;
        let mut pending_rename_edit: Option<lsp_types::WorkspaceEdit> = None;
        let mut pending_code_action_edit: Option<lsp_types::WorkspaceEdit> = None;
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                // Check if there's a pending completion response
                let had_popup_before = editor.has_completion_popup();
                editor.poll_completion();
                let has_popup_now = editor.has_completion_popup();

                // Check auto-completion timer if enabled
                if self.state.config.lsp.auto_completion {
                    if let Some(ref lsp_manager) = self.state.lsp_manager {
                        let delay_ms = self.state.config.lsp.completion_delay_ms;
                        if editor.check_auto_completion(lsp_manager, delay_ms) {
                            // Completion request triggered, needs redraw
                            self.state.needs_redraw = true;
                        }
                    }
                }

                // Trigger redraw if popup state changed
                if had_popup_before != has_popup_now {
                    self.state.needs_redraw = true;
                }

                // Check hover timer and request hover if expired
                if let Some(ref lsp_manager) = self.state.lsp_manager {
                    let delay_ms = self.state.config.lsp.hover_delay_ms;
                    if editor.check_hover_timer(lsp_manager, delay_ms) {
                        self.state.needs_redraw = true;
                    }
                }

                // Poll for hover response
                let had_hover_popup = editor.has_hover_popup();
                editor.poll_hover();
                if had_hover_popup != editor.has_hover_popup() {
                    self.state.needs_redraw = true;
                }

                // Poll for definition response (Ctrl+click go-to-definition)
                if let Some(event) = editor.poll_definition() {
                    // Store event to be processed after we release the borrow
                    pending_definition_event = Some(event);
                    self.state.needs_redraw = true;
                }

                // Poll for rename response (F2)
                if let Some(edit) = editor.poll_rename() {
                    pending_rename_edit = Some(edit);
                    self.state.needs_redraw = true;
                }

                // Poll for code-action response (opens the popup when actions
                // arrive after the keypress that requested them).
                let had_code_action_popup = editor.has_code_action_popup();
                editor.poll_code_action();
                if had_code_action_popup != editor.has_code_action_popup() {
                    self.state.needs_redraw = true;
                }

                // Resolve a deferred edit for an accepted action, then collect
                // the ready edit to apply after the borrow.
                if let Some(action) = editor.take_code_action_resolve() {
                    if let Some(ref lsp_manager) = self.state.lsp_manager {
                        editor.request_code_action_resolve(action, lsp_manager);
                    }
                }
                editor.poll_code_action_resolve();
                if let Some(edit) = editor.take_code_action_edit() {
                    pending_code_action_edit = Some(edit);
                    self.state.needs_redraw = true;
                }
                // Run a command-based action; its edit returns via applyEdit.
                if let Some(command) = editor.take_code_action_command() {
                    if let Some(ref lsp_manager) = self.state.lsp_manager {
                        editor.request_execute_command(command, lsp_manager);
                    }
                }

                // Poll for references response (Shift+F12)
                if let Some(locations) = editor.poll_references() {
                    let ref_locations: Vec<termide_core::ReferenceLocation> = locations
                        .into_iter()
                        .filter_map(|loc| {
                            let uri_str = loc.uri.as_str();
                            if !uri_str.starts_with("file://") {
                                return None;
                            }
                            let path_str = &uri_str[7..];
                            #[cfg(unix)]
                            let path = std::path::PathBuf::from(path_str);
                            #[cfg(windows)]
                            let path = std::path::PathBuf::from(path_str.trim_start_matches('/'));
                            Some(termide_core::ReferenceLocation {
                                path,
                                line: loc.range.start.line as usize,
                                column: loc.range.start.character as usize,
                            })
                        })
                        .collect();
                    pending_references_event = Some(ref_locations);
                    self.state.needs_redraw = true;
                }
            }
        }

        // Process pending definition event (outside of panel borrow)
        if let Some(event) = pending_definition_event {
            if let Err(e) = self.process_panel_events(vec![event]) {
                log::error!("Error processing definition event: {}", e);
            }
        }

        // Process pending references event (outside of panel borrow)
        if let Some(locations) = pending_references_event {
            let event = if locations.is_empty() {
                termide_core::PanelEvent::SetStatusMessage {
                    message: "No references found".to_string(),
                    is_error: false,
                }
            } else {
                termide_core::PanelEvent::OpenReferencesPanel {
                    locations,
                    symbol_name: None,
                }
            };
            if let Err(e) = self.process_panel_events(vec![event]) {
                log::error!("Error processing references event: {}", e);
            }
        }

        // Apply an accepted code action's WorkspaceEdit (outside the panel borrow)
        if let Some(edit) = pending_code_action_edit {
            match self.apply_workspace_edit(edit) {
                Ok(0) => self
                    .state
                    .set_info("Code action made no changes".to_string()),
                Ok(count) => self
                    .state
                    .set_info(format!("Code action applied to {count} file(s)")),
                Err(e) => self.state.set_error(format!("Code action failed: {e}")),
            }
        }

        // Apply pending rename WorkspaceEdit (outside of panel borrow)
        if let Some(edit) = pending_rename_edit {
            let t = termide_i18n::t();
            match self.apply_workspace_edit(edit) {
                Ok(0) => {
                    // Valid reply with no changes — typically means the LSP server
                    // couldn't find references or rejected the rename silently.
                    self.state.set_info(t.lsp_rename_no_changes().to_string());
                }
                Ok(n) => self.state.set_info(t.lsp_rename_result(n)),
                Err(e) => {
                    log::error!("Rename failed: {}", e);
                    self.show_error_modal(format!("Rename failed: {}", e));
                }
            }
        }
    }

    /// Update system resource monitoring (CPU, RAM, network)
    /// Respects the configured update interval.
    /// Only triggers redraw if display values actually changed.
    pub(super) fn update_system_resources(&mut self) {
        let interval =
            std::time::Duration::from_millis(self.state.config.general.resource_monitor_interval);
        let elapsed = self.state.last_resource_update.elapsed();

        if elapsed >= interval {
            let old_stats = self.state.system_monitor.stats();
            let old_net_down = self.state.system_monitor.net_download_rate();
            let old_net_up = self.state.system_monitor.net_upload_rate();
            self.state.system_monitor.update();
            self.state.last_resource_update = std::time::Instant::now();
            let new_stats = self.state.system_monitor.stats();
            let new_net_down = self.state.system_monitor.net_download_rate();
            let new_net_up = self.state.system_monitor.net_upload_rate();
            // Only redraw if display values actually changed
            if old_stats.cpu_usage.round() as u8 != new_stats.cpu_usage.round() as u8
                || old_stats.memory_used / (1024 * 1024) != new_stats.memory_used / (1024 * 1024)
                || old_net_down / 1024 != new_net_down / 1024
                || old_net_up / 1024 != new_net_up / 1024
            {
                self.state.needs_redraw = true;
            }
            self.update_disk_space();
        }
    }

    /// Update cached disk space for the active panel.
    /// Called on each resource tick so status bar reads from cache instead of per-render statvfs.
    fn update_disk_space(&mut self) {
        let disk = self.get_active_panel_disk_space();
        if disk != self.state.cache.disk_space {
            self.state.cache.disk_space = disk;
            self.state.needs_redraw = true;
        }
    }

    /// Update spinner in all modals that support animation
    /// Throttled to 125ms (8 FPS) to reduce unnecessary redraws
    pub(super) fn update_modal_spinners(&mut self) {
        const SPINNER_INTERVAL: std::time::Duration = std::time::Duration::from_millis(125);

        // Throttle spinner updates for all modals
        let should_update = self
            .state
            .last_spinner_update
            .is_none_or(|t| t.elapsed() >= SPINNER_INTERVAL);

        if !should_update {
            return;
        }

        match &mut self.state.active_modal {
            Some(ActiveModal::Info(ref mut modal)) => {
                // Update spinner only if calculation is still ongoing
                if self.state.dir_size_receiver.is_some() {
                    modal.advance_spinner();
                    self.state.last_spinner_update = Some(std::time::Instant::now());
                    self.state.needs_redraw = true;
                }
            }
            Some(ActiveModal::InfoAction(ref mut modal)) => {
                // Update spinner only if operation is still ongoing
                if modal.is_operation_in_progress() {
                    modal.advance_spinner();
                    self.state.last_spinner_update = Some(std::time::Instant::now());
                    self.state.needs_redraw = true;
                }
            }
            _ => {}
        }

        // Auto-refresh resource modal per resource_monitor_interval config
        self.refresh_resource_modal();
    }

    /// Refresh resource modal content if one is open and interval has elapsed.
    fn refresh_resource_modal(&mut self) {
        let interval =
            std::time::Duration::from_millis(self.state.config.general.resource_monitor_interval);

        let Some(kind) = self.state.resource_modal_kind else {
            return;
        };

        let should_refresh = self
            .state
            .last_resource_modal_refresh
            .is_none_or(|t| t.elapsed() >= interval);

        if !should_refresh {
            return;
        }

        use crate::state::ResourceModalKind;
        let lines = match kind {
            ResourceModalKind::Cpu | ResourceModalKind::Ram => self.build_process_lines(kind),
            ResourceModalKind::Network => self.build_network_modal_lines(),
            ResourceModalKind::Disk => self.build_disk_modal_lines(),
        };

        if let Some(ActiveModal::Info(ref mut modal)) = self.state.active_modal {
            modal.set_lines(lines);
            self.state.last_resource_modal_refresh = Some(std::time::Instant::now());
            self.state.needs_redraw = true;
        }
    }
}
