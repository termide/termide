//! `shell`: run a shell command, stream its output, kill it on timeout or
//! cancel, keep head and tail of long output inline and the full log on disk.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use termide_agent_core::{
    CancelToken, ShellSyntax, Tool, ToolCall, ToolContext, ToolResultMessage, ToolTraits,
    ToolUpdate,
};
use termide_agent_core::{TerminalRun, TerminalRunEnd, TerminalSessions, ToolText, SHELL_TOOL};

use crate::args::{optional_bool, optional_str, optional_u64, required_str};
use crate::capture::{Capture, Captured, LiveView};
use crate::clean::{clean_output, Cleaned};
use crate::truncate::{head_tail, head_tail_split, SHELL_MAX_BYTES};

/// How often the accumulated output is pushed to the UI while running.
const UPDATE_INTERVAL: Duration = Duration::from_millis(200);
/// How often the child is polled for exit, cancel and timeout.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
/// Once the command is over, how long its output may go quiet before what
/// still holds the pipes (a process that left its group) is left behind.
const DRAIN_QUIET: Duration = Duration::from_millis(500);
/// And how long the output is read at most once the command is over.
const DRAIN_MAX: Duration = Duration::from_secs(5);
/// Output kept in memory from each end of a command's output; the middle of
/// a longer one is only counted, and goes to the log file on disk.
const CAPTURE_SIDE_BYTES: usize = 1024 * 1024;
/// Cap on the log of one command's output, so a command printing without
/// end until its timeout cannot fill the disk.
const MAX_LOG_BYTES: u64 = 256 * 1024 * 1024;
/// How long a command started in the background is watched before the call
/// returns: long enough to report a command that fails at once.
const BACKGROUND_WAIT: Duration = Duration::from_secs(1);
/// The longest name of a terminal session.
const MAX_SESSION_NAME: usize = 40;

/// The shell the agent's commands run in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellProgram {
    /// What to start: a name found on `PATH` or a path.
    pub program: String,
    /// What the model is told it writes for: `bash`, `zsh`, `fish`.
    pub name: String,
    pub syntax: ShellSyntax,
    /// The tool's description, which names the shell.
    description: String,
}

impl ShellProgram {
    /// The shell at `program`, when it is one the agent can run its commands
    /// in (see [`ShellSyntax::of`]).
    #[must_use]
    pub fn new(program: &str) -> Option<Self> {
        let file = Path::new(program).file_name()?.to_str()?;
        let name = file
            .strip_suffix(".exe")
            .unwrap_or(file)
            .to_ascii_lowercase();
        let syntax = ShellSyntax::of(&name)?;
        let description = render_description(&ToolText::seed(SHELL_TOOL).description, &name);
        Some(Self {
            program: program.to_string(),
            name,
            syntax,
            description,
        })
    }

    /// bash, found on `PATH`.
    #[must_use]
    pub fn bash() -> Self {
        Self::new("bash").expect("bash is a shell the agent runs in")
    }
}

/// `{{shell}}` in `template` takes the shell's name.
fn render_description(template: &str, shell: &str) -> String {
    template.replace("{{shell}}", shell)
}

pub struct ShellTool {
    /// The shell a command runs in: one apart (`-c`) or a terminal session.
    pub shell: ShellProgram,
    pub default_timeout: Duration,
    pub max_timeout: Duration,
    pub max_output_bytes: usize,
    /// Bytes kept in memory from each end of the raw output.
    pub capture_bytes: usize,
    /// Cap on the full-output log file.
    pub max_log_bytes: u64,
    /// Where full logs of truncated output go; the system temp dir if `None`.
    pub log_dir: Option<PathBuf>,
    /// Clean and compact output for the model (strip escapes, collapse noise)
    /// before truncation. The user still sees the raw stream and full log.
    pub clean: bool,
    /// Directory of command shims prepended to `PATH`, so an executable named
    /// after a command shadows the real one (a token-saving wrapper). `None`
    /// leaves `PATH` untouched.
    pub shim_path: Option<PathBuf>,
    /// The terminal sessions `session` and `background` run commands in;
    /// `None` where there are none (a run outside termide's own loop).
    pub terminals: Option<Arc<dyn TerminalSessions>>,
}

impl Default for ShellTool {
    fn default() -> Self {
        Self {
            shell: ShellProgram::bash(),
            default_timeout: Duration::from_secs(120),
            max_timeout: Duration::from_secs(600),
            max_output_bytes: SHELL_MAX_BYTES,
            capture_bytes: CAPTURE_SIDE_BYTES,
            max_log_bytes: MAX_LOG_BYTES,
            log_dir: None,
            clean: true,
            shim_path: None,
            terminals: None,
        }
    }
}

impl Tool for ShellTool {
    fn name(&self) -> &str {
        SHELL_TOOL
    }

    fn description(&self) -> &str {
        &self.shell.description
    }

    /// `{{shell}}` takes the name of the shell commands run in.
    fn render_description(&self, template: &str) -> String {
        render_description(template, &self.shell.name)
    }

    fn traits(&self) -> ToolTraits {
        ToolTraits {
            shell: self.shell.syntax,
            ..ToolTraits::default()
        }
    }

