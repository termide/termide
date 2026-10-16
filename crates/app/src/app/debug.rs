//! Debug launches: a debug adapter driven over DAP, shown in the debug
//! panel, or a command-line debugger started in a terminal panel — either
//! with the breakpoints of the project's files set.
//!
//! What to debug comes from the commands with a `debugger` in
//! `commands.toml`, or — for a Python or Go file — from the file alone.

use std::path::{Path, PathBuf};

use anyhow::Result;
use termide_config::commands::{CommandItem, CommandsRegistry};
use termide_debug::dap::{self, Adapter, Console, LaunchSpec, Session};
use termide_debug::{breakpoints, execution, Breakpoint, Debugger, Launch};

use super::App;
use crate::PanelExt;

/// One entry of the debug launch list.
#[derive(Debug, Clone)]
pub(super) enum DebugChoice {
    /// A command with a `debugger`.
    Command(CommandItem),
    /// The launch a file needs no configuration for.
    File { launch: Launch, cwd: PathBuf },
    /// A file debugged over DAP without configuration.
    Dap(LaunchSpec),
}

impl App {
    /// Start debugging: the only launch there is, or a list to choose from.
    /// `file` is the file the request came from.
    pub(in crate::app) fn start_debug(&mut self, file: Option<PathBuf>) -> Result<()> {
        // While a session runs, the key resumes it rather than starting another.
        {
            let mut slot = self.debug_session.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(session) = slot.as_mut().filter(|s| s.is_active()) {
                session.continue_();
                self.state.needs_redraw = true;
                return Ok(());
            }
        }
        // Read the files afresh, as the Commands menu does when it opens.
        let registry = CommandsRegistry::load_merged(Some(&self.project_root)).unwrap_or_default();
        let mut choices: Vec<DebugChoice> = registry
            .debug_commands()
            .into_iter()
            .cloned()
            .map(DebugChoice::Command)
            .collect();
        if let Some(choice) = file.as_deref().and_then(|f| self.file_launch(f)) {
            choices.push(choice);
        }

        let t = termide_i18n::t();
        match choices.len() {
            0 => {
                self.state.set_info(t.debug_nothing_to_launch().to_string());
                Ok(())
            }
            1 => self.run_debug_choice(choices.remove(0)),
            _ => {
                let labels = choices
                    .iter()
                    .map(|choice| choice_label(choice, file.as_deref()))
                    .collect();
                let modal = termide_modal::SelectModal::single(t.debug_select_title(), "", labels);
                self.debug_choices = Some(choices);
                self.state.set_pending_action(
                    termide_state::PendingAction::ChooseDebugLaunch,
                    crate::state::ActiveModal::Select(Box::new(modal)),
                );
                Ok(())
            }
        }
    }

    /// The launch picked from the list `start_debug` opened.
    pub(in crate::app) fn handle_debug_choice(
        &mut self,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        let Some(choices) = self.debug_choices.take() else {
            return Ok(());
        };
        match value
            .downcast_ref::<usize>()
            .and_then(|&i| choices.into_iter().nth(i))
        {
            Some(choice) => self.run_debug_choice(choice),
            None => Ok(()),
        }
    }

    fn run_debug_choice(&mut self, choice: DebugChoice) -> Result<()> {
        match choice {
            // Through the usual entry, so its parameters are asked for first.
            DebugChoice::Command(command) => self.start_command(command),
            DebugChoice::File { launch, cwd } => self.launch_debugger(&launch, cwd, &[]),
            DebugChoice::Dap(spec) => {
                self.launch_dap(spec, true);
                Ok(())
            }
        }
    }

    /// Run a command that has a `debugger`: its command line is the program
    /// to debug. It runs in the project root, where a path such as
    /// `target/debug/app` is meant from, whichever panel is focused.
    pub(in crate::app) fn run_debug_command(
        &mut self,
        command: &CommandItem,
        env: &[(String, String)],
    ) -> Result<()> {
        let name = command.debugger().unwrap_or_default();
        if Adapter::parse(name).is_some() {
            match self.dap_spec(command, env) {
                Ok(spec) => self.launch_dap(spec, true),
                Err(message) => self.show_error_modal(message),
            }
            return Ok(());
        }
        let Some(debugger) = Debugger::parse(name) else {
            self.show_error_modal(termide_i18n::t().debug_unknown_debugger_fmt(name));
            return Ok(());
        };
        let launch = Launch {
            debugger,
            target: command.command.clone().unwrap_or_default(),
            build: command.metadata.as_ref().and_then(|m| m.build.clone()),
        };
        let cwd = self.project_root.clone();
        self.launch_debugger(&launch, cwd, env)
    }

