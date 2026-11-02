//! The terminals of the agent sessions: the shells `shell` opens for
//! `session` and `background`, kept here by their owner — the session id of
//! the agent that opened them — and name. A terminal ends with its owner:
//! when the session is no longer open in a panel, when a subagent's task is
//! over, or when the agent closes it. Each is an ordinary terminal, held
//! here while it is out of the layout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock};
use std::time::Instant;

use termide_agent_core::terminals::duration;
use termide_agent_core::{
    TerminalEntry, TerminalOutput, TerminalRun, TerminalRunEnd, TerminalRunOutcome,
    TerminalScreenText, TerminalSend, TerminalSendEnd, TerminalSendOutcome, TerminalSessions,
    TerminalState,
};

use termide_agent_tools::ShellProgram;
use termide_panel_terminal::agent_shell::{
    AgentShell, RunEnd, RunError, ScreenText, SendEnd, SendWait, ShellStatus,
};
use termide_panel_terminal::Terminal;

/// Where an agent's terminal is, for a panel to show it.
pub(crate) enum Expand {
    /// Here, taken out for the panel.
    Here(Box<Terminal>),
    /// In a panel already: the one whose shell has this pid.
    Shown(Option<u32>),
    /// The session has no terminal by that name (any more).
    Missing,
}

/// A new terminal's size, until the user opens it as a panel.
const ROWS: u16 = 40;
const COLS: u16 = 160;

struct Entry {
    name: String,
    shell: AgentShell,
    /// The terminal, while it is out of the layout.
    terminal: Option<Terminal>,
    opened: Instant,
    /// How far the agent has read the output.
    read_to: u64,
}

impl Entry {
    fn state(&self) -> TerminalState {
        match self.shell.status() {
            ShellStatus::Idle => TerminalState::Idle,
            ShellStatus::Running { command, since } => TerminalState::Running {
                command,
                elapsed: since.elapsed(),
            },
            ShellStatus::Exited => TerminalState::Exited,
        }
    }

    fn info(&self) -> TerminalEntry {
        TerminalEntry {
            name: self.name.clone(),
            state: self.state(),
            age: self.opened.elapsed(),
            log: self.shell.tap().log_path(),
        }
    }
}

/// The registry.
pub(crate) struct AgentTerminals {
    owners: Mutex<HashMap<String, Vec<Entry>>>,
    limit: AtomicUsize,
    /// The shell the agent runs its commands in, here and apart.
    shell: RwLock<ShellProgram>,
    /// Where the output logs go, by owner; the system temp dir when none.
    log_root: Option<PathBuf>,
}

/// The one registry of the process.
pub(crate) fn agent_terminals() -> &'static Arc<AgentTerminals> {
    static REGISTRY: OnceLock<Arc<AgentTerminals>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let log_root = termide_config::get_config_dir().ok().map(|dir| {
            dir.join(termide_agent_core::GLOBAL_AGENT_DIR)
                .join("terminals")
        });
        Arc::new(AgentTerminals::new(log_root))
    })
}

/// End `shell` and drop `terminal` off the calling thread: ending a shell
/// waits for it.
fn end_off_thread(shell: AgentShell, terminal: Option<Terminal>) {
    let spawned = std::thread::Builder::new()
        .name("agent-terminal-end".into())
        .spawn(move || {
            shell.terminate();
            drop(terminal);
        });
    if let Err(error) = spawned {
        log::warn!("cannot end an agent terminal: {error}");
    }
}