    fn parameters(&self) -> Value {
        let command = format!("The command line to run with {} -c", self.shell.name);
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": command },
                "timeout": { "type": "integer", "minimum": 1, "description": "Seconds before the command is killed (default 120, max 600); in a terminal session, seconds to wait for it, after which it runs on" },
                "session": { "type": "string", "description": "Run in the terminal session of this name, opened on first use: the directory, variables and background jobs carry over from one command to the next (letters, digits, `.`, `_`, `-`)" },
                "background": { "type": "boolean", "description": "Start the command and return at once, leaving it running in a terminal session (`session`, or a new one); read its output later with `terminals`" }
            },
            "required": ["command"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed(SHELL_TOOL).snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed(SHELL_TOOL).guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> ToolResultMessage {
        let command = match required_str(call, "command") {
            Ok(command) if !command.trim().is_empty() => command,
            Ok(_) => return ToolResultMessage::error(call, "`command` is empty"),
            Err(message) => return ToolResultMessage::error(call, message),
        };
        let timeout = match optional_u64(call, "timeout") {
            Ok(Some(seconds)) => Duration::from_secs(seconds).min(self.max_timeout),
            Ok(None) => self.default_timeout,
            Err(message) => return ToolResultMessage::error(call, message),
        };

        let session = match optional_str(call, "session") {
            Ok(session) => session.map(str::trim).filter(|name| !name.is_empty()),
            Err(message) => return ToolResultMessage::error(call, message),
        };
        let background = match optional_bool(call, "background") {
            Ok(background) => background,
            Err(message) => return ToolResultMessage::error(call, message),
        };
        if session.is_some() || background {
            let wait = if background { BACKGROUND_WAIT } else { timeout };
            return self.run_in_terminal(
                call, ctx, command, session, wait, background, on_update, cancel,
            );
        }

        let run = match self.run(command, &call.id, ctx, timeout, on_update, cancel) {
            Ok(run) => run,
            Err(message) => return ToolResultMessage::error(call, message),
        };
        self.render(call, run)
    }
}

struct Run {
    output: Captured,
    exit_code: Option<i32>,
    timed_out: bool,
    cancelled: bool,
    /// A process that left the command's group (a daemon, an ssh
    /// `ControlPersist` master) still held its output when the command was
    /// over, so the output was no longer read.
    detached: bool,
    duration: Duration,
    /// The command that ran, for the command-aware cleaning rules.
    command: String,
}

impl ShellTool {
    fn run(
        &self,
        command: &str,
        call_id: &str,
        ctx: &ToolContext,
        timeout: Duration,
        on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> Result<Run, String> {
        let mut child = spawn(&self.shell, command, ctx, self.shim_path.as_deref())?;
        let started = Instant::now();
        let output = Arc::new(Mutex::new(Capture::new(
            self.capture_bytes,
            self.log_path(call_id),
            self.max_log_bytes,
        )));
        let stdout: Option<Box<dyn Read + Send>> = child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>);
        let stderr: Option<Box<dyn Read + Send>> = child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>);
        let readers = [stdout, stderr]
            .into_iter()
            .flatten()
            .map(|stream| {
                let sink = Arc::clone(&output);
                std::thread::spawn(move || pump(stream, &sink))
            })
            .collect::<Vec<_>>();

