//! `debug`: drive the debugger the way the user does in the debug panel —
//! start a launch, set breakpoints, step, read the stack and the variables,
//! evaluate expressions. Like `task`, the tool holds no session machinery:
//! the app supplies the closure that reaches its debug session.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use termide_agent_core::{
    CancelToken, Tool, ToolCall, ToolContext, ToolResultMessage, ToolText, ToolUpdate,
};

use crate::args::{optional_str, optional_u64, required_str, resolve_path};

/// What a `debug` call asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugAction {
    /// The session as it stands.
    State,
    /// Start debugging: a command of `commands.toml` by its identifier or
    /// label, or a file that needs no command.
    Start {
        command: Option<String>,
        file: Option<PathBuf>,
    },
    /// Set (or, with `set` false, remove) the breakpoint at a 1-based line.
    Breakpoint {
        file: PathBuf,
        line: usize,
        set: bool,
    },
    Continue,
    StepOver,
    StepIn,
    StepOut,
    Pause,
    Stop,
    Evaluate {
        expression: String,
    },
    /// Show frame `index` of the call stack.
    Frame {
        index: usize,
    },
}

impl DebugAction {
    /// Whether the action only reads the session.
    #[must_use]
    pub fn reads_only(&self) -> bool {
        matches!(self, Self::State)
    }
}

/// Runs an action and waits for its outcome: the session's state as text,
/// or why the action could not be taken.
pub type DebugRun =
    Arc<dyn Fn(&ToolContext, DebugAction, &CancelToken) -> Result<String, String> + Send + Sync>;

const ACTIONS: [&str; 11] = [
    "state",
    "start",
    "breakpoint",
    "continue",
    "step_over",
    "step_in",
    "step_out",
    "pause",
    "stop",
    "evaluate",
    "frame",
];

pub struct DebugTool {
    run: DebugRun,
}

impl DebugTool {
    #[must_use]
    pub fn new(run: DebugRun) -> Self {
        Self { run }
    }
}

/// The action a call names, with its arguments checked.
pub fn parse_action(call: &ToolCall, ctx: &ToolContext) -> Result<DebugAction, String> {
    let action = required_str(call, "action")?;
    let file = |required: bool| -> Result<Option<PathBuf>, String> {
        match optional_str(call, "file")? {
            Some(raw) => resolve_path(ctx, raw).map(Some),
            None if required => Err(format!("`{action}` needs `file`")),
            None => Ok(None),
        }
    };
    Ok(match action {
        "state" => DebugAction::State,
        "start" => {
            let command = optional_str(call, "command")?.map(str::to_string);
            let file = file(false)?;
            if command.is_none() && file.is_none() {
                return Err("`start` needs `command` (from commands.toml) or `file`".into());
            }
            DebugAction::Start { command, file }
        }
        "breakpoint" => {
            let file = file(true)?.unwrap_or_default();
            let line = optional_u64(call, "line")?
                .filter(|&l| l > 0)
                .ok_or("`breakpoint` needs `line`, 1-based")?;
            let set = match call.arguments.get("set") {
                None | Some(Value::Null) => true,
                Some(value) => value.as_bool().ok_or("`set` must be a boolean")?,
            };
            DebugAction::Breakpoint {
                file,
                line: line as usize,
                set,
            }
        }
        "continue" => DebugAction::Continue,
        "step_over" => DebugAction::StepOver,
        "step_in" => DebugAction::StepIn,
        "step_out" => DebugAction::StepOut,
        "pause" => DebugAction::Pause,
        "stop" => DebugAction::Stop,
        "evaluate" => {
            let expression = optional_str(call, "expression")?
                .map(str::trim)
                .filter(|e| !e.is_empty())
                .ok_or("`evaluate` needs `expression`")?;
            DebugAction::Evaluate {
                expression: expression.to_string(),
            }
        }
        "frame" => {
            let index = optional_u64(call, "frame")?.ok_or("`frame` needs `frame`, from 0")?;
            DebugAction::Frame {
                index: index as usize,
            }
        }
        other => {
            return Err(format!(
                "`action` is one of {}, not {other}",
                ACTIONS.join(", ")
            ))
        }
    })
}

impl Tool for DebugTool {
    fn name(&self) -> &str {
        "debug"
    }

    fn description(&self) -> &str {
        &ToolText::seed("debug").description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ACTIONS,
                    "description": "What to do"
                },
                "command": {
                    "type": "string",
                    "description": "start: the identifier or label of a command with a `debugger` in commands.toml"
                },
                "file": {
                    "type": "string",
                    "description": "start: a Python, Go or Rust file to debug without a command (a Rust file: the main file of its binary, or any file of a package with one binary); breakpoint: the source file"
                },
                "line": {
                    "type": "integer",
                    "description": "breakpoint: the 1-based line"
                },
                "set": {
                    "type": "boolean",
                    "description": "breakpoint: true to set it (the default), false to remove it"
                },
                "expression": {
                    "type": "string",
                    "description": "evaluate: the expression, in the debugged language, in the selected frame"
                },
                "frame": {
                    "type": "integer",
                    "description": "frame: the call stack index to select, 0 the innermost"
                }
            },
            "required": ["action"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("debug").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("debug").guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> ToolResultMessage {
        let action = match parse_action(call, ctx) {
            Ok(action) => action,
            Err(message) => return ToolResultMessage::error(call, message),
        };
        match (self.run)(ctx, action, cancel) {
            Ok(text) => ToolResultMessage::text(call, text),
            Err(message) => ToolResultMessage::error(call, message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(arguments: Value) -> ToolCall {
        ToolCall {
            id: "1".into(),
            name: "debug".into(),
            arguments,
            extra_content: None,
        }
    }

    fn ctx() -> ToolContext {
        ToolContext::new("/work")
    }

    #[test]
    fn actions_parse_with_their_arguments() {
        let parse = |args| parse_action(&call(args), &ctx());
        assert_eq!(
            parse(json!({ "action": "breakpoint", "file": "src/main.rs", "line": 4 })),
            Ok(DebugAction::Breakpoint {
                file: "/work/src/main.rs".into(),
                line: 4,
                set: true
            })
        );
        assert_eq!(
            parse(json!({ "action": "start", "command": "debug-app" })),
            Ok(DebugAction::Start {
                command: Some("debug-app".into()),
                file: None
            })
        );
        assert_eq!(
            parse(json!({ "action": "evaluate", "expression": " a + 1 " })),
            Ok(DebugAction::Evaluate {
                expression: "a + 1".into()
            })
        );
        assert!(parse(json!({ "action": "start" })).is_err());
        assert!(parse(json!({ "action": "breakpoint", "file": "a.rs", "line": 0 })).is_err());
        assert!(parse(json!({ "action": "jump" }))
            .unwrap_err()
            .contains("step_over"));
    }

    #[test]
    fn the_session_answers_through_the_closure() {
        let tool = DebugTool::new(Arc::new(|_, action, _| match action {
            DebugAction::State => Ok("Session: app — running".into()),
            _ => Err("no".into()),
        }));
        let result = tool.execute(
            &call(json!({ "action": "state" })),
            &ctx(),
            &mut |_| {},
            &CancelToken::new(),
        );
        assert!(!result.is_error);
    }
}
