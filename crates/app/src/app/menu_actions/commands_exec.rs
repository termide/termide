//! Command execution — running commands as terminals, background jobs, or reports.

use anyhow::Result;
use std::path::PathBuf;
use termide_config::commands::{decode_command_menu_key, CommandMenuKeyKind};

use super::super::App;

impl App {
    pub(in crate::app) fn run_command_by_menu_key(&mut self, key: &str) -> Result<()> {
        let registry = match self.commands_registry() {
            Some(r) => r,
            None => return Ok(()),
        };

        let Some(decoded) = decode_command_menu_key(key) else {
            return Ok(());
        };
        if decoded.kind != CommandMenuKeyKind::Command {
            return Ok(());
        }

        let command = match registry.find_command_anywhere_scoped(&decoded.name, decoded.is_project)
        {
            Some(command) => command.clone(),
            None => return Ok(()),
        };

        if let Some(ref meta) = command.metadata {
            if !meta.params.is_empty() {
                let modal = termide_modal::CommandParamsModal::new(
                    command.name.clone(),
                    meta.params.clone(),
                );
                self.state.set_pending_action(
                    termide_state::PendingAction::RunCommandWithParams { command },
                    crate::state::ActiveModal::CommandParams(Box::new(modal)),
                );
                return Ok(());
            }
        }

        self.run_command(&command)
    }

    /// Run a command with user-provided parameters (from CommandParamsModal).
    pub(in crate::app) fn run_command_with_params(
        &mut self,
        command: &termide_config::commands::CommandItem,
        params: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        use termide_config::commands::CommandMode;
        use termide_panel_terminal::Terminal;

        let cwd = self.get_focused_panel_cwd();
        let mut cmd = build_command_command(command, &cwd);

        // Pass parameters as TERMIDE_PARAM_<NAME> env vars
        for (name, value) in params {
            let env_key = format!("TERMIDE_PARAM_{}", name.to_uppercase().replace('-', "_"));
            cmd.env(&env_key, value);
        }

        if command.mode == CommandMode::Report {
            self.run_report_command_with_cmd(command, cmd)?;
        } else if command.mode == CommandMode::Background {
            log::info!(
                "Running background command '{}' with {} params",
                command.name,
                params.len()
            );
            match cmd
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .stdin(std::process::Stdio::null())
                .spawn()
            {
                Ok(mut child) => {
                    let pid = child.id();
                    let op_id = self.state.next_synthetic_operation_id();
                    self.state.track_operation(
                        op_id,
                        termide_state::OperationType::CommandBackground,
                        command.name.clone(),
                        String::new(),
                        0,
                        0,
                    );
                    let (tx, rx) = std::sync::mpsc::channel::<()>();
                    std::thread::spawn(move || {
                        let _ = child.wait();
                        let _ = tx.send(());
                    });
                    self.state.bg_command_handles.push((op_id, rx, pid));
                    let _ = self.open_operations_panel();
                }
                Err(e) => {
                    log::error!("Failed to run background command '{}': {}", command.name, e);
                    self.show_error_modal(format!("Failed to run command: {}", e));
                }
            }
        } else {
            log::info!(
                "Running command '{}' with {} params",
                command.name,
                params.len()
            );
            self.close_help_panels();
            let width = self.state.terminal.width;
            let height = self.state.terminal.height;
            let term_height = height.saturating_sub(3);
            let term_width = width.saturating_sub(2);
            // Can't pass env to Terminal::new_with_cwd, so just run without params for terminal mode
            let command_str = command_terminal_command(command);
            match Terminal::new_with_cwd(term_height, term_width, Some(cwd)) {
                Ok(mut terminal) => {
                    let _ = terminal.send_command(&command_str);
                    self.add_panel(Box::new(terminal));
                    self.auto_save_session();
                }
                Err(e) => {
                    log::error!(
                        "Failed to create terminal for command '{}': {}",
                        command.name,
                        e
                    );
                    self.show_error_modal(format!("Failed to run command: {}", e));
                }
            }
        }

        Ok(())
    }