    /// Open a terminal in `cwd` running `launch` with the project's
    /// breakpoints set.
    fn launch_debugger(
        &mut self,
        launch: &Launch,
        cwd: PathBuf,
        env: &[(String, String)],
    ) -> Result<()> {
        use termide_panel_terminal::Terminal;

        self.sync_editor_breakpoints();
        breakpoints::flush();
        let set = project_breakpoints(&breakpoints::store().all(), &self.project_root);
        let script_dir = std::env::temp_dir().join("termide-debug");
        let line = match launch.command_line(&set, &script_dir) {
            Ok(line) => line,
            Err(e) => {
                self.show_error_modal(termide_i18n::t().command_run_failed_fmt(&format!("{e:#}")));
                return Ok(());
            }
        };
        log::info!(
            "Debugging with {} ({} breakpoints) in {:?}: {}",
            launch.debugger.as_str(),
            set.len(),
            cwd,
            line
        );

        self.close_help_panels();
        let term_height = self.state.terminal.height.saturating_sub(3);
        let term_width = self.state.terminal.width.saturating_sub(2);
        match Terminal::new_with_cwd_env(term_height, term_width, Some(cwd), env) {
            Ok(mut terminal) => {
                let _ = terminal.send_command(&line);
                self.add_panel(Box::new(terminal));
                self.auto_save_layout();
            }
            Err(e) => {
                log::error!("Failed to create a terminal for the debugger: {e}");
                self.show_error_modal(termide_i18n::t().command_run_failed_fmt(&e.to_string()));
            }
        }
        Ok(())
    }

