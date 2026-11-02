//! The agent's shell session: an interactive shell in a terminal of its own,
//! which the agent runs commands in one after another — the directory, the
//! variables and what runs in the background carry over from one to the
//! next — and the user can watch, or type into, as a terminal panel.
//!
//! The terminal is an ordinary [`Terminal`], kept out of the layout until
//! the user opens it; [`AgentShell`] is the agent's handle on it, usable
//! from any thread. Where a command's output starts and ends comes from the
//! shell itself: an init script, run after the user's own (`~/.bashrc`,
//! `.zshrc`, fish's `config.fish`), marks both with OSC 133 `C` and `D`,
//! which the screen ignores and the output tap notes (see [`OutputTap`]).
//! bash, zsh and fish can be told to; see [`SessionShell`].

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

/// The shells a session runs in, each told to mark where a command starts
/// and ends in its own way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionShell {
    Bash,
    Zsh,
    Fish,
}

impl SessionShell {
    /// The kind of shell `program` starts; bash for any other.
    pub fn of(program: &str) -> Self {
        match crate::shell_utils::shell_name(program).as_str() {
            "zsh" => Self::Zsh,
            "fish" => Self::Fish,
            _ => Self::Bash,
        }
    }
}

/// Run after the user's `~/.bashrc`. Job control is off so that every
/// command stays in the shell's process group, which ending the session
/// signals — turned off again at each prompt, as bash turns it on for an
/// interactive shell only after the startup files; history expansion is
/// off so that a `!` in a command is taken as typed; the history file is
/// left alone; pagers and editors that would wait for keys nobody presses
/// are turned off. The `DEBUG` trap marks the start of the first command
/// after a prompt (bash 3.2, the one macOS ships, has no `PS0`), and the
/// first entry of `PROMPT_COMMAND` its end, with its exit code.
const BASH_INIT: &str = r#"# termide: the agent's shell session.
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

/// zsh reads it in place of the user's `.zshenv`, which it reads itself
/// from where the user's `ZDOTDIR` points (`TERMIDE_USER_ZDOTDIR`, the home
/// by default) — the user's own may move it — and then points `ZDOTDIR`
/// back here, so that zsh reads [`ZSH_RC`] next.
const ZSH_ENV: &str = r#"# termide: the agent's shell session.
__termide_dotdir=$ZDOTDIR
ZDOTDIR=${TERMIDE_USER_ZDOTDIR:-$HOME}
[[ -f $ZDOTDIR/.zshenv ]] && . $ZDOTDIR/.zshenv
TERMIDE_USER_ZDOTDIR=$ZDOTDIR
ZDOTDIR=$__termide_dotdir
unset __termide_dotdir
"#;

/// zsh reads it in place of the user's `.zshrc`, which it reads first, with
/// `ZDOTDIR` the user's again. Then, as for bash: job control off, at each
/// prompt too; `!` taken as typed; no history file; no pagers; no mark of a
/// last line left open, which zsh prints ahead of the end mark. `preexec`
/// marks a command's start, the first `precmd` its end — zsh gives each of
/// them the command's exit code. Pasting, which is how a command is typed
/// here, is bound to zsh's own widget, under a name no plugin wraps: a
/// plugin that quotes URLs or pairs brackets must not change the command.
const ZSH_RC: &str = r#"# termide: the agent's shell session.
ZDOTDIR=$TERMIDE_USER_ZDOTDIR
[[ -f $ZDOTDIR/.zshrc ]] && . $ZDOTDIR/.zshrc
unsetopt monitor bang_hist prompt_sp
unset HISTFILE
export PAGER=cat GIT_PAGER=cat MANPAGER=cat SYSTEMD_PAGER= GIT_EDITOR=true
__termide_ran=
__termide_preexec() {
  __termide_ran=1
  printf '\033]133;C\007'
}
__termide_precmd() {
  local code=$?
  unsetopt monitor
  if [[ -n $__termide_ran ]]; then printf '\033]133;D;%s\007' $code; fi
  __termide_ran=
}
preexec_functions+=(__termide_preexec)
precmd_functions=(__termide_precmd $precmd_functions)
__termide_paste() { zle .bracketed-paste }
zle -N __termide_paste
for __termide_keymap in emacs viins vicmd; do
  bindkey -M $__termide_keymap '^[[200~' __termide_paste
done
unset __termide_keymap
"#;

