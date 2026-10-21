//! Panel event processing for the application.
//!
//! Processes `PanelEvent`s emitted by panels and translates them
//! into application state changes.
//!
//! The concrete handlers are grouped into sibling submodules by concern
//! (file/view opening, navigation, prompts, git operations, filesystem
//! watching, and the operations panel); this module retains the event
//! dispatch surface.

// Note: PanelExt is used for panel-specific operations (mouse clicks, resize)
// that require concrete type access. Common operations use Panel::handle_command().

use anyhow::Result;

use super::App;
use termide_core::{PanelCommand, PanelEvent};

mod file_view;
mod git_ops;
mod navigation;
mod operations_panel;
mod prompts;
mod watch;

impl App {
    /// Process events emitted by a panel.
    ///
    /// This method handles all `PanelEvent` variants and translates them
    /// into appropriate application state changes.
    pub(super) fn process_panel_events(&mut self, events: Vec<PanelEvent>) -> Result<()> {
        for event in events {
            self.process_single_event(event)?;
        }
        Ok(())
    }

    /// Process a single panel event.
    pub(super) fn process_single_event(&mut self, event: PanelEvent) -> Result<()> {
        match event {
            // === File operations ===
            PanelEvent::OpenFile(path) => {
                self.event_open_file(path)?;
            }

            PanelEvent::ViewFile(path) => {
                self.event_view_file(path)?;
            }

            PanelEvent::OpenFileAt { path, line, column } => {
                self.event_open_file_at(path, line, column)?;
            }

            PanelEvent::ExecuteFile(path) => {
                self.event_execute_file(path)?;
            }

            PanelEvent::StartDebug { file } => {
                self.start_debug(file)?;
            }

            PanelEvent::ConfigureDebug { file } => self.open_debug_setup(file.as_deref()),
            PanelEvent::RestartDebug => self.restart_debug(),

            PanelEvent::ClearBreakpoints => self.clear_breakpoints(),

            PanelEvent::RunCommand { command, cwd } => {
                self.event_run_command(command, cwd)?;
            }

            PanelEvent::PreviewMedia(path) => {
                self.event_preview_media(path)?;
            }

            PanelEvent::ViewBinary(path) => {
                self.event_view_binary(path)?;
            }

            PanelEvent::ViewDatabase(path) => {
                self.event_view_database(path)?;
            }

            PanelEvent::EditBinary(path) => {
                self.event_edit_binary(path)?;
            }

            PanelEvent::SwapActiveToHex(path) => {
                self.event_swap_active_to_hex(path)?;
            }

            PanelEvent::SwapActiveToText(path) => {
                self.event_swap_active_to_text(path)?;
            }

            PanelEvent::ViewMarkdown(path) => {
                self.event_view_markdown(path)?;
            }

            PanelEvent::SwapActiveToMarkdown(path) => {
                self.event_swap_active_to_markdown(path)?;
            }

            PanelEvent::ViewMermaid(path) => {
                self.event_view_mermaid(path)?;
            }

            PanelEvent::SwapActiveToMermaid(path) => {
                self.event_swap_active_to_mermaid(path)?;
            }

            PanelEvent::SaveContentAs {
                content,
                default_name,
            } => {
                self.event_save_content_as(content, default_name)?;
            }

            PanelEvent::NavigateUrl(url) => {
                self.start_url_fetch_in_place(url);
            }

            PanelEvent::OpenLink(link) => {
                self.event_open_link(link)?;
            }

            PanelEvent::OpenLinkExternal(link) => {
                self.event_open_link_external(link)?;
            }

            PanelEvent::ViewHtml(path) => {
                self.event_view_html(path)?;
            }

            PanelEvent::SwapActiveToHtml(path) => {
                self.event_swap_active_to_html(path)?;
            }

            PanelEvent::AgentRun { panel, finished } => {
                let (method, params) = super::plugins::agent_run_event(panel, finished);
                let project = self.open_projects.current().to_path_buf();
                self.plugins.host.emit(&project, method, &params);
            }
            PanelEvent::PluginAction { panel, action } => {
                self.plugin_panel_action(panel, &action);
            }
            PanelEvent::OpenExternal(path) => {
                self.event_open_external(path)?;
            }

            PanelEvent::OpenRemoteFile(url) => {
                self.event_open_remote_file(url)?;
            }

            PanelEvent::ClosePanel => {
                // Request close of current panel (with confirmation if needed)
                self.handle_close_panel_request()?;
            }

            // === Status messages ===
            PanelEvent::ShowMessage(message) => {
                self.state.set_info(message);
            }

            PanelEvent::ShowInfo { title, rows } => {
                let modal = termide_modal::InfoModal::new(title, rows);
                self.state
                    .show_message_modal(crate::state::ActiveModal::Info(Box::new(modal)));
                self.state.needs_redraw = true;
            }

            PanelEvent::ShowError(message) => {
                self.show_error_modal(message);
            }

            PanelEvent::RequestAttention => {
                self.state.attention_bell();
            }

            PanelEvent::SetStatusMessage { message, is_error } => {
                if is_error {
                    self.show_error_modal(message);
                } else {
                    self.state.set_info(message);
                }
            }

            PanelEvent::ClearStatus => {
                self.state.clear_status();
            }

            // === Panel navigation ===
            PanelEvent::NextPanel => {
                self.layout_manager.next_group();
                self.notify_outline_file_opened();
            }

            PanelEvent::PrevPanel => {
                self.layout_manager.prev_group();
                self.notify_outline_file_opened();
            }

            PanelEvent::VimPanelNavigation { direction } => {
                use termide_core::VimPanelDirection;
                match direction {
                    VimPanelDirection::Left => self.layout_manager.prev_group(),
                    VimPanelDirection::Right => self.layout_manager.next_group(),
                    VimPanelDirection::Up => {
                        self.layout_manager.prev_panel_in_group();
                    }
                    VimPanelDirection::Down => {
                        self.layout_manager.next_panel_in_group();
                    }
                }
                self.notify_outline_file_opened();
            }

            // === Open panels ===
            PanelEvent::OpenDiagnosticsPanel => {
                self.handle_open_diagnostics()?;
            }

            // === Clipboard ===
            PanelEvent::CopyToClipboard(text) => {
                if let Err(e) = termide_clipboard::copy(&text) {
                    log::error!("Failed to copy to clipboard: {}", e);
                }
            }

            // === Events not yet implemented ===
            PanelEvent::NeedsRedraw => {
                self.state.needs_redraw = true;
            }

            PanelEvent::Quit => {
                self.handle_quit_request()?;
            }

            PanelEvent::WorkingDirectoryChanged => {
                // Re-register watchers and re-sync the git panels' repository
                // paths, the same as after an explicit navigation.
                self.state.needs_watcher_registration = true;
            }

            PanelEvent::SaveFile(path) => {
                self.event_save_file(path)?;
            }

            PanelEvent::CloseFile => {
                // Same as ClosePanel for now
                self.handle_close_panel_request()?;
            }

            PanelEvent::NavigateTo(path) => {
                self.event_navigate_to(path)?;
            }

            PanelEvent::OpenPath { path, select_file } => {
                self.event_open_path(path, select_file)?;
            }

            PanelEvent::GotoLine(line) => {
                self.event_goto_line(line);
            }

            PanelEvent::ShowConfirm {
                message,
                on_confirm,
            } => {
                self.event_show_confirm(message, on_confirm);
            }

            PanelEvent::ShowInput {
                prompt,
                initial_value,
                on_submit,
            } => {
                self.event_show_input(prompt, initial_value, on_submit);
            }

            PanelEvent::ShowSelect {
                title,
                options,
                on_select,
            } => {
                self.event_show_select(title, options, on_select);
            }
            PanelEvent::ShowChecklist {
                title,
                prompt,
                items,
                groups,
                action,
            } => {
                let modal = termide_modal::ChecklistModal::new(title, prompt, items, groups);
                self.state.set_pending_action(
                    termide_state::PendingAction::PanelChecklist { action },
                    crate::state::ActiveModal::Checklist(Box::new(modal)),
                );
            }

            PanelEvent::RefreshChecklist(refresh) => {
                self.event_refresh_checklist(refresh);
            }

            PanelEvent::ShowConflict {
                source,
                destination,
                remaining,
            } => {
                self.event_show_conflict(source, destination, remaining);
            }

            PanelEvent::WatchPath(path) => {
                self.event_watch_path(path);
            }

            PanelEvent::UnwatchPath(path) => {
                self.event_unwatch_path(path);
            }

            PanelEvent::FileChangedOnDisk(path) => {
                self.fan_out_fs_changes(&std::collections::HashSet::from([path]));
            }

            PanelEvent::RefreshGitStatus(path) => {
                self.event_refresh_git_status(path);
            }

            PanelEvent::RequestPaste => {
                self.event_paste_to_active_panel()?;
            }

            PanelEvent::FocusPanel(name) => {
                self.event_focus_panel(&name);
            }

            PanelEvent::SplitPanel { direction, .. } => {
                self.event_split_panel(direction);
            }

            PanelEvent::GitOperation {
                operation,
                repo_path,
            } => {
                self.event_git_operation(operation, repo_path, None)?;
            }

            PanelEvent::CredentialsRequired { url, attempt } => {
                self.event_credentials_required(url, attempt);
            }

            PanelEvent::CredentialsAccepted { url } => {
                self.event_credentials_accepted(&url);
            }

            PanelEvent::CancelGitOperation => {
                self.event_cancel_git_operation();
            }

            PanelEvent::OpenGitDiff {
                repo_path,
                commit_hash,
                file_path,
            } => {
                self.event_open_git_diff(repo_path, commit_hash, file_path)?;
            }

            PanelEvent::OpenGitLog { repo_path, branch } => {
                self.handle_open_git_log()?;
                // The log panel is focused now, found or just created.
                if let Some(panel) = self
                    .layout_manager
                    .active_panel_mut()
                    .filter(|p| p.name() == "git_log")
                {
                    panel.handle_command(PanelCommand::ShowGitLog { repo_path, branch });
                }
            }

            PanelEvent::OpenStashDropdown {
                repo_path,
                button_area,
                has_changes,
            } => {
                // Toggle: if already open, close
                if self.state.ui.stash_submenu.open {
                    self.state.ui.stash_submenu.close();
                    self.state.needs_redraw = true;
                } else {
                    // Load stash entries and open dropdown
                    let entries = termide_git::stash_list(&repo_path);
                    self.state.stash.entries = entries;
                    self.state.ui.stash_button_area = Some(button_area);
                    self.state.stash.repo_path = Some(repo_path);
                    self.state.stash.has_changes = has_changes;
                    self.state.ui.stash_submenu.open();
                    self.state.needs_redraw = true;
                }
            }

            // === Operations panel ===
            PanelEvent::ToggleOperationPause(op_id) => {
                self.event_toggle_operation_pause(op_id);
            }

            PanelEvent::CancelOperation(op_id) => {
                self.event_cancel_operation(op_id);
            }

            PanelEvent::OpenOperationActionMenu {
                op_id,
                anchor_x,
                anchor_y,
            } => {
                let (is_paused, is_command) = self
                    .state
                    .active_operations
                    .get(&op_id)
                    .map(|op| (op.is_paused, op.op_type.is_command()))
                    .unwrap_or((false, false));
                self.state.ui.close_all_submenus();
                self.state
                    .ui
                    .operation_action_menu
                    .open(op_id.0, anchor_x, anchor_y, is_paused, is_command);
                self.state.needs_redraw = true;
            }

            PanelEvent::OpenOperationsPanel => {
                self.open_operations_panel()?;
            }

            PanelEvent::OpenOutlinePanel => {
                self.handle_open_outline()?;
            }

            PanelEvent::OpenReferencesPanel {
                locations,
                symbol_name,
            } => {
                self.handle_open_references_panel(locations, symbol_name)?;
            }

            PanelEvent::OpenDirectorySwitcher => {
                self.handle_open_directory_switcher()?;
            }

            PanelEvent::ForkAgentSession { session, cwd } => {
                self.fork_agent_session(&session, &cwd)?;
            }

            PanelEvent::ChangeAgentCwd { session, cwd } => {
                self.ask_agent_cwd(session, cwd);
            }

            PanelEvent::OpenAgentSession(path) => {
                self.ai_open_session(path)?;
            }

            PanelEvent::RestartDelegates { panel, items } => {
                // The panel's own runner carries its delegates on, so they run
                // on its connections and rules and ask through its prompter.
                let catalog = self
                    .layout_manager
                    .iter_all_panels_mut()
                    .find_map(|candidate| {
                        candidate
                            .as_any_mut()
                            .downcast_mut::<termide_panel_agent::AgentPanel>()
                            .filter(|agent| agent.panel_id() == panel)
                            .map(|agent| (agent.catalog(), agent.agent().to_string()))
                    });
                let Some((catalog, caller)) = catalog else {
                    log::warn!("panel {panel} is gone: its delegates stay unfinished");
                    return Ok(());
                };
                let mut failed: Vec<String> = Vec::new();
                for item in &items {
                    if !catalog.restart_delegate(panel, &caller, item) {
                        failed.push(item.agent.clone());
                    }
                }
                if !failed.is_empty() {
                    let t = termide_i18n::t();
                    let text = t.agent_carry_failed_fmt(&failed.join(", "));
                    log::warn!("{text}");
                    if let Some(target) =
                        self.layout_manager
                            .iter_all_panels_mut()
                            .find_map(|candidate| {
                                candidate
                                    .as_any_mut()
                                    .downcast_mut::<termide_panel_agent::AgentPanel>()
                                    .filter(|agent| agent.panel_id() == panel)
                            })
                    {
                        target.notice(text, termide_panel_agent::NoticeKind::Warn);
                    }
                }
            }
            PanelEvent::DelegateAnswerToParent {
                parent_session,
                from,
                agent,
                text,
            } => {
                self.delegate_answer_to_parent(parent_session, from, agent, text);
            }

            PanelEvent::FocusDelegate { panel, call } => {
                let found = self.layout_manager.focus_panel_where(|candidate| {
                    candidate
                        .as_any()
                        .downcast_ref::<termide_panel_agent::AgentPanel>()
                        .is_some_and(|agent| agent.runs_task(panel, &call))
                });
                // A task running out of sight is followed in a panel opened
                // for it.
                if found || self.watch_delegate(panel, &call) {
                    self.state.needs_redraw = true;
                }
            }
            // A view shown over the layout moves into a panel of its own.
            PanelEvent::DockOverlay => self.dock_overlay(),

            PanelEvent::FocusAgentPanel(id) => {
                let found = self.layout_manager.focus_panel_where(|candidate| {
                    candidate
                        .as_any()
                        .downcast_ref::<termide_panel_agent::AgentPanel>()
                        .is_some_and(|agent| agent.panel_id() == id)
                });
                if found {
                    self.state.needs_redraw = true;
                }
            }
        }
        Ok(())
    }
}