impl AgentTerminals {
    pub(crate) fn new(log_root: Option<PathBuf>) -> Self {
        Self {
            owners: Mutex::new(HashMap::new()),
            limit: AtomicUsize::new(termide_config::AiSettings::default().max_terminal_sessions),
            shell: RwLock::new(ShellProgram::bash()),
            log_root,
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Vec<Entry>>> {
        self.owners.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many terminals an agent session may have open at once.
    pub(crate) fn set_limit(&self, limit: usize) {
        self.limit.store(limit.max(1), Ordering::Relaxed);
    }

    /// Run the agent's commands in the shell picked for terminals —
    /// `configured` (`terminal.default_shell`), else the one found on the
    /// system — when it is bash, zsh or fish, and in bash otherwise. A
    /// terminal already open keeps its shell.
    pub(crate) fn set_shell(&self, configured: Option<&str>) {
        let program = configured
            .map(str::to_string)
            .unwrap_or_else(termide_panel_terminal::shell_utils::detect_shell);
        let shell = ShellProgram::new(&program).unwrap_or_else(ShellProgram::bash);
        *self.shell.write().unwrap_or_else(PoisonError::into_inner) = shell;
    }

    /// The shell the agent runs its commands in.
    pub(crate) fn shell(&self) -> ShellProgram {
        self.shell
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn log_path(&self, owner: &str, name: &str) -> PathBuf {
        let root = self
            .log_root
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("termide-agent-terminals"));
        let safe_owner: String = owner
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            .collect();
        root.join(safe_owner)
            .join(format!("{name}-{}.log", termide_agent_core::now_millis()))
    }

    /// The owner's terminal `name`, opened in `cwd` when there is none.
    /// Whether it was opened comes back with it.
    fn shell_for(
        &self,
        owner: &str,
        name: &str,
        cwd: &Path,
        shim_path: Option<&Path>,
    ) -> Result<(AgentShell, bool), String> {
        let mut owners = self.lock();
        let entries = owners.entry(owner.to_string()).or_default();
        // A shell that exited is gone: its name is free again.
        if let Some(index) = entries
            .iter()
            .position(|entry| entry.name == name && !entry.shell.is_alive())
        {
            let entry = entries.remove(index);
            end_off_thread(entry.shell, entry.terminal);
        }
        if let Some(entry) = entries.iter().find(|entry| entry.name == name) {
            return Ok((entry.shell.clone(), false));
        }
        let limit = self.limit.load(Ordering::Relaxed);
        if entries.len() >= limit {
            let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
            return Err(format!(
                "you have {} terminal sessions open, as many as you may ({}); close one with \
                 `terminals` first, or run the command in one of them",
                entries.len(),
                names.join(", ")
            ));
        }
        let mut env = Vec::new();
        if let Some(dir) = shim_path {
            let base = std::env::var_os("PATH").unwrap_or_default();
            let paths = std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&base));
            if let Ok(joined) = std::env::join_paths(paths) {
                env.push(("PATH".to_string(), joined.to_string_lossy().into_owned()));
            }
        }
        let log = self.log_path(owner, name);
        let program = self.shell().program;
        let (terminal, shell) =
            Terminal::new_agent_shell(ROWS, COLS, &program, cwd, &env, Some(&log), name)
                .map_err(|error| format!("cannot open a terminal session: {error}"))?;
        entries.push(Entry {
            name: name.to_string(),
            shell: shell.clone(),
            terminal: Some(terminal),
            opened: Instant::now(),
            read_to: 0,
        });
        Ok((shell, true))
    }

    /// Run `f` on the owner's entry `name`.
    fn with_entry<T>(
        &self,
        owner: &str,
        name: &str,
        f: impl FnOnce(&mut Entry) -> T,
    ) -> Result<T, String> {
        let mut owners = self.lock();
        owners
            .get_mut(owner)
            .and_then(|entries| entries.iter_mut().find(|entry| entry.name == name))
            .map(f)
            .ok_or_else(|| {
                format!("you have no terminal session `{name}`; `terminals` lists yours")
            })
    }

    /// Take the terminal of `owner`'s session `name` out, to show it in a
    /// panel.
    pub(crate) fn expand(&self, owner: &str, name: &str) -> Expand {
        let mut owners = self.lock();
        let Some(entry) = owners
            .get_mut(owner)
            .and_then(|entries| entries.iter_mut().find(|entry| entry.name == name))
        else {
            return Expand::Missing;
        };
        match entry.terminal.take() {
            Some(terminal) => Expand::Here(Box::new(terminal)),
            None => Expand::Shown(entry.shell.pid()),
        }
    }

    /// Take back a terminal shown in a panel that closes; it runs on. One
    /// whose session has ended meanwhile comes back, to be dropped.
    pub(crate) fn collapse(&self, terminal: Terminal) -> Option<Terminal> {
        let pid = terminal.child_pid()?;
        let mut owners = self.lock();
        let entry = owners
            .values_mut()
            .flatten()
            .find(|entry| entry.terminal.is_none() && entry.shell.pid() == Some(pid));
        match entry {
            Some(entry) => {
                entry.terminal = Some(terminal);
                None
            }
            None => Some(terminal),
        }
    }

    /// End every terminal and wait for them to go, all at once: termide
    /// quits next, and a thread left to end one would die with it.
    pub(crate) fn release_all(&self) {
        let entries: Vec<Entry> = self
            .lock()
            .drain()
            .flat_map(|(_, entries)| entries)
            .collect();
        std::thread::scope(|scope| {
            for entry in entries {
                scope.spawn(move || {
                    entry.shell.terminate();
                    drop(entry.terminal);
                });
            }
        });
    }
}

fn screen_text(screen: ScreenText) -> TerminalScreenText {
    TerminalScreenText {
        text: screen.text,
        alternate: screen.alternate,
        cursor: screen.cursor,
    }
}

impl TerminalSessions for AgentTerminals {
    fn run(
        &self,
        run: &TerminalRun<'_>,
        on_output: &mut dyn FnMut(&[u8]),
    ) -> Result<TerminalRunOutcome, String> {
        let (shell, opened) = self.shell_for(run.owner, run.name, run.cwd, run.shim_path)?;
        let result = shell.run(
            run.command,
            run.timeout,
            &|| run.cancel.is_cancelled(),
            on_output,
        );
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(RunError::Busy { command, since }) => {
                return Err(format!(
                    "terminal session `{}` still runs `{}` (for {}): read, interrupt or close it \
                     with `terminals`, or use another session",
                    run.name,
                    termide_agent_tools::jobs_text::first_line(&command, 80),
                    duration(since.elapsed())
                ))
            }
            Err(RunError::Exited) => {
                return Err(format!(
                    "the shell of terminal session `{}` has exited; run the command again to \
                     open a new one",
                    run.name
                ))
            }
            Err(RunError::Input(error)) => {
                return Err(format!(
                    "cannot type into terminal session `{}`: {error}",
                    run.name
                ))
            }
        };
        // What the command printed is read.
        let _ = self.with_entry(run.owner, run.name, |entry| {
            entry.read_to = shell.tap().end();
        });
        let end = match outcome.end {
            RunEnd::Finished(code) => TerminalRunEnd::Finished(code),
            RunEnd::TimedOut => TerminalRunEnd::TimedOut,
            RunEnd::Cancelled => TerminalRunEnd::Cancelled,
            RunEnd::ShellExited => TerminalRunEnd::ShellExited,
        };
        Ok(TerminalRunOutcome {
            end,
            dropped: outcome.dropped,
            log: shell.tap().log_path(),
            opened,
        })
    }

