//! Command execution — running commands as terminals, background jobs, or reports.

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use termide_config::commands::{decode_command_menu_key, CommandItem, CommandMenuKeyKind};

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

        match registry.find_command_anywhere_scoped(&decoded.name, decoded.is_project) {
            Some(command) => self.start_command(command.clone()),
            None => Ok(()),
        }
    }

    /// The approvals of the current project's launches, kept in termide's
    /// data directory.
    fn project_approvals(&self) -> termide_agent_core::approvals::Approvals {
        termide_agent_core::approvals::Approvals::new(
            termide_project::approvals_path(&self.project_root).ok(),
        )
    }

    /// Approve the project's command `key` as the registry has it now: one
    /// the user has just written in a form.
    pub(in crate::app) fn approve_project_command(&mut self, key: &str) {
        self.state.cache.commands_registry = None;
        let Some(command) = self
            .commands_registry()
            .and_then(|registry| registry.find_command_anywhere_scoped(key, true).cloned())
        else {
            return;
        };
        if let Err(e) = self
            .project_approvals()
            .approve_command(key, &command_spec(&command))
        {
            log::warn!("Cannot keep the approval of {key}: {e}");
        }
        self.state.cache.hotkey_table = None;
    }

    /// Approve the project's `command` as it is, the user having seen it.
    /// Its hotkey, if any, takes effect.
    pub(in crate::app) fn approve_shown_command(&mut self, command: &CommandItem) {
        if let Err(e) = self
            .project_approvals()
            .approve_command(&command.name, &command_spec(command))
        {
            log::warn!("Cannot keep the approval of {}: {e}", command.name);
        }
        self.state.cache.hotkey_table = None;
    }

    /// Whether the project's `command` is approved as it is now.
    pub(in crate::app) fn project_command_approved(&self, command: &CommandItem) -> bool {
        self.project_approvals()
            .command_approved(&command.name, &command_spec(command))
    }

    /// Run `command` the way every entry point does — the menu, a hotkey,
    /// the palette, `F5`: a command the project's `commands.toml` brings is
    /// shown first, with what it runs, until approved as it is; a command
    /// with parameters asks for them next.
    pub(in crate::app) fn start_command(&mut self, command: CommandItem) -> Result<()> {
        if command.is_project && !self.project_command_approved(&command) {
            let t = termide_i18n::t();
            let modal = termide_modal::ConfirmModal::new(
                t.command_confirm_project_title(),
                t.command_confirm_project_fmt(&command.name, &command_text(&command)),
            );
            self.state.set_pending_action(
                termide_state::PendingAction::ConfirmProjectCommand { command },
                crate::state::ActiveModal::Confirm(Box::new(modal)),
            );
            return Ok(());
        }
        if let Some(meta) = command.metadata.as_ref().filter(|m| !m.params.is_empty()) {
            let modal =
                termide_modal::CommandParamsModal::new(command.name.clone(), meta.params.clone());
            self.state.set_pending_action(
                termide_state::PendingAction::RunCommandWithParams { command },
                crate::state::ActiveModal::CommandParams(Box::new(modal)),
            );
            return Ok(());
        }
        self.run_command_with_params(&command, &HashMap::new())
    }

    /// Run a command, its parameters (from CommandParamsModal) passed as
    /// `TERMIDE_PARAM_<NAME>` environment variables in every mode.
    pub(in crate::app) fn run_command_with_params(
        &mut self,
        command: &CommandItem,
        params: &HashMap<String, String>,
    ) -> Result<()> {
        use termide_config::commands::CommandMode;
        use termide_panel_terminal::Terminal;

        let env = param_env(params);
        if command.debugger().is_some() {
            return self.run_debug_command(command, &env);
        }
        let cwd = self.get_focused_panel_cwd();
        log::info!(
            "Running {} command '{}' in {:?} with {} params",
            command.mode.as_str(),
            command.name,
            cwd,
            env.len()
        );

        match command.mode {
            CommandMode::Report => {
                let mut cmd = build_command_command(command, &cwd);
                cmd.envs(env);
                self.run_report_command_with_cmd(command, cmd)?;
            }
            CommandMode::Background => {
                let mut cmd = build_command_command(command, &cwd);
                cmd.envs(env);
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
                            command_label(command),
                            String::new(),
                            0,
                            0,
                        );
                        // Track completion in a background thread, polled
                        // from the main loop.
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
                        self.show_error_modal(
                            termide_i18n::t().command_run_failed_fmt(&e.to_string()),
                        );
                    }
                }
            }
            CommandMode::Terminal => {
                self.close_help_panels();
                let term_height = self.state.terminal.height.saturating_sub(3);
                let term_width = self.state.terminal.width.saturating_sub(2);
                match Terminal::new_with_cwd_env(term_height, term_width, Some(cwd), &env) {
                    Ok(mut terminal) => {
                        let _ = terminal.send_command(&command_terminal_command(command));
                        self.add_panel(Box::new(terminal));
                        self.auto_save_layout();
                    }
                    Err(e) => {
                        log::error!(
                            "Failed to create terminal for command '{}': {}",
                            command.name,
                            e
                        );
                        self.show_error_modal(
                            termide_i18n::t().command_run_failed_fmt(&e.to_string()),
                        );
                    }
                }
            }
        }

        Ok(())
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
                let command_name = command_label(command);
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
                    command_label(command),
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
                        project: self.project_root.clone(),
                    });

                self.open_operations_panel()?;
            }
            Err(e) => {
                log::error!("Failed to run report command '{}': {}", command.name, e);
                self.show_error_modal(termide_i18n::t().command_run_failed_fmt(&e.to_string()));
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

/// The name a command shows in the menu: its `name`, or its identifier.
fn command_label(command: &CommandItem) -> String {
    command
        .metadata
        .as_ref()
        .and_then(|m| m.display_name.clone())
        .unwrap_or_else(|| command.name.clone())
}

/// The environment variables a command's parameters become:
/// `TERMIDE_PARAM_<NAME>`, upper-cased, with `-` as `_`.
fn param_env(params: &HashMap<String, String>) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = params
        .iter()
        .map(|(name, value)| {
            (
                format!("TERMIDE_PARAM_{}", name.to_uppercase().replace('-', "_")),
                value.clone(),
            )
        })
        .collect();
    env.sort();
    env
}

