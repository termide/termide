//! `terminals`: the agent's terminal sessions — list them, read what one
//! printed since the agent last looked, interrupt or close one. `shell`
//! opens them; see [`termide_agent_core::terminals`].

use std::sync::Arc;

use serde_json::{json, Value};
use termide_agent_core::terminals::duration;
use termide_agent_core::{
    CancelToken, TerminalSessions, TerminalState, Tool, ToolCall, ToolContext, ToolResultMessage,
    ToolText, ToolUpdate,
};

use crate::args::optional_str;
use crate::clean::clean_output;
use crate::shell::check_session_name;
use crate::truncate::{head_tail, SHELL_MAX_BYTES};

const NAME: &str = "terminals";

/// The `terminals` tool.
pub struct TerminalsTool {
    terminals: Arc<dyn TerminalSessions>,
}

impl TerminalsTool {
    #[must_use]
    pub fn new(terminals: Arc<dyn TerminalSessions>) -> Self {
        Self { terminals }
    }
}

fn state(state: &TerminalState) -> String {
    match state {
        TerminalState::Idle => "idle".into(),
        TerminalState::Running { command, elapsed } => format!(
            "running `{}` for {}",
            crate::jobs::first_line(command, 80),
            duration(*elapsed)
        ),
        TerminalState::Exited => "exited".into(),
    }
}

impl Tool for TerminalsTool {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        &ToolText::seed(NAME).description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "read", "interrupt", "close"],
                    "description": "What to do; `list` when left out"
                },
                "session": {
                    "type": "string",
                    "description": "read, interrupt, close: the terminal session's name"
                }
            }
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed(NAME).snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed(NAME).guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        let Some(owner) = ctx.session.as_ref().and_then(|view| view.id.as_deref()) else {
            return ToolResultMessage::error(call, "terminal sessions are not available here");
        };
        let action = match optional_str(call, "action") {
            Ok(action) => action.unwrap_or("list"),
            Err(message) => return ToolResultMessage::error(call, message),
        };
        if action == "list" {
            let entries = self.terminals.list(owner);
            if entries.is_empty() {
                return ToolResultMessage::text(call, "You have no terminal sessions open.");
            }
            let lines: Vec<String> = entries
                .iter()
                .map(|entry| {
                    format!(
                        "- {} · {} · open {}",
                        entry.name,
                        state(&entry.state),
                        duration(entry.age)
                    )
                })
                .collect();
            return ToolResultMessage::text(call, lines.join("\n"));
        }
        let name = match optional_str(call, "session") {
            Ok(Some(name)) => name.trim(),
            Ok(None) => {
                return ToolResultMessage::error(call, format!("`{action}` needs `session`"))
            }
            Err(message) => return ToolResultMessage::error(call, message),
        };
        if let Err(message) = check_session_name(name) {
            return ToolResultMessage::error(call, message);
        }
        let done = match action {
            "read" => return self.read(call, owner, name),
            "interrupt" => self
                .terminals
                .interrupt(owner, name)
                .map(|()| format!("Sent Ctrl+C to terminal session `{name}`.")),
            "close" => self
                .terminals
                .close(owner, name)
                .map(|()| format!("Closed terminal session `{name}`.")),
            other => Err(format!(
                "unknown action `{other}`: use list, read, interrupt or close"
            )),
        };
        match done {
            Ok(text) => ToolResultMessage::text(call, text),
            Err(message) => ToolResultMessage::error(call, message),
        }
    }
}