        let mut timed_out = false;
        let mut cancelled = false;
        let mut last_update = Instant::now();
        let mut view = LiveView::default();
        let exit_code = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.code(),
                Ok(None) => {}
                Err(error) => {
                    kill_group(&mut child);
                    return Err(format!("failed while waiting for the command: {error}"));
                }
            }
            if cancel.is_cancelled() {
                cancelled = true;
                kill_group(&mut child);
                break child.wait().ok().and_then(|status| status.code());
            }
            if started.elapsed() >= timeout {
                timed_out = true;
                kill_group(&mut child);
                break child.wait().ok().and_then(|status| status.code());
            }
            if last_update.elapsed() >= UPDATE_INTERVAL {
                let changed = {
                    let capture = lock(&output);
                    let changed = capture.total() != view.consumed;
                    if changed {
                        capture.update_view(&mut view);
                    }
                    changed
                };
                if changed {
                    on_update(ToolUpdate::Output(view.text.clone()));
                }
                last_update = Instant::now();
            }
            std::thread::sleep(POLL_INTERVAL);
        };
        let detached = drain(readers, &output);
        let output = std::mem::replace(&mut *lock(&output), Capture::new(1, None, 0)).finish();
        Ok(Run {
            output,
            exit_code,
            timed_out,
            cancelled,
            detached,
            duration: started.elapsed(),
            command: command.to_string(),
        })
    }

    fn render(&self, call: &ToolCall, run: Run) -> ToolResultMessage {
        let (cleaned, mut body, full_output_path) = self.body(call, &run.output, &run.command);

        let mut is_error = false;
        if run.cancelled {
            body.push_str("[cancelled; process killed]");
            is_error = true;
        } else if run.timed_out {
            body.push_str(&format!(
                "[timed out after {} s; process killed]",
                run.duration.as_secs()
            ));
            is_error = true;
        } else {
            match run.exit_code {
                Some(0) => {}
                Some(code) => {
                    body.push_str(&format!("[exit code {code}]"));
                    is_error = true;
                }
                None => {
                    body.push_str("[terminated by signal]");
                    is_error = true;
                }
            }
        }

        if run.detached {
            body.push_str(
                "\n[a background process that left the command (a daemon, an ssh ControlPersist \
                 master) still holds its output; stopped reading it]",
            );
        }

        let details = json!({
            "detached": run.detached,
            "exit_code": run.exit_code,
            "timed_out": run.timed_out,
            "cancelled": run.cancelled,
            "duration_ms": run.duration.as_millis() as u64,
            "truncated": full_output_path.is_some(),
            "full_output_path": full_output_path,
            "raw_bytes": cleaned.original_bytes,
            "cleaned_bytes": cleaned.cleaned_bytes,
            "raw_chars": cleaned.original_chars,
            "cleaned_chars": cleaned.cleaned_chars,
            "dropped_bytes": run.output.dropped,
        });
        let result = if is_error {
            ToolResultMessage::error(call, body)
        } else {
            ToolResultMessage::text(call, body)
        };
        result.with_details(details)
    }

    /// Run `command` in terminal session `session` of the caller's agent
    /// session — or in a new one, named after nothing in particular, when it
    /// goes to the background unnamed — waiting `wait` for it at most.
    #[allow(clippy::too_many_arguments)]
    fn run_in_terminal(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        command: &str,
        session: Option<&str>,
        wait: Duration,
        background: bool,
        on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> ToolResultMessage {
        let owner = ctx.session.as_ref().and_then(|view| view.id.as_deref());
        let (Some(terminals), Some(owner)) = (&self.terminals, owner) else {
            return ToolResultMessage::error(
                call,
                "terminal sessions are not available here; run the command without `session` \
                 and `background`",
            );
        };
        let name = match session {
            Some(name) => match check_session_name(name) {
                Ok(()) => name.to_string(),
                Err(message) => return ToolResultMessage::error(call, message),
            },
            None => terminals.free_name(owner, "bg"),
        };
        let output = Mutex::new(Capture::new(self.capture_bytes, None, 0));
        let mut view = LiveView::default();
        let mut last_update = Instant::now();
        let outcome = terminals.run(
            &TerminalRun {
                owner,
                name: &name,
                cwd: &ctx.cwd,
                command,
                timeout: wait,
                cancel,
                shim_path: self.shim_path.as_deref(),
            },
            &mut |bytes| {
                let mut capture = lock(&output);
                capture.push(bytes);
                if last_update.elapsed() >= UPDATE_INTERVAL {
                    capture.update_view(&mut view);
                    drop(capture);
                    on_update(ToolUpdate::Output(view.text.clone()));
                    last_update = Instant::now();
                }
            },
        );
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(message) => return ToolResultMessage::error(call, message),
        };
        let captured = output
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .finish();
        let (cleaned, mut body, full_output_path) = self.body(call, &captured, command);
        let mut is_error = false;
        let mut exit_code = None;
        match outcome.end {
            TerminalRunEnd::Finished(code) => {
                exit_code = code;
                if let Some(code) = code.filter(|code| *code != 0) {
                    body.push_str(&format!("[exit code {code}]"));
                    is_error = true;
                }
            }
            TerminalRunEnd::TimedOut if background => body.push_str(&format!(
                "[runs on in the background in terminal session `{name}`; `terminals` reads \
                 what it prints, interrupts or closes it]"
            )),
            TerminalRunEnd::TimedOut => body.push_str(&format!(
                "[still running in terminal session `{name}` after {} s; `terminals` reads what \
                 it prints, interrupts or closes it]",
                wait.as_secs()
            )),
            TerminalRunEnd::Cancelled => {
                body.push_str(&format!(
                    "[cancelled; Ctrl+C sent to terminal session `{name}`]"
                ));
                is_error = true;
            }
            TerminalRunEnd::ShellExited => {
                body.push_str(&format!(
                    "[the shell of terminal session `{name}` exited; the session is closed]"
                ));
                is_error = true;
            }
        }
        if outcome.opened && session.is_none() {
            body = format!("[terminal session `{name}`]\n{body}");
        }
        let details = json!({
            "session": name,
            "opened": outcome.opened,
            "background": background,
            "running": outcome.end == TerminalRunEnd::TimedOut,
            "session_log": outcome.log,
            "exit_code": exit_code,
            "timed_out": outcome.end == TerminalRunEnd::TimedOut && !background,
            "cancelled": outcome.end == TerminalRunEnd::Cancelled,
            "truncated": full_output_path.is_some(),
            "full_output_path": full_output_path,
            "raw_bytes": cleaned.original_bytes,
            "cleaned_bytes": cleaned.cleaned_bytes,
            "raw_chars": cleaned.original_chars,
            "cleaned_chars": cleaned.cleaned_chars,
            "dropped_bytes": captured.dropped + outcome.dropped,
        });
        let result = if is_error {
            ToolResultMessage::error(call, body)
        } else {
            ToolResultMessage::text(call, body)
        };
        result.with_details(details)
    }

    /// What the model reads of `output`: cleaned, its middle cut when long
    /// (the raw output then saved to a log, whose path comes back), never
    /// empty, ending with a line break.
    fn body(
        &self,
        call: &ToolCall,
        output: &Captured,
        command: &str,
    ) -> (Cleaned, String, Option<PathBuf>) {
        let mut full_output_path = None;
        let (cleaned, mut body) = if output.dropped == 0 {
            let raw = String::from_utf8_lossy(&output.head).into_owned();
            let cleaned = self.clean(&raw, command);
            let body = match head_tail(&cleaned.text, self.max_output_bytes) {
                None => cleaned.text.clone(),
                Some(cut) => {
                    // Save the raw output, not the cleaned text: the log is
                    // the full record for the user.
                    let path = self.save_log(call, &raw);
                    let marker = omission_marker(cut.omitted_lines, path.as_deref(), 0);
                    full_output_path = path;
                    format!("{}{marker}{}", cut.head, cut.tail)
                }
            };
            (cleaned, body)
        } else {
            // The middle was never kept: clean the two ends on their own
            // whole lines and count the dropped lines as omitted.
            let head = &output.head[..output
                .head
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(output.head.len(), |i| i + 1)];
            let tail = &output.tail[output
                .tail
                .iter()
                .position(|&b| b == b'\n')
                .map_or(0, |i| i + 1)..];
            let head = self.clean(&String::from_utf8_lossy(head), command);
            let tail = self.clean(&String::from_utf8_lossy(tail), command);
            let cut = head_tail_split(&head.text, &tail.text, self.max_output_bytes);
            let omitted = cut.omitted_lines + output.dropped_lines as usize;
            let skipped = output.log.as_ref().map_or(0, |log| log.skipped);
            let path = output.log.as_ref().map(|log| log.path.clone());
            let marker = omission_marker(omitted, path.as_deref(), skipped);
            let body = format!("{}{marker}{}", cut.head, cut.tail);
            full_output_path = path;
            // The cleaning stats cover what was cleaned, not what was cut.
            let cleaned = Cleaned {
                original_bytes: head.original_bytes + tail.original_bytes,
                cleaned_bytes: head.cleaned_bytes + tail.cleaned_bytes,
                original_chars: head.original_chars + tail.original_chars,
                cleaned_chars: head.cleaned_chars + tail.cleaned_chars,
                text: String::new(),
            };
            (cleaned, body)
        };
        if body.trim().is_empty() {
            body = "(no output)".to_string();
        }
        if !body.ends_with('\n') {
            body.push('\n');
        }
        (cleaned, body, full_output_path)
    }

    /// The model reads cleaned output; the user's live view and the full log
    /// on disk stay raw. `command` selects the command-aware rules.
    fn clean(&self, raw: &str, command: &str) -> Cleaned {
        if self.clean {
            clean_output(raw, command)
        } else {
            let chars = raw.chars().count();
            Cleaned {
                original_bytes: raw.len(),
                cleaned_bytes: raw.len(),
                original_chars: chars,
                cleaned_chars: chars,
                text: raw.to_string(),
            }
        }
    }

    /// Where the full output of call `call_id` is saved when it is cut.
    fn log_path(&self, call_id: &str) -> Option<PathBuf> {
        let dir = self.log_dir.clone().unwrap_or_else(std::env::temp_dir);
        if std::fs::create_dir_all(&dir).is_err() {
            return None;
        }
        let safe_id: String = call_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .take(32)
            .collect();
        Some(dir.join(format!(
            "termide-agent-shell-{}-{safe_id}.log",
            termide_agent_core::now_millis()
        )))
    }

    fn save_log(&self, call: &ToolCall, text: &str) -> Option<PathBuf> {
        let path = self.log_path(&call.id)?;
        match std::fs::write(&path, text) {
            Ok(()) => Some(path),
            Err(error) => {
                log::warn!(
                    "cannot save full shell output to {}: {error}",
                    path.display()
                );
                None
            }
        }
    }
}