/// Get the command string to send to a terminal panel.
fn command_terminal_command(command: &CommandItem) -> String {
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
    use std::sync::{Mutex, PoisonError};

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

    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
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

/// Everything a command runs, as one line: what its approval is for.
fn command_spec(command: &CommandItem) -> String {
    let meta = command.metadata.as_ref();
    serde_json::json!({
        "command": command.command,
        "mode": command.mode.as_str(),
        "build": meta.and_then(|m| m.build.as_ref()),
        "debugger": meta.and_then(|m| m.debugger.as_ref()),
        "debugger_args": meta.map(|m| &m.debugger_args),
        // A key bound later is asked about again: it decides what a press runs.
        "key": meta.and_then(|m| m.key.as_ref()),
    })
    .to_string()
}

/// What a command runs, for a person to read: its build first, if any.
fn command_text(command: &CommandItem) -> String {
    let meta = command.metadata.as_ref();
    let mut lines = Vec::new();
    if let Some(build) = meta.and_then(|m| m.build.as_deref()) {
        lines.push(build.to_string());
    }
    let mut run = command.command.clone().unwrap_or_default();
    if let Some(debugger) = meta.and_then(|m| m.debugger.as_deref()) {
        run = format!("{debugger}: {run}");
    }
    lines.push(run);
    lines.join("\n")
}

#[cfg(test)]
mod approval_tests {
    use super::*;
    use termide_config::commands::CommandMetadata;

    fn command(run: &str, build: Option<&str>) -> CommandItem {
        CommandItem {
            name: "go".into(),
            command: Some(run.into()),
            mode: Default::default(),
            is_project: true,
            metadata: Some(CommandMetadata {
                build: build.map(str::to_string),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn an_approval_covers_what_runs_and_nothing_else() {
        let approved = command_spec(&command("./run", Some("cargo build")));
        assert_eq!(
            approved,
            command_spec(&command("./run", Some("cargo build")))
        );
        assert_ne!(
            approved,
            command_spec(&command("./run --evil", Some("cargo build")))
        );
        assert_ne!(
            approved,
            command_spec(&command("./run", Some("curl x | sh")))
        );
        let mut keyed = command("./run", Some("cargo build"));
        keyed.metadata.as_mut().unwrap().key = Some("e".into());
        assert_ne!(
            approved,
            command_spec(&keyed),
            "a key bound later asks again"
        );
        assert_eq!(
            command_text(&command("./run", Some("cargo build"))),
            "cargo build\n./run"
        );
    }
}
