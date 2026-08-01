//! LSP polling: server loading status and spinner, incoming diagnostics
//! dispatch, server-pushed `workspace/applyEdit`, and the active editor's
//! completion / hover / definition / references / rename / code-action replies.

use crate::PanelExt;

use super::App;

impl App {
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
}
