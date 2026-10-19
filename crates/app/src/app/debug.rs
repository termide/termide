//! Debug launches: a debug adapter driven over DAP, shown in the debug
//! panel, or a command-line debugger started in a terminal panel — either
//! with the breakpoints of the project's files set.
//!
//! What to debug comes from the commands with a `debugger` in
//! `commands.toml`, or — for a Python or Go file — from the file alone.

use std::path::{Path, PathBuf};

use anyhow::Result;
use termide_config::commands::{CommandItem, CommandsRegistry};
use termide_config::DebuggerSettings;
use termide_debug::dap::{self, Adapter, Console, LaunchSpec, Session};
use termide_debug::{breakpoints, execution, Breakpoint, Debugger, Launch};

use super::App;
use crate::PanelExt;

/// One entry of the debug launch list.
#[derive(Debug, Clone)]
pub(super) enum DebugChoice {
    /// A command with a `debugger`.
    Command(CommandItem),
    /// The launch a file needs no configuration for, under a command-line
    /// debugger in a terminal; `label` names it in the list.
    File {
        launch: Launch,
        cwd: PathBuf,
        label: String,
    },
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
        let mut missing = None;
        match file.as_deref().map(|f| self.file_launches(f)) {
            Some(Ok(launches)) => choices.extend(launches),
            Some(Err(why)) => missing = Some(why),
            None => {}
        }