impl TerminalsTool {
    fn read(&self, call: &ToolCall, owner: &str, name: &str) -> ToolResultMessage {
        let output = match self.terminals.read(owner, name) {
            Ok(output) => output,
            Err(message) => return ToolResultMessage::error(call, message),
        };
        let raw = String::from_utf8_lossy(&output.data);
        let cleaned = clean_output(&raw, "");
        let mut body = match head_tail(&cleaned.text, SHELL_MAX_BYTES) {
            None => cleaned.text.clone(),
            Some(cut) => format!(
                "{}\n[... {} lines omitted ...]\n{}",
                cut.head, cut.omitted_lines, cut.tail
            ),
        };
        if body.trim().is_empty() {
            body = "(nothing new)".into();
        }
        if !body.ends_with('\n') {
            body.push('\n');
        }
        if output.dropped > 0 {
            body = format!(
                "[{} earlier bytes are no longer in memory]\n{body}",
                output.dropped
            );
        }
        body.push_str(&format!(
            "[terminal session `{name}`: {}",
            state(&output.state)
        ));
        if let Some(log) = &output.log {
            body.push_str(&format!("; complete output in {}", log.display()));
        }
        body.push(']');
        ToolResultMessage::text(call, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;
    use termide_agent_core::{
        IntentLog, ModelSpec, SessionView, TerminalEntry, TerminalOutput, TerminalRun,
        TerminalRunOutcome,
    };

    struct NoProvider;

    impl termide_agent_core::Provider for NoProvider {
        fn name(&self) -> &str {
            "none"
        }
        fn stream(
            &self,
            _request: &termide_agent_core::Request<'_>,
            _on_event: &mut dyn FnMut(termide_agent_core::StreamEvent),
            _cancel: &CancelToken,
        ) -> termide_agent_core::AssistantMessage {
            unreachable!("no model is called")
        }
    }

    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
    }

    impl TerminalSessions for Fake {
        fn run(
            &self,
            _run: &TerminalRun<'_>,
            _on_output: &mut dyn FnMut(&[u8]),
        ) -> Result<TerminalRunOutcome, String> {
            unreachable!()
        }
        fn free_name(&self, _owner: &str, stem: &str) -> String {
            format!("{stem}-1")
        }
        fn list(&self, owner: &str) -> Vec<TerminalEntry> {
            assert_eq!(owner, "s1");
            vec![TerminalEntry {
                name: "dev".into(),
                state: TerminalState::Running {
                    command: "npm run dev".into(),
                    elapsed: Duration::from_secs(75),
                },
                age: Duration::from_secs(80),
                log: None,
            }]
        }
        fn read(&self, _owner: &str, name: &str) -> Result<TerminalOutput, String> {
            self.calls.lock().unwrap().push(format!("read {name}"));
            Ok(TerminalOutput {
                data: b"\x1b[32mready\x1b[0m on :3000\r\n".to_vec(),
                dropped: 0,
                state: TerminalState::Idle,
                log: None,
            })
        }
        fn interrupt(&self, _owner: &str, name: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("interrupt {name}"));
            Ok(())
        }
        fn close(&self, _owner: &str, name: &str) -> Result<(), String> {
            Err(format!("you have no terminal session `{name}`"))
        }
        fn release(&self, _owner: &str) {}
    }

    fn call(tool: &TerminalsTool, args: Value, session: Option<&str>) -> ToolResultMessage {
        let mut ctx = ToolContext::new("/tmp");
        ctx.session = session.map(|id| SessionView {
            id: Some(id.into()),
            intent: IntentLog::new(),
            provider: Arc::new(NoProvider),
            model: ModelSpec {
                provider: "none".into(),
                id: "m".into(),
                context_window: 8192,
                max_tokens: None,
                thinking: termide_agent_core::ThinkingLevel::Off,
            },
        });
        let call = ToolCall {
            id: "t".into(),
            name: NAME.into(),
            arguments: args,
            extra_content: None,
        };
        tool.execute(&call, &ctx, &mut |_| {}, &CancelToken::new())
    }

    #[test]
    fn lists_reads_and_acts_on_the_callers_sessions() {
        let fake = Arc::new(Fake::default());
        let tool = TerminalsTool::new(fake.clone());
        let listed = call(&tool, json!({}), Some("s1"));
        assert_eq!(
            listed.plain_text(),
            "- dev · running `npm run dev` for 1 min · open 1 min"
        );
        let read = call(
            &tool,
            json!({"action": "read", "session": "dev"}),
            Some("s1"),
        );
        assert_eq!(
            read.plain_text(),
            "ready on :3000\n[terminal session `dev`: idle]"
        );
        let stopped = call(
            &tool,
            json!({"action": "interrupt", "session": "dev"}),
            Some("s1"),
        );
        assert!(!stopped.is_error);
        let closed = call(
            &tool,
            json!({"action": "close", "session": "nope"}),
            Some("s1"),
        );
        assert!(closed.is_error);
        assert!(call(&tool, json!({"action": "read"}), Some("s1")).is_error);
        assert!(call(&tool, json!({}), None).is_error);
        assert_eq!(*fake.calls.lock().unwrap(), ["read dev", "interrupt dev"]);
    }
}
