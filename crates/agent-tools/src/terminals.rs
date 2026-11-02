//! `terminals`: the agent's terminal sessions — list them, read what one
//! printed since the agent last looked, type into one and wait for its
//! answer, look at its screen, interrupt or close one. `shell` opens them;
//! see [`termide_agent_core::terminals`].

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use termide_agent_core::terminals::duration;
use termide_agent_core::{
    CancelToken, TerminalOutput, TerminalScreenText, TerminalSend, TerminalSendEnd,
    TerminalSessions, TerminalState, Tool, ToolCall, ToolContext, ToolResultMessage, ToolText,
    ToolUpdate,
};

use crate::args::{optional_str, optional_u64};
use crate::clean::{clean_output, resolve_screen};
use crate::shell::check_session_name;
use crate::truncate::{head_tail, SHELL_MAX_BYTES};

const NAME: &str = "terminals";
/// How long `send` waits for the output to stop, when told nothing.
const DEFAULT_QUIET: Duration = Duration::from_millis(500);
/// How long `send` waits at most, when told nothing, and the most it may.
const DEFAULT_SEND_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SEND_TIMEOUT: Duration = Duration::from_secs(600);

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
                    "enum": ["list", "read", "send", "screen", "interrupt", "close"],
                    "description": "What to do; `list` when left out"
                },
                "session": {
                    "type": "string",
                    "description": "Every action but list: the terminal session's name"
                },
                "input": {
                    "type": "string",
                    "description": "send: text to type; a line break inside it is Enter — to submit the line, press Enter in `keys`"
                },
                "keys": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "send: keys to press after the input, by name: Enter, Tab, Esc, Backspace, Delete, Up, Down, Left, Right, Home, End, PageUp, PageDown, Space, F1-F12, Ctrl+<letter>, Alt+<key>"
                },
                "wait_for": {
                    "type": "string",
                    "description": "send: text to wait for in the output after the input"
                },
                "quiet_ms": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "send: wait until the output stops for this many ms (default 500 when wait_for is not given)"
                },
                "timeout": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "send: seconds to wait at most (default 10, max 600)"
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
        cancel: &CancelToken,
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
            "send" => return self.send(call, owner, name, cancel),
            "screen" => {
                return match self.terminals.screen(owner, name) {
                    Ok(screen) => ToolResultMessage::text(call, screen_body(name, &screen)),
                    Err(message) => ToolResultMessage::error(call, message),
                }
            }
            "interrupt" => self
                .terminals
                .interrupt(owner, name)
                .map(|()| format!("Sent Ctrl+C to terminal session `{name}`.")),
            "close" => self
                .terminals
                .close(owner, name)
                .map(|()| format!("Closed terminal session `{name}`.")),
            other => Err(format!(
                "unknown action `{other}`: use list, read, send, screen, interrupt or close"
            )),
        };
        match done {
            Ok(text) => ToolResultMessage::text(call, text),
            Err(message) => ToolResultMessage::error(call, message),
        }
    }
}

/// A terminal's screen for the agent: its rows, then where the cursor is.
fn screen_body(name: &str, screen: &TerminalScreenText) -> String {
    let program = if screen.alternate {
        "; a full-screen program has it"
    } else {
        ""
    };
    format!(
        "{}\n[the screen of terminal session `{name}`; cursor at row {}, column {}{program}]",
        screen.text,
        screen.cursor.0 + 1,
        screen.cursor.1 + 1
    )
}

