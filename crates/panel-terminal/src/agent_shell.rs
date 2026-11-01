//! The agent's shell session: an interactive bash in a terminal of its own,
//! which the agent runs commands in one after another — the directory, the
//! variables and what runs in the background carry over from one to the
//! next — and the user can watch, or type into, as a terminal panel.
//!
//! The terminal is an ordinary [`Terminal`], kept out of the layout until
//! the user opens it; [`AgentShell`] is the agent's handle on it, usable
//! from any thread. Where a command's output starts and ends comes from the
//! shell itself: an init script, run after the user's `~/.bashrc`, marks
//! both with OSC 133 `C` and `D`, which the screen ignores and the output
//! tap notes (see [`OutputTap`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use portable_pty::CommandBuilder;

use crate::pty_input::InputSender;
use crate::tap::{MarkKind, OutputTap};
use crate::{SharedChild, Terminal};

/// Output kept in memory for the agent to read; the log on disk has all.
const KEEP_BYTES: usize = 2 * 1024 * 1024;
/// Cap on a session's output log.
const MAX_LOG_BYTES: u64 = 256 * 1024 * 1024;
/// How long the output is waited for between checks of cancel and timeout.
const POLL: Duration = Duration::from_millis(50);
/// How long the shell has to exit after `SIGHUP` before it is killed.
const HANGUP_GRACE: Duration = Duration::from_secs(2);

/// Run after the user's `~/.bashrc`. Job control is off so that every
/// command stays in the shell's process group, which ending the session
/// signals — turned off again at each prompt, as bash turns it on for an
/// interactive shell only after the startup files; history expansion is off so that a `!` in a command is taken as
/// typed; pagers and editors that would wait for keys nobody presses are
/// turned off. The `DEBUG` trap marks the start of the first command after
/// a prompt (bash 3.2, the one macOS ships, has no `PS0`), and the first
/// entry of `PROMPT_COMMAND` its end, with its exit code.
const INIT_SCRIPT: &str = r#"# termide: the agent's shell session.
if [ -f ~/.bashrc ]; then . ~/.bashrc; fi
set +m +H
unset HISTFILE
export PAGER=cat GIT_PAGER=cat MANPAGER=cat SYSTEMD_PAGER= GIT_EDITOR=true
__termide_ready=
__termide_ran=
__termide_preexec() {
  [ -n "$__termide_ready" ] || return 0
  [ "$BASH_COMMAND" = __termide_precmd ] && return 0
  __termide_ready=
  __termide_ran=1
  printf '\033]133;C\007'
}
__termide_precmd() {
  local code=$?
  __termide_ready=
  set +m
  if [ -n "$__termide_ran" ]; then printf '\033]133;D;%s\007' "$code"; fi
  __termide_ran=
}
trap '__termide_preexec' DEBUG
if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
  PROMPT_COMMAND=(__termide_precmd "${PROMPT_COMMAND[@]}" __termide_ready=1)
else
  PROMPT_COMMAND="__termide_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND};__termide_ready=1"
fi
"#;

/// The init script's file, written once per process.
fn init_script() -> std::io::Result<&'static Path> {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    if let Some(path) = PATH.get() {
        return Ok(path);
    }
    let path =
        std::env::temp_dir().join(format!("termide-agent-shell-{}.bash", std::process::id()));
    std::fs::write(&path, INIT_SCRIPT)?;
    Ok(PATH.get_or_init(|| path))
}

/// The line typed into the shell for `command`: always `eval $'…'`, a
/// simple command, so that the shell reads it as one command with one start
/// and one end — the start mark comes from the `DEBUG` trap, which bash runs
/// before a simple command and not before a subshell such as `( … ) &` —
/// and a line break or another control character in it is typed safely.
fn typed_line(command: &str) -> String {
    let command = command.trim_end_matches(['\n', '\r']);
    let mut line = String::from("eval $'");
    for c in command.chars() {
        match c {
            '\\' => line.push_str("\\\\"),
            '\'' => line.push_str("\\'"),
            '\n' => line.push_str("\\n"),
            '\t' => line.push_str("\\t"),
            c if c.is_control() && (c as u32) < 0x100 => {
                line.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => line.push(c),
        }
    }
    line.push_str("'\n");
    line
}

/// How a [`AgentShell::run`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEnd {
    /// The command is over; its exit code, when the shell gave one.
    Finished(Option<i32>),
    /// The time was up; the command runs on.
    TimedOut,
    /// The caller gave up; the command was sent `Ctrl+C`.
    Cancelled,
    /// The shell itself exited.
    ShellExited,
}