    /// Run a command
    pub(in crate::app::menu_actions) fn run_command(
        &mut self,
        command: &termide_config::commands::CommandItem,
    ) -> Result<()> {
        use termide_config::commands::CommandMode;
        use termide_panel_terminal::Terminal;

        let cwd = self.get_focused_panel_cwd();

        if command.mode == CommandMode::Report {
            // Run in background with output capture, show result in modal
            self.run_report_command(command, &cwd)?;
        } else if command.mode == CommandMode::Background {
            // Background spawn — tracked in Operations panel
            log::info!("Running background command '{}' in {:?}", command.name, cwd);
            match build_command_command(command, &cwd)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .stdin(std::process::Stdio::null())
                .spawn()
            {
                Ok(mut child) => {
                    let pid = child.id();
                    let op_id = self.state.next_synthetic_operation_id();
                    self.state.track_operation(
                        op_id,
                        termide_state::OperationType::CommandBackground,
                        command.name.clone(),
                        String::new(),
                        0,
                        0,
                    );
                    // Track completion in background thread
                    let (tx, rx) = std::sync::mpsc::channel::<()>();
                    std::thread::spawn(move || {
                        let _ = child.wait();
                        let _ = tx.send(());
                    });
                    // Store handle to poll for completion
                    self.state.bg_command_handles.push((op_id, rx, pid));
                    // Open operations panel to show progress
                    let _ = self.open_operations_panel();
                }
                Err(e) => {
                    log::error!("Failed to run background command '{}': {}", command.name, e);
                    self.show_error_modal(format!("Failed to run command: {}", e));
                }
            }
        } else {
            // Run in new terminal panel
            log::info!("Running command '{}' in {:?}", command.name, cwd);

            self.close_help_panels();

            let width = self.state.terminal.width;
            let height = self.state.terminal.height;
            let term_height = height.saturating_sub(3);
            let term_width = width.saturating_sub(2);

            let command_str = command_terminal_command(command);

            match Terminal::new_with_cwd(term_height, term_width, Some(cwd)) {
                Ok(mut terminal) => {
                    let _ = terminal.send_command(&command_str);
                    self.add_panel(Box::new(terminal));
                    self.auto_save_session();
                }
                Err(e) => {
                    log::error!(
                        "Failed to create terminal for command '{}': {}",
                        command.name,
                        e
                    );
                    self.show_error_modal(format!("Failed to run command: {}", e));
                }
            }
        }

        Ok(())
    }

    /// Run a report command in background, capturing output for modal display
    fn run_report_command(
        &mut self,
        command: &termide_config::commands::CommandItem,
        cwd: &std::path::Path,
    ) -> Result<()> {
        log::info!("Running report command '{}' in {:?}", command.name, cwd);

        let cmd = build_command_command(command, cwd);
        self.run_report_command_with_cmd(command, cmd)
    }

    /// Run a report command with a pre-built Command (e.g. with env vars from params).
    fn run_report_command_with_cmd(
        &mut self,
        command: &termide_config::commands::CommandItem,
        mut cmd: std::process::Command,
    ) -> Result<()> {
        use crate::state::{CommandOperationHandle, CommandOperationResult};

        let child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();

        match child {
            Ok(child) => {
                let pid = child.id();
                let command_name = command.name.clone();
                let (tx, rx) = std::sync::mpsc::channel();

                std::thread::spawn(move || {
                    let output = child.wait_with_output();
                    let result = match output {
                        Ok(out) => CommandOperationResult {
                            command_name: command_name.clone(),
                            success: out.status.success(),
                            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
                            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
                        },
                        Err(e) => CommandOperationResult {
                            command_name: command_name.clone(),
                            success: false,
                            stdout: String::new(),
                            stderr: e.to_string(),
                        },
                    };
                    let _ = tx.send(result);
                });

                let op_id = self.state.next_synthetic_operation_id();
                self.state.track_operation(
                    op_id,
                    termide_state::OperationType::CommandReport,
                    command.name.clone(),
                    String::new(),
                    0,
                    0,
                );

                self.state
                    .command_operation_handles
                    .push(CommandOperationHandle {
                        receiver: rx,
                        command_name: command.name.clone(),
                        operation_id: Some(op_id),
                        pid: Some(pid),
                    });

                self.open_operations_panel()?;
            }
            Err(e) => {
                log::error!("Failed to run report command '{}': {}", command.name, e);
                self.show_error_modal(format!("Failed to run command: {}", e));
            }
        }

        Ok(())
    }