/// fish runs it after the user's `config.fish`. As for bash: job control
/// off; no history file; no pagers; and no greeting. The `fish_preexec`
/// event marks a command's start, `fish_postexec` its end with its exit
/// code, and `fish_prompt` the prompt: fish drops what is typed while it
/// waits for the terminal's answers to its queries, at start, so a command
/// is typed only at a prompt. (fish 4 marks all three itself too.)
const FISH_INIT: &str = r#"# termide: the agent's shell session.
status job-control none
set -g fish_history ''
set -gx PAGER cat
set -gx GIT_PAGER cat
set -gx MANPAGER cat
set -gx SYSTEMD_PAGER ''
set -gx GIT_EDITOR true
set -g fish_greeting ''
function __termide_preexec --on-event fish_preexec
  set -g __termide_ran 1
  printf '\e]133;C\a'
end
function __termide_prompt --on-event fish_prompt
  printf '\e]133;A\a'
end
function __termide_postexec --on-event fish_postexec
  set -l code $status
  status job-control none
  if set -q __termide_ran
    set -e __termide_ran
    printf '\e]133;D;%s\a' $code
  end
end
"#;

/// The directory of the init scripts, written once per process: one of a
/// name no one can guess, that only the user can write to, since a shell
/// runs what is in it.
fn init_dir() -> std::io::Result<&'static Path> {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    if let Some(dir) = DIR.get() {
        return Ok(dir);
    }
    let dir = tempfile::Builder::new()
        .prefix("termide-agent-shell-")
        .tempdir()?
        .keep();
    for (name, text) in [
        ("init.bash", BASH_INIT),
        (".zshenv", ZSH_ENV),
        (".zshrc", ZSH_RC),
        ("init.fish", FISH_INIT),
    ] {
        std::fs::write(dir.join(name), text)?;
    }
    Ok(DIR.get_or_init(|| dir))
}

/// What is typed into `shell` for `command`: one `eval` of it, so that the
/// shell reads it as one command with one start and one end, however many
/// lines it has, and a control character in it is typed as an escape. zsh
/// and fish get it pasted, so that a key binding, an abbreviation or a
/// plugin of the user's line editor does not change it, then `Enter`.
fn typed(shell: SessionShell, command: &str) -> Vec<u8> {
    let line = match shell {
        SessionShell::Bash => return typed_line(command).into_bytes(),
        SessionShell::Zsh => typed_line(command),
        SessionShell::Fish => fish_line(command),
    };
    let mut bytes = b"\x1b[200~".to_vec();
    bytes.extend(line.trim_end_matches('\n').as_bytes());
    bytes.extend(b"\x1b[201~\r");
    bytes
}

/// The line typed into bash or zsh for `command`: always `eval $'…'`, a
/// simple command — the start mark comes from bash's `DEBUG` trap, which
/// bash runs before a simple command and not before a subshell such as
/// `( … ) &`.
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

/// The line typed into fish for `command`: `eval` of it in single quotes,
/// with `\'` and `\\` escaped in them, and a control character written
/// outside them as fish's escape for it.
fn fish_line(command: &str) -> String {
    let command = command.trim_end_matches(['\n', '\r']);
    let mut line = String::from("eval '");
    for c in command.chars() {
        match c {
            '\\' => line.push_str("\\\\"),
            '\'' => line.push_str("\\'"),
            '\n' => line.push_str("'\\n'"),
            '\t' => line.push_str("'\\t'"),
            c if c.is_control() => line.push_str(&format!("'\\u{:04x}'", c as u32)),
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
    kind: SessionShell,
    input: InputSender,
    tap: Arc<OutputTap>,
    child: SharedChild,
    pid: Option<u32>,
    /// One command or input at a time: a caller waits here for the one
    /// before, so the agent's writes reach the terminal in order.
    turn: Arc<Mutex<()>>,
    in_flight: Arc<Mutex<Option<InFlight>>>,
    /// The terminal's screen, for a snapshot and the keys' encoding.
    screen: Arc<std::sync::RwLock<crate::terminal::TerminalScreen>>,
}

/// Says from raw output whether it is what was waited for.
pub type OutputTest = dyn Fn(&[u8]) -> bool;

/// What [`AgentShell::send`] waits for once the input is sent.
pub struct SendWait<'a> {
    /// Says from the raw output since the input whether it is what was
    /// waited for.
    pub until: Option<&'a OutputTest>,
    /// Output that stops for this long ends the wait.
    pub quiet: Option<Duration>,
    pub timeout: Duration,
}

/// How a [`AgentShell::send`] wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendEnd {
    /// The output showed what was waited for.
    Matched,
    /// The output stopped for the quiet period.
    Quiet,
    TimedOut,
    Cancelled,
    ShellExited,
}

