//! LSP polling: server loading status and spinner, incoming diagnostics
//! dispatch, server-pushed `workspace/applyEdit`, and the active editor's
//! completion / hover / definition / references / rename / code-action replies.

use crate::PanelExt;

use super::App;

impl App {
    /// Poll LSP status for visible editors and completion for active editor
    pub(super) fn poll_lsp_completion(&mut self) {
        self.ask_lsp_trust();

        // A restarted server knows none of the open documents: every editor
        // whose server is new opens its document again. Done only when a
        // server went live, not on every tick.
        if let Some(lsp_manager) = self.state.lsp_manager.as_mut() {
            if lsp_manager.poll_restarts() {
                for panel in self.layout_manager.iter_all_panels_mut() {
                    if let Some(editor) = panel.as_editor_mut() {
                        editor.resync_lsp(lsp_manager);
                    }
                }
                self.state.needs_redraw = true;
            }
        }

        // Update LSP loading status for editors showing content only;
        // collapsed editors catch up when they grow again
        let mut any_loading = false;
        let area_height = self.panel_area_height();
        for panel in self.layout_manager.iter_visible_panels_mut(area_height) {
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
        let mut published = Vec::new();
        if let Some(ref lsp_manager) = self.state.lsp_manager {
            while let Some(params) = lsp_manager.poll_diagnostics() {
                // A file:// URI, percent-decoded, to a path.
                if let Some(path) = termide_lsp::uri_to_path(&params.uri) {
                    published.push((path, params.diagnostics));
                }
            }
        }
        for (path, diagnostics) in published {
            self.note_lsp_report(&path, &diagnostics);
            self.state.lsp_diagnostics.insert(path.clone(), diagnostics);
            self.publish_diagnostics(path);
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
                    .set_info(termide_i18n::t().lsp_code_action_applied_fmt(count)),
                Err(e) => self
                    .state
                    .set_error(termide_i18n::t().lsp_code_action_failed_fmt(&e.to_string())),
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

                // A fold from the menu asks here; the answer, or the wait
                // running out, folds.
                if editor.take_folding_request() {
                    editor.request_folding_ranges(self.state.lsp_manager.as_ref());
                    self.state.needs_redraw = true;
                }
                if editor.poll_folding_ranges() {
                    self.state.needs_redraw = true;
                }

                // Poll for references response (Shift+F12)
                if let Some(locations) = editor.poll_references() {
                    let ref_locations: Vec<termide_core::ReferenceLocation> = locations
                        .into_iter()
                        .filter_map(|loc| {
                            let path = termide_lsp::uri_to_path(&loc.uri)?;
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
                    message: termide_i18n::t().references_none().to_string(),
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
                    .set_info(termide_i18n::t().lsp_code_action_no_changes().to_string()),
                Ok(count) => self
                    .state
                    .set_info(termide_i18n::t().lsp_code_action_applied_fmt(count)),
                Err(e) => self
                    .state
                    .set_error(termide_i18n::t().lsp_code_action_failed_fmt(&e.to_string())),
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
                    self.show_error_modal(termide_i18n::t().lsp_rename_failed_fmt(&e.to_string()));
                }
            }
        }
    }
}

impl App {
    /// Ask about the workspaces a language server was held back in, since
    /// it would run their code: one window at a time, when none is open.
    fn ask_lsp_trust(&mut self) {
        if let Some(manager) = self.state.lsp_manager.as_mut() {
            for withheld in manager.take_withheld() {
                if !self.state.lsp_trust_asks.contains(&withheld) {
                    self.state.lsp_trust_asks.push_back(withheld);
                }
            }
        }
        if self.state.active_modal.is_some() {
            return;
        }
        let Some(withheld) = self.state.lsp_trust_asks.pop_front() else {
            return;
        };
        let t = termide_i18n::t();
        let modal = termide_modal::ConfirmModal::new(
            t.lsp_trust_title(),
            t.lsp_trust_fmt(&withheld.command, &withheld.root.display().to_string()),
        )
        // It comes up unasked, maybe while the user types: a stray Enter
        // must not grant the trust.
        .defaulting_to_no();
        self.state.set_pending_action(
            termide_state::PendingAction::TrustLanguageWorkspace {
                lang: withheld.lang,
                root: withheld.root,
            },
            crate::state::ActiveModal::Confirm(Box::new(modal)),
        );
        self.state.needs_redraw = true;
    }

    /// Trust the workspace at `root` with the language server of `lang`,
    /// the user having agreed, and start it for the editors waiting on it.
    pub(super) fn trust_language_workspace(&mut self, lang: &str, root: &std::path::Path) {
        let approvals = crate::state::workspace_approvals(&self.project_root);
        if let Err(e) = approvals.trust_workspace(root) {
            log::warn!("Cannot keep the trust of {}: {e}", root.display());
            self.state.set_error(e);
            return;
        }
        let Some(manager) = self.state.lsp_manager.as_mut() else {
            return;
        };
        manager.release(lang, root);
        let parked = self
            .open_projects
            .parked_mut()
            .flat_map(|(_, project)| project.panels_mut());
        for panel in self.layout_manager.iter_all_panels_mut().chain(parked) {
            let Some(editor) = panel.as_editor_mut() else {
                continue;
            };
            let Some(path) = editor.file_path().map(std::path::Path::to_path_buf) else {
                continue;
            };
            // Only the editors left without a server: one already opened in
            // a server must not open its document there twice.
            let waits = termide_lsp::LspManager::detect_language(&path)
                .is_some_and(|l| l == lang && manager.server_generation(&l, &path).is_none());
            if waits {
                editor.detach_lsp();
                editor.init_lsp(manager);
            }
        }
        self.state.needs_redraw = true;
    }

    /// Restart the language server that handles `file`; its editors open
    /// their documents in the new process once it is up.
    pub(super) fn restart_language_server(&mut self, file: Option<std::path::PathBuf>) {
        let restarted = file.as_deref().is_some_and(|file| {
            let lang = termide_lsp::LspManager::detect_language(file);
            match (lang, self.state.lsp_manager.as_mut()) {
                (Some(lang), Some(lsp_manager)) => lsp_manager.restart_server(&lang, file),
                _ => false,
            }
        });
        if restarted {
            self.state.needs_redraw = true;
        } else {
            let t = termide_i18n::t();
            self.state.set_info(t.lsp_restart_no_server().to_string());
        }
    }

    /// Show `path`'s diagnostics — the language servers' and the plugins'
    /// together — in its editor and the diagnostics panel, and keep them for
    /// the editors and panels opened later.
    pub(super) fn publish_diagnostics(&mut self, path: std::path::PathBuf) {
        let mut diagnostics = self
            .state
            .lsp_diagnostics
            .get(&path)
            .cloned()
            .unwrap_or_default();
        if let Some(by_plugin) = self.state.plugin_diagnostics.get(&path) {
            diagnostics.extend(by_plugin.values().flatten().cloned());
        }

        // Single pass over panels: the matching editor takes an owned copy
        // via clone(); the diagnostics panel borrows.
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

        if diagnostics.is_empty() {
            self.state.all_diagnostics.remove(&path);
        } else {
            self.state.all_diagnostics.insert(path, diagnostics);
        }
    }

    /// Carry out a change of `lsp.enabled` or of the server list in the
    /// running config. Turned off, the servers are shut
    /// down and the editors detached; turned on, the editors open their
    /// documents in new servers; with a server list changed, only the
    /// servers of the languages whose entry changed are replaced, and only
    /// their editors open their documents again. Shutting servers down
    /// waits on them a bounded while, so it is done off this thread.
    pub(super) fn reconfigure_lsp(&mut self) {
        let config = std::sync::Arc::clone(&self.state.config);
        let servers = crate::state::AppState::create_lsp_config(&config);
        let retired = match (self.state.lsp_manager.is_some(), config.lsp.enabled) {
            (true, true) => {
                let Some(manager) = self.state.lsp_manager.as_mut() else {
                    return;
                };
                // Their servers clear their diagnostics as they go.
                Some(manager.set_config(servers))
            }
            (true, false) => {
                if let Some(manager) = self.state.lsp_manager.take() {
                    std::thread::spawn(move || drop(manager));
                }
                // The servers clear nothing when the manager goes.
                let paths: Vec<_> = self.state.lsp_diagnostics.drain().map(|(p, _)| p).collect();
                for path in paths {
                    self.publish_diagnostics(path);
                }
                None
            }
            (false, true) => {
                self.state.lsp_manager = Some(crate::state::AppState::new_lsp_manager(
                    servers,
                    &self.state.lsp_trust_root,
                ));
                None
            }
            (false, false) => return,
        };
        if retired.as_ref().is_some_and(|langs| langs.is_empty()) {
            return;
        }
        self.forget_lsp_checks(retired.as_ref());

        let parked = self
            .open_projects
            .parked_mut()
            .flat_map(|(_, project)| project.panels_mut());
        for panel in self.layout_manager.iter_all_panels_mut().chain(parked) {
            let Some(editor) = panel.as_editor_mut() else {
                continue;
            };
            let affected = retired.as_ref().is_none_or(|langs| {
                editor
                    .file_path()
                    .and_then(termide_lsp::LspManager::detect_language)
                    .is_some_and(|lang| langs.contains(&lang))
            });
            if !affected {
                continue;
            }
            editor.detach_lsp();
            if let Some(manager) = self.state.lsp_manager.as_mut() {
                editor.init_lsp(manager);
            } else if let Some(path) = editor.file_path() {
                // A parked project's editors take diagnostics only from
                // `all_diagnostics` when it comes back, which has no entry
                // for a file left without any.
                let left = self.state.all_diagnostics.get(path).cloned();
                editor.update_diagnostics(left.unwrap_or_default());
            }
        }
        self.state.needs_redraw = true;
    }
}