/// What a [`AgentShell::run`] produced, besides the output it streamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOutcome {
    pub end: RunEnd,
    /// Output bytes no longer in memory when they were read; the log has
    /// them.
    pub dropped: u64,
}

/// Why a command was not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// An earlier command still runs: the one given, since then.
    Busy { command: String, since: Instant },
    /// The shell has exited.
    Exited,
    /// The command could not be typed.
    Input(String),
}

/// What the session is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellStatus {
    Idle,
    Running { command: String, since: Instant },
    Exited,
}

/// A command sent and not yet seen to end.
struct InFlight {
    command: String,
    since: Instant,
    /// The output offset when it was sent.
    sent: u64,
}

/// The agent's handle on its shell session; cheap to clone, usable from
/// any thread. The terminal itself goes to the UI.
#[derive(Clone)]
pub struct AgentShell {
    input: InputSender,
    tap: Arc<OutputTap>,
    child: SharedChild,
    pid: Option<u32>,
    /// One command at a time: a caller waits here for the one before.
    turn: Arc<Mutex<()>>,
    in_flight: Arc<Mutex<Option<InFlight>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Terminal {
    /// An agent's shell session in `cwd`: bash, run with the init script,
    /// with `env` added (a `PATH` with the command shims first, say), its
    /// output logged to `log`. The terminal is titled `title`.
    pub fn new_agent_shell(
        rows: u16,
        cols: u16,
        cwd: &Path,
        env: &[(String, String)],
        log: Option<&Path>,
        title: &str,
    ) -> anyhow::Result<(Self, AgentShell)> {
        let mut cmd = CommandBuilder::new("bash");
        cmd.arg("--rcfile");
        cmd.arg(init_script()?);
        cmd.arg("-i");
        let mut env = env.to_vec();
        env.push(("BASH_SILENCE_DEPRECATION_WARNING".into(), "1".into()));
        env.push(("TERMIDE_AGENT_SHELL".into(), "1".into()));
        let tap = Arc::new(OutputTap::new(
            KEEP_BYTES,
            log.map(|path| (path, MAX_LOG_BYTES)),
        ));
        let (mut term, _) = Self::spawn_command(
            rows,
            cols,
            cmd,
            Some(cwd.to_path_buf()),
            &env,
            title.to_string(),
            Some(Arc::clone(&tap)),
        )?;
        term.shell_kind = crate::shell_utils::ShellKind::from_shell_path("bash");
        term.shell_name = "bash".into();
        term.agent_shell = true;
        let shell = AgentShell {
            input: term.input.sender(),
            tap,
            child: Arc::clone(&term.child),
            pid: term.shell_pid,
            turn: Arc::new(Mutex::new(())),
            in_flight: Arc::new(Mutex::new(None)),
        };
        Ok((term, shell))
    }
}

impl AgentShell {
    /// The session's output, for reading it apart from a command.
    pub fn tap(&self) -> &Arc<OutputTap> {
        &self.tap
    }

    /// The shell's pid, which its terminal has as well.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Whether the shell still runs.
    pub fn is_alive(&self) -> bool {
        !self.tap.is_closed()
    }

    /// The command sent last, if it still runs. Clears it once it ended.
    fn running(&self) -> Option<(String, Instant)> {
        let mut in_flight = lock(&self.in_flight);
        let flight = in_flight.as_ref()?;
        let ended = self.tap.is_closed() || ended_after(&self.tap, flight.sent).is_some();
        if ended {
            *in_flight = None;
            return None;
        }
        Some((flight.command.clone(), flight.since))
    }

    /// What the session is doing.
    pub fn status(&self) -> ShellStatus {
        match self.running() {
            Some((command, since)) => ShellStatus::Running { command, since },
            None if self.tap.is_closed() => ShellStatus::Exited,
            None => ShellStatus::Idle,
        }
    }

