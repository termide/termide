//! Command hooks: external programs run on tool calls, in the shape Claude
//! Code, Gemini CLI and Cursor share — the event as JSON on stdin, a verdict
//! as JSON on stdout, exit code 2 to block with stderr as the reason.
//!
//! On `before_tool_call` the program receives
//! `{"event","hook","cwd","tool","arguments"}` and may answer
//! `{"decision":"allow"|"block"|"ask","reason":…,"arguments":{…}}`:
//! `block` skips the call, `allow` runs it without the mode's question (a
//! `deny` or `ask` rule and the `.git` guard still hold),
//! `ask` (or no decision) leaves that to the permission rules; `arguments`
//! replaces the model's. On `after_tool_call` it also receives
//! `"result":{"text","is_error"}` and may answer `{"text",…,"is_error"}` to
//! rewrite the result. A program that exits 2 blocks (before) or turns the
//! result into an error (after) with its stderr as the text. Any other
//! failure — another exit code, a timeout, a program that cannot start,
//! stdout that is not JSON — is logged and ignored, so a broken hook never
//! stops the agent.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use termide_agent_core::permissions::wildcard_match;
use termide_agent_core::{
    AssistantMessage, HookConfig, HookEvent, Hooks, ToolCall, ToolContext, ToolDecision,
    ToolResultContent, ToolResultMessage,
};

pub struct CommandHooks {
    hooks: Vec<(String, HookConfig)>,
    cwd: PathBuf,
}

impl CommandHooks {
    #[must_use]
    pub fn new(hooks: BTreeMap<String, HookConfig>, cwd: PathBuf) -> Self {
        Self {
            hooks: hooks.into_iter().collect(),
            cwd,
        }
    }

    fn matching(&self, event: HookEvent, tool: &str) -> Vec<(String, HookConfig)> {
        self.hooks
            .iter()
            .filter(|(_, config)| {
                config.event == event
                    && (config.tools.is_empty()
                        || config
                            .tools
                            .iter()
                            .any(|pattern| wildcard_match(pattern, tool)))
            })
            .cloned()
            .collect()
    }
}

impl Hooks for CommandHooks {
    fn before_tool_call(&mut self, call: &ToolCall, ctx: &ToolContext) -> ToolDecision {
        let mut arguments = call.arguments.clone();
        let mut replaced = false;
        for (name, config) in self.matching(HookEvent::BeforeToolCall, &call.name) {
            let input = json!({
                "event": "before_tool_call",
                "hook": name,
                "cwd": ctx.cwd,
                "tool": call.name,
                "arguments": arguments,
            });
            let Some(run) = run_hook(&name, &config, &self.cwd, &input) else {
                continue;
            };
            if run.code == Some(2) {
                let reason = run.stderr.trim();
                return ToolDecision::Block {
                    reason: if reason.is_empty() {
                        format!("blocked by hook {name}")
                    } else {
                        format!("hook {name}: {reason}")
                    },
                };
            }
            let Some(answer) = run.answer(&name) else {
                continue;
            };
            if let Some(new_arguments) = answer.get("arguments").filter(|a| a.is_object()) {
                arguments = new_arguments.clone();
                replaced = true;
            }
            match answer["decision"].as_str() {
                Some("block") => {
                    let reason = answer["reason"].as_str().unwrap_or("").trim();
                    return ToolDecision::Block {
                        reason: if reason.is_empty() {
                            format!("blocked by hook {name}")
                        } else {
                            format!("hook {name}: {reason}")
                        },
                    };
                }
                Some("allow") => {
                    return ToolDecision::Approve {
                        arguments: replaced.then_some(arguments),
                    };
                }
                _ => {}
            }
        }
        if replaced {
            ToolDecision::Replace { arguments }
        } else {
            ToolDecision::Allow
        }
    }

