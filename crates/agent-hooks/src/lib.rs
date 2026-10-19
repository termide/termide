//! Command hooks: external programs run on tool calls, in the shape Claude
//! Code, Gemini CLI and Cursor share — the event as JSON on stdin, a verdict
//! as JSON on stdout, exit code 2 to block with stderr as the reason.
//!
//! On `before_tool_call` the program receives
//! `{"event","hook","cwd","tool","arguments"}` and may answer
//! `{"decision":"allow"|"block"|"ask","reason":…,"arguments":{…}}`:
//! `block` skips the call, `allow` runs it without asking anyone else,
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
    let mut child = match Command::new(&config.command)
        .args(&config.args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            log::warn!("hook {name}: cannot start {}: {error}", config.command);
            return None;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.to_string().as_bytes());
    }
    let stdout = child.stdout.take().map(read_all_in_background);
    let stderr = child.stderr.take().map(read_all_in_background);

    let deadline = Instant::now() + Duration::from_secs(config.timeout_secs.max(1));
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
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
    let collect = |handle: Option<std::thread::JoinHandle<String>>| {
        handle.and_then(|h| h.join().ok()).unwrap_or_default()
    };
    Some(HookRun {
        code,
        stdout: collect(stdout),
        stderr: collect(stderr),
    })
}

/// Drain a pipe on a thread so a chatty hook cannot block on a full pipe.
fn read_all_in_background(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = pipe.read_to_string(&mut text);
        text
    })
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
            name: "bash".into(),
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
                &["bash"],
                r#"input=$(cat); case "$input" in *'"tool":"bash"'*) printf '{"arguments":{"command":"echo rewritten"}}';; esac"#,
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
                &["ba*"],
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
                &["bash"],
                r#"input=$(cat); case "$input" in *'a long output'*) printf '{"text":"trimmed output"}';; esac"#,
                5,
            ),
        );
        hooks.insert(
            "b-reject".to_string(),
            hook(HookEvent::AfterToolCall, &["bash"], r#"input=$(cat); case "$input" in *secret*) echo "leaks a secret" >&2; exit 2;; esac"#, 5),
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
}