    /// Run `command` and pass its output to `on_output` as it comes, until
    /// it ends, `timeout` passes (it then runs on) or `cancelled` says so
    /// (it is then sent `Ctrl+C`). A caller while another runs waits for it;
    /// a command still running from an earlier call is refused.
    pub fn run(
        &self,
        command: &str,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
        on_output: &mut dyn FnMut(&[u8]),
    ) -> Result<RunOutcome, RunError> {
        let _turn = lock(&self.turn);
        if let Some((command, since)) = self.running() {
            return Err(RunError::Busy { command, since });
        }
        if self.tap.is_closed() {
            return Err(RunError::Exited);
        }
        let sent = self.tap.end();
        self.input
            .send(typed_line(command).into_bytes())
            .map_err(|e| RunError::Input(e.to_string()))?;
        let since = Instant::now();
        *lock(&self.in_flight) = Some(InFlight {
            command: command.to_string(),
            since,
            sent,
        });
        let deadline = since + timeout;
        let mut read_to: Option<u64> = None;
        let mut dropped = 0;
        loop {
            let seen = self.tap.end();
            let marks = self.tap.marks_since(sent);
            let start = marks.iter().find(|m| m.kind == MarkKind::Start);
            let end = start.and_then(|start| {
                marks
                    .iter()
                    .find(|m| m.at >= start.after && matches!(m.kind, MarkKind::End(_)))
            });
            if let Some(start) = start {
                let from = read_to.unwrap_or(start.after);
                let chunk = self.tap.read_since(from);
                let limit = end.map_or(chunk.end, |end| end.at);
                dropped += chunk.from - from;
                if limit > chunk.from {
                    let len = usize::try_from(limit - chunk.from).unwrap_or(usize::MAX);
                    on_output(&chunk.data[..len.min(chunk.data.len())]);
                }
                read_to = Some(limit.max(chunk.from));
            }
            if let Some(end) = end {
                *lock(&self.in_flight) = None;
                let MarkKind::End(code) = end.kind else {
                    unreachable!("found as an end")
                };
                return Ok(RunOutcome {
                    end: RunEnd::Finished(code),
                    dropped,
                });
            }
            if self.tap.is_closed() && self.tap.end() == seen {
                *lock(&self.in_flight) = None;
                return Ok(RunOutcome {
                    end: RunEnd::ShellExited,
                    dropped,
                });
            }
            if cancelled() {
                let _ = self.interrupt();
                return Ok(RunOutcome {
                    end: RunEnd::Cancelled,
                    dropped,
                });
            }
            if Instant::now() >= deadline {
                return Ok(RunOutcome {
                    end: RunEnd::TimedOut,
                    dropped,
                });
            }
            self.tap.wait_past(seen, POLL);
        }
    }

    /// Send `Ctrl+C`, as a user pressing it would.
    pub fn interrupt(&self) -> Result<(), String> {
        self.input.send(vec![0x03]).map_err(|e| e.to_string())
    }

    /// End the shell and whatever runs in it: `SIGHUP` to its process
    /// group, as a closed terminal would send, and `SIGKILL` to what is left
    /// after a grace period. Blocks for that period at most.
    pub fn terminate(&self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid.and_then(|pid| i32::try_from(pid).ok()) {
            use nix::sys::signal::{killpg, Signal};
            use nix::unistd::Pid;
            if self.has_exited() {
                return;
            }
            let group = Pid::from_raw(pid);
            let _ = killpg(group, Signal::SIGHUP);
            let deadline = Instant::now() + HANGUP_GRACE;
            while Instant::now() < deadline {
                if self.has_exited() {
                    // What it started may outlive it in its group.
                    let _ = killpg(group, Signal::SIGKILL);
                    return;
                }
                std::thread::sleep(POLL);
            }
            let _ = killpg(group, Signal::SIGKILL);
        }
    }