    fn free_name(&self, owner: &str, stem: &str) -> String {
        let owners = self.lock();
        let taken = |name: &str| {
            owners
                .get(owner)
                .is_some_and(|entries| entries.iter().any(|entry| entry.name == name))
        };
        (1..)
            .map(|n| format!("{stem}-{n}"))
            .find(|name| !taken(name))
            .unwrap_or_else(|| stem.to_string())
    }

    fn list(&self, owner: &str) -> Vec<TerminalEntry> {
        self.lock()
            .get(owner)
            .map(|entries| entries.iter().map(Entry::info).collect())
            .unwrap_or_default()
    }

    fn read(&self, owner: &str, name: &str) -> Result<TerminalOutput, String> {
        self.with_entry(owner, name, |entry| {
            let chunk = entry.shell.tap().read_since(entry.read_to);
            let dropped = chunk.from - entry.read_to;
            entry.read_to = chunk.end;
            TerminalOutput {
                data: chunk.data,
                dropped,
                state: entry.state(),
                log: entry.shell.tap().log_path(),
            }
        })
    }

    fn send(&self, send: &TerminalSend<'_>) -> Result<TerminalSendOutcome, String> {
        let shell = self.with_entry(send.owner, send.name, |entry| entry.shell.clone())?;
        let until = send.until.map(|until| until as &dyn Fn(&[u8]) -> bool);
        let wait = SendWait {
            until,
            quiet: send.quiet,
            timeout: send.timeout,
        };
        let outcome = shell
            .send(send.input, send.keys, &wait, &|| send.cancel.is_cancelled())
            .map_err(|error| match error {
                RunError::Exited => format!(
                    "the shell of terminal session `{}` has exited; close it",
                    send.name
                ),
                RunError::Input(error) => {
                    format!("cannot type into terminal session `{}`: {error}", send.name)
                }
                RunError::Busy { .. } => unreachable!("input is never refused as busy"),
            })?;
        let chunk = shell.tap().read_since(outcome.from);
        let dropped = chunk.from - outcome.from;
        let state = self.with_entry(send.owner, send.name, |entry| {
            // What came after the input is read.
            entry.read_to = entry.read_to.max(chunk.end);
            entry.state()
        })?;
        let screen = shell.screen();
        let end = match outcome.end {
            SendEnd::Matched => TerminalSendEnd::Matched,
            SendEnd::Finished(code) => TerminalSendEnd::Finished(code),
            SendEnd::Quiet => TerminalSendEnd::Quiet,
            SendEnd::TimedOut => TerminalSendEnd::TimedOut,
            SendEnd::Cancelled => TerminalSendEnd::Cancelled,
            SendEnd::ShellExited => TerminalSendEnd::ShellExited,
        };
        Ok(TerminalSendOutcome {
            end,
            output: TerminalOutput {
                data: chunk.data,
                dropped,
                state,
                log: shell.tap().log_path(),
            },
            screen: screen.alternate.then(|| screen_text(screen)),
        })
    }

