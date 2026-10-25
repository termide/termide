//! `debug` for the agents in the panels. The call runs on the agent's
//! thread and the session lives with the app, so the call sends its action
//! down a process-wide channel and waits; the app takes the action on its
//! tick and answers once the program has got where the action leads — the
//! next stop, the end, an evaluated value — or a deadline passes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use termide_agent_core::{CancelToken, Tool, ToolContext};
use termide_agent_tools::{DebugAction, DebugTool};
use termide_config::commands::CommandsRegistry;
use termide_debug::breakpoints;

use super::super::debug::DebugChoice;
use crate::app::App;
use crate::PanelExt;

/// Output lines an answer shows.
const OUTPUT_LINES: usize = 30;
/// How long a launch may take to reach its first stop, a build included.
const START_WAIT: Duration = Duration::from_secs(600);
/// How long a resume may run before the answer says it still runs.
const RUN_WAIT: Duration = Duration::from_secs(60);
const SHORT_WAIT: Duration = Duration::from_secs(15);

struct Request {
    project_root: PathBuf,
    action: DebugAction,
    reply: Sender<Result<String, String>>,
}

/// An answer the app owes once the session gets somewhere.
pub(in crate::app) struct Waiter {
    until: Until,
    deadline: Instant,
    reply: Sender<Result<String, String>>,
}

/// What a taken action leaves to do.
enum Taken {
    /// Answer now: this text, then the session.
    Now(Option<String>),
    /// Answer once the session gets there, or the time runs out.
    Wait(Until, Duration),
}

enum Until {
    /// The program stopped with its stack and variables in, or ended.
    Settled,
    /// More evaluations were answered than this.
    Evaluated(u64),
}

fn port() -> &'static (Sender<Request>, Mutex<Receiver<Request>>) {
    static PORT: OnceLock<(Sender<Request>, Mutex<Receiver<Request>>)> = OnceLock::new();
    PORT.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        (sender, Mutex::new(receiver))
    })
}

static SERVED: AtomicBool = AtomicBool::new(false);

/// `debug` for an agent of a panel in the project at `project_root`.
pub(super) fn debug_tool(project_root: &Path) -> Arc<dyn Tool> {
    let root = project_root.to_path_buf();
    Arc::new(DebugTool::new(Arc::new(
        move |_ctx: &ToolContext, action: DebugAction, cancel: &CancelToken| {
            if !SERVED.load(Ordering::Acquire) {
                return Err("there is no debugger to drive here".into());
            }
            let (reply, answer) = mpsc::channel();
            port()
                .0
                .send(Request {
                    project_root: root.clone(),
                    action,
                    reply,
                })
                .map_err(|_| "the debugger cannot be reached".to_string())?;
            // The app answers by its deadline; this one only guards against
            // an app that went away.
            let give_up = Instant::now() + START_WAIT + Duration::from_secs(30);
            loop {
                match answer.recv_timeout(Duration::from_millis(200)) {
                    Ok(result) => return result,
                    Err(RecvTimeoutError::Disconnected) => {
                        return Err("the debugger did not answer".into())
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if cancel.is_cancelled() {
                            return Err("cancelled; the program is left as it was".into());
                        }
                        if Instant::now() > give_up {
                            return Err("the debugger did not answer".into());
                        }
                    }
                }
            }
        },
    )))
}

impl App {
    /// Take the agents' debug actions and answer the ones whose outcome has
    /// come. Called on every tick, after the session was polled.
    pub(in crate::app) fn serve_debug_tool(&mut self) {
        SERVED.store(true, Ordering::Release);
        let requests: Vec<Request> = port()
            .1
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_iter()
            .collect();
        for request in requests {
            let Request {
                project_root,
                action,
                reply,
            } = request;
            let taken = self
                .in_project(&project_root, |app| app.take_debug_action(action))
                .and_then(|taken| taken);
            match taken {
                Ok(Taken::Wait(until, wait)) => {
                    self.debug_waiters.push(Waiter {
                        until,
                        deadline: Instant::now() + wait,
                        reply,
                    });
                }
                Ok(Taken::Now(note)) => {
                    let state = self.describe_debug();
                    let text = match note {
                        Some(note) => format!("{note}\n\n{state}"),
                        None => state,
                    };
                    let _ = reply.send(Ok(text));
                }
                Err(message) => {
                    let _ = reply.send(Err(message));
                }
            }
            self.state.needs_redraw = true;
        }
        self.answer_debug_waiters();
    }

