//! `task`: hand a self-contained piece of work to another agent.
//!
//! The agent picks one of the other agent definitions by name and gives it a
//! prompt; that agent runs its own loop to the end — reading, editing, asking
//! nothing, since no one is watching a nested run — and its final answer comes
//! back as the tool result. It keeps a focused sub-task, and the files it read
//! along the way, out of the main conversation, the way Claude Code's `Task`
//! and OpenCode's sub-sessions do.
//!
//! The tool holds no agent machinery of its own: the app supplies a closure
//! that builds and runs the named agent, so this crate stays free of the
//! provider and the catalog.

use std::sync::Arc;

use serde_json::{json, Value};
use termide_agent_core::{CancelToken, Tool, ToolCall, ToolContext, ToolResultMessage, ToolUpdate};

use crate::args::required_str;

/// Runs the named agent on `prompt` to completion and returns its final
/// answer, or an error message. Progress is forwarded through `on_update`;
/// the context is the delegating call's, whose session the subagent's
/// reviewer judges against.
pub type SubagentRun = Arc<
    dyn Fn(
            &str,
            &str,
            &ToolContext,
            &CancelToken,
            &mut dyn FnMut(ToolUpdate),
        ) -> Result<String, String>
        + Send
        + Sync,
>;

/// The `task` tool: delegate to one of a fixed set of agents.
pub struct TaskTool {
    /// The agents that can be delegated to, as `(name, description)`.
    agents: Vec<(String, String)>,
    run: SubagentRun,
    description: String,
}

impl TaskTool {
    #[must_use]
    pub fn new(agents: Vec<(String, String)>, run: SubagentRun) -> Self {
        let mut description = String::from(
            "Delegate a self-contained task to another agent. It runs on its own — reading, \
searching and editing as its permissions allow, without asking — and returns a final report. \
Use it to keep a focused sub-task and the files it touches out of this conversation. Give it \
everything it needs in the prompt; it does not see our history. Available agents:",
        );
        for (name, desc) in &agents {
            description.push_str(&format!("\n- {name}"));
            if !desc.is_empty() {
                description.push_str(&format!(": {desc}"));
            }
        }
        Self {
            agents,
            run,
            description,
        }
    }
}

impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        let names: Vec<&str> = self.agents.iter().map(|(name, _)| name.as_str()).collect();
        json!({
            "type": "object",
            "properties": {
                "agent": {
                    "type": "string",
                    "enum": names,
                    "description": "Which agent to hand the task to"
                },
                "prompt": {
                    "type": "string",
                    "description": "The task, complete in itself: the agent does not see this conversation"
                }
            },
            "required": ["agent", "prompt"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some("delegate a self-contained task to another agent, which reports back")
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> ToolResultMessage {
        let agent = match required_str(call, "agent") {
            Ok(agent) => agent,
            Err(message) => return ToolResultMessage::error(call, message),
        };
        let prompt = match required_str(call, "prompt") {
            Ok(prompt) => prompt,
            Err(message) => return ToolResultMessage::error(call, message),
        };
        if !self.agents.iter().any(|(name, _)| name == agent) {
            let names: Vec<&str> = self.agents.iter().map(|(name, _)| name.as_str()).collect();
            return ToolResultMessage::error(
                call,
                format!("no agent named {agent}; available: {}", names.join(", ")),
            );
        }
        match (self.run)(agent, prompt, ctx, cancel, on_update) {
            Ok(report) => ToolResultMessage::text(call, report),
            Err(message) => ToolResultMessage::error(call, message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(args: Value) -> ToolCall {
        ToolCall {
            id: "t".into(),
            name: "task".into(),
            arguments: args,
            extra_content: None,
        }
    }

    fn run_with(tool: &TaskTool, args: Value) -> ToolResultMessage {
        tool.execute(
            &call(args),
            &ToolContext::new(std::env::temp_dir()),
            &mut |_| {},
            &CancelToken::new(),
        )
    }

    #[test]
    fn it_lists_agents_delegates_and_reports_errors() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_run = Arc::clone(&seen);
        let run: SubagentRun = Arc::new(move |agent: &str, prompt: &str, _ctx, _c, _u| {
            seen_run
                .lock()
                .unwrap()
                .push((agent.to_string(), prompt.to_string()));
            if agent == "reviewer" {
                Ok("looks fine".into())
            } else {
                Err("the run failed".into())
            }
        });
        let tool = TaskTool::new(
            vec![
                ("reviewer".into(), "Reviews diffs".into()),
                ("writer".into(), String::new()),
            ],
            run,
        );
        assert!(tool.description().contains("reviewer: Reviews diffs"));
        let schema = tool.parameters();
        assert_eq!(
            schema["properties"]["agent"]["enum"],
            json!(["reviewer", "writer"])
        );

        let ok = run_with(
            &tool,
            json!({ "agent": "reviewer", "prompt": "check the diff" }),
        );
        assert!(!ok.is_error);
        assert_eq!(ok.plain_text(), "looks fine");
        assert_eq!(
            *seen.lock().unwrap(),
            vec![("reviewer".to_string(), "check the diff".to_string())]
        );

        let failed = run_with(&tool, json!({ "agent": "writer", "prompt": "x" }));
        assert!(failed.is_error);
        assert_eq!(failed.plain_text(), "the run failed");

        let unknown = run_with(&tool, json!({ "agent": "ghost", "prompt": "x" }));
        assert!(unknown.is_error);
        assert!(unknown.plain_text().contains("no agent named ghost"));

        let missing = run_with(&tool, json!({ "agent": "reviewer" }));
        assert!(missing.is_error);
    }
}