    fn screen(&self, owner: &str, name: &str) -> Result<TerminalScreenText, String> {
        let shell = self.with_entry(owner, name, |entry| entry.shell.clone())?;
        Ok(screen_text(shell.screen()))
    }

    fn interrupt(&self, owner: &str, name: &str) -> Result<(), String> {
        self.with_entry(owner, name, |entry| entry.shell.interrupt())?
    }

    fn close(&self, owner: &str, name: &str) -> Result<(), String> {
        let entry = {
            let mut owners = self.lock();
            let entries = owners.get_mut(owner);
            let index = entries
                .as_ref()
                .and_then(|entries| entries.iter().position(|entry| entry.name == name));
            match (entries, index) {
                (Some(entries), Some(index)) => entries.remove(index),
                _ => {
                    return Err(format!(
                        "you have no terminal session `{name}`; `terminals` lists yours"
                    ))
                }
            }
        };
        end_off_thread(entry.shell, entry.terminal);
        Ok(())
    }

    fn release(&self, owner: &str) {
        let entries = self.lock().remove(owner).unwrap_or_default();
        for entry in entries {
            end_off_thread(entry.shell, entry.terminal);
        }
    }
}

impl crate::app::App {
    /// Show the agent session `owner`'s terminal `name` in a panel: the one
    /// showing it comes to the front, else it opens in a new one.
    pub(in crate::app) fn open_agent_terminal(&mut self, owner: &str, name: &str) {
        match agent_terminals().expand(owner, name) {
            Expand::Here(terminal) => {
                self.add_panel(terminal);
            }
            Expand::Shown(pid) => {
                self.layout_manager.focus_panel_where(|panel| {
                    panel
                        .as_any()
                        .downcast_ref::<Terminal>()
                        .is_some_and(|terminal| {
                            terminal.is_agent_shell() && terminal.child_pid() == pid
                        })
                });
            }
            Expand::Missing => {}
        }
        self.state.needs_redraw = true;
    }

