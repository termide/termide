//! Main keyboard event handling for the application.
//!
//! Dispatches key events to modals, menus, global hotkeys, or active panels.

// Note: PanelExt is still used for panel-specific resource extraction
// (take_config_update, dir_size_receiver) which don't fit the command pattern.

use anyhow::Result;
use std::sync::Arc;

use super::App;
use crate::state::{ActiveModal, PendingAction};
use crate::PanelExt;
use termide_i18n as i18n;

impl App {
    /// Whether an app-level dropdown/submenu list is open (the cases the key
    /// dispatch below routes to). Used so the mouse wheel scrolls the open list
    /// instead of the panel underneath. The menu bar alone (no submenu) is not
    /// a scrollable list, so it is excluded.
    pub(in crate::app) fn any_menu_dropdown_open(&self) -> bool {
        let ui = &self.state.ui;
        ui.projects_submenu.open
            || ui.tools_submenu.open
            || ui.commands_submenu.open
            || ui.ai_submenu.open
            || ui.stash_submenu.open
            || ui.bookmarks_submenu.open
            || ui.panel_action_menu.open
            || ui.operation_action_menu.open
            || ui.options_submenu.open
    }

    /// Handle keyboard event
    pub(super) fn handle_key_event(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        // Raw key — no translation here.
        // HotkeyTable.matches() handles Cyrillic normalization internally.
        // Clear status message on any key press
        if self.state.ui.status_message.is_some() {
            self.state.clear_status();
        }

        // Cancel an in-progress panel drag on Esc before anything else
        if self.state.ui.panel_drag.is_pending_or_active()
            && key.code == crossterm::event::KeyCode::Esc
        {
            self.state.ui.panel_drag.cancel();
            self.state.needs_redraw = true;
            return Ok(());
        }

        // If modal window is open, handle it
        if self.state.has_modal() {
            return self.handle_modal_key(key);
        }

        // If Sessions submenu is open, handle its navigation
        if self.state.ui.projects_submenu.open {
            return self.handle_projects_submenu_key(key);
        }

        // If Tools submenu is open, handle its navigation
        if self.state.ui.tools_submenu.open {
            return self.handle_tools_submenu_key(key);
        }

        // If Commands submenu is open, handle its navigation
        if self.state.ui.commands_submenu.open {
            return self.handle_commands_submenu_key(key);
        }

        // If the AI submenu is open, handle its navigation
        if self.state.ui.ai_submenu.open {
            return self.handle_ai_submenu_key(key);
        }

        // If Stash submenu is open, handle its navigation
        if self.state.ui.stash_submenu.open {
            return self.handle_stash_submenu_key(key);
        }

        // If Bookmarks submenu is open, handle its navigation
        if self.state.ui.bookmarks_submenu.open {
            return self.handle_bookmarks_submenu_key(key);
        }

        // If Panel action context menu is open, handle its navigation
        if self.state.ui.panel_action_menu.open {
            return self.handle_panel_action_menu_key(key);
        }

        // If per-operation popup menu is open, route keys to its handler
        if self.state.ui.operation_action_menu.open {
            return self.handle_operation_action_menu_key(key);
        }

        // If Options submenu is open, handle submenu navigation
        if self.state.ui.options_submenu.open {
            return self.handle_submenu_key(key);
        }

        // If menu is open, handle menu navigation
        if self.state.is_menu_open() {
            return self.handle_menu_key(key);
        }

        // Handle app-level actions via HotkeyTable
        if self.handle_global_hotkey(&key)? {
            return Ok(());
        }

        // Pass key to active panel
        // `pending_status` carries a (message, is_error) pair to be applied to AppState
        // after the mutable panel borrow is released below.
        let mut pending_status: Option<(String, bool)> = None;
        // WorkspaceEdit from an accepted code action, applied after the panel
        // borrow is released (it may touch other files / panels).
        let mut pending_code_action_edit: Option<lsp_types::WorkspaceEdit> = None;
        let chord = termide_core::KeyChord::new(key, &self.normalizer);
        let (events, modal_request, config_update, escape_close) = if let Some(panel) =
            self.layout_manager.active_panel_mut()
        {
            // Escape: decide BEFORE handling the key, because handling it may
            // clear the very state that captures Escape (e.g. close an inline
            // search bar). The question is whether the panel had a mode to exit
            // when Escape arrived, not after.
            let captured_escape = panel.captures_escape();
            let mut events = panel.handle_key(chord);

            // If the panel had nothing to capture, Escape requests panel close.
            let escape_close = key.code == crossterm::event::KeyCode::Esc
                && key.modifiers.is_empty()
                && !captured_escape;

            // Legacy methods still in use
            let modal_request = panel.take_modal_request();
            let config_update = if let Some(editor) = panel.as_editor_mut() {
                // Cancel hover timer and close popup on any key press
                editor.cancel_hover_and_close_popup();

                // Flush pending LSP changes
                if let Some(ref lsp_manager) = self.state.lsp_manager {
                    editor.flush_lsp_changes(lsp_manager);
                }

                // Handle completion request (Ctrl+Space)
                if editor.take_completion_request().is_some() {
                    if let Some(ref lsp_manager) = self.state.lsp_manager {
                        editor.request_completion(lsp_manager);
                    }
                }

                // Handle auto-completion on character insertion
                if self.state.config.lsp.auto_completion {
                    if let Some(ch) = editor.take_last_inserted_char() {
                        if let Some(ref lsp_manager) = self.state.lsp_manager {
                            editor.schedule_auto_completion(ch, lsp_manager);
                        }
                    }
                }

                // Poll for completion response
                editor.poll_completion();

                // Handle hover request (mouse hover)
                if let Some((line, col)) = editor.take_hover_request() {
                    if let Some(ref lsp_manager) = self.state.lsp_manager {
                        editor.request_hover(line, col, lsp_manager);
                    }
                }

                // Poll for hover response
                editor.poll_hover();

                // Handle go-to-definition request (Ctrl+click)
                if let Some((line, col)) = editor.take_definition_request() {
                    if let Some(ref lsp_manager) = self.state.lsp_manager {
                        editor.request_definition(line, col, lsp_manager);
                    }
                }

                // Poll for definition response (returns PanelEvent::OpenFileAt)
                if let Some(event) = editor.poll_definition() {
                    events.push(event);
                }

                // Handle find-references request (Shift+F12)
                if let Some((line, col)) = editor.take_references_request() {
                    if let Some(ref lsp_manager) = self.state.lsp_manager {
                        editor.request_references(line, col, lsp_manager);
                    }
                }

                // Handle rename-symbol request (F2)
                if let Some((line, col)) = editor.take_rename_request() {
                    let word = editor.get_word_at_cursor();
                    let path_opt = editor.file_path().map(|p| p.to_path_buf());
                    if word.is_empty() {
                        let t = termide_i18n::t();
                        pending_status = Some((t.lsp_rename_no_identifier().to_string(), false));
                    } else if path_opt.is_none() {
                        let t = termide_i18n::t();
                        pending_status = Some((t.lsp_rename_unsaved_file().to_string(), true));
                    } else if let Some(path) = path_opt {
                        events.push(termide_core::PanelEvent::ShowInput {
                            prompt: format!("Rename '{}':", word),
                            initial_value: word,
                            on_submit: termide_core::InputAction::RenameSymbol {
                                file_path: path,
                                line,
                                column: col,
                            },
                        });
                    }
                }

                // Handle code-action request (Alt+Enter)
                if editor.take_code_action_request() {
                    if let Some(ref lsp_manager) = self.state.lsp_manager {
                        editor.request_code_action(lsp_manager);
                    }
                }

                // Poll for code-action response (opens the popup)
                editor.poll_code_action();

                // An accepted action with a deferred edit needs a resolve round-trip.
                if let Some(action) = editor.take_code_action_resolve() {
                    if let Some(ref lsp_manager) = self.state.lsp_manager {
                        editor.request_code_action_resolve(action, lsp_manager);
                    }
                }
                editor.poll_code_action_resolve();

                // A command-based action (e.g. phpactor "Import class") runs via
                // executeCommand; its edit returns later through applyEdit.
                if let Some(command) = editor.take_code_action_command() {
                    if let Some(ref lsp_manager) = self.state.lsp_manager {
                        editor.request_execute_command(command, lsp_manager);
                    }
                }

                // Collect a ready code-action edit to apply after the borrow.
                pending_code_action_edit = editor.take_code_action_edit();

                // Poll for references response
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
                    if ref_locations.is_empty() {
                        events.push(termide_core::PanelEvent::SetStatusMessage {
                            message: "No references found".to_string(),
                            is_error: false,
                        });
                    } else {
                        events.push(termide_core::PanelEvent::OpenReferencesPanel {
                            locations: ref_locations,
                            symbol_name: None,
                        });
                    }
                }

                editor.take_config_update()
            } else {
                None
            };

            (events, modal_request, config_update, escape_close)
        } else {
            (vec![], None, None, false)
        };

