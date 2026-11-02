//! The agent's terminal sessions, as the tools see them: shells that stay
//! open between calls, each by a name of the agent's choosing, and owned by
//! the agent session that opened them — a subagent has its own. The app
//! keeps them (as terminals the user can open as panels) and ends them with
//! their agent session; `shell` with `session` or `background` runs
//! commands in them, and `terminals` lists, reads, interrupts and closes
//! them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::CancelToken;

/// What a command run in a session asks for.
pub struct TerminalRun<'a> {
    /// The agent session the terminal belongs to.
    pub owner: &'a str,
    /// The terminal's name; a new one opens in `cwd`.
    pub name: &'a str,
    pub cwd: &'a Path,
    pub command: &'a str,
    /// How long to wait for the command; it runs on past it.
    pub timeout: Duration,
    /// The caller gives up: the command is sent `Ctrl+C`.
    pub cancel: &'a CancelToken,
    /// A directory of command shims for the front of a new terminal's
    /// `PATH`, as a plain call gets.
    pub shim_path: Option<&'a Path>,
}

/// How waiting for a command in a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalRunEnd {
    /// The command is over, with its exit code when the shell gave one.
    Finished(Option<i32>),
    /// The time to wait was up; the command runs on.
    TimedOut,
    /// The wait was given up and the command sent `Ctrl+C`.
    Cancelled,
    /// The shell exited; the terminal is gone.
    ShellExited,
}

/// What a command run in a session came to, besides its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalRunOutcome {
    pub end: TerminalRunEnd,
    /// Output bytes that were no longer in memory when read.
    pub dropped: u64,
    /// The session's complete output log.
    pub log: Option<PathBuf>,
    /// The terminal was opened for this command.
    pub opened: bool,
}

/// What a terminal is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalState {
    Idle,
    /// A command runs, for `elapsed` so far.
    Running {
        command: String,
        elapsed: Duration,
    },
    Exited,
}

/// One of an agent session's terminals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalEntry {
    pub name: String,
    pub state: TerminalState,
    /// How long it has been open.
    pub age: Duration,
    pub log: Option<PathBuf>,
}

/// Output of a terminal the agent had not read yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalOutput {
    pub data: Vec<u8>,
    /// Bytes of it no longer in memory.
    pub dropped: u64,
    pub state: TerminalState,
    pub log: Option<PathBuf>,
}

/// Says from a terminal's raw output whether it is what was waited for.
pub type OutputTest = dyn Fn(&[u8]) -> bool + Sync;

/// Input for a terminal, and what to wait for after it.
pub struct TerminalSend<'a> {
    pub owner: &'a str,
    pub name: &'a str,
    /// Typed as it is, a line break as `Enter`.
    pub input: &'a str,
    /// Keys pressed after the input, by name (`Enter`, `Ctrl+C`, `Up`…).
    pub keys: &'a [String],
    /// Says from the raw output since the input whether it is what was
    /// waited for.
    pub until: Option<&'a OutputTest>,
    /// Output that stops for this long ends the wait.
    pub quiet: Option<Duration>,
    pub timeout: Duration,
    pub cancel: &'a CancelToken,
}

/// How the wait after a [`TerminalSend`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSendEnd {
    Matched,
    /// The shell finished the command the input started, with its exit code.
    Finished(Option<i32>),
    Quiet,
    TimedOut,
    Cancelled,
    ShellExited,
}

/// A terminal's screen as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalScreenText {
    pub text: String,
    /// A full-screen program has the alternate screen up.
    pub alternate: bool,
    /// The cursor's row and column, from 0.
    pub cursor: (usize, usize),
}

/// What a [`TerminalSend`] got back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSendOutcome {
    pub end: TerminalSendEnd,
    /// What the terminal printed after the input; it counts as read.
    pub output: TerminalOutput,
    /// The screen, when a full-screen program has it: its output is no
    /// stream to read.
    pub screen: Option<TerminalScreenText>,
}