impl TerminalsTool {
    fn send(
        &self,
        call: &ToolCall,
        owner: &str,
        name: &str,
        cancel: &CancelToken,
    ) -> ToolResultMessage {
        let input = match optional_str(call, "input") {
            Ok(input) => input.unwrap_or(""),
            Err(message) => return ToolResultMessage::error(call, message),
        };
        let keys: Vec<String> = match &call.arguments["keys"] {
            serde_json::Value::Null => Vec::new(),
            serde_json::Value::Array(keys) => {
                match keys
                    .iter()
                    .map(|key| key.as_str().map(str::to_string))
                    .collect()
                {
                    Some(keys) => keys,
                    None => return ToolResultMessage::error(call, "`keys` holds key names"),
                }
            }
            serde_json::Value::String(key) => vec![key.clone()],
            _ => return ToolResultMessage::error(call, "`keys` is a list of key names"),
        };
        if input.is_empty() && keys.is_empty() {
            return ToolResultMessage::error(call, "`send` needs `input` or `keys`");
        }
        let wait_for = match optional_str(call, "wait_for") {
            Ok(text) => text.filter(|text| !text.is_empty()).map(str::to_string),
            Err(message) => return ToolResultMessage::error(call, message),
        };
        let quiet = match optional_u64(call, "quiet_ms") {
            Ok(ms) => ms.map(Duration::from_millis),
            Err(message) => return ToolResultMessage::error(call, message),
        };
        let timeout = match optional_u64(call, "timeout") {
            Ok(seconds) => seconds
                .map_or(DEFAULT_SEND_TIMEOUT, Duration::from_secs)
                .min(MAX_SEND_TIMEOUT),
            Err(message) => return ToolResultMessage::error(call, message),
        };
        let quiet = quiet.or(wait_for.is_none().then_some(DEFAULT_QUIET));
        let until = wait_for.clone().map(|text| waits_for(input, text));
        let sent = self.terminals.send(&TerminalSend {
            owner,
            name,
            input,
            keys: &keys,
            until: until
                .as_ref()
                .map(|until| until as &(dyn Fn(&[u8]) -> bool + Sync)),
            quiet,
            timeout,
            cancel,
        });
        let outcome = match sent {
            Ok(outcome) => outcome,
            Err(message) => return ToolResultMessage::error(call, message),
        };
        let ended = match outcome.end {
            TerminalSendEnd::Matched => "the text waited for appeared".to_string(),
            TerminalSendEnd::Finished(code) => match code {
                Some(0) | None => "the command finished".into(),
                Some(code) => format!("the command finished with exit code {code}"),
            },
            TerminalSendEnd::Quiet => format!(
                "the output stopped for {} ms",
                quiet.unwrap_or_default().as_millis()
            ),
            TerminalSendEnd::TimedOut => match &wait_for {
                Some(text) => format!("`{text}` did not appear within {}", duration(timeout)),
                None => format!("still printing after {}", duration(timeout)),
            },
            TerminalSendEnd::Cancelled => "stopped waiting: the call was cancelled".into(),
            TerminalSendEnd::ShellExited => "the shell exited".into(),
        };
        let body = match &outcome.screen {
            Some(screen) => format!("{}\n[{ended}]", screen_body(name, screen)),
            None => output_body(name, &outcome.output, &ended),
        };
        ToolResultMessage::text(call, body)
    }

    fn read(&self, call: &ToolCall, owner: &str, name: &str) -> ToolResultMessage {
        let output = match self.terminals.read(owner, name) {
            Ok(output) => output,
            Err(message) => return ToolResultMessage::error(call, message),
        };
        ToolResultMessage::text(call, output_body(name, &output, ""))
    }
}

/// The matcher for `wait_for`: the delta is resolved to what the screen
/// shows, and the match starts after the line echoing what was typed — the
/// echo carries the prompt too, so waiting for the prompt would otherwise
/// report the answer before the program gave one. A program that does not
/// echo has nothing to skip and is matched whole.
fn waits_for(input: &str, text: String) -> impl Fn(&[u8]) -> bool + Sync {
    let echo = input
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(str::to_string);
    move |raw: &[u8]| {
        let screen = resolve_screen(&String::from_utf8_lossy(raw));
        let after = echo
            .as_ref()
            .and_then(|echo| screen.iter().position(|line| line.contains(echo)))
            .map_or(0, |at| at + 1);
        screen[after..].iter().any(|line| line.contains(&text))
    }
}