/// Why `name` cannot name a terminal session, if it cannot.
pub(crate) fn check_session_name(name: &str) -> Result<(), String> {
    let fits = !name.is_empty()
        && name.chars().count() <= MAX_SESSION_NAME
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if fits {
        Ok(())
    } else {
        Err(format!(
            "`{name}` cannot name a terminal session: use up to {MAX_SESSION_NAME} letters, \
             digits, `.`, `_` and `-`"
        ))
    }
}

/// The line standing for the omitted middle of the output. `skipped` bytes
/// at the end of the output did not fit the log.
fn omission_marker(omitted_lines: usize, log: Option<&Path>, skipped: u64) -> String {
    match log {
        Some(path) if skipped > 0 => format!(
            "\n[... {omitted_lines} lines omitted; output saved to {} except its last {skipped} bytes ...]\n",
            path.display()
        ),
        Some(path) => format!(
            "\n[... {omitted_lines} lines omitted; full output saved to {} ...]\n",
            path.display()
        ),
        None => format!("\n[... {omitted_lines} lines omitted ...]\n"),
    }
}

fn spawn(
    shell: &ShellProgram,
    command: &str,
    ctx: &ToolContext,
    shim_path: Option<&Path>,
) -> Result<Child, String> {
    let mut cmd = Command::new(&shell.program);
    cmd.arg("-c")
        .arg(command)
        .current_dir(&ctx.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Prepend the shim directory to PATH so a shim shadows the real command,
    // including inside pipelines. The child's own PATH is used as the base.
    if let Some(dir) = shim_path {
        let base = std::env::var_os("PATH").unwrap_or_default();
        let mut entries = vec![dir.to_path_buf()];
        entries.extend(std::env::split_paths(&base));
        if let Ok(joined) = std::env::join_paths(entries) {
            cmd.env("PATH", joined);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own session, hence own process group: killing the group takes
        // pipelines and background children with it, a Ctrl-C in termide
        // does not reach them, and with no controlling terminal nothing they
        // start (a debugger, `stty`, a password prompt) can open `/dev/tty`
        // and take termide's terminal out of raw mode.
        // SAFETY: setsid is async-signal-safe and touches no Rust state.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    cmd.spawn().map_err(|error| {
        format!(
            "cannot start {} in {}: {error}",
            shell.name,
            ctx.cwd.display()
        )
    })
}

fn kill_group(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as libc::pid_t;
        // SAFETY: plain syscall on a pid we spawned; a stale pid only yields ESRCH.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

/// Read what is left of the output once the command is over: until every
/// pipe is closed, or, when something outside the killed group still holds
/// one open, until the output has been quiet for [`DRAIN_QUIET`] (at most
/// [`DRAIN_MAX`]). Readers still blocked then are left to end with the pipe;
/// `true` when any was.
fn drain(readers: Vec<std::thread::JoinHandle<()>>, output: &Mutex<Capture>) -> bool {
    let started = Instant::now();
    let mut last_len = lock(output).total();
    let mut last_growth = started;
    while readers.iter().any(|reader| !reader.is_finished()) {
        let len = lock(output).total();
        if len != last_len {
            last_len = len;
            last_growth = Instant::now();
        }
        if last_growth.elapsed() >= DRAIN_QUIET || started.elapsed() >= DRAIN_MAX {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    let mut detached = false;
    for reader in readers {
        if reader.is_finished() {
            let _ = reader.join();
        } else {
            detached = true;
        }
    }
    detached
}

fn pump(mut stream: impl Read, sink: &Mutex<Capture>) {
    let mut buffer = [0u8; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => lock(sink).push(&buffer[..n]),
        }
    }
}

fn lock(output: &Mutex<Capture>) -> std::sync::MutexGuard<'_, Capture> {
    output
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(dir: &std::path::Path) -> ShellTool {
        ShellTool {
            log_dir: Some(dir.to_path_buf()),
            ..ShellTool::default()
        }
    }

    fn run_with(
        tool: &ShellTool,
        dir: &std::path::Path,
        args: Value,
        cancel: &CancelToken,
    ) -> (ToolResultMessage, Vec<String>) {
        let call = ToolCall {
            id: "b-1".into(),
            name: "shell".into(),
            arguments: args,
            extra_content: None,
        };
        let ctx = ToolContext::new(dir.to_path_buf());
        let mut updates = Vec::new();
        let result = tool.execute(
            &call,
            &ctx,
            &mut |update| {
                let ToolUpdate::Output(text) = update;
                updates.push(text);
            },
            cancel,
        );
        (result, updates)
    }

    fn run(dir: &std::path::Path, args: Value) -> ToolResultMessage {
        run_with(&tool(dir), dir, args, &CancelToken::new()).0
    }

    #[test]
    fn the_shell_is_the_one_chosen_and_named_to_the_model() {
        assert!(ShellProgram::new("/usr/local/bin/nu").is_none());
        assert!(ShellProgram::new("pwsh.exe").is_none());
        let fish = ShellProgram::new("/opt/homebrew/bin/fish").unwrap();
        assert_eq!(
            (fish.name.as_str(), fish.syntax),
            ("fish", ShellSyntax::Fish)
        );
        let bash = ShellTool::default();
        assert!(bash.description().contains("Run a command with bash in"));
        assert_eq!(bash.traits().shell, ShellSyntax::Posix);

        if Command::new("zsh").arg("-c").arg("true").status().is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let zsh = ShellTool {
            shell: ShellProgram::new("zsh").unwrap(),
            ..tool(dir.path())
        };
        assert!(zsh.description().contains("Run a command with zsh in"));
        assert!(!zsh.description().contains("{{"));
        assert!(zsh.parameters().to_string().contains("run with zsh -c"));
        assert_eq!(zsh.traits().shell, ShellSyntax::Zsh);
        let (result, _) = run_with(
            &zsh,
            dir.path(),
            json!({ "command": "echo ${ZSH_VERSION:+zsh}" }),
            &CancelToken::new(),
        );
        assert!(
            result.plain_text().starts_with("zsh"),
            "{}",
            result.plain_text()
        );

        if Command::new("fish").arg("-c").arg("true").status().is_err() {
            return;
        }
        let fish = ShellTool {
            shell: ShellProgram::new("fish").unwrap(),
            ..tool(dir.path())
        };
        let (result, _) = run_with(
            &fish,
            dir.path(),
            json!({ "command": "set x (string join - a b); and echo $x" }),
            &CancelToken::new(),
        );
        assert!(
            result.plain_text().starts_with("a-b"),
            "{}",
            result.plain_text()
        );
    }

    /// A command must not reach termide's own terminal: a debugger, `stty` or
    /// a password prompt opening `/dev/tty` would take it out of raw mode and
    /// leave every key echoed as an escape code. Only meaningful when the
    /// test itself runs with a controlling terminal.
    #[cfg(unix)]
    #[test]
    fn a_command_has_no_controlling_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let result = run(
            dir.path(),
            json!({ "command": "if (exec 3</dev/tty) 2>/dev/null; then echo TTY; else echo NO_TTY; fi" }),
        );
        assert!(
            result.plain_text().starts_with("NO_TTY"),
            "{}",
            result.plain_text()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_shim_on_path_shadows_the_real_command() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let shims = dir.path().join("shims");
        std::fs::create_dir_all(&shims).unwrap();
        let shim = shims.join("date");
        std::fs::write(&shim, "#!/bin/sh\necho SHIMMED\n").unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();

        let tool = ShellTool {
            log_dir: Some(dir.path().to_path_buf()),
            shim_path: Some(shims),
            ..ShellTool::default()
        };
        let result = run_with(
            &tool,
            dir.path(),
            json!({ "command": "date" }),
            &CancelToken::new(),
        )
        .0;
        assert!(!result.is_error, "{}", result.plain_text());
        assert_eq!(result.plain_text().trim(), "SHIMMED");

        // Without the shim dir the real `date` runs (not our fixed string).
        let plain = run(dir.path(), json!({ "command": "date +SHIMMED" }));
        assert_eq!(plain.plain_text().trim(), "SHIMMED");
        let real = run(dir.path(), json!({ "command": "date +%Y" }));
        assert_ne!(real.plain_text().trim(), "SHIMMED");
    }

    #[test]
    fn output_is_cleaned_for_the_model_but_the_raw_log_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        // Colour escapes plus a carriage-return progress redraw.
        let result = run(
            dir.path(),
            json!({ "command": "printf '\\033[32mok\\033[0m\\nP 10%%\\rP 100%%\\n'" }),
        );
        assert!(!result.is_error);
        assert_eq!(result.plain_text(), "ok\nP 100%\n");
        // Cleaning stats are reported for the UI.
        let details = result.details.unwrap();
        assert!(
            details["raw_bytes"].as_u64().unwrap() > details["cleaned_bytes"].as_u64().unwrap()
        );
        // And in characters, which tokens are estimated from.
        assert_eq!(details["cleaned_chars"].as_u64(), Some(10));
        assert!(details["raw_chars"].as_u64().unwrap() > 10);

        // Turning cleaning off passes the escapes through.
        let raw_tool = ShellTool {
            clean: false,
            ..tool(dir.path())
        };
        let (raw_result, _) = run_with(
            &raw_tool,
            dir.path(),
            json!({ "command": "printf '\\033[32mok\\033[0m\\n'" }),
            &CancelToken::new(),
        );
        assert!(raw_result.plain_text().contains("\u{1b}[32m"));
    }

    #[test]
    fn captures_stdout_stderr_and_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let ok = run(dir.path(), json!({ "command": "echo out; echo err >&2" }));
        assert!(!ok.is_error, "{}", ok.plain_text());
        let text = ok.plain_text();
        assert!(text.contains("out\n"));
        assert!(text.contains("err\n"));
        assert_eq!(ok.details.unwrap()["exit_code"], 0);

        let failed = run(dir.path(), json!({ "command": "exit 3" }));
        assert!(failed.is_error);
        assert!(failed.plain_text().contains("(no output)"));
        assert!(failed.plain_text().ends_with("[exit code 3]"));
    }

    #[test]
    fn runs_in_the_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker"), "").unwrap();
        let result = run(dir.path(), json!({ "command": "ls" }));
        assert_eq!(result.plain_text(), "marker\n");
    }

    #[test]
    fn timeout_kills_the_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let result = run(dir.path(), json!({ "command": "sleep 30", "timeout": 1 }));
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(result.is_error);
        assert!(result.plain_text().contains("timed out"));
        assert_eq!(result.details.unwrap()["timed_out"], true);
    }

    /// A process that leaves the group and keeps the output open (as an ssh
    /// `ControlPersist` master does) neither holds a timed-out command past
    /// its timeout nor a finished one past its end.
    #[cfg(unix)]
    #[test]
    fn a_detached_process_holding_the_output_does_not_hang_the_call() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = "perl -e 'use POSIX; fork and exit; POSIX::setsid(); sleep 30' &";

        let started = Instant::now();
        let result = run(
            dir.path(),
            json!({ "command": format!("{daemon} echo done") }),
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(!result.is_error, "{}", result.plain_text());
        assert!(result.plain_text().starts_with("done\n"));
        assert!(result.plain_text().contains("stopped reading"));
        assert_eq!(result.details.unwrap()["detached"], true);

        let started = Instant::now();
        let result = run(
            dir.path(),
            json!({ "command": format!("{daemon} sleep 30"), "timeout": 1 }),
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(result.plain_text().contains("timed out"));
    }

    #[test]
    fn cancel_stops_a_running_command() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancelToken::new();
        let canceller = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            canceller.cancel();
        });
        let started = Instant::now();
        let (result, _) = run_with(
            &tool(dir.path()),
            dir.path(),
            json!({ "command": "sleep 30" }),
            &cancel,
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(result.is_error);
        assert!(result.plain_text().contains("cancelled"));
    }

    #[test]
    fn long_output_is_truncated_and_saved() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ShellTool {
            max_output_bytes: 2048,
            ..tool(dir.path())
        };
        let (result, _) = run_with(
            &tool,
            dir.path(),
            json!({ "command": "seq 1 5000" }),
            &CancelToken::new(),
        );
        assert!(!result.is_error, "{}", result.plain_text());
        let text = result.plain_text();
        assert!(text.starts_with("1\n2\n"));
        assert!(text.contains("lines omitted; full output saved to"));
        assert!(text.contains("\n5000\n"));
        let details = result.details.unwrap();
        assert_eq!(details["truncated"], true);
        let saved = std::fs::read_to_string(details["full_output_path"].as_str().unwrap()).unwrap();
        assert_eq!(saved.lines().count(), 5000);
    }

    /// A command printing far more than is kept inline holds only both ends
    /// in memory; the log on disk still has every byte, and the model sees
    /// the usual head, omission marker and tail.
    #[test]
    fn endless_output_is_bounded_in_memory_and_spooled_to_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ShellTool {
            max_output_bytes: 2048,
            capture_bytes: 16 * 1024,
            ..tool(dir.path())
        };
        let (result, updates) = run_with(
            &tool,
            dir.path(),
            json!({ "command": "seq 1 200000" }),
            &CancelToken::new(),
        );
        assert!(!result.is_error, "{}", result.plain_text());
        let text = result.plain_text();
        assert!(text.starts_with("1\n2\n"), "{text}");
        assert!(
            text.contains("lines omitted; full output saved to"),
            "{text}"
        );
        assert!(text.contains("\n200000\n"), "{text}");
        // Head and tail lines plus the omitted count add up to the output.
        let shown = text.lines().filter(|l| l.parse::<u32>().is_ok()).count();
        let omitted: usize = text
            .split("[... ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .and_then(|n| n.parse().ok())
            .unwrap();
        assert!(
            (shown + omitted).abs_diff(200_000) <= 2,
            "{shown} + {omitted}"
        );
        let details = result.details.unwrap();
        assert_eq!(details["truncated"], true);
        assert!(details["dropped_bytes"].as_u64().unwrap() > 1_000_000);
        let saved = std::fs::read_to_string(details["full_output_path"].as_str().unwrap()).unwrap();
        assert_eq!(saved.lines().count(), 200_000);
        // No live update carries more than both ends and a marker.
        assert!(updates.iter().all(|u| u.len() < 2 * 16 * 1024 + 100));
    }

    /// Past its cap the log stops, and the marker says it is not whole.
    #[test]
    fn the_output_log_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ShellTool {
            max_output_bytes: 2048,
            capture_bytes: 4096,
            max_log_bytes: 64 * 1024,
            ..tool(dir.path())
        };
        let (result, _) = run_with(
            &tool,
            dir.path(),
            json!({ "command": "seq 1 100000" }),
            &CancelToken::new(),
        );
        let text = result.plain_text();
        assert!(text.contains("except its last"), "{text}");
        let details = result.details.unwrap();
        let path = details["full_output_path"].as_str().unwrap();
        let size = std::fs::metadata(path).unwrap().len();
        assert!(size < 64 * 1024 + 200, "{size}");
    }

    #[test]
    fn streams_partial_output_while_running() {
        let dir = tempfile::tempdir().unwrap();
        let (result, updates) = run_with(
            &tool(dir.path()),
            dir.path(),
            json!({ "command": "echo first; sleep 0.5; echo second" }),
            &CancelToken::new(),
        );
        assert!(!result.is_error);
        assert!(updates
            .iter()
            .any(|u| u.contains("first") && !u.contains("second")));
    }

    #[test]
    fn empty_command_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(dir.path(), json!({ "command": "   " })).is_error);
        assert!(run(dir.path(), json!({}))
            .plain_text()
            .contains("`command`"));
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use termide_agent_core::{
        AssistantMessage, IntentLog, ModelSpec, Provider, Request, SessionView, StreamEvent,
        TerminalEntry, TerminalOutput, TerminalRunOutcome, ThinkingLevel,
    };

    struct NoProvider;

    impl Provider for NoProvider {
        fn name(&self) -> &str {
            "none"
        }
        fn stream(
            &self,
            _request: &Request<'_>,
            _on_event: &mut dyn FnMut(StreamEvent),
            _cancel: &CancelToken,
        ) -> AssistantMessage {
            unreachable!("no model is called")
        }
    }

    /// Prints the command back and ends as told by `end`.
    struct Echo {
        end: TerminalRunEnd,
        seen: Mutex<Vec<(String, String, Duration)>>,
    }

    impl TerminalSessions for Echo {
        fn run(
            &self,
            run: &TerminalRun<'_>,
            on_output: &mut dyn FnMut(&[u8]),
        ) -> Result<TerminalRunOutcome, String> {
            self.seen.lock().unwrap().push((
                run.owner.to_string(),
                run.name.to_string(),
                run.timeout,
            ));
            on_output(format!("ran {}\r\n", run.command).as_bytes());
            Ok(TerminalRunOutcome {
                end: self.end,
                dropped: 0,
                log: None,
                opened: true,
            })
        }
        fn free_name(&self, _owner: &str, stem: &str) -> String {
            format!("{stem}-1")
        }
        fn list(&self, _owner: &str) -> Vec<TerminalEntry> {
            Vec::new()
        }
        fn read(&self, _owner: &str, _name: &str) -> Result<TerminalOutput, String> {
            Err("unused".into())
        }
        fn send(
            &self,
            _send: &termide_agent_core::TerminalSend<'_>,
        ) -> Result<termide_agent_core::TerminalSendOutcome, String> {
            Err("unused".into())
        }
        fn screen(
            &self,
            _owner: &str,
            _name: &str,
        ) -> Result<termide_agent_core::TerminalScreenText, String> {
            Err("unused".into())
        }
        fn interrupt(&self, _owner: &str, _name: &str) -> Result<(), String> {
            Ok(())
        }
        fn close(&self, _owner: &str, _name: &str) -> Result<(), String> {
            Ok(())
        }
        fn release(&self, _owner: &str) {}
    }

    fn call(tool: &ShellTool, args: Value, session: bool) -> ToolResultMessage {
        let mut ctx = ToolContext::new("/tmp");
        if session {
            ctx.session = Some(SessionView {
                id: Some("s1".into()),
                intent: IntentLog::new(),
                provider: Arc::new(NoProvider),
                model: ModelSpec {
                    provider: "none".into(),
                    id: "m".into(),
                    context_window: 8192,
                    max_tokens: None,
                    thinking: ThinkingLevel::Off,
                },
            });
        }
        let call = ToolCall {
            id: "c".into(),
            name: SHELL_TOOL.into(),
            arguments: args,
            extra_content: None,
        };
        tool.execute(&call, &ctx, &mut |_| {}, &CancelToken::new())
    }

    fn tool(end: TerminalRunEnd) -> (ShellTool, Arc<Echo>) {
        let echo = Arc::new(Echo {
            end,
            seen: Mutex::new(Vec::new()),
        });
        let tool = ShellTool {
            terminals: Some(echo.clone()),
            ..ShellTool::default()
        };
        (tool, echo)
    }

    #[test]
    fn a_named_session_runs_the_command_in_the_callers_terminal() {
        let (tool, echo) = tool(TerminalRunEnd::Finished(Some(0)));
        let result = call(&tool, json!({"command": "make", "session": "build"}), true);
        assert_eq!(result.plain_text(), "ran make\n");
        let (tool, _) = self::tool(TerminalRunEnd::TimedOut);
        let result = call(
            &tool,
            json!({"command": "npm run dev", "session": "dev", "timeout": 5}),
            true,
        );
        assert!(!result.is_error);
        assert!(result
            .plain_text()
            .ends_with("[still running in terminal session `dev` after 5 s; `terminals` reads what it prints, interrupts or closes it]"));
        assert_eq!(
            echo.seen.lock().unwrap()[0],
            ("s1".into(), "build".into(), Duration::from_secs(120))
        );
    }

    #[test]
    fn the_background_gets_a_session_of_its_own_and_a_short_wait() {
        let (tool, echo) = tool(TerminalRunEnd::TimedOut);
        let result = call(&tool, json!({"command": "watch", "background": true}), true);
        assert!(result
            .plain_text()
            .starts_with("[terminal session `bg-1`]\nran watch\n"));
        assert_eq!(
            echo.seen.lock().unwrap()[0],
            ("s1".into(), "bg-1".into(), BACKGROUND_WAIT)
        );
    }

    #[test]
    fn sessions_need_a_session_a_registry_and_a_fitting_name() {
        let (tool, _) = tool(TerminalRunEnd::Finished(Some(0)));
        assert!(call(&tool, json!({"command": "ls", "session": "a"}), false).is_error);
        assert!(call(&tool, json!({"command": "ls", "session": "a b"}), true).is_error);
        let bare = ShellTool::default();
        assert!(call(&bare, json!({"command": "ls", "background": true}), true).is_error);
        let (tool, _) = self::tool(TerminalRunEnd::Finished(Some(3)));
        let failed = call(&tool, json!({"command": "false", "session": "a"}), true);
        assert!(failed.is_error);
        assert!(failed.plain_text().ends_with("[exit code 3]"));
    }
}