/// The terminals the app keeps for agent sessions. Each method takes the
/// owner's session id; a name is only ever looked up among the owner's.
pub trait TerminalSessions: Send + Sync {
    /// Run a command in the owner's terminal `name`, opening it if there is
    /// none, and pass its output on as it comes. `Err` says why it did not
    /// run: an earlier command still runs, the owner has as many terminals
    /// as it may.
    fn run(
        &self,
        run: &TerminalRun<'_>,
        on_output: &mut dyn FnMut(&[u8]),
    ) -> Result<TerminalRunOutcome, String>;

    /// A name no terminal of the owner's has, starting with `stem`.
    fn free_name(&self, owner: &str, stem: &str) -> String;

    /// The owner's terminals, in the order they were opened.
    fn list(&self, owner: &str) -> Vec<TerminalEntry>;

    /// What terminal `name` printed since the agent last read it — a
    /// command's output counts as read.
    fn read(&self, owner: &str, name: &str) -> Result<TerminalOutput, String>;

    /// Type into terminal `name` and wait for its answer.
    fn send(&self, send: &TerminalSend<'_>) -> Result<TerminalSendOutcome, String>;

    /// Terminal `name`'s screen as it stands.
    fn screen(&self, owner: &str, name: &str) -> Result<TerminalScreenText, String>;

    /// Send `Ctrl+C` to terminal `name`.
    fn interrupt(&self, owner: &str, name: &str) -> Result<(), String>;

    /// End terminal `name` and whatever runs in it.
    fn close(&self, owner: &str, name: &str) -> Result<(), String>;

    /// End every terminal of `owner`: its agent session is over, or no
    /// longer open anywhere. Returns at once; the shells end on their own.
    fn release(&self, owner: &str);
}

/// The [`ContextNote`](crate::ContextNote) that tells the agent, at each
/// model call, which of its terminal sessions are open.
pub fn context_note(terminals: std::sync::Arc<dyn TerminalSessions>) -> crate::ContextNote {
    std::sync::Arc::new(move |owner| open_terminals_note(&terminals.list(owner?)))
}

/// One line per open terminal, for the note the agent gets with each model
/// call while it has any; `None` when it has none.
#[must_use]
pub fn open_terminals_note(entries: &[TerminalEntry]) -> Option<String> {
    let open: Vec<String> = entries
        .iter()
        .filter(|entry| entry.state != TerminalState::Exited)
        .map(|entry| match &entry.state {
            TerminalState::Running { command, elapsed } => format!(
                "- {}: running `{}` for {}",
                entry.name,
                one_line(command),
                duration(*elapsed)
            ),
            _ => format!("- {}: idle, open {}", entry.name, duration(entry.age)),
        })
        .collect();
    if open.is_empty() {
        return None;
    }
    Some(format!(
        "Your open terminal sessions (close those you no longer need with `terminals`):\n{}",
        open.join("\n")
    ))
}

/// A command's first line, cut to a readable length.
fn one_line(command: &str) -> String {
    let line = command.lines().next().unwrap_or_default();
    let mut short: String = line.chars().take(60).collect();
    if short.len() < line.len() || command.lines().nth(1).is_some() {
        short.push('…');
    }
    short
}

/// `elapsed` the way a person says it: `12 s`, `4 min`, `2 h 5 min`.
#[must_use]
pub fn duration(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", secs / 60),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, state: TerminalState) -> TerminalEntry {
        TerminalEntry {
            name: name.into(),
            state,
            age: Duration::from_secs(600),
            log: None,
        }
    }

    #[test]
    fn the_note_lists_what_is_open_and_nothing_when_nothing_is() {
        assert_eq!(open_terminals_note(&[]), None);
        assert_eq!(
            open_terminals_note(&[entry("old", TerminalState::Exited)]),
            None
        );
        let note = open_terminals_note(&[
            entry(
                "dev",
                TerminalState::Running {
                    command: "npm run dev\necho done".into(),
                    elapsed: Duration::from_secs(3720),
                },
            ),
            entry("tests", TerminalState::Idle),
        ])
        .unwrap();
        assert!(note
            .ends_with("- dev: running `npm run dev…` for 1 h 2 min\n- tests: idle, open 10 min"));
    }
}