    /// Close the active panel when it shows an agent's terminal: the
    /// terminal goes back to the agent, still running. Whether it did.
    pub(in crate::app) fn collapse_agent_terminal(&mut self) -> bool {
        let shown = self.layout_manager.active_panel().is_some_and(|panel| {
            panel
                .as_any()
                .downcast_ref::<Terminal>()
                .is_some_and(Terminal::is_agent_shell)
        });
        let Some(position) = self
            .layout_manager
            .active_panel_position()
            .filter(|_| shown)
        else {
            return false;
        };
        let width = self.state.terminal.width;
        let Some(panel) = self.layout_manager.remove_panel_at(position, width) else {
            return false;
        };
        let panel: Box<dyn std::any::Any> = panel;
        if let Ok(terminal) = panel.downcast::<Terminal>() {
            // One whose session ended meanwhile is dropped here.
            drop(agent_terminals().collapse(*terminal));
        }
        self.state.needs_redraw = true;
        true
    }
}

/// Whether closing a panel ends what runs in it. An agent's terminal is only
/// shown in a panel — closing that one hands it back to the agent, still
/// running — so a close that kills the panel's processes must skip it.
pub(crate) fn ends_on_close(panel: &dyn termide_core::Panel) -> bool {
    !panel
        .as_any()
        .downcast_ref::<Terminal>()
        .is_some_and(Terminal::is_agent_shell)
}

impl crate::app::App {
    /// Take down the panels showing an agent's terminal whose terminal is
    /// gone: the agent closed it, its session ended, or the shell exited in
    /// the panel. The strip no longer lists the terminal, so its panel has
    /// nothing left to show; waiting for the panel to be focused leaves one
    /// of a dead terminal on screen that vanishes the moment it is chosen.
    /// Whether any came down comes back with it.
    pub(in crate::app) fn close_ended_agent_terminals(&mut self) -> bool {
        let ended = self.layout_manager.remove_panels_where(
            |panel| {
                panel
                    .as_any()
                    .downcast_ref::<Terminal>()
                    .is_some_and(|terminal| terminal.is_agent_shell() && !terminal.is_alive())
            },
            self.state.terminal.width,
        );
        if ended.is_empty() {
            return false;
        }
        for panel in ended {
            let panel: Box<dyn std::any::Any> = panel;
            if let Ok(terminal) = panel.downcast::<Terminal>() {
                // One whose session ended meanwhile is dropped here.
                drop(agent_terminals().collapse(*terminal));
            }
        }
        self.state.needs_redraw = true;
        true
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;
    use termide_agent_core::CancelToken;

    fn run(
        registry: &AgentTerminals,
        owner: &str,
        name: &str,
        cwd: &Path,
        command: &str,
    ) -> Result<(String, TerminalRunOutcome), String> {
        let cancel = CancelToken::new();
        let mut out = Vec::new();
        let outcome = registry.run(
            &TerminalRun {
                owner,
                name,
                cwd,
                command,
                timeout: Duration::from_secs(20),
                cancel: &cancel,
                shim_path: None,
            },
            &mut |bytes| out.extend_from_slice(bytes),
        )?;
        Ok((String::from_utf8_lossy(&out).replace("\r\n", "\n"), outcome))
    }

    #[test]
    fn terminals_are_kept_by_owner_and_name_up_to_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let registry = AgentTerminals::new(Some(dir.path().join("logs")));
        registry.set_limit(2);
        let Ok((_, first)) = run(&registry, "a", "x", dir.path(), "V=1") else {
            return; // No bash or no PTY here.
        };
        assert!(first.opened);
        let (out, again) = run(&registry, "a", "x", dir.path(), "echo $V").unwrap();
        assert_eq!((out.as_str(), again.opened), ("1\n", false));
        // Another owner's `x` is another shell.
        let (out, _) = run(&registry, "b", "x", dir.path(), "echo \"[$V]\"").unwrap();
        assert_eq!(out, "[]\n");
        run(&registry, "a", "y", dir.path(), "true").unwrap();
        let refused = run(&registry, "a", "z", dir.path(), "true").unwrap_err();
        assert!(refused.contains("(x, y)"), "{refused}");
        assert_eq!(registry.free_name("a", "x"), "x-1");

        // Output since the last read only; a command's own counts as read.
        run(&registry, "a", "y", dir.path(), "(sleep 0.3; echo later) &").unwrap();
        std::thread::sleep(Duration::from_millis(800));
        let read = registry.read("a", "y").unwrap();
        assert!(String::from_utf8_lossy(&read.data).contains("later"));
        assert!(registry.read("a", "y").unwrap().data.is_empty());
        assert!(read.log.is_some_and(|log| log.starts_with(dir.path())));

        registry.close("a", "y").unwrap();
        assert!(registry.close("a", "y").is_err());
        registry.release("a");
        assert!(registry.list("a").is_empty());
        assert_eq!(registry.list("b").len(), 1);
        registry.release_all();
        assert!(registry.list("b").is_empty());
    }