/// What a terminal printed, for the agent: cleaned, its middle cut when
/// long, then the terminal's state and `note`.
fn output_body(name: &str, output: &TerminalOutput, note: &str) -> String {
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
    if !note.is_empty() {
        body.push_str(&format!("; {note}"));
    }
    if let Some(log) = &output.log {
        body.push_str(&format!("; complete output in {}", log.display()));
    }
    body.push(']');
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;
    use termide_agent_core::{
        IntentLog, ModelSpec, SessionView, TerminalEntry, TerminalRun, TerminalRunOutcome,
        TerminalSendOutcome,
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
        fn send(&self, send: &TerminalSend<'_>) -> Result<TerminalSendOutcome, String> {
            self.calls.lock().unwrap().push(format!(
                "send {} {:?} {:?} {:?} {:?}",
                send.name, send.input, send.keys, send.quiet, send.timeout
            ));
            let out = b"\x1b[1m>>> \x1b[0m2\r\n>>> ";
            let matched = send.until.is_some_and(|until| until(out));
            let alternate = send.input == "vim";
            Ok(TerminalSendOutcome {
                end: if matched {
                    TerminalSendEnd::Matched
                } else {
                    TerminalSendEnd::Quiet
                },
                output: TerminalOutput {
                    data: out.to_vec(),
                    dropped: 0,
                    state: TerminalState::Idle,
                    log: None,
                },
                screen: alternate.then(|| TerminalScreenText {
                    text: "~\n~".into(),
                    alternate: true,
                    cursor: (0, 0),
                }),
            })
        }
        fn screen(&self, _owner: &str, _name: &str) -> Result<TerminalScreenText, String> {
            Ok(TerminalScreenText {
                text: "$ ls\na b".into(),
                alternate: false,
                cursor: (2, 2),
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

    /// The echo of what was typed carries the prompt, so waiting for the
    /// prompt must not report the answer before the program gave one.
    #[test]
    fn waiting_for_text_starts_after_the_echo_of_what_was_typed() {
        let typed = "print(\"sum\", 6*7)";
        let mut echo = String::new();
        for len in 1..=typed.len() {
            let line = format!(">>> {}", &typed[..len]);
            echo.push_str(&format!("\x1b[{}D>>> {}", line.len(), &typed[..len]));
            echo.push_str(&format!("\x1b[{}D", line.len()));
        }
        let waiting = waits_for(typed, ">>> ".into());
        assert!(!waiting(echo.as_bytes()), "the echo alone is not an answer");
        let answered = format!("{echo}\r\nsum 42\r\n>>> ");
        assert!(waiting(answered.as_bytes()), "the prompt after the answer");

        // A program that does not echo is matched whole.
        let waiting = waits_for("", "ready".into());
        assert!(waiting(b"starting\nready\n"));
    }

    #[test]
    fn sends_input_and_returns_the_answer_or_the_screen() {
        let fake = Arc::new(Fake::default());
        let tool = TerminalsTool::new(fake.clone());
        let sent = call(
            &tool,
            json!({"action": "send", "session": "py", "input": "1+1", "keys": ["Enter"], "wait_for": ">>> "}),
            Some("s1"),
        );
        assert_eq!(
            sent.plain_text(),
            ">>> 2\n>>> \n[terminal session `py`: idle; the text waited for appeared]"
        );
        let quiet = call(
            &tool,
            json!({"action": "send", "session": "py", "keys": "Ctrl+C"}),
            Some("s1"),
        );
        assert!(
            quiet
                .plain_text()
                .ends_with("the output stopped for 500 ms]"),
            "{}",
            quiet.plain_text()
        );
        // A full-screen program answers with its screen.
        let vim = call(
            &tool,
            json!({"action": "send", "session": "py", "input": "vim", "timeout": 3}),
            Some("s1"),
        );
        assert!(vim
            .plain_text()
            .starts_with("~\n~\n[the screen of terminal session `py`"));
        let screen = call(
            &tool,
            json!({"action": "screen", "session": "py"}),
            Some("s1"),
        );
        assert_eq!(
            screen.plain_text(),
            "$ ls\na b\n[the screen of terminal session `py`; cursor at row 3, column 3]"
        );
        assert!(
            call(
                &tool,
                json!({"action": "send", "session": "py"}),
                Some("s1")
            )
            .is_error
        );
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls[0], "send py \"1+1\" [\"Enter\"] None 10s");
        assert_eq!(calls[1], "send py \"\" [\"Ctrl+C\"] Some(500ms) 10s");
        assert!(calls[2].ends_with("Some(500ms) 3s"), "{}", calls[2]);
    }
}