        // Check if panel already handled Escape by emitting ClosePanel
        let panel_handled_escape = escape_close
            && events
                .iter()
                .any(|e| matches!(e, termide_core::PanelEvent::ClosePanel));

        // Process panel events (new event-based architecture)
        self.process_panel_events(events)?;

        // Escape close: show confirmation before closing panel
        // Skip if panel already handled Escape by emitting ClosePanel
        if escape_close && !panel_handled_escape {
            self.handle_escape_close_request()?;
        }

        // Apply an accepted code action's WorkspaceEdit (outside the panel
        // borrow, since it can write other files and reload their editors).
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

        // Apply pending status message (e.g., from rename guard rails)
        if let Some((msg, is_error)) = pending_status {
            if is_error {
                self.state.set_error(msg);
            } else {
                self.state.set_info(msg);
            }
        }

        // Apply config update if present (legacy, still used by Editor)
        if let Some(new_config) = config_update {
            super::agent_panel::publish_ai_settings(&new_config.ai);
            self.state.config = Arc::new(new_config.clone());
            self.state.set_theme(&new_config.general.theme);
            self.state
                .set_info(termide_i18n::t().status_config_saved().to_string());
        }

        // Handle modal window request from panel (legacy, still used)
        if let Some((action, modal)) = modal_request {
            self.handle_modal_request(action, modal)?;
        }