    fn answer_debug_waiters(&mut self) {
        if self.debug_waiters.is_empty() {
            return;
        }
        let now = Instant::now();
        let waiters = std::mem::take(&mut self.debug_waiters);
        for waiter in waiters {
            let outcome = {
                let slot = self.debug_session.lock().unwrap_or_else(|e| e.into_inner());
                match (slot.as_ref(), &waiter.until) {
                    (None, _) => Some(Err("the debug session is gone".to_string())),
                    (Some(s), Until::Evaluated(before)) if s.evaluations > *before => {
                        s.last_evaluation.clone()
                    }
                    (Some(s), Until::Evaluated(_)) if !s.is_active() => Some(Err(format!(
                        "the session ended\n\n{}",
                        s.describe(OUTPUT_LINES)
                    ))),
                    (Some(s), Until::Settled) if !s.is_busy() => Some(Ok(s.describe(OUTPUT_LINES))),
                    (Some(s), _) if now >= waiter.deadline => Some(Ok(format!(
                        "{}\nNot there yet: the program still runs. Call `debug` with `state` \
                         later, or `pause` it.",
                        s.describe(OUTPUT_LINES)
                    ))),
                    _ => None,
                }
            };
            match outcome {
                Some(result) => {
                    let _ = waiter.reply.send(result);
                }
                None => self.debug_waiters.push(waiter),
            }
        }
    }

    fn describe_debug(&self) -> String {
        let slot = self.debug_session.lock().unwrap_or_else(|e| e.into_inner());
        match slot.as_ref() {
            Some(session) => session.describe(OUTPUT_LINES),
            None => "No debug session. Start one with `start`.".into(),
        }
    }

    /// Take `action`, and say when to answer.
    fn take_debug_action(&mut self, action: DebugAction) -> Result<Taken, String> {
        let control = |app: &mut App, act: &dyn Fn(&mut termide_debug::dap::Session)| {
            let mut slot = app.debug_session.lock().unwrap_or_else(|e| e.into_inner());
            match slot.as_mut() {
                Some(s) if s.is_stopped() => {
                    act(s);
                    Ok(Taken::Wait(Until::Settled, RUN_WAIT))
                }
                Some(s) if s.is_active() => Err(format!(
                    "the program is not stopped\n\n{}",
                    s.describe(OUTPUT_LINES)
                )),
                _ => Err("no debug session runs; start one with `start`".into()),
            }
        };
        match action {
            DebugAction::State => Ok(Taken::Now(None)),
            DebugAction::Start { command, file } => {
                self.agent_start_debug(command.as_deref(), file.as_deref())?;
                Ok(Taken::Wait(Until::Settled, START_WAIT))
            }
            DebugAction::Breakpoint { file, line, set } => {
                Ok(Taken::Now(Some(self.agent_breakpoint(&file, line, set)?)))
            }
            DebugAction::Continue => control(self, &|s| s.continue_()),
            DebugAction::StepOver => control(self, &|s| s.step_over()),
            DebugAction::StepIn => control(self, &|s| s.step_in()),
            DebugAction::StepOut => control(self, &|s| s.step_out()),
            DebugAction::Frame { index } => {
                let mut slot = self.debug_session.lock().unwrap_or_else(|e| e.into_inner());
                match slot.as_mut() {
                    Some(s) if s.is_stopped() && index < s.frames.len() => {
                        s.select_frame(index);
                        Ok(Taken::Wait(Until::Settled, SHORT_WAIT))
                    }
                    Some(s) if s.is_stopped() => Err(format!(
                        "there are {} frames, numbered from 0",
                        s.frames.len()
                    )),
                    _ => Err("the program is not stopped".into()),
                }
            }
            DebugAction::Pause => {
                let mut slot = self.debug_session.lock().unwrap_or_else(|e| e.into_inner());
                match slot.as_mut() {
                    Some(s) if s.is_active() => {
                        s.pause();
                        Ok(Taken::Wait(Until::Settled, SHORT_WAIT))
                    }
                    _ => Err("no debug session runs".into()),
                }
            }
            DebugAction::Stop => {
                if let Some(s) = self
                    .debug_session
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_mut()
                {
                    s.stop();
                }
                Ok(Taken::Now(None))
            }
            DebugAction::Evaluate { expression } => {
                let mut slot = self.debug_session.lock().unwrap_or_else(|e| e.into_inner());
                match slot.as_mut() {
                    Some(s) if s.is_active() => {
                        let before = s.evaluations;
                        s.evaluate(&expression);
                        Ok(Taken::Wait(Until::Evaluated(before), SHORT_WAIT))
                    }
                    _ => Err("no debug session runs; start one with `start`".into()),
                }
            }
        }
    }