    /// Get the working directory from the focused panel
    fn get_focused_panel_cwd(&self) -> PathBuf {
        // Use the Panel::get_working_directory() method
        if let Some(panel) = self.layout_manager.active_panel() {
            if let Some(cwd) = panel.get_working_directory() {
                return cwd;
            }
        }

        // Fallback to project root
        self.project_root.clone()
    }
}

// =========================================================================
// Command execution utilities (private module-level functions)
// =========================================================================

/// Get the command string to send to a terminal panel.
fn command_terminal_command(command: &termide_config::commands::CommandItem) -> String {
    command.command.clone().unwrap_or_default()
}

/// Build a Command for executing a command via `sh -c`.
fn build_command_command(
    command: &termide_config::commands::CommandItem,
    cwd: &std::path::Path,
) -> std::process::Command {
    let command_str = match &command.command {
        Some(cmd) => cmd.clone(),
        None => String::new(),
    };

    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c").arg(&command_str);
    cmd.current_dir(cwd);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    if let Some(env) = get_direnv_json(cwd) {
        for (key, value) in &env {
            match value {
                Some(v) => {
                    cmd.env(key, v);
                }
                None => {
                    cmd.env_remove(key);
                }
            }
        }
    }

    cmd
}

/// Get project environment via `direnv export json`.
///
/// Returns a map of KEY → Some(value) for set vars, KEY → None for unset vars.
/// Uses caching with 60s TTL to avoid repeated subprocess calls.
#[cfg(unix)]
fn get_direnv_json(
    cwd: &std::path::Path,
) -> Option<std::collections::HashMap<String, Option<String>>> {
    use std::sync::Mutex;

    // Check if direnv is available
    static DIRENV_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let available = *DIRENV_AVAILABLE.get_or_init(|| {
        std::process::Command::new("direnv")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    });
    if !available {
        return None;
    }

    // Cache with TTL
    type Cache = std::collections::HashMap<
        std::path::PathBuf,
        (
            std::collections::HashMap<String, Option<String>>,
            std::time::Instant,
        ),
    >;
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    const TTL: std::time::Duration = std::time::Duration::from_secs(60);

    let mut cache = CACHE.lock().unwrap();
    let cache = cache.get_or_insert_with(std::collections::HashMap::new);

    if let Some((env, ts)) = cache.get(cwd) {
        if ts.elapsed() < TTL {
            return Some(env.clone());
        }
    }

    // Run direnv export json
    let output = std::process::Command::new("direnv")
        .args(["export", "json"])
        .current_dir(cwd)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return None;
    }

    // Parse JSON: { "KEY": "value", "KEY2": null }
    // Minimal JSON parser — no serde dependency needed
    let mut env = std::collections::HashMap::new();
    // Simple line-by-line parse of direnv JSON output
    for line in stdout.lines() {
        let line = line.trim().trim_end_matches(',');
        if line.starts_with('{') || line.starts_with('}') {
            continue;
        }
        // "KEY": "VALUE" or "KEY": null
        if let Some((key_part, val_part)) = line.split_once(':') {
            let key = key_part.trim().trim_matches('"').to_string();
            let val = val_part.trim();
            if val == "null" {
                env.insert(key, None);
            } else {
                // Remove surrounding quotes, handle escaped chars
                let v = val.trim_matches('"');
                // Unescape JSON string basics
                let v = v
                    .replace("\\\"", "\"")
                    .replace("\\\\", "\\")
                    .replace("\\n", "\n")
                    .replace("\\t", "\t");
                env.insert(key, Some(v));
            }
        }
    }

    cache.insert(cwd.to_path_buf(), (env.clone(), std::time::Instant::now()));
    Some(env)
}

#[cfg(not(unix))]
fn get_direnv_json(
    _cwd: &std::path::Path,
) -> Option<std::collections::HashMap<String, Option<String>>> {
    None
}