        Ok(())
    }

    /// Handle modal request from panel
    pub(super) fn handle_modal_request(
        &mut self,
        mut action: PendingAction,
        mut modal: ActiveModal,
    ) -> Result<()> {
        // For Copy/Move - find all other FM panels and create suggestions
        let is_copy = matches!(action, PendingAction::CopyPath { .. });

        match &mut action {
            PendingAction::CopyPath {
                target_directory,
                sources,
                ..
            }
            | PendingAction::MovePath {
                target_directory,
                sources,
                ..
            } => {
                if target_directory.is_none() && !sources.is_empty() {
                    modal = self.prepare_copy_move_modal(sources, target_directory, is_copy);
                }
            }
            _ => {}
        }

        if let (PendingAction::GoToPath { current_directory }, ActiveModal::Input(input)) =
            (&action, &mut modal)
        {
            input.set_suggestions(Box::new(crate::app::modal::PathSuggestions::new(
                current_directory.clone(),
                self.project_root.clone(),
            )));
        }

        // Handle navigation actions without modal window
        match action {
            PendingAction::NextPanel => {
                self.layout_manager.next_group();
                return Ok(());
            }
            PendingAction::PrevPanel => {
                self.layout_manager.prev_group();
                return Ok(());
            }
            _ => {}
        }

        self.state.set_pending_action(action, modal);

        // Check if there's a channel receiver for directory size in panel
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(fm) = panel.as_file_manager_mut() {
                if let Some(rx) = fm.dir_size_receiver.take() {
                    self.state.dir_size_receiver = Some(rx);
                }
            }
        }

        Ok(())
    }

    /// Prepare modal for copy/move operations
    fn prepare_copy_move_modal(
        &mut self,
        sources: &[std::path::PathBuf],
        target_directory: &mut Option<std::path::PathBuf>,
        is_copy: bool,
    ) -> ActiveModal {
        // CRITICAL: Close any existing modal first to prevent stale modals during operations
        self.state.close_modal();

        // Get source directory to exclude from default selection
        let source_dir = sources[0].parent().map(|p| p.to_path_buf());
        let source_dir_str = source_dir.as_ref().map(|p| p.display().to_string());

        // Find all unique paths from other panels
        let options = self.find_all_other_panel_paths();
        let unique_paths_count = options.len();

        // For single file: append filename to each option so dropdown shows full paths
        let options = if sources.len() == 1 {
            if let Some(file_name) = sources[0].file_name().and_then(|n| n.to_str()) {
                options
                    .into_iter()
                    .map(|mut opt| {
                        let base = opt.value.trim_end_matches('/');
                        opt.value = format!("{}/{}", base, file_name);
                        opt.display = opt.value.clone();
                        opt
                    })
                    .collect()
            } else {
                options
            }
        } else {
            options
        };

        // Filter out source directory for default selection
        let source_dir_prefix = source_dir_str.as_ref().map(|s| format!("{}/", s));
        let default_dest = options
            .iter()
            .find(|opt| {
                // Compare directory part of option value against source directory
                source_dir_prefix
                    .as_ref()
                    .map(|prefix| !opt.value.starts_with(prefix.as_str()))
                    .unwrap_or(true)
            })
            .map(|opt| opt.value.clone())
            .or_else(|| {
                source_dir.as_ref().map(|p| {
                    let base = format!("{}/", p.display());
                    if sources.len() == 1 {
                        if let Some(name) = sources[0].file_name().and_then(|n| n.to_str()) {
                            return format!("{}{}", base, name);
                        }
                    }
                    base
                })
            })
            .unwrap_or_else(|| "/".to_string());

        *target_directory = Some(std::path::PathBuf::from(default_dest.trim_end_matches('/')));

        // Prepare title and prompt
        let t = i18n::t();
        let (title, prompt) = if sources.len() == 1 {
            let source_name = sources[0]
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?");

            if is_copy {
                (
                    t.modal_copy_single_title(source_name),
                    t.modal_copy_single_prompt(source_name),
                )
            } else {
                (
                    t.modal_move_single_title(source_name),
                    t.modal_move_single_prompt(source_name),
                )
            }
        } else if is_copy {
            (
                t.modal_copy_multiple_title(sources.len()),
                t.modal_copy_multiple_prompt(sources.len()),
            )
        } else {
            (
                t.modal_move_multiple_title(sources.len()),
                t.modal_move_multiple_prompt(sources.len()),
            )
        };

        // Choose modal based on number of unique paths
        if unique_paths_count >= 2 {
            let mut new_modal =
                termide_modal::EditableSelectModal::new(title, prompt, &default_dest, options);
            if is_copy {
                let t = i18n::t();
                new_modal = new_modal.with_checkbox(t.checkbox_create_symlink().to_string());
                new_modal =
                    new_modal.with_conditional_checkbox(t.checkbox_relative_symlink().to_string());
            }
            ActiveModal::EditableSelect(Box::new(new_modal))
        } else {
            let mut new_modal =
                termide_modal::InputModal::with_default(title, prompt, &default_dest);
            if is_copy {
                let t = i18n::t();
                new_modal = new_modal.with_checkbox(t.checkbox_create_symlink().to_string());
                new_modal =
                    new_modal.with_conditional_checkbox(t.checkbox_relative_symlink().to_string());
            }
            ActiveModal::Input(Box::new(new_modal))
        }
    }
}