    /// Start a DAP session for an agent, beside the panels it does not take
    /// the focus from.
    fn agent_start_debug(
        &mut self,
        command: Option<&str>,
        file: Option<&Path>,
    ) -> Result<(), String> {
        let spec = match (command, file) {
            (Some(name), _) => {
                let registry =
                    CommandsRegistry::load_merged(Some(&self.project_root)).unwrap_or_default();
                let commands = registry.debug_commands();
                let wanted = name.trim();
                let Some(command) = commands.iter().find(|c| {
                    c.name.eq_ignore_ascii_case(wanted)
                        || c.metadata
                            .as_ref()
                            .and_then(|m| m.display_name.as_deref())
                            .is_some_and(|label| label.eq_ignore_ascii_case(wanted))
                }) else {
                    let known: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
                    return Err(if known.is_empty() {
                        format!(
                            "no command `{wanted}`: commands.toml has no command with a `debugger`"
                        )
                    } else {
                        format!(
                            "no command `{wanted}`; the debug commands are {}",
                            known.join(", ")
                        )
                    });
                };
                if command
                    .metadata
                    .as_ref()
                    .is_some_and(|m| !m.params.is_empty())
                {
                    return Err(format!(
                        "`{}` asks the user for parameters; ask the user to start it",
                        command.name
                    ));
                }
                let texts = [
                    command.command.as_deref(),
                    command.metadata.as_ref().and_then(|m| m.build.as_deref()),
                ];
                if let Some(name) = texts
                    .into_iter()
                    .flatten()
                    .flat_map(termide_config::placeholder_names)
                    .find(|name| termide_config::CONTEXT_PLACEHOLDERS.contains(name))
                {
                    return Err(format!(
                        "`{}` takes {{{name}}} from the panel the user has focused; ask the user \
                         to start it",
                        command.name
                    ));
                }
                if termide_debug::dap::Adapter::parse(command.debugger().unwrap_or_default())
                    .is_none()
                {
                    return Err(format!(
                        "`{}` runs a command-line debugger in a terminal, which is the user's to \
                         drive; only a debug adapter (lldb-dap, gdb-dap, debugpy, dlv-dap) can be \
                         driven with `debug`",
                        command.name
                    ));
                }
                self.dap_spec(command, &[])?
            }
            (None, Some(file)) => {
                let launches = self.file_launches(file)?;
                if launches.is_empty() {
                    return Err(format!(
                        "{} cannot be debugged without a command; give `command`, one with a \
                         `debugger` in commands.toml",
                        file.display()
                    ));
                }
                let mut specs: Vec<_> = launches
                    .into_iter()
                    .filter_map(|choice| match choice {
                        DebugChoice::Dap(spec) => Some(spec),
                        _ => None,
                    })
                    .collect();
                match specs.len() {
                    0 => {
                        return Err(format!(
                            "{} needs a debug adapter installed to be debugged here: debugpy \
                             (Python), Delve (Go), lldb-dap or GDB 14+ (Rust)",
                            file.display()
                        ))
                    }
                    1 => specs.remove(0),
                    _ => {
                        let names: Vec<&str> = specs.iter().map(|s| s.label.as_str()).collect();
                        return Err(format!(
                            "{} belongs to several binaries ({}); give the main file of one, \
                             such as src/main.rs or src/bin/<name>.rs",
                            file.display(),
                            names.join(", ")
                        ));
                    }
                }
            }
            (None, None) => return Err("`start` needs `command` or `file`".into()),
        };
        // The agent reads the output from the session; a program waiting for
        // the keyboard in a terminal would leave it waiting.
        let spec = termide_debug::dap::LaunchSpec {
            console: termide_debug::dap::Console::Panel,
            ..spec
        };
        self.launch_dap(spec, false);
        Ok(())
    }

    /// Set or remove the breakpoint at 1-based `line` of `file`, in the store
    /// and in the editors that have the file.
    fn agent_breakpoint(&mut self, file: &Path, line: usize, set: bool) -> Result<String, String> {
        let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let lines = text.lines().count().max(1);
        if line > lines {
            return Err(format!("{} has {lines} lines", file.display()));
        }
        self.sync_editor_breakpoints();
        let now = {
            let mut store = breakpoints::store();
            let mut current = store.lines(file);
            if set {
                current.insert(line - 1);
            } else {
                current.remove(&(line - 1));
            }
            store.set_lines(file, current.iter().copied());
            current
        };
        breakpoints::flush();
        let canonical = dunce::canonicalize(file).ok();
        for panel in self.layout_manager.iter_all_panels_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                let same = editor.file_path().is_some_and(|p| {
                    p == file || canonical.is_some() && dunce::canonicalize(p).ok() == canonical
                });
                if same {
                    editor.reload_breakpoints();
                }
            }
        }
        let listed: Vec<String> = now.iter().map(|l| (l + 1).to_string()).collect();
        Ok(format!(
            "Breakpoints in {}: {}",
            file.display(),
            if listed.is_empty() {
                "none".to_string()
            } else {
                listed.join(", ")
            }
        ))
    }
}
