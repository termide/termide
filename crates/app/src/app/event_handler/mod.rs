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
#![allow(deprecated)]

use anyhow::Result;

use super::App;
use termide_core::PanelEvent;

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

            PanelEvent::OpenUrl(url) => {
                self.start_url_fetch(url);
            }

            PanelEvent::ViewHtml(path) => {
                self.event_view_html(path)?;
            }

            PanelEvent::SwapActiveToHtml(path) => {
                self.event_swap_active_to_html(path)?;
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

            PanelEvent::ShowError(message) => {
                self.show_error_modal(message);
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

            PanelEvent::OpenGitLog { repo_path: _ } => {
                self.handle_open_git_log()?;
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
        }
        Ok(())
    }
}
