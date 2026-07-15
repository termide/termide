//! Git operation event handlers (push/pull/fetch and diff panels).

#![allow(deprecated)]

use anyhow::Result;
use std::path::PathBuf;

use crate::app::App;
use termide_core::{GitOperationType, PanelCommand};
use termide_i18n as i18n;

impl App {
    /// Handle GitOperation event - run git push/pull in background thread
    pub(in crate::app) fn event_git_operation(
        &mut self,
        operation: GitOperationType,
        repo_path: PathBuf,
        passphrase: Option<String>,
    ) -> Result<()> {
        use crate::state::{GitOperationHandle, GitOperationResult};
        use std::sync::mpsc;
        use std::thread;

        // Write an SSH key passphrase to a private, short-lived file the askpass
        // helper reads. Owner-only perms; removed once git finishes.
        fn write_askpass_secret(secret: &str) -> std::io::Result<PathBuf> {
            use std::io::Write;
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);

            let dir = std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = dir.join(format!("termide-askpass-{}-{}", std::process::id(), n));

            #[cfg(unix)]
            let mut file = {
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&path)?
            };
            #[cfg(not(unix))]
            let mut file = std::fs::File::create(&path)?;

            file.write_all(secret.as_bytes())?;
            Ok(path)
        }

        // Prevent multiple concurrent operations
        if self.state.ui.git_operation_in_progress {
            return Ok(());
        }

        // Reuse a passphrase entered earlier this session so repeated ops don't
        // re-prompt; an explicit retry value takes precedence.
        let passphrase = passphrase.or_else(|| self.state.git_ssh_passphrase.clone());

        let cmd = match operation {
            GitOperationType::Push => "push",
            GitOperationType::Pull => "pull",
            GitOperationType::Fetch => "fetch",
        };
        let cmd_str = cmd.to_string();

        // With a passphrase, write it to a private (0600) temp file and run ssh
        // through our askpass helper so the passphrase is supplied without a
        // terminal prompt. Without one, run non-interactively (BatchMode) so a
        // missing passphrase fails cleanly instead of corrupting the TUI.
        let secret_file = match &passphrase {
            Some(pp) => match write_askpass_secret(pp) {
                Ok(path) => Some(path),
                Err(e) => {
                    self.show_error_modal(format!("Failed to prepare SSH askpass: {e}"));
                    return Ok(());
                }
            },
            None => None,
        };
        let helper = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("termide"));
        let auth = match &secret_file {
            Some(sf) => termide_git::SshAuth::Askpass {
                helper: &helper,
                secret_file: sf,
            },
            None => termide_git::SshAuth::Batch,
        };

        // stdout/stderr are piped to capture output and keep it off the TUI.
        let child = match termide_git::network_command(&repo_path, &[&cmd_str], auth).spawn() {
            Ok(child) => child,
            Err(e) => {
                if let Some(sf) = &secret_file {
                    let _ = std::fs::remove_file(sf);
                }
                self.show_error_modal(format!("Failed to spawn git: {}", e));
                return Ok(());
            }
        };

        // Get PID before moving child to thread
        let pid = child.id();

        // Set operation state
        self.state.ui.git_operation_in_progress = true;
        self.notify_git_operation_state(true, Some(cmd_str.clone()), 0);

        // Show status message
        let t = i18n::t();
        let msg = match operation {
            GitOperationType::Push => t.git_push_in_progress(),
            GitOperationType::Pull => t.git_pull_in_progress(),
            GitOperationType::Fetch => t.git_fetch_in_progress(),
        };
        self.state.set_info(msg);

        // Spawn background thread to wait for result
        let (tx, rx) = mpsc::channel();
        let cmd_for_thread = cmd_str.clone();
        let secret_for_thread = secret_file.clone();

        thread::spawn(move || {
            let output = child.wait_with_output();

            // Drop the transient passphrase file as soon as git is done.
            if let Some(sf) = secret_for_thread {
                let _ = std::fs::remove_file(sf);
            }

            let result = match output {
                Ok(out) => GitOperationResult {
                    operation: cmd_for_thread,
                    success: out.status.success(),
                    stdout: String::from_utf8_lossy(&out.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&out.stderr).to_string(),
                },
                Err(e) => GitOperationResult {
                    operation: cmd_for_thread,
                    success: false,
                    stdout: String::new(),
                    stderr: e.to_string(),
                },
            };
            let _ = tx.send(result);
        });

        // Store handle for polling and cancellation
        self.state.git_operation_handle = Some(GitOperationHandle {
            receiver: rx,
            pid,
            operation: cmd_str,
            repo_path,
            started_at: std::time::Instant::now(),
        });

        Ok(())
    }

    /// Handle CancelGitOperation event - kill running git process
    pub(in crate::app) fn event_cancel_git_operation(&mut self) {
        if let Some(handle) = self.state.git_operation_handle.take() {
            // Kill process by PID
            #[cfg(unix)]
            {
                let _ = std::process::Command::new("kill")
                    .arg("-TERM")
                    .arg(handle.pid.to_string())
                    .status();
            }
            #[cfg(windows)]
            {
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &handle.pid.to_string(), "/F"])
                    .status();
            }
        }

        self.state.ui.git_operation_in_progress = false;
        self.notify_git_operation_state(false, None, 0);

        // Show cancellation message
        let t = i18n::t();
        self.state.set_info(t.git_operation_cancelled().to_string());
    }

    /// Notify all panels about git operation in progress state
    pub(in crate::app) fn notify_git_operation_state(
        &mut self,
        in_progress: bool,
        operation: Option<String>,
        spinner_frame: usize,
    ) {
        for panel in self.layout_manager.iter_all_panels_mut() {
            panel.handle_command(PanelCommand::SetGitOperationInProgress {
                in_progress,
                operation: operation.clone(),
                spinner_frame,
            });
        }
    }

    /// Handle OpenGitDiff event - open git diff panel for repository.
    /// Reuses existing panel if one with matching arguments is already open.
    pub(super) fn event_open_git_diff(
        &mut self,
        repo_path: PathBuf,
        commit_hash: Option<String>,
        file_path: Option<PathBuf>,
    ) -> Result<()> {
        use termide_panel_git_diff::GitDiffPanel;

        self.close_help_panels();

        // Check if a matching GitDiffPanel is already open
        let file_filter_str = file_path.as_ref().map(|p| p.to_string_lossy().to_string());
        for (group_idx, group) in self.layout_manager.panel_groups.iter_mut().enumerate() {
            for (panel_idx, panel) in group.panels().iter().enumerate() {
                if let Some(diff) = panel.as_any().downcast_ref::<GitDiffPanel>() {
                    if diff.repo_path() == repo_path
                        && diff.commit_hash() == commit_hash.as_deref()
                        && diff.file_filter() == file_filter_str.as_deref()
                    {
                        self.layout_manager.focus = group_idx;
                        group.set_expanded(panel_idx);
                        return Ok(());
                    }
                }
            }
        }

        let panel = match (&commit_hash, &file_path) {
            (_, Some(file)) => GitDiffPanel::new_with_file_filter(repo_path, file.clone()),
            (Some(hash), None) => GitDiffPanel::new_for_commit(repo_path, hash.clone()),
            (None, None) => GitDiffPanel::new(repo_path),
        };
        self.add_panel(Box::new(panel));
        self.auto_save_session();

        Ok(())
    }
}