    /// What debugging `file` needs no command for: a Python script or a Go
    /// package under its debug adapter in the debug panel when the adapter is
    /// installed, under its command-line debugger in a terminal otherwise.
    pub(in crate::app) fn file_launch(&self, file: &Path) -> Option<DebugChoice> {
        let cwd = file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.project_root.clone());
        let terminal = Launch::for_file(file)?;
        let adapter = match terminal.debugger {
            Debugger::Pdb if dap::debugpy_available() => Some(Adapter::Debugpy),
            Debugger::Dlv if Adapter::Delve.command().is_some() => Some(Adapter::Delve),
            _ => None,
        };
        Some(match adapter {
            Some(adapter) => {
                let program = match adapter {
                    Adapter::Delve => ".".to_string(),
                    _ => file.to_string_lossy().into_owned(),
                };
                DebugChoice::Dap(LaunchSpec {
                    adapter,
                    program,
                    args: Vec::new(),
                    cwd,
                    build: None,
                    env: Vec::new(),
                    label: file_label(file),
                    console: Console::Terminal,
                    tty: None,
                })
            }
            None => DebugChoice::File {
                launch: terminal,
                cwd,
            },
        })
    }

    /// The DAP launch of `command`, whose `debugger` is a debug adapter: its
    /// command line is the program and its arguments, run in the project root.
    pub(in crate::app) fn dap_spec(
        &self,
        command: &CommandItem,
        env: &[(String, String)],
    ) -> Result<LaunchSpec, String> {
        let name = command.debugger().unwrap_or_default();
        let adapter = Adapter::parse(name)
            .ok_or_else(|| termide_i18n::t().debug_unknown_debugger_fmt(name))?;
        let text = command.command.clone().unwrap_or_default();
        let (program, args) = dap::split_words(&text)
            .and_then(|words| words.split_first().map(|(p, a)| (p.clone(), a.to_vec())))
            .ok_or_else(|| termide_i18n::t().command_run_failed_fmt(&text))?;
        let label = command
            .metadata
            .as_ref()
            .and_then(|m| m.display_name.clone())
            .unwrap_or_else(|| command.name.clone());
        let console = command
            .metadata
            .as_ref()
            .and_then(|m| m.console.as_deref())
            .and_then(Console::parse)
            .unwrap_or_default();
        Ok(LaunchSpec {
            adapter,
            program,
            args,
            cwd: self.project_root.clone(),
            build: command.metadata.as_ref().and_then(|m| m.build.clone()),
            env: env.to_vec(),
            label,
            console,
            tty: None,
        })
    }

    /// Start a DAP session in the debug panel, ending the one before. The
    /// panel takes the focus when `focus` (the user started it), and opens
    /// beside the others otherwise (an agent did).
    pub(in crate::app) fn launch_dap(&mut self, mut spec: LaunchSpec, focus: bool) {
        self.sync_editor_breakpoints();
        breakpoints::flush();
        let (set, version) = {
            let store = breakpoints::store();
            (
                project_breakpoints(&store.all(), &self.project_root),
                store.version(),
            )
        };
        log::info!(
            "Debugging {} with {} ({} breakpoints) in {:?}",
            spec.program,
            spec.adapter.as_str(),
            set.len(),
            spec.cwd
        );
        self.end_debug_session();
        if !spec.adapter.supports_terminal() {
            spec.console = Console::Panel;
        }
        // lldb-dap is handed a terminal of termide's to point the program at;
        // the other adapters ask for one when they launch it.
        if spec.console == Console::Terminal && !spec.adapter.asks_for_terminal() {
            spec.tty = self.open_debug_terminal(&spec.label);
            if spec.tty.is_none() {
                spec.console = Console::Panel;
            }
        }
        *self.debug_session.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Session::start(spec, set));
        self.debug_breakpoints_version = version;
        self.debug_revealed = None;

        let open = if focus {
            self.layout_manager
                .focus_panel_where(|p| p.name() == "debug")
        } else {
            self.layout_manager
                .iter_all_panels_mut()
                .any(|p| p.name() == "debug")
        };
        if !open {
            let panel = Box::new(termide_panel_debug::DebugPanel::new(
                self.debug_session.clone(),
            ));
            if focus {
                self.close_help_panels();
                self.add_panel(panel);
            } else {
                self.add_panel_without_focus(panel);
            }
        }
        self.state.needs_redraw = true;
    }

    /// A terminal panel whose device a program can be pointed at, opened
    /// beside the others; `None` where there is no such device.
    fn open_debug_terminal(&mut self, label: &str) -> Option<PathBuf> {
        #[cfg(unix)]
        {
            use termide_panel_terminal::Terminal;
            let rows = self.state.terminal.height.saturating_sub(3);
            let cols = self.state.terminal.width.saturating_sub(2);
            match Terminal::new_for_device(rows, cols, label) {
                Ok((terminal, device)) => {
                    self.debug_terminal_pid = terminal.child_pid();
                    self.add_panel_without_focus(Box::new(terminal));
                    Some(device)
                }
                Err(e) => {
                    log::warn!("Debug: no terminal for the program: {e}");
                    None
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = label;
            None
        }
    }

    /// Start the program an adapter asked to have run in a terminal, in a
    /// terminal panel beside the others, and tell the adapter its pid.
    fn start_requested_terminal(&mut self, request: dap::TerminalRequest) {
        use termide_panel_terminal::Terminal;
        let rows = self.state.terminal.height.saturating_sub(3);
        let cols = self.state.terminal.width.saturating_sub(2);
        let pid = match Terminal::new_with_program(
            rows,
            cols,
            &request.args,
            request.cwd.clone(),
            &request.env,
        ) {
            Ok(terminal) => {
                let pid = terminal.child_pid();
                self.add_panel_without_focus(Box::new(terminal));
                pid.ok_or_else(|| "the program has no process id".to_string())
            }
            Err(e) => Err(format!("cannot start the program in a terminal: {e}")),
        };
        if let Some(session) = self
            .debug_session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            session.answer_terminal(&request, pid);
        }
        self.state.needs_redraw = true;
    }

    /// End what holds the terminal termide gave the program (lldb-dap), so
    /// the panel shows that the session is over.
    fn release_debug_terminal(&mut self) {
        let Some(pid) = self.debug_terminal_pid.take() else {
            return;
        };
        for panel in self.all_editor_panels_mut() {
            if let Some(terminal) = panel
                .as_any_mut()
                .downcast_mut::<termide_panel_terminal::Terminal>()
            {
                if terminal.child_pid() == Some(pid) {
                    termide_core::Panel::kill_processes(terminal);
                }
            }
        }
    }

    /// End the running session, ending its program.
    pub(in crate::app) fn end_debug_session(&mut self) {
        self.release_debug_terminal();
        if let Some(mut old) = self
            .debug_session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            old.stop();
        }
        if execution::set(None) {
            self.state.needs_redraw = true;
        }
    }

    /// Apply what the adapter sent, pass breakpoint changes on, and show
    /// where the program stopped. Called every tick.
    pub(in crate::app) fn poll_debug_session(&mut self) {
        let (changed, point, active, terminal) = {
            let mut slot = self.debug_session.lock().unwrap_or_else(|e| e.into_inner());
            let Some(session) = slot.as_mut() else {
                return;
            };
            let mut changed = session.poll();
            let terminal = session.take_terminal_request();
            if session.is_active() {
                let store = breakpoints::store();
                if store.version() != self.debug_breakpoints_version {
                    self.debug_breakpoints_version = store.version();
                    let set = project_breakpoints(&store.all(), &self.project_root);
                    drop(store);
                    session.update_breakpoints(&set);
                    changed = true;
                }
            }
            let point = session.execution_point().filter(|(path, _)| path.is_file());
            (changed, point, session.is_active(), terminal)
        };
        if let Some(request) = terminal {
            self.start_requested_terminal(request);
        }
        if execution::set(point.clone()) || changed {
            self.state.needs_redraw = true;
        }
        if point.is_some() && point != self.debug_revealed {
            if let Some((path, line)) = point.clone() {
                self.reveal_execution_point(&path, line);
            }
            self.debug_revealed = point;
        } else if point.is_none() {
            self.debug_revealed = None;
        }
        if !active {
            self.debug_revealed = None;
            self.release_debug_terminal();
        }
    }

    /// Show `line` (1-based) of `path` in an editor without taking the focus
    /// from the debug panel: the editor that has the file, or a new one.
    fn reveal_execution_point(&mut self, path: &Path, line: usize) {
        let canonical = dunce::canonicalize(path).ok();
        for panel in self.layout_manager.iter_all_panels_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                let same = editor.file_path().is_some_and(|p| {
                    p == path || canonical.is_some() && dunce::canonicalize(p).ok() == canonical
                });
                if same {
                    editor.reveal_line(line.saturating_sub(1));
                    return;
                }
            }
        }
        match termide_panel_editor::Editor::open_file_with_config(
            path.to_path_buf(),
            self.state.editor_config(),
        ) {
            Ok(mut editor) => {
                editor.reveal_line(line.saturating_sub(1));
                if let Some(ref mut lsp_manager) = self.state.lsp_manager {
                    editor.init_lsp(lsp_manager);
                }
                self.add_panel_without_focus(Box::new(editor));
            }
            Err(e) => log::warn!("Debug: cannot open {}: {e}", path.display()),
        }
    }

    /// Remove every breakpoint, from the store and from open editors.
    pub(in crate::app) fn clear_breakpoints(&mut self) {
        breakpoints::store().clear();
        breakpoints::flush();
        for panel in self.all_editor_panels_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                editor.reload_breakpoints();
            }
        }
        self.state
            .set_info(termide_i18n::t().debug_breakpoints_cleared().to_string());
        self.state.needs_redraw = true;
    }

    /// Write the breakpoints of every open editor to the store: an edit made
    /// by the app (a rename across files) moves them without the editor
    /// seeing input.
    pub(in crate::app) fn sync_editor_breakpoints(&mut self) {
        for panel in self.all_editor_panels_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                editor.sync_breakpoints();
            }
        }
    }

    /// The panels of the current project and of the projects parked in the
    /// background.
    fn all_editor_panels_mut(&mut self) -> Vec<&mut Box<dyn termide_core::Panel>> {
        let mut panels: Vec<_> = self.layout_manager.iter_all_panels_mut().collect();
        for (_, parked) in self.open_projects.parked_mut() {
            panels.extend(parked.panels_mut());
        }
        panels
    }
}