    /// Whether the shell has exited, reaping it if so.
    fn has_exited(&self) -> bool {
        lock(&self.child)
            .try_wait()
            .is_ok_and(|status| status.is_some())
    }
}

/// The end mark of a command sent at output offset `sent`, once it came.
fn ended_after(tap: &OutputTap, sent: u64) -> Option<Option<i32>> {
    let marks = tap.marks_since(sent);
    let start = marks.iter().find(|m| m.kind == MarkKind::Start)?;
    marks.iter().find_map(|m| match m.kind {
        MarkKind::End(code) if m.at >= start.after => Some(code),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_is_typed_as_one_eval() {
        assert_eq!(typed_line("ls -la\n"), "eval $'ls -la'\n");
        assert_eq!(
            typed_line("cd /tmp\necho 'a\\b'\tc"),
            "eval $'cd /tmp\\necho \\'a\\\\b\\'\\tc'\n"
        );
        assert_eq!(typed_line("printf \u{1b}x"), "eval $'printf \\x1bx'\n");
    }
}

#[cfg(all(test, unix))]
mod shell_tests {
    use super::*;

    /// A session in a scratch home, so the user's `~/.bashrc` stays out.
    fn session(home: &Path) -> Option<(Terminal, AgentShell)> {
        if std::process::Command::new("bash")
            .arg("-c")
            .arg("true")
            .status()
            .is_err()
        {
            return None;
        }
        let env = [("HOME".to_string(), home.display().to_string())];
        Terminal::new_agent_shell(24, 120, home, &env, None, "test").ok()
    }

    fn run(shell: &AgentShell, command: &str, timeout: Duration) -> (String, RunOutcome) {
        let mut out = Vec::new();
        let outcome = shell
            .run(command, timeout, &|| false, &mut |bytes| {
                out.extend_from_slice(bytes)
            })
            .unwrap();
        (String::from_utf8_lossy(&out).replace("\r\n", "\n"), outcome)
    }

    #[test]
    fn state_carries_over_from_one_command_to_the_next() {
        let home = tempfile::tempdir().unwrap();
        let Some((_term, shell)) = session(home.path()) else {
            return;
        };
        let long = Duration::from_secs(20);
        let (out, outcome) = run(&shell, "mkdir sub && cd sub && X=42", long);
        assert_eq!((out.as_str(), outcome.end), ("", RunEnd::Finished(Some(0))));
        let (out, outcome) = run(&shell, "echo \"$X $(basename \"$PWD\")\"\nfalse", long);
        assert_eq!(out, "42 sub\n");
        assert_eq!(outcome.end, RunEnd::Finished(Some(1)));
        assert_eq!(shell.status(), ShellStatus::Idle);
    }

    #[test]
    fn a_compound_command_has_its_start_and_end() {
        let home = tempfile::tempdir().unwrap();
        let Some((_term, shell)) = session(home.path()) else {
            return;
        };
        let long = Duration::from_secs(20);
        // Bash runs no `DEBUG` trap before a subshell or a group.
        let (_, outcome) = run(&shell, "(true) &", long);
        assert_eq!(outcome.end, RunEnd::Finished(Some(0)));
        let (out, outcome) = run(&shell, "{ echo a; }", long);
        assert_eq!(
            (out.as_str(), outcome.end),
            ("a\n", RunEnd::Finished(Some(0)))
        );
    }

    #[test]
    fn a_command_that_outlives_its_timeout_runs_on_and_blocks_the_next() {
        let home = tempfile::tempdir().unwrap();
        let Some((_term, shell)) = session(home.path()) else {
            return;
        };
        let (_, outcome) = run(&shell, "sleep 30", Duration::from_millis(300));
        assert_eq!(outcome.end, RunEnd::TimedOut);
        assert!(
            matches!(shell.status(), ShellStatus::Running { ref command, .. } if command == "sleep 30")
        );
        let busy = shell.run("true", Duration::from_secs(1), &|| false, &mut |_| {});
        assert!(matches!(busy, Err(RunError::Busy { .. })));
        shell.interrupt().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while shell.status() != ShellStatus::Idle && Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        let (out, outcome) = run(&shell, "echo after", Duration::from_secs(20));
        assert_eq!(
            (out.as_str(), outcome.end),
            ("after\n", RunEnd::Finished(Some(0)))
        );
    }

    #[test]
    fn terminating_ends_the_shell_and_what_runs_in_it() {
        let home = tempfile::tempdir().unwrap();
        let Some((_term, shell)) = session(home.path()) else {
            return;
        };
        let pid_file = home.path().join("pid");
        let command = format!("sleep 60 & echo $! > {}", pid_file.display());
        run(&shell, &command, Duration::from_secs(20));
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        shell.terminate();
        let deadline = Instant::now() + Duration::from_secs(5);
        while nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
            && Instant::now() < deadline
        {
            std::thread::sleep(POLL);
        }
        assert!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err(),
            "the background job outlived its session"
        );
        assert!(shell.has_exited());
    }
}

#[cfg(test)]
const _: fn() = || {
    fn send<T: Send>() {}
    send::<Terminal>();
    send::<AgentShell>();
};
