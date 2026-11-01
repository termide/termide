//! The terminals of the agent sessions: the shells `shell` opens for
//! `session` and `background`, kept here by their owner — the session id of
//! the agent that opened them — and name. A terminal ends with its owner:
//! when the session is no longer open in a panel, when a subagent's task is
//! over, or when the agent closes it. Each is an ordinary terminal, held
//! here while it is out of the layout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Instant;

use termide_agent_core::terminals::duration;
use termide_agent_core::{
    TerminalEntry, TerminalOutput, TerminalRun, TerminalRunEnd, TerminalRunOutcome,
    TerminalSessions, TerminalState,
};
use termide_panel_terminal::agent_shell::{AgentShell, RunEnd, RunError, ShellStatus};
use termide_panel_terminal::Terminal;

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
        let (terminal, shell) = Terminal::new_agent_shell(ROWS, COLS, cwd, &env, Some(&log), name)
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

    /// End every terminal of every owner: termide is quitting.
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
}