fn file_label(file: &Path) -> String {
    file.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn choice_label(choice: &DebugChoice, file: Option<&Path>) -> String {
    match choice {
        DebugChoice::Dap(spec) => {
            termide_i18n::t().debug_current_file_fmt(&spec.label, spec.adapter.as_str())
        }
        DebugChoice::Command(command) => command
            .metadata
            .as_ref()
            .and_then(|m| m.display_name.clone())
            .unwrap_or_else(|| command.name.clone()),
        DebugChoice::File { launch, .. } => {
            let name = file
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            termide_i18n::t().debug_current_file_fmt(&name, launch.debugger.as_str())
        }
    }
}

/// The breakpoints in files under `root`: others belong to other projects,
/// and a debugger would only report them as unresolved. Paths are resolved
/// through symlinks where they exist, as the running program names its
/// files that way (on macOS `/var` is `/private/var`, and pdb then misses a
/// breakpoint set by the other name).
fn project_breakpoints(all: &[Breakpoint], root: &Path) -> Vec<Breakpoint> {
    let canonical_root = dunce::canonicalize(root).ok();
    all.iter()
        .filter_map(|b| {
            let canonical = dunce::canonicalize(&b.path).ok();
            let inside = b.path.starts_with(root)
                || canonical
                    .as_deref()
                    .zip(canonical_root.as_deref())
                    .is_some_and(|(path, root)| path.starts_with(root));
            inside.then(|| Breakpoint {
                path: canonical.unwrap_or_else(|| b.path.clone()),
                line: b.line,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_projects_breakpoints_are_set() {
        let bp = |path: &str| Breakpoint {
            path: path.into(),
            line: 1,
        };
        let all = [bp("/work/app/src/main.rs"), bp("/work/other/lib.rs")];
        let set = project_breakpoints(&all, Path::new("/work/app"));
        assert_eq!(set, [bp("/work/app/src/main.rs")]);
    }
}