    /// Closing the panel that shows an agent's terminal hands it back: the
    /// shell is not the panel's to end. A killed one could not be shown
    /// again — its panel would open and auto-close at once.
    #[test]
    fn closing_the_panel_of_an_agent_terminal_neither_ends_it_nor_hides_it() {
        let dir = tempfile::tempdir().unwrap();
        let registry = AgentTerminals::new(None);
        let Ok(_) = run(&registry, "a", "x", dir.path(), "X=7") else {
            return; // No bash or no PTY here.
        };
        let Expand::Here(terminal) = registry.expand("a", "x") else {
            panic!("the terminal is here");
        };
        assert!(
            !ends_on_close(&*terminal),
            "the confirm dialog must not kill an agent's shell"
        );
        assert!(registry.collapse(*terminal).is_none(), "handed back");
        // It answers, and a second click shows it again.
        let cancel = CancelToken::new();
        let until = |raw: &[u8]| String::from_utf8_lossy(raw).contains("got-7");
        let sent = registry
            .send(&TerminalSend {
                owner: "a",
                name: "x",
                input: "echo got-$((3+4))\r",
                keys: &[],
                until: Some(&until),
                quiet: None,
                timeout: Duration::from_secs(20),
                cancel: &cancel,
            })
            .unwrap();
        assert_eq!(sent.end, TerminalSendEnd::Matched);
        assert!(matches!(registry.expand("a", "x"), Expand::Here(_)));
    }

    #[test]
    fn a_terminal_shown_in_a_panel_comes_back_running() {
        let dir = tempfile::tempdir().unwrap();
        let registry = AgentTerminals::new(None);
        let Ok(_) = run(&registry, "a", "x", dir.path(), "X=7") else {
            return; // No bash or no PTY here.
        };
        let Expand::Here(terminal) = registry.expand("a", "x") else {
            panic!("the terminal is here");
        };
        assert!(terminal.is_agent_shell());
        let pid = terminal.child_pid();
        assert!(matches!(registry.expand("a", "x"), Expand::Shown(shown) if shown == pid));
        assert!(matches!(registry.expand("a", "y"), Expand::Missing));
        // Typed into, it answers; what it answered counts as read.
        let cancel = CancelToken::new();
        let until = |raw: &[u8]| String::from_utf8_lossy(raw).contains("got-7");
        let sent = registry
            .send(&TerminalSend {
                owner: "a",
                name: "x",
                input: "echo got-$((3+4))\n",
                keys: &[],
                until: Some(&until),
                quiet: None,
                timeout: Duration::from_secs(20),
                cancel: &cancel,
            })
            .unwrap();
        assert_eq!(sent.end, TerminalSendEnd::Matched);
        assert!(String::from_utf8_lossy(&sent.output.data).contains("got-7"));
        assert!(sent.screen.is_none());
        let screen = registry.screen("a", "x").unwrap();
        assert!(screen.text.contains("got-7"), "{}", screen.text);
        let unread = registry.read("a", "x").unwrap();
        assert!(!String::from_utf8_lossy(&unread.data).contains("got-7"));
        // The agent goes on with it while it is shown.
        let (out, _) = run(&registry, "a", "x", dir.path(), "echo $X").unwrap();
        assert_eq!(out, "7\n");
        assert!(registry.collapse(*terminal).is_none());
        assert!(matches!(registry.expand("a", "x"), Expand::Here(_)));
        registry.release_all();
    }
}