        let t = termide_i18n::t();
        match choices.len() {
            // Nothing to launch: say what is missing, or let the user set
            // the launch up.
            0 => {
                match missing {
                    Some(why) => self.show_debug_message(why),
                    None => self.open_debug_setup(file.as_deref()),
                }
                Ok(())
            }
            1 => self.run_debug_choice(choices.remove(0)),
            _ => {
                let labels = choices.iter().map(choice_label).collect();
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

    /// The settings of debugger `name` (`[debug.debuggers.<name>]`).
    pub(in crate::app) fn debugger_settings(&self, name: &str) -> DebuggerSettings {
        self.state.config.debug.debugger(name)
    }

    /// Whether `adapter` can be started as the settings say: its program is
    /// there (and for debugpy, its Python imports it).
    pub(in crate::app) fn adapter_installed(&self, adapter: Adapter) -> bool {
        let settings = self.debugger_settings(adapter.as_str());
        match adapter {
            Adapter::Debugpy => dap::debugpy_available_with(&settings),
            _ => adapter.command_with(&settings).is_some(),
        }
    }

    /// The program of a command-line debugger: the settings', or the usual
    /// one on `PATH`; `None` when it is not there.
    fn terminal_program(&self, debugger: Debugger) -> Option<PathBuf> {
        match self.debugger_settings(debugger.as_str()).command {
            Some(command) => dap::resolve_program(&command),
            None => dap::find_in_path(match debugger {
                Debugger::Pdb => "python3",
                other => other.as_str(),
            }),
        }
    }

    /// A launch under a command-line debugger with its settings applied:
    /// their program, their flags, then the launch's own `extra_args`.
    fn terminal_launch(
        &self,
        debugger: Debugger,
        target: String,
        build: Option<String>,
        extra_args: &[String],
    ) -> Launch {
        let settings = self.debugger_settings(debugger.as_str());
        let mut debugger_args = settings.args.unwrap_or_default();
        debugger_args.extend_from_slice(extra_args);
        Launch {
            debugger,
            target,
            build,
            debugger_args,
            // Resolved, so a `~/…` path works though the shell gets it quoted.
            program: settings.command.map(|command| {
                dap::resolve_program(&command)
                    .map_or(command, |path| path.to_string_lossy().into_owned())
            }),
        }
    }

    /// Where a debug adapter runs the program when the launch does not say.
    fn default_console(&self) -> Console {
        let name = &self.state.config.debug.console;
        Console::parse(name).unwrap_or_else(|| {
            log::warn!("[debug] console = {name:?} is neither terminal nor panel; using terminal");
            Console::default()
        })
    }

    /// The debugger a Rust file gets without a command: the first installed
    /// of `[debug] native`.
    pub(in crate::app) fn native_debugger(&self) -> Option<NativeDebugger> {
        self.state.config.debug.native.iter().find_map(|name| {
            match (Adapter::parse(name), Debugger::parse(name)) {
                (Some(adapter @ (Adapter::LldbDap | Adapter::GdbDap)), _) => self
                    .adapter_installed(adapter)
                    .then_some(NativeDebugger::Adapter(adapter)),
                (None, Some(d @ (Debugger::Lldb | Debugger::Gdb))) => self
                    .terminal_program(d)
                    .map(|_| NativeDebugger::Terminal(d)),
                _ => {
                    log::warn!(
                        "[debug] native: {name:?} is not a debugger for Rust \
                         (lldb-dap, gdb-dap, lldb, gdb)"
                    );
                    None
                }
            }
        })
    }

    /// A message about debugging, in a modal: the user asked for something
    /// and has to learn why it did not happen.
    pub(in crate::app) fn show_debug_message(&mut self, message: String) {
        let t = termide_i18n::t();
        let modal =
            termide_modal::InfoModal::new(t.debug_select_title(), vec![(String::new(), message)]);
        self.state
            .show_message_modal(crate::state::ActiveModal::Info(Box::new(modal)));
        self.state.needs_redraw = true;
    }

    /// Run the last launch through a debug adapter again — its build first,
    /// with the parameters it was given — ending the session if it still
    /// runs.
    pub(in crate::app) fn restart_debug(&mut self) {
        if let Some(spec) = self.debug_last_launch.clone() {
            self.launch_dap(spec, true);
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
        match picked_choice(value.as_ref(), choices) {
            Some(choice) => self.run_debug_choice(choice),
            None => Ok(()),
        }
    }

    fn run_debug_choice(&mut self, choice: DebugChoice) -> Result<()> {
        match choice {
            // Through the usual entry, so its parameters are asked for first.
            DebugChoice::Command(command) => self.start_command(command),
            DebugChoice::File { launch, cwd, .. } => self.launch_debugger(&launch, cwd, &[]),
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
        let launch = self.terminal_launch(
            debugger,
            command.command.clone().unwrap_or_default(),
            command.metadata.as_ref().and_then(|m| m.build.clone()),
            command.debugger_args(),
        );
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

    /// What debugging `file` needs no command for: a Python script, a Go
    /// package or the binaries of a Cargo project, under a debug adapter in
    /// the debug panel when one is installed, under a command-line debugger
    /// in a terminal otherwise. Empty for other files; `Err` says what is
    /// missing for a file that could be debugged.
    pub(in crate::app) fn file_launches(&self, file: &Path) -> Result<Vec<DebugChoice>, String> {
        let t = termide_i18n::t();
        let is_rust = file
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("rs"));
        if is_rust {
            if termide_debug::cargo::manifest_for(file).is_none() {
                return Ok(Vec::new());
            }
            return self.cargo_launches(file);
        }
        let cwd = file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.project_root.clone());
        let Some(found) = Launch::for_file(file) else {
            return Ok(Vec::new());
        };
        let terminal = self.terminal_launch(found.debugger, found.target, found.build, &[]);
        let adapter = match terminal.debugger {
            Debugger::Pdb if self.adapter_installed(Adapter::Debugpy) => Some(Adapter::Debugpy),
            Debugger::Dlv if self.adapter_installed(Adapter::Delve) => Some(Adapter::Delve),
            Debugger::Dlv if self.terminal_program(Debugger::Dlv).is_none() => {
                return Err(t.debug_no_delve().to_string());
            }
            _ => None,
        };
        Ok(vec![match adapter {
            Some(adapter) => {
                let program = match adapter {
                    Adapter::Delve => ".".to_string(),
                    _ => file.to_string_lossy().into_owned(),
                };
                DebugChoice::Dap(LaunchSpec {
                    adapter,
                    debugger_settings: self.debugger_settings(adapter.as_str()),
                    adapter_args: Vec::new(),
                    program,
                    args: Vec::new(),
                    cwd,
                    build: None,
                    env: Vec::new(),
                    label: file_label(file),
                    console: self.default_console(),
                    tty: None,
                })
            }
            None => DebugChoice::File {
                label: t.debug_current_file_fmt(&file_label(file), terminal.debugger.as_str()),
                launch: terminal,
                cwd,
            },
        }])
    }

    /// One launch per binary of the Cargo project `file` is in, built with
    /// cargo first and run in the workspace root.
    fn cargo_launches(&self, file: &Path) -> Result<Vec<DebugChoice>, String> {
        let t = termide_i18n::t();
        let debugger = self
            .native_debugger()
            .ok_or_else(|| t.debug_no_native_debugger().to_string())?;
        let bins = termide_debug::cargo::bins_for(file)
            .map_err(|e| t.debug_cargo_failed_fmt(&format!("{e:#}")))?;
        Ok(bins
            .into_iter()
            .map(|bin| {
                let cwd = bin.workspace_root.clone();
                let build = bin.build_command(&cwd);
                let program = bin.program.to_string_lossy().into_owned();
                match debugger {
                    NativeDebugger::Adapter(adapter) => DebugChoice::Dap(LaunchSpec {
                        adapter,
                        debugger_settings: self.debugger_settings(adapter.as_str()),
                        adapter_args: Vec::new(),
                        program,
                        args: Vec::new(),
                        cwd,
                        build: Some(build),
                        env: Vec::new(),
                        label: bin.name,
                        console: self.default_console(),
                        tty: None,
                    }),
                    NativeDebugger::Terminal(debugger) => DebugChoice::File {
                        label: t.debug_current_file_fmt(&bin.name, debugger.as_str()),
                        launch: self.terminal_launch(
                            debugger,
                            termide_debug::shell_quote(&program),
                            Some(build),
                            &[],
                        ),
                        cwd,
                    },
                }
            })
            .collect())
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
            .unwrap_or_else(|| self.default_console());
        Ok(LaunchSpec {
            adapter,
            debugger_settings: self.debugger_settings(adapter.as_str()),
            adapter_args: command.debugger_args().to_vec(),
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
        self.debug_last_launch = Some(spec.clone());
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

fn choice_label(choice: &DebugChoice) -> String {
    match choice {
        DebugChoice::Dap(spec) => {
            termide_i18n::t().debug_current_file_fmt(&spec.label, spec.adapter.as_str())
        }
        DebugChoice::Command(command) => command
            .metadata
            .as_ref()
            .and_then(|m| m.display_name.clone())
            .unwrap_or_else(|| command.name.clone()),
        DebugChoice::File { label, .. } => label.clone(),
    }
}

/// The debugger a Rust file gets without a command (`[debug] native`).
#[derive(Debug, Clone, Copy)]
pub(super) enum NativeDebugger {
    Adapter(Adapter),
    Terminal(Debugger),
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

/// The launch the list's answer names. A `SelectModal` answers with the
/// picked indices (`Vec<usize>`), one here; anything else picks nothing.
fn picked_choice<T>(value: &dyn std::any::Any, choices: Vec<T>) -> Option<T> {
    let index = value.downcast_ref::<Vec<usize>>()?.first().copied()?;
    choices.into_iter().nth(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launch_picked_from_the_list_is_the_one_run() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use termide_core::KeyChord;
        use termide_modal::{ActiveModal, ModalResult, SelectModal};

        // The answer as the app gets it: through the type-erased modal.
        let mut modal = ActiveModal::Select(Box::new(SelectModal::single(
            "Debug",
            "",
            vec!["Debug app".into(), "main.py under debugpy".into()],
        )));
        let press = |modal: &mut ActiveModal, code| {
            modal
                .handle_key_erased(KeyChord::identity(KeyEvent::new(code, KeyModifiers::NONE)))
                .unwrap()
        };
        assert!(press(&mut modal, KeyCode::Down).is_none());
        let Some(ModalResult::Confirmed(value)) = press(&mut modal, KeyCode::Enter) else {
            panic!("Enter should confirm the list");
        };
        assert_eq!(
            picked_choice(value.as_ref(), vec!["app", "main.py"]),
            Some("main.py")
        );
        assert_eq!(picked_choice(&7usize, vec!["app"]), None);
    }

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
