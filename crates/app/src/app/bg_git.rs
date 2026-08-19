//! Background git network operations (push/pull/fetch): result polling with a
//! timeout watchdog, spinner animation, and SSH-passphrase retry prompting.

use std::sync::mpsc::TryRecvError;

use termide_core::PanelCommand;
use termide_modal::InfoModal;

use crate::state::ActiveModal;

use super::App;

impl App {
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
                        let repo = handle.repo_path;
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
                    let repo = handle.repo_path;
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
                    lines.push((String::new(), t.git_completed()));
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
}