/// What a [`AgentShell::send`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendOutcome {
    pub end: SendEnd,
    /// Where the output after the input starts in the tap.
    pub from: u64,
}

/// The terminal's screen as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenText {
    /// The visible rows, trailing blanks cut, trailing empty rows left out.
    pub text: String,
    /// Whether a full-screen program has the alternate screen up.
    pub alternate: bool,
    /// The cursor's row and column, from 0.
    pub cursor: (usize, usize),
}

/// The bytes a key sends, by its name: `Enter`, `Tab`, `Esc`, `Backspace`,
/// `Delete`, `Up`, `Down`, `Left`, `Right`, `Home`, `End`, `PageUp`,
/// `PageDown`, `Insert`, `Space`, `F1`–`F12`, and `Ctrl+`/`Alt+` with a
/// letter or another key. The arrows follow the program's cursor-key mode.
pub fn key_bytes(name: &str, application_cursor: bool) -> Option<Vec<u8>> {
    let name = name.trim();
    let lower = name.to_ascii_lowercase();
    if let Some(rest) = lower
        .strip_prefix("ctrl+")
        .or_else(|| lower.strip_prefix("c-"))
    {
        let mut chars = rest.chars();
        return match (chars.next(), chars.next()) {
            (Some(c @ ('a'..='z' | '@' | '[' | '\\' | ']' | '^' | '_')), None) => {
                Some(vec![(c.to_ascii_uppercase() as u8) & 0x1f])
            }
            (Some(' '), None) => Some(vec![0]),
            _ if rest == "space" => Some(vec![0]),
            _ => None,
        };
    }
    if let Some(rest) = name
        .strip_prefix("Alt+")
        .or_else(|| name.strip_prefix("alt+"))
        .or_else(|| name.strip_prefix("M-"))
    {
        let mut bytes = vec![0x1b];
        let mut chars = rest.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => bytes.extend(c.to_string().into_bytes()),
            _ => bytes.extend(key_bytes(rest, application_cursor)?),
        }
        return Some(bytes);
    }
    let arrow = |c: u8| {
        if application_cursor {
            vec![0x1b, b'O', c]
        } else {
            vec![0x1b, b'[', c]
        }
    };
    let bytes: Vec<u8> = match lower.as_str() {
        "enter" | "return" => b"\r".to_vec(),
        "tab" => b"\t".to_vec(),
        "esc" | "escape" => vec![0x1b],
        "backspace" => vec![0x7f],
        "space" => b" ".to_vec(),
        "delete" | "del" => b"\x1b[3~".to_vec(),
        "insert" => b"\x1b[2~".to_vec(),
        "up" => arrow(b'A'),
        "down" => arrow(b'B'),
        "right" => arrow(b'C'),
        "left" => arrow(b'D'),
        "home" => arrow(b'H'),
        "end" => arrow(b'F'),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => b"\x1b[15~".to_vec(),
        "f6" => b"\x1b[17~".to_vec(),
        "f7" => b"\x1b[18~".to_vec(),
        "f8" => b"\x1b[19~".to_vec(),
        "f9" => b"\x1b[20~".to_vec(),
        "f10" => b"\x1b[21~".to_vec(),
        "f11" => b"\x1b[23~".to_vec(),
        "f12" => b"\x1b[24~".to_vec(),
        _ => return None,
    };
    Some(bytes)
}

