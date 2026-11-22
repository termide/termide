//! Command execution — running commands as terminals, background jobs, or reports.

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use termide_config::commands::{decode_command_menu_key, CommandItem, CommandMenuKeyKind};
use termide_config::{PlaceholderValue, Quoting, CONTEXT_PLACEHOLDERS};

use super::super::App;
use super::direnv;
use crate::state::PendingCommandLaunch;

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
        // A placeholder the focused panel cannot fill is told before the
        // parameters are asked for, not after.
        if let Err(message) = self.placeholder_values(&command, &declared_params(&command)) {
            self.show_error_modal(message);
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
        use termide_config::commands::{CommandCwd, CommandMode};
        use termide_config::expand_placeholders;
        use termide_panel_terminal::Terminal;

        let values = match self.placeholder_values(command, params) {
            Ok(values) => values,
            Err(message) => {
                self.show_error_modal(message);
                return Ok(());
            }
        };
        let env = command_env(command, &values, params);
        if command.debugger().is_some() {
            return self.run_debug_command(&expand_debug_command(command, &values), &env);
        }
        // `sh -c` runs the line of a background or report command.
        let sh_line = || {
            let line = command.command.clone().unwrap_or_default();
            expand_placeholders(&line, &values, Quoting::Posix)
        };
        // A command runs where it says: the focused panel's directory, or the
        // project root — the static point its relative paths are written against;
        // for a project command, where its linked `commands.toml` really lives.
        let cwd = match command.cwd {
            CommandCwd::Project if command.is_project => {
                termide_config::commands::project_commands_root(&self.project_root)
            }
            CommandCwd::Project => self.project_root.clone(),
            CommandCwd::Panel => self.get_focused_panel_cwd(),
        };
        log::info!(
            "Running {} command '{}' in {:?} (cwd: {}) with {} params",
            command.mode.as_str(),
            command.name,
            cwd,
            command.cwd.as_str(),
            env.len()
        );

        match command.mode {
            CommandMode::Report | CommandMode::Background => {
                self.launch_shell_command(PendingCommandLaunch {
                    command: command.clone(),
                    line: sh_line(),
                    cwd,
                    env,
                });
            }
            CommandMode::Terminal => {
                let term_height = self.state.terminal.height.saturating_sub(3);
                let term_width = self.state.terminal.width.saturating_sub(2);
                match Terminal::new_with_cwd_env(term_height, term_width, Some(cwd), &env) {
                    Ok(mut terminal) => {
                        let line = command.command.clone().unwrap_or_default();
                        let line =
                            expand_placeholders(&line, &values, terminal.shell_kind().quoting());
                        let _ = terminal.send_command(&line);
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

    /// Start a background or report command with its directory's direnv
    /// environment, or queue it until direnv answers for the directory.
    fn launch_shell_command(&mut self, launch: PendingCommandLaunch) {
        match direnv::lookup(&launch.cwd) {
            direnv::Lookup::Ready(direnv_env) => self.spawn_shell_command(launch, direnv_env),
            direnv::Lookup::Pending => {
                log::info!(
                    "Command '{}' waits for direnv in {:?}",
                    launch.command.name,
                    launch.cwd
                );
                self.state.pending_command_launches.push(launch);
            }
        }
    }

    /// Start the queued commands whose direnv environment has come.
    pub(in crate::app) fn start_pending_command_launches(&mut self) {
        if self.state.pending_command_launches.is_empty() {
            return;
        }
        let mut waiting = Vec::new();
        for launch in std::mem::take(&mut self.state.pending_command_launches) {
            match direnv::lookup(&launch.cwd) {
                direnv::Lookup::Ready(direnv_env) => {
                    self.spawn_shell_command(launch, direnv_env);
                    self.state.needs_redraw = true;
                }
                direnv::Lookup::Pending => waiting.push(launch),
            }
        }
        self.state.pending_command_launches = waiting;
    }

    fn spawn_shell_command(
        &mut self,
        launch: PendingCommandLaunch,
        direnv_env: Option<direnv::DirenvEnv>,
    ) {
        use termide_config::commands::CommandMode;

        let PendingCommandLaunch {
            command,
            line,
            cwd,
            env,
        } = launch;
        let mut cmd = build_command_command(&line, &cwd, direnv_env.as_ref());
        cmd.envs(env);
        if command.mode == CommandMode::Report {
            if let Err(e) = self.run_report_command_with_cmd(&command, cmd) {
                log::error!("Failed to run report command '{}': {}", command.name, e);
            }
            return;
        }
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
                    command_label(&command),
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
                self.show_error_modal(termide_i18n::t().command_run_failed_fmt(&e.to_string()));
            }
        }
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

    /// A debug command run with parameter values and no panel to draw on:
    /// its placeholders filled from `params` alone, and the values as the
    /// environment, as a confirmed form gives them.
    pub(in crate::app) fn debug_command_with_params(
        command: &CommandItem,
        params: &HashMap<String, String>,
    ) -> (CommandItem, Vec<(String, String)>) {
        let values = params
            .iter()
            .map(|(name, value)| (name.clone(), PlaceholderValue::One(value.clone())))
            .collect();
        let env = command_env(command, &values, params);
        (expand_debug_command(command, &values), env)
    }

    /// The names of the placeholders `command` uses anywhere: its line, its
    /// build and its `env`.
    pub(in crate::app) fn command_placeholder_names(command: &CommandItem) -> Vec<&str> {
        placeholder_texts(command)
            .flat_map(termide_config::placeholder_names)
            .collect()
    }

    /// The values of the placeholders `command` uses: its parameters', then
    /// the focused panel's and the project's; the message to show when the
    /// panel has no value for one it uses.
    fn placeholder_values(
        &self,
        command: &CommandItem,
        params: &HashMap<String, String>,
    ) -> Result<HashMap<String, PlaceholderValue>, String> {
        let mut values: HashMap<String, PlaceholderValue> = params
            .iter()
            .map(|(name, value)| (name.clone(), PlaceholderValue::One(value.clone())))
            .collect();
        let used: Vec<&str> = placeholder_texts(command)
            .flat_map(termide_config::placeholder_names)
            .filter(|name| CONTEXT_PLACEHOLDERS.contains(name) && !values.contains_key(*name))
            .collect();
        if used.is_empty() {
            return Ok(values);
        }
        let context = self
            .layout_manager
            .active_panel()
            .map(|panel| panel.command_context())
            .unwrap_or_default();
        let path = |p: &std::path::Path| p.to_string_lossy().into_owned();
        for name in used {
            let value = match name {
                "file" => context
                    .file
                    .as_deref()
                    .map(|f| PlaceholderValue::One(path(f))),
                "files" => (!context.files.is_empty()).then(|| {
                    PlaceholderValue::Many(context.files.iter().map(|f| path(f)).collect())
                }),
                "line" => context.line.map(|l| PlaceholderValue::One(l.to_string())),
                "col" => context.col.map(|c| PlaceholderValue::One(c.to_string())),
                "dir" => Some(PlaceholderValue::One(path(&self.get_focused_panel_cwd()))),
                "project" => Some(PlaceholderValue::One(path(&self.project_root))),
                _ => None,
            };
            match value {
                Some(value) => {
                    values.insert(name.to_string(), value);
                }
                None => {
                    let placeholder = format!("{{{name}}}");
                    // Only an editor has a cursor in a file.
                    return Err(match name {
                        "line" | "col" => termide_i18n::t().command_needs_editor_fmt(&placeholder),
                        _ => termide_i18n::t().command_needs_context_fmt(&placeholder),
                    });
                }
            }
        }
        Ok(values)
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

/// The texts of `command` that may hold placeholders: its line, its build
/// and the values of its `env`.
fn placeholder_texts(command: &CommandItem) -> impl Iterator<Item = &str> {
    let meta = command.metadata.as_ref();
    command
        .command
        .as_deref()
        .into_iter()
        .chain(meta.and_then(|m| m.build.as_deref()))
        .chain(command.env().map(|(_, value)| value))
}

/// The environment `command` runs with: its `env`, the placeholders in its
/// values filled from `values`, then its parameters as `TERMIDE_PARAM_<NAME>`
/// (upper-cased, with `-` as `_`).
fn command_env(
    command: &CommandItem,
    values: &HashMap<String, PlaceholderValue>,
    params: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut param_env: Vec<(String, String)> = params
        .iter()
        .map(|(name, value)| {
            (
                format!("TERMIDE_PARAM_{}", name.to_uppercase().replace('-', "_")),
                value.clone(),
            )
        })
        .collect();
    param_env.sort();
    command
        .env()
        .map(|(name, value)| {
            (
                name.to_string(),
                termide_config::expand_placeholder_text(value, values),
            )
        })
        .chain(param_env)
        .collect()
}

/// The parameters `command` declares, with no values yet: what its
/// placeholders may take from the form rather than from the panel.
fn declared_params(command: &CommandItem) -> HashMap<String, String> {
    command
        .metadata
        .iter()
        .flat_map(|m| &m.params)
        .map(|p| (p.name.clone(), String::new()))
        .collect()
}

/// A debug command with its placeholders filled. Its line is a program and
/// its arguments, split into words before any value goes in, so a value
/// stays one word (`{files}` as many); the build runs through a shell. A line
/// with no placeholder stays as written.
fn expand_debug_command(
    command: &CommandItem,
    values: &HashMap<String, PlaceholderValue>,
) -> CommandItem {
    let uses = |text: &str| {
        termide_config::placeholder_names(text)
            .iter()
            .any(|name| values.contains_key(*name))
    };
    let mut command = command.clone();
    if let Some(line) = command.command.clone().filter(|line| uses(line)) {
        if let Some(words) = termide_config::expand_placeholder_line(&line, values) {
            command.command = Some(
                words
                    .iter()
                    .map(|w| Quoting::Posix.quote(w))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
    }
    if let Some(meta) = command.metadata.as_mut() {
        if let Some(build) = meta.build.clone().filter(|b| uses(b)) {
            meta.build = Some(termide_config::expand_placeholders(
                &build,
                values,
                Quoting::Posix,
            ));
        }
    }
    command
}

/// Build a Command for executing a command via `sh -c`, with the direnv
/// environment of its directory applied.
fn build_command_command(
    line: &str,
    cwd: &std::path::Path,
    direnv_env: Option<&direnv::DirenvEnv>,
) -> std::process::Command {
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c").arg(line);
    cmd.current_dir(cwd);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: the hook runs in the forked child before exec and calls only
        // setsid, which is async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    if let Some(env) = direnv_env {
        for (key, value) in env {
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

/// Everything a command runs, as one line: what its approval is for.
fn command_spec(command: &CommandItem) -> String {
    let meta = command.metadata.as_ref();
    let mut spec = serde_json::json!({
        "command": command.command,
        "mode": command.mode.as_str(),
        // Where it runs is part of what was approved: the same line in another
        // directory builds, deploys and deletes something else.
        "cwd": command.cwd.as_str(),
        "build": meta.and_then(|m| m.build.as_ref()),
        "debugger": meta.and_then(|m| m.debugger.as_ref()),
        "debugger_args": meta.map(|m| &m.debugger_args),
        // A key bound later is asked about again: it decides what a press runs.
        "key": meta.and_then(|m| m.key.as_ref()),
    });
    // The environment changes what runs (`LD_PRELOAD`, `PATH`); left out when
    // empty, so the approvals given before it existed still hold.
    if let Some(env) = meta.map(|m| &m.env).filter(|env| !env.is_empty()) {
        spec["env"] = serde_json::json!(env);
    }
    spec.to_string()
}

/// What a command runs, for a person to read: its build first, if any.
pub(in crate::app) fn command_text(command: &CommandItem) -> String {
    let meta = command.metadata.as_ref();
    let mut lines = Vec::new();
    if let Some(build) = meta.and_then(|m| m.build.as_deref()) {
        lines.push(build.to_string());
    }
    let mut run = command.command.clone().unwrap_or_default();
    if let Some(debugger) = meta.and_then(|m| m.debugger.as_deref()) {
        run = format!("{debugger}: {run}");
    }
    // As a shell would read it, before what runs with it.
    let env: Vec<String> = command
        .env()
        .map(|(name, value)| format!("{name}={}", Quoting::Posix.quote(value)))
        .collect();
    if !env.is_empty() {
        run = format!("{} {run}", env.join(" "));
    }
    lines.push(run);
    lines.join("\n")
}

#[cfg(test)]
mod approval_tests {
    use super::*;
    use termide_config::commands::{CommandCwd, CommandMetadata};

    fn command(run: &str, build: Option<&str>) -> CommandItem {
        CommandItem {
            name: "go".into(),
            command: Some(run.into()),
            mode: Default::default(),
            cwd: CommandCwd::Panel,
            is_project: true,
            metadata: Some(CommandMetadata {
                build: build.map(str::to_string),
                ..Default::default()
            }),
        }
    }

    // The same line run from the project root is a different thing: it must not
    // pass under an approval given for the focused panel's directory.
    #[test]
    fn a_different_cwd_is_a_different_approval() {
        let mut from_project = command("./run", None);
        from_project.cwd = CommandCwd::Project;
        assert_ne!(
            command_spec(&command("./run", None)),
            command_spec(&from_project),
            "cwd is not part of the approved spec"
        );
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

    // An environment is part of what runs (`LD_PRELOAD`); none at all keeps
    // the approvals given before `env` existed.
    #[test]
    fn the_environment_is_approved_with_the_command() {
        let plain = command("./run", None);
        let mut with_env = command("./run", None);
        with_env
            .metadata
            .as_mut()
            .unwrap()
            .env
            .insert("LD_PRELOAD".into(), "/tmp/x.so".into());
        assert!(!command_spec(&plain).contains("env"));
        assert_ne!(command_spec(&plain), command_spec(&with_env));
        assert_eq!(command_text(&with_env), "LD_PRELOAD='/tmp/x.so' ./run");
    }
}

#[cfg(test)]
mod placeholder_tests {
    use super::*;
    use termide_config::commands::{CommandCwd, CommandMetadata};

    fn values(pairs: &[(&str, PlaceholderValue)]) -> HashMap<String, PlaceholderValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn debug_command(run: &str, build: Option<&str>) -> CommandItem {
        CommandItem {
            name: "dbg".into(),
            command: Some(run.into()),
            mode: Default::default(),
            cwd: CommandCwd::Project,
            is_project: false,
            metadata: Some(CommandMetadata {
                build: build.map(str::to_string),
                debugger: Some("debugpy".into()),
                ..Default::default()
            }),
        }
    }

    // What `sh -c` gets is what the program sees: one argument per value,
    // however the value is spelled.
    #[cfg(unix)]
    #[test]
    fn a_shell_command_gets_each_value_as_one_argument() {
        let v = values(&[
            (
                "file",
                PlaceholderValue::One("my dir/it's $HOME \"x\".c".into()),
            ),
            (
                "files",
                PlaceholderValue::Many(vec!["a b".into(), "c;rm".into()]),
            ),
        ]);
        let line = termide_config::expand_placeholders(
            "printf '[%s]\\n' edit {file} \"{file}\" {files}",
            &v,
            Quoting::Posix,
        );
        let out = build_command_command(&line, std::path::Path::new("."), None)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "[edit]\n[my dir/it's $HOME \"x\".c]\n[my dir/it's $HOME \"x\".c]\n[a b]\n[c;rm]\n"
        );
    }

    #[test]
    fn a_debug_command_keeps_each_value_one_word() {
        let v = values(&[
            ("file", PlaceholderValue::One("a b's.py".into())),
            ("line", PlaceholderValue::One("7".into())),
        ]);
        let expanded = expand_debug_command(
            &debug_command("python3 {file} --line={line} \\{file}", Some("make {file}")),
            &v,
        );
        let line = expanded.command.as_deref().unwrap();
        assert_eq!(
            termide_config::split_words(line).unwrap(),
            ["python3", "a b's.py", "--line=7", "{file}"]
        );
        assert_eq!(
            expanded.metadata.unwrap().build.as_deref(),
            Some(r"make 'a b'\''s.py'")
        );
    }

    // A value goes into a variable as it is, with no shell to quote for; the
    // parameters come after, as `TERMIDE_PARAM_<NAME>`.
    #[test]
    fn env_values_take_placeholders_literally() {
        let mut command = debug_command("app", None);
        let env = &mut command.metadata.as_mut().unwrap().env;
        env.insert("TARGET".into(), "{file}:{port}".into());
        env.insert("PLAIN".into(), "$HOME {other}".into());
        let v = values(&[
            ("file", PlaceholderValue::One("a b's.py".into())),
            ("port", PlaceholderValue::One("8091".into())),
        ]);
        let params = HashMap::from([("port".to_string(), "8091".to_string())]);
        assert_eq!(
            command_env(&command, &v, &params),
            [
                ("PLAIN".to_string(), "$HOME {other}".to_string()),
                ("TARGET".to_string(), "a b's.py:8091".to_string()),
                ("TERMIDE_PARAM_PORT".to_string(), "8091".to_string()),
            ]
        );
        assert_eq!(
            App::command_placeholder_names(&command),
            ["other", "file", "port"],
            "a panel placeholder in `env` is asked of the panel too"
        );
    }

    #[test]
    fn a_debug_command_without_placeholders_stays_as_written() {
        let command = debug_command("target/debug/app \"x y\" {other}", Some("cargo build"));
        let expanded = expand_debug_command(
            &command,
            &values(&[("file", PlaceholderValue::One("f".into()))]),
        );
        assert_eq!(expanded.command, command.command);
        assert_eq!(
            expanded.metadata.unwrap().build.as_deref(),
            Some("cargo build")
        );
    }
}
