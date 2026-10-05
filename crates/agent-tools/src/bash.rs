//! `bash`: run a shell command, stream its output, kill it on timeout or
//! cancel, keep head and tail of long output inline and the full log on disk.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use termide_agent_core::ToolText;
use termide_agent_core::{CancelToken, Tool, ToolCall, ToolContext, ToolResultMessage, ToolUpdate};

use crate::args::{optional_u64, required_str};
use crate::clean::clean_output;
use crate::truncate::{head_tail, SHELL_MAX_BYTES};

/// How often the accumulated output is pushed to the UI while running.
const UPDATE_INTERVAL: Duration = Duration::from_millis(200);
/// How often the child is polled for exit, cancel and timeout.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

pub struct BashTool {
    pub default_timeout: Duration,
    pub max_timeout: Duration,
    pub max_output_bytes: usize,
    /// Where full logs of truncated output go; the system temp dir if `None`.
    pub log_dir: Option<PathBuf>,
    /// Clean and compact output for the model (strip escapes, collapse noise)
    /// before truncation. The user still sees the raw stream and full log.
    pub clean: bool,
    /// Directory of command shims prepended to `PATH`, so an executable named
    /// after a command shadows the real one (a token-saving wrapper). `None`
    /// leaves `PATH` untouched.
    pub shim_path: Option<PathBuf>,
}

impl Default for BashTool {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(120),
            max_timeout: Duration::from_secs(600),
            max_output_bytes: SHELL_MAX_BYTES,
            log_dir: None,
            clean: true,
            shim_path: None,
        }
    }
}

impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        &ToolText::seed("bash").description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The command line to run with bash -c" },
                "timeout": { "type": "integer", "minimum": 1, "description": "Seconds before the command is killed (default 120, max 600)" }
            },
            "required": ["command"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("bash").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("bash").guidelines
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

        let run = match self.run(command, ctx, timeout, on_update, cancel) {
            Ok(run) => run,
            Err(message) => return ToolResultMessage::error(call, message),
        };
        self.render(call, run)
    }
}

struct Run {
    output: Vec<u8>,
    exit_code: Option<i32>,
    timed_out: bool,
    cancelled: bool,
    duration: Duration,
    /// The command that ran, for the command-aware cleaning rules.
    command: String,
}

impl BashTool {
    fn run(
        &self,
        command: &str,
        ctx: &ToolContext,
        timeout: Duration,
        on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> Result<Run, String> {
        let mut child = spawn(command, ctx, self.shim_path.as_deref())?;
        let started = Instant::now();
        let output = Arc::new(Mutex::new(Vec::new()));
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
        let mut last_len = 0;
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
                let snapshot = lock(&output);
                if snapshot.len() != last_len {
                    last_len = snapshot.len();
                    on_update(ToolUpdate::Output(
                        String::from_utf8_lossy(&snapshot).into_owned(),
                    ));
                }
                last_update = Instant::now();
            }
            std::thread::sleep(POLL_INTERVAL);
        };
        for reader in readers {
            let _ = reader.join();
        }
        let output = std::mem::take(&mut *lock_mut(&output));
        Ok(Run {
            output,
            exit_code,
            timed_out,
            cancelled,
            duration: started.elapsed(),
            command: command.to_string(),
        })
    }

    fn render(&self, call: &ToolCall, run: Run) -> ToolResultMessage {
        let raw = String::from_utf8_lossy(&run.output).into_owned();
        // The model reads cleaned output; the user's live view and the full
        // log on disk stay raw. `command` selects the command-aware rules.
        let cleaned = if self.clean {
            clean_output(&raw, &run.command)
        } else {
            crate::clean::Cleaned {
                original_bytes: raw.len(),
                cleaned_bytes: raw.len(),
                text: raw.clone(),
            }
        };
        let text = cleaned.text;
        let mut full_output_path = None;
        let mut body = match head_tail(&text, self.max_output_bytes) {
            None => text.clone(),
            Some(cut) => {
                // Save the raw output, not the cleaned text: the log is the
                // full record for the user.
                let path = self.save_log(call, &raw);
                let marker = match &path {
                    Some(path) => format!(
                        "\n[... {} lines omitted; full output saved to {} ...]\n",
                        cut.omitted_lines,
                        path.display()
                    ),
                    None => format!("\n[... {} lines omitted ...]\n", cut.omitted_lines),
                };
                full_output_path = path;
                format!("{}{marker}{}", cut.head, cut.tail)
            }
        };
        if body.trim().is_empty() {
            body = "(no output)".to_string();
        }
        if !body.ends_with('\n') {
            body.push('\n');
        }

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

        let details = json!({
            "exit_code": run.exit_code,
            "timed_out": run.timed_out,
            "cancelled": run.cancelled,
            "duration_ms": run.duration.as_millis() as u64,
            "truncated": full_output_path.is_some(),
            "full_output_path": full_output_path,
            "raw_bytes": cleaned.original_bytes,
            "cleaned_bytes": cleaned.cleaned_bytes,
        });
        let result = if is_error {
            ToolResultMessage::error(call, body)
        } else {
            ToolResultMessage::text(call, body)
        };
        result.with_details(details)
    }

    fn save_log(&self, call: &ToolCall, text: &str) -> Option<PathBuf> {
        let dir = self.log_dir.clone().unwrap_or_else(std::env::temp_dir);
        if std::fs::create_dir_all(&dir).is_err() {
            return None;
        }
        let safe_id: String = call
            .id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .take(32)
            .collect();
        let path = dir.join(format!(
            "termide-agent-bash-{}-{safe_id}.log",
            termide_agent_core::now_millis()
        ));
        match std::fs::write(&path, text) {
            Ok(()) => Some(path),
            Err(error) => {
                log::warn!(
                    "cannot save full bash output to {}: {error}",
                    path.display()
                );
                None
            }
        }
    }
}

fn spawn(command: &str, ctx: &ToolContext, shim_path: Option<&Path>) -> Result<Child, String> {
    let mut cmd = Command::new("bash");
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
        // Own process group, so killing the group takes pipelines and
        // background children with it and a Ctrl-C in termide does not.
        // SAFETY: setpgid is async-signal-safe and touches no Rust state.
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
    }
    cmd.spawn()
        .map_err(|error| format!("cannot start bash in {}: {error}", ctx.cwd.display()))
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

fn pump(mut stream: impl Read, sink: &Mutex<Vec<u8>>) {
    let mut buffer = [0u8; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => lock_mut(sink).extend_from_slice(&buffer[..n]),
        }
    }
}

fn lock(output: &Mutex<Vec<u8>>) -> Vec<u8> {
    lock_mut(output).clone()
}

fn lock_mut(output: &Mutex<Vec<u8>>) -> std::sync::MutexGuard<'_, Vec<u8>> {
    output
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(dir: &std::path::Path) -> BashTool {
        BashTool {
            log_dir: Some(dir.to_path_buf()),
            ..BashTool::default()
        }
    }

    fn run_with(
        tool: &BashTool,
        dir: &std::path::Path,
        args: Value,
        cancel: &CancelToken,
    ) -> (ToolResultMessage, Vec<String>) {
        let call = ToolCall {
            id: "b-1".into(),
            name: "bash".into(),
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

        let tool = BashTool {
            log_dir: Some(dir.path().to_path_buf()),
            shim_path: Some(shims),
            ..BashTool::default()
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

        // Turning cleaning off passes the escapes through.
        let raw_tool = BashTool {
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
        let tool = BashTool {
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