fn read_screen(
    screen: &std::sync::RwLock<crate::terminal::TerminalScreen>,
) -> std::sync::RwLockReadGuard<'_, crate::terminal::TerminalScreen> {
    screen.read().unwrap_or_else(|e| e.into_inner())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Terminal {
    /// An agent's shell session in `cwd`: `program`, which is bash, zsh or
    /// fish (bash for any other), run with its init script, with `env`
    /// added (a `PATH` with the command shims first, say), its output logged
    /// to `log`. The terminal is titled `title`.
    #[allow(clippy::too_many_arguments)]
    pub fn new_agent_shell(
        rows: u16,
        cols: u16,
        program: &str,
        cwd: &Path,
        env: &[(String, String)],
        log: Option<&Path>,
        title: &str,
    ) -> anyhow::Result<(Self, AgentShell)> {
        let kind = SessionShell::of(program);
        let program = match kind {
            SessionShell::Bash if crate::shell_utils::shell_name(program) != "bash" => "bash",
            _ => program,
        };
        let dir = init_dir()?;
        let mut cmd = CommandBuilder::new(program);
        let mut env = env.to_vec();
        match kind {
            SessionShell::Bash => {
                cmd.arg("--rcfile");
                cmd.arg(dir.join("init.bash"));
                env.push(("BASH_SILENCE_DEPRECATION_WARNING".into(), "1".into()));
            }
            SessionShell::Zsh => {
                let given = |name: &str| {
                    env.iter()
                        .rev()
                        .find(|(key, _)| key == name)
                        .map(|(_, value)| value.clone())
                        .or_else(|| std::env::var(name).ok())
                };
                let user = given("ZDOTDIR")
                    .or_else(|| given("HOME"))
                    .unwrap_or_default();
                env.push(("TERMIDE_USER_ZDOTDIR".into(), user));
                env.push(("ZDOTDIR".into(), dir.display().to_string()));
            }
            SessionShell::Fish => {
                let init = crate::shell_utils::ShellKind::Fish
                    .quote(&dir.join("init.fish").to_string_lossy());
                cmd.arg("--init-command");
                cmd.arg(format!("source {init}"));
            }
        }
        cmd.arg("-i");
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
        term.shell_kind = crate::shell_utils::ShellKind::from_shell_path(program);
        term.shell_name = crate::shell_utils::shell_name(program);
        term.agent_shell = true;
        let shell = AgentShell {
            kind,
            input: term.input.sender(),
            tap,
            child: Arc::clone(&term.child),
            pid: term.shell_pid,
            turn: Arc::new(Mutex::new(())),
            in_flight: Arc::new(Mutex::new(None)),
            screen: Arc::clone(&term.screen),
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
        let since = Instant::now();
        let deadline = since + timeout;
        if self.kind == SessionShell::Fish {
            self.await_prompt(deadline);
        }
        let sent = self.tap.end();
        self.input
            .send(typed(self.kind, command))
            .map_err(|e| RunError::Input(e.to_string()))?;
        *lock(&self.in_flight) = Some(InFlight {
            command: command.to_string(),
            since,
            sent,
        });
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

    /// Wait, until `deadline` at most, for the shell to show its prompt: the
    /// latest mark is one of a prompt.
    fn await_prompt(&self, deadline: Instant) {
        loop {
            let seen = self.tap.end();
            let at_prompt = self
                .tap
                .marks_since(0)
                .last()
                .is_some_and(|mark| mark.kind == MarkKind::Prompt);
            let left = deadline.saturating_duration_since(Instant::now());
            if at_prompt || self.tap.is_closed() || left.is_zero() {
                return;
            }
            self.tap.wait_past(seen, POLL.min(left));
        }
    }

    /// Type `input` — a line break as `Enter` — then press `keys` (see
    /// [`key_bytes`]), and wait as `wait` says: for the output to show what
    /// it waits for, to stop for a while, or for the timeout. A command of
    /// the agent's running there takes it as its input; so does the shell
    /// at its prompt.
    pub fn send(
        &self,
        input: &str,
        keys: &[String],
        wait: &SendWait<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SendOutcome, RunError> {
        let application_cursor = read_screen(&self.screen).application_cursor_keys;
        let mut bytes = input.replace("\r\n", "\r").replace('\n', "\r").into_bytes();
        for key in keys {
            let Some(key) = key_bytes(key, application_cursor) else {
                return Err(RunError::Input(format!("unknown key `{key}`")));
            };
            bytes.extend(key);
        }
        let _turn = lock(&self.turn);
        if self.tap.is_closed() {
            return Err(RunError::Exited);
        }
        let from = self.tap.end();
        if !bytes.is_empty() {
            self.input
                .send(bytes)
                .map_err(|e| RunError::Input(e.to_string()))?;
        }
        let outcome = |end| Ok(SendOutcome { end, from });
        let deadline = Instant::now() + wait.timeout;
        let mut seen = from;
        let mut last_output = Instant::now();
        loop {
            let end = self.tap.end();
            if end != seen {
                seen = end;
                last_output = Instant::now();
                if let Some(until) = wait.until {
                    if until(&self.tap.read_since(from).data) {
                        return outcome(SendEnd::Matched);
                    }
                }
            } else if self.tap.is_closed() {
                return outcome(SendEnd::ShellExited);
            }
            if wait
                .quiet
                .is_some_and(|quiet| last_output.elapsed() >= quiet)
            {
                return outcome(SendEnd::Quiet);
            }
            if cancelled() {
                return outcome(SendEnd::Cancelled);
            }
            let now = Instant::now();
            if now >= deadline {
                return outcome(SendEnd::TimedOut);
            }
            self.tap.wait_past(seen, POLL.min(deadline - now));
        }
    }

    /// The terminal's screen as it stands.
    pub fn screen(&self) -> ScreenText {
        let screen = read_screen(&self.screen);
        let mut rows: Vec<String> = screen
            .active_buffer()
            .iter()
            .map(|row| {
                let mut text = String::new();
                for cell in row {
                    cell.push_text(&mut text);
                }
                text.trim_end().to_string()
            })
            .collect();
        while rows.last().is_some_and(String::is_empty) {
            rows.pop();
        }
        ScreenText {
            text: rows.join("\n"),
            alternate: screen.use_alt_screen,
            cursor: screen.cursor,
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
        assert_eq!(
            fish_line("cd /tmp\necho 'a\\b'\tc\u{1b}"),
            "eval 'cd /tmp'\\n'echo \\'a\\\\b\\''\\t'c'\\u001b''\n"
        );
        // zsh and fish get the line pasted, and `Enter` after it.
        assert_eq!(typed(SessionShell::Bash, "ls"), b"eval $'ls'\n");
        assert_eq!(
            typed(SessionShell::Zsh, "ls"),
            b"\x1b[200~eval $'ls'\x1b[201~\r"
        );
        assert_eq!(
            typed(SessionShell::Fish, "ls"),
            b"\x1b[200~eval 'ls'\x1b[201~\r"
        );
        assert_eq!(
            SessionShell::of("/opt/homebrew/bin/fish"),
            SessionShell::Fish
        );
        assert_eq!(SessionShell::of("zsh"), SessionShell::Zsh);
        assert_eq!(SessionShell::of("/usr/bin/nu"), SessionShell::Bash);
    }

    #[test]
    fn keys_are_sent_by_name() {
        assert_eq!(key_bytes("Enter", false), Some(b"\r".to_vec()));
        assert_eq!(key_bytes("Ctrl+C", false), Some(vec![3]));
        assert_eq!(key_bytes("ctrl+d", false), Some(vec![4]));
        assert_eq!(key_bytes("Up", false), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_bytes("Up", true), Some(b"\x1bOA".to_vec()));
        assert_eq!(key_bytes("Alt+b", false), Some(b"\x1bb".to_vec()));
        assert_eq!(key_bytes("Alt+Left", false), Some(b"\x1b\x1b[D".to_vec()));
        assert_eq!(key_bytes("F5", false), Some(b"\x1b[15~".to_vec()));
        assert_eq!(key_bytes("Hyper", false), None);
        assert_eq!(key_bytes("Ctrl+Left", false), None);
    }
}

#[cfg(all(test, unix))]
mod shell_tests {
    use super::*;

    /// A bash session in a scratch home, so the user's `~/.bashrc` stays out.
    fn session(home: &Path) -> Option<(Terminal, AgentShell)> {
        session_in("bash", home)
    }

    /// `out` without escape sequences and carriage returns: fish sets the
    /// title and reports the directory while a command runs, which the
    /// `shell` tool cleans away.
    fn plain(out: &str) -> String {
        let mut text = String::new();
        let mut chars = out.chars().peekable();
        while let Some(c) = chars.next() {
            match (c, chars.peek()) {
                ('\x1b', Some(']')) => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                ('\x1b', Some('[')) => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            break;
                        }
                    }
                }
                ('\r', _) => {}
                _ => text.push(c),
            }
        }
        text
    }

    /// A session of `program` in a scratch home, its startup files there;
    /// `None` when the shell is not installed.
    fn session_in(program: &str, home: &Path) -> Option<(Terminal, AgentShell)> {
        if std::process::Command::new(program)
            .arg("-c")
            .arg("true")
            .status()
            .is_err()
        {
            return None;
        }
        let home = home.display().to_string();
        let env = [
            ("HOME".to_string(), home.clone()),
            ("ZDOTDIR".to_string(), home.clone()),
            ("XDG_CONFIG_HOME".to_string(), format!("{home}/.config")),
            ("XDG_DATA_HOME".to_string(), format!("{home}/.local/share")),
        ];
        Terminal::new_agent_shell(24, 120, program, Path::new(&home), &env, None, "test").ok()
    }

    /// The same commands in bash, zsh and fish, each after a startup file
    /// of the user's; a shell that is not installed is left out.
    #[test]
    fn every_shell_marks_its_commands_and_keeps_its_state() {
        for (program, rc, user, set) in [
            ("bash", ".bashrc", "RC=ran\n", "X=42"),
            // A widget wrapping pasting must not change the command.
            (
                "zsh",
                ".zshrc",
                "shout() { zle .bracketed-paste; BUFFER=${BUFFER:u} }\n\
                 zle -N bracketed-paste shout\n\
                 RC=ran\n",
                "X=42",
            ),
            // Nor must an abbreviation.
            (
                "fish",
                ".config/fish/config.fish",
                "set -g RC ran\nabbr -a eval 'echo ABBR'\n",
                "set X 42",
            ),
        ] {
            let home = tempfile::tempdir().unwrap();
            let rc = home.path().join(rc);
            std::fs::create_dir_all(rc.parent().unwrap()).unwrap();
            std::fs::write(&rc, user).unwrap();
            let Some((_term, shell)) = session_in(program, home.path()) else {
                continue;
            };
            let long = Duration::from_secs(20);
            let (out, outcome) = run(&shell, &format!("mkdir sub && cd sub && {set}"), long);
            assert_eq!(
                (plain(&out).as_str(), outcome.end),
                ("", RunEnd::Finished(Some(0))),
                "{program}"
            );
            let (out, outcome) = run(
                &shell,
                "echo \"$X $RC $(basename \"$PWD\")\"\nprintf '%s|\\n' 'a  b' \"c'd\"\nfalse",
                long,
            );
            assert_eq!(plain(&out), "42 ran sub\na  b|\nc'd|\n", "{program}");
            assert_eq!(outcome.end, RunEnd::Finished(Some(1)), "{program}");
            let (_, outcome) = run(&shell, "sh -c 'exit 7'", long);
            assert_eq!(outcome.end, RunEnd::Finished(Some(7)), "{program}");
            assert_eq!(shell.status(), ShellStatus::Idle, "{program}");
        }
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
    fn a_command_waiting_for_input_gets_it_and_answers() {
        let home = tempfile::tempdir().unwrap();
        let Some((_term, shell)) = session(home.path()) else {
            return;
        };
        let (_, outcome) = run(
            &shell,
            "read x; echo \"got $x\"",
            Duration::from_millis(300),
        );
        assert_eq!(outcome.end, RunEnd::TimedOut);
        let until = |out: &[u8]| String::from_utf8_lossy(out).contains("got hi");
        let wait = SendWait {
            until: Some(&until),
            quiet: None,
            timeout: Duration::from_secs(20),
        };
        let sent = shell.send("hi\n", &[], &wait, &|| false).unwrap();
        assert_eq!(sent.end, SendEnd::Matched);
        let out = String::from_utf8_lossy(&shell.tap().read_since(sent.from).data).into_owned();
        assert!(out.contains("got hi"), "{out:?}");
        // The command is over: the next one runs.
        let deadline = Instant::now() + Duration::from_secs(5);
        while shell.status() != ShellStatus::Idle {
            assert!(Instant::now() < deadline, "still running");
            std::thread::sleep(POLL);
        }
        let quiet = SendWait {
            until: None,
            quiet: Some(Duration::from_millis(300)),
            timeout: Duration::from_secs(20),
        };
        let sent = shell
            .send("printf 'a\\nb\\n'", &["Enter".to_string()], &quiet, &|| {
                false
            })
            .unwrap();
        assert_eq!(sent.end, SendEnd::Quiet);
        let screen = shell.screen();
        assert!(!screen.alternate);
        assert!(screen.text.contains("\na\nb\n"), "{:?}", screen.text);
        assert!(shell
            .send("", &["Hyper".to_string()], &quiet, &|| false)
            .is_err());
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