    fn after_tool_call(
        &mut self,
        call: &ToolCall,
        mut result: ToolResultMessage,
    ) -> ToolResultMessage {
        for (name, config) in self.matching(HookEvent::AfterToolCall, &call.name) {
            let input = json!({
                "event": "after_tool_call",
                "hook": name,
                "cwd": self.cwd,
                "tool": call.name,
                "arguments": call.arguments,
                "result": { "text": result.plain_text(), "is_error": result.is_error },
            });
            let Some(run) = run_hook(&name, &config, &self.cwd, &input) else {
                continue;
            };
            if run.code == Some(2) {
                let text = run.stderr.trim();
                result.content = vec![ToolResultContent::Text {
                    text: if text.is_empty() {
                        format!("result rejected by hook {name}")
                    } else {
                        format!("hook {name}: {text}")
                    },
                }];
                result.is_error = true;
                continue;
            }
            let Some(answer) = run.answer(&name) else {
                continue;
            };
            if let Some(text) = answer["text"].as_str() {
                result.content = vec![ToolResultContent::Text {
                    text: text.to_string(),
                }];
            }
            if let Some(is_error) = answer["is_error"].as_bool() {
                result.is_error = is_error;
            }
        }
        result
    }

    fn should_stop_after_turn(&mut self, _message: &AssistantMessage) -> bool {
        false
    }
}

struct HookRun {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl HookRun {
    /// The JSON the hook printed, when it exited 0 and printed any; other
    /// outcomes are logged and count as "no opinion".
    fn answer(&self, name: &str) -> Option<Value> {
        if self.code != Some(0) {
            log::warn!(
                "hook {name} exited with {:?}, ignored; stderr: {}",
                self.code,
                self.stderr.trim()
            );
            return None;
        }
        let stdout = self.stdout.trim();
        if stdout.is_empty() {
            return None;
        }
        match serde_json::from_str::<Value>(stdout) {
            Ok(value) if value.is_object() => Some(value),
            Ok(_) => {
                log::warn!("hook {name}: stdout is JSON but not an object, ignored");
                None
            }
            Err(error) => {
                log::warn!("hook {name}: stdout is not JSON ({error}), ignored");
                None
            }
        }
    }
}

/// Run the program with `input` on stdin, killing it at the timeout.
/// `None` when it could not be started or did not finish in time.
fn run_hook(name: &str, config: &HookConfig, cwd: &Path, input: &Value) -> Option<HookRun> {
    let mut command = Command::new(&config.command);
    command
        .args(&config.args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A group of its own, so what the hook starts in the background ends
    // with it at the timeout.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            log::warn!("hook {name}: cannot start {}: {error}", config.command);
            return None;
        }
    };
    // Written from a thread: the input carries a whole tool result, more
    // than a pipe holds, and a hook that does not read it, or writes before
    // it reads, would otherwise hold the agent past the timeout. Once the
    // hook ends, the write fails and the thread ends with it.
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_string();
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let stdout = child.stdout.take().map(Drain::start);
    let stderr = child.stderr.take().map(Drain::start);

    let deadline = Instant::now() + Duration::from_secs(config.timeout_secs.max(1));
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                kill_group(&child);
                let _ = child.kill();
                let _ = child.wait();
                log::warn!(
                    "hook {name}: no answer within {} s, ignored",
                    config.timeout_secs
                );
                return None;
            }
            Err(error) => {
                log::warn!("hook {name}: {error}");
                return None;
            }
        }
    };
    // Something the hook left running in the background can keep its
    // output open past its exit: that waits a moment, within the timeout,
    // then the group is killed and what arrived is read.
    let closed_by = deadline.min(Instant::now() + EXIT_GRACE);
    let closed = [&stdout, &stderr]
        .into_iter()
        .flatten()
        .all(|drain| drain.wait_closed(closed_by));
    if !closed {
        kill_group(&child);
        let settled = Instant::now() + Duration::from_millis(100);
        for drain in [&stdout, &stderr].into_iter().flatten() {
            drain.wait_closed(settled);
        }
    }
    let collect = |drain: Option<Drain>| drain.map(|d| d.text()).unwrap_or_default();
    Some(HookRun {
        code,
        stdout: collect(stdout),
        stderr: collect(stderr),
    })
}

/// How long the hook's output may stay open after it exits.
const EXIT_GRACE: Duration = Duration::from_millis(500);

