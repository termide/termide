//! `task`: hand a self-contained piece of work to another agent.
//!
//! The agent picks an agent definition by name — its own included, for a
//! fresh copy of itself — and gives it a prompt; that agent runs its own loop
//! to the end — reading, editing, asking nothing, since no one is watching a
//! nested run — and its final answer comes back as the tool result. It keeps a
//! focused sub-task, and the files it read along the way, out of the main
//! conversation, the way Claude Code's `Task` and OpenCode's sub-sessions do.
//!
//! The tool holds no agent machinery of its own: the app supplies a closure
//! that builds and runs the named agent, so this crate stays free of the
//! provider and the catalog.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use termide_agent_core::{
    CancelToken, Failure, Tool, ToolCall, ToolContext, ToolResultMessage, ToolUpdate, Usage,
};
use termide_agent_core::{ToolText, SUBAGENT_SESSION_KEY};

use crate::args::required_str;

/// How a subagent's run ended: its final answer, or an error message, the
/// tokens its model calls spent either way, and the log its session went to,
/// when it had one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentOutcome {
    pub answer: Result<String, String>,
    pub spent: Usage,
    pub session: Option<PathBuf>,
    /// The task is parked at a limit that passes on its own: `answer` only
    /// says where it waits and when it goes again, so nothing is reported to
    /// the panel that delegated it — its real answer is still owed there.
    pub parked: bool,
    /// What stopped it, when its last reply says: a limit and the moment it
    /// resets, so the runner can wait for that rather than guess.
    pub failure: Option<Failure>,
}

impl From<Result<String, String>> for SubagentOutcome {
    /// An outcome that spent nothing and left no log.
    fn from(answer: Result<String, String>) -> Self {
        Self {
            answer,
            spent: Usage::default(),
            session: None,
            parked: false,
            failure: None,
        }
    }
}

/// A task handed to a subagent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentTask<'a> {
    /// The agent to run.
    pub agent: &'a str,
    pub prompt: &'a str,
    /// The id of the delegating `task` call.
    pub call: &'a str,
    /// Hand the task over and go on: the call returns once the task is
    /// placed, and the delegate's answer comes later as a message of its own.
    pub background: bool,
}

/// Runs the named agent on the task to completion. Progress is forwarded
/// through `on_update`; the context is the delegating call's, whose session
/// the subagent's reviewer judges against.
pub type SubagentRun = Arc<
    dyn Fn(&SubagentTask, &ToolContext, &CancelToken, &mut dyn FnMut(ToolUpdate)) -> SubagentOutcome
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
        let mut tool = Self {
            agents,
            run,
            description: String::new(),
        };
        tool.description = tool.render_description(&ToolText::seed("task").description);
        tool
    }
}

impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        &self.description
    }

    /// `{{agents}}` takes the agents it can delegate to, one per line.
    fn render_description(&self, template: &str) -> String {
        let agents: Vec<String> = self
            .agents
            .iter()
            .map(|(name, desc)| {
                if desc.is_empty() {
                    format!("- {name}")
                } else {
                    format!("- {name}: {desc}")
                }
            })
            .collect();
        template.replace("{{agents}}", &agents.join("\n"))
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
                },
                "background": {
                    "type": "boolean",
                    "description": "Run it in a panel of its own and carry on without waiting; its answer comes later as a message"
                }
            },
            "required": ["agent", "prompt"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("task").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("task").guidelines
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
        let background = call
            .arguments
            .get("background")
            .is_some_and(|value| value.as_bool() == Some(true) || value.as_str() == Some("true"));
        let task = SubagentTask {
            agent,
            prompt,
            call: &call.id,
            background,
        };
        let outcome = (self.run)(&task, ctx, cancel, on_update);
        // What the subagent spent and where its session went ride in the
        // details, for the panel; the model reads the answer alone.
        let result = match outcome.answer {
            Ok(report) => ToolResultMessage::text(call, report),
            Err(message) => ToolResultMessage::error(call, message),
        };
        let result = match outcome.session {
            Some(path) => result.with_details(json!({
                SUBAGENT_SESSION_KEY: path.to_string_lossy(),
            })),
            None => result,
        };
        result.with_spent(outcome.spent)
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
        let run: SubagentRun = Arc::new(move |task: &SubagentTask, _ctx, _c, _u| {
            let agent = task.agent;
            seen_run.lock().unwrap().push((
                agent.to_string(),
                task.prompt.to_string(),
                task.call.to_string(),
                task.background,
            ));
            let spent = Usage {
                input: 7,
                output: 3,
                ..Usage::default()
            };
            let answer = if agent == "reviewer" {
                Ok("looks fine".into())
            } else {
                Err("the run failed".into())
            };
            let session = (agent == "reviewer").then(|| PathBuf::from("/logs/s.jsonl"));
            SubagentOutcome {
                answer,
                spent,
                session,
                parked: false,
                failure: None,
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
        assert_eq!(ok.spent().map(|spent| spent.output), Some(3));
        // The log beside what was spent.
        assert_eq!(
            ok.details.as_ref().unwrap()[SUBAGENT_SESSION_KEY],
            "/logs/s.jsonl"
        );
        assert_eq!(
            *seen.lock().unwrap(),
            vec![(
                "reviewer".to_string(),
                "check the diff".to_string(),
                "t".to_string(),
                false
            )]
        );
        assert!(schema["properties"]["background"].is_object());
        run_with(
            &tool,
            json!({ "agent": "reviewer", "prompt": "later", "background": true }),
        );
        assert!(seen.lock().unwrap().last().unwrap().3);

        let failed = run_with(&tool, json!({ "agent": "writer", "prompt": "x" }));
        assert!(failed.is_error);
        assert_eq!(failed.plain_text(), "the run failed");
        // A failed run spent tokens too.
        assert_eq!(failed.spent().map(|spent| spent.input), Some(7));

        let unknown = run_with(&tool, json!({ "agent": "ghost", "prompt": "x" }));
        assert!(unknown.is_error);
        assert!(unknown.plain_text().contains("no agent named ghost"));

        let missing = run_with(&tool, json!({ "agent": "reviewer" }));
        assert!(missing.is_error);
    }
}