/// Kill the hook's process group: it and whatever it started.
fn kill_group(child: &std::process::Child) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: plain syscall; the group is the hook's own (`process_group(0)`).
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = child;
}

/// A pipe drained on a thread so a chatty hook cannot block on a full pipe;
/// what it read so far can be taken without waiting for the end.
struct Drain {
    text: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    closed: std::sync::mpsc::Receiver<()>,
}

impl Drain {
    fn start(mut pipe: impl Read + Send + 'static) -> Self {
        let text = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (done, closed) = std::sync::mpsc::channel::<()>();
        let sink = text.clone();
        std::thread::spawn(move || {
            let _done = done;
            let mut buf = [0u8; 8192];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => sink
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .extend_from_slice(&buf[..n]),
                }
            }
        });
        Self { text, closed }
    }

    /// Whether the pipe reached its end by `until`.
    fn wait_closed(&self, until: Instant) -> bool {
        let left = until.saturating_duration_since(Instant::now());
        matches!(
            self.closed.recv_timeout(left),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        )
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(
            &self
                .text
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_owned()
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    fn hook(event: HookEvent, tools: &[&str], script: &str, timeout: u64) -> HookConfig {
        HookConfig {
            event,
            command: "sh".into(),
            args: vec!["-c".into(), script.into()],
            tools: tools.iter().map(|t| t.to_string()).collect(),
            timeout_secs: timeout,
            enabled: true,
            from_project: false,
        }
    }

    fn bash(command: &str) -> ToolCall {
        ToolCall {
            id: "c".into(),
            name: "shell".into(),
            arguments: json!({ "command": command }),
            extra_content: None,
        }
    }

    fn ctx() -> ToolContext {
        ToolContext::new(PathBuf::from("/tmp"))
    }

    #[test]
    fn before_hooks_block_rewrite_and_approve_in_name_order() {
        let mut hooks = BTreeMap::new();
        // a: rewrite the command through the JSON answer.
        hooks.insert(
            "a-rewrite".to_string(),
            hook(
                HookEvent::BeforeToolCall,
                &["shell"],
                r#"input=$(cat); case "$input" in *'"tool":"shell"'*) printf '{"arguments":{"command":"echo rewritten"}}';; esac"#,
                5,
            ),
        );
        // b: block anything mentioning rm with exit 2 and a reason on stderr.
        hooks.insert(
            "b-guard".to_string(),
            hook(
                HookEvent::BeforeToolCall,
                &[],
                r#"input=$(cat); case "$input" in *rm*) echo "no deleting" >&2; exit 2;; esac; printf '{"decision":"ask"}'"#,
                5,
            ),
        );
        // c: approve echo commands, so the permission prompt is skipped.
        hooks.insert(
            "c-approve".to_string(),
            hook(
                HookEvent::BeforeToolCall,
                &["sh*"],
                r#"input=$(cat); case "$input" in *echo*) printf '{"decision":"allow"}';; esac"#,
                5,
            ),
        );
        // d: broken hooks change nothing.
        hooks.insert(
            "d-crash".to_string(),
            hook(HookEvent::BeforeToolCall, &[], "echo oops >&2; exit 1", 5),
        );
        hooks.insert(
            "e-slow".to_string(),
            hook(HookEvent::BeforeToolCall, &[], "sleep 5", 1),
        );
        hooks.insert(
            "f-missing".to_string(),
            HookConfig {
                command: "/nonexistent/termide-hook".into(),
                ..hook(HookEvent::BeforeToolCall, &[], "", 5)
            },
        );
        let mut chain = CommandHooks::new(hooks, PathBuf::from("/tmp"));

        // The rewrite from a reaches b (which sees "echo", not rm) and c approves it.
        assert_eq!(
            chain.before_tool_call(&bash("ls"), &ctx()),
            ToolDecision::Approve {
                arguments: Some(json!({ "command": "echo rewritten" }))
            }
        );
        // A hook that does not match the tool is skipped: read gets no rewrite
        // and no approval, only the guard, which lets it pass.
        let read = ToolCall {
            id: "r".into(),
            name: "read".into(),
            arguments: json!({ "path": "x" }),
            extra_content: None,
        };
        assert_eq!(chain.before_tool_call(&read, &ctx()), ToolDecision::Allow);
        let rm = ToolCall {
            id: "r".into(),
            name: "read".into(),
            arguments: json!({ "path": "rm-notes" }),
            extra_content: None,
        };
        assert_eq!(
            chain.before_tool_call(&rm, &ctx()),
            ToolDecision::Block {
                reason: "hook b-guard: no deleting".into()
            }
        );
    }

    #[test]
    fn after_hooks_rewrite_or_reject_the_result() {
        let mut hooks = BTreeMap::new();
        hooks.insert(
            "a-trim".to_string(),
            hook(
                HookEvent::AfterToolCall,
                &["shell"],
                r#"input=$(cat); case "$input" in *'a long output'*) printf '{"text":"trimmed output"}';; esac"#,
                5,
            ),
        );
        hooks.insert(
            "b-reject".to_string(),
            hook(HookEvent::AfterToolCall, &["shell"], r#"input=$(cat); case "$input" in *secret*) echo "leaks a secret" >&2; exit 2;; esac"#, 5),
        );
        let mut chain = CommandHooks::new(hooks, PathBuf::from("/tmp"));
        let call = bash("cat file");
        let result = chain.after_tool_call(&call, ToolResultMessage::text(&call, "a long output"));
        assert_eq!(result.plain_text(), "trimmed output");
        assert!(!result.is_error);
        let result =
            chain.after_tool_call(&call, ToolResultMessage::text(&call, "the secret is x"));
        assert_eq!(result.plain_text(), "hook b-reject: leaks a secret");
        assert!(result.is_error);
    }

    /// A result larger than a pipe holds, given to a hook that never reads
    /// it, still ends at the hook's timeout, and leaves the result as it was.
    #[test]
    fn a_hook_that_reads_nothing_still_times_out() {
        let mut hooks = BTreeMap::new();
        hooks.insert(
            "idle".to_string(),
            hook(HookEvent::AfterToolCall, &["shell"], "sleep 10", 1),
        );
        let mut chain = CommandHooks::new(hooks, PathBuf::from("/tmp"));
        let call = bash("cat big");
        let big = "x".repeat(1 << 20);
        let started = Instant::now();
        let result = chain.after_tool_call(&call, ToolResultMessage::text(&call, &big));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(result.plain_text().len(), big.len());
    }

    /// A hook that answers and exits but leaves a job holding its output is
    /// read at once, and the job ends with it.
    #[test]
    fn a_background_job_neither_holds_the_hook_nor_outlives_it() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let script = format!(
            r#"cat >/dev/null; (sleep 2; touch '{}') & printf '{{"decision":"block","reason":"held"}}'"#,
            marker.display()
        );
        let mut hooks = BTreeMap::new();
        hooks.insert(
            "bg".to_string(),
            hook(HookEvent::BeforeToolCall, &[], &script, 30),
        );
        let mut chain = CommandHooks::new(hooks, PathBuf::from("/tmp"));
        let started = Instant::now();
        let decision = chain.before_tool_call(&bash("ls"), &ctx());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            decision,
            ToolDecision::Block {
                reason: "hook bg: held".into()
            }
        );
        std::thread::sleep(Duration::from_secs(3));
        assert!(!marker.exists(), "the background job outlived the hook");
    }

    /// At the timeout what the hook started goes with it.
    #[test]
    fn the_timeout_kills_what_the_hook_started() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let script = format!(
            "cat >/dev/null; (sleep 2; touch '{}') >/dev/null 2>&1 & sleep 10",
            marker.display()
        );
        let mut hooks = BTreeMap::new();
        hooks.insert(
            "slow".to_string(),
            hook(HookEvent::BeforeToolCall, &[], &script, 1),
        );
        let mut chain = CommandHooks::new(hooks, PathBuf::from("/tmp"));
        assert_eq!(
            chain.before_tool_call(&bash("ls"), &ctx()),
            ToolDecision::Allow
        );
        std::thread::sleep(Duration::from_secs(3));
        assert!(!marker.exists(), "the background job outlived the timeout");
    }
}
