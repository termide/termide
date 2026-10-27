//! `jobs`: what the agent set going that still runs, and a way to stop it.
//!
//! An agent hands tasks over in the background and goes on; an external
//! agent moves a long call — a delegate it waits for, a command — to the
//! background to take a message meanwhile. Any of them can stop mattering
//! while it runs. `jobs` lists them, each by its id, and `stop` stops one —
//! without asking anyone: it is the agent's own work. What the user set going
//! (a `/goal`, a `/loop`) is not listed: it is the user's to stop.
//!
//! The tool knows two sources: the tasks the app keeps for the panel
//! ([`OwnTasks`]), and the calls running beside this one, which the context
//! brings where an external agent's calls outlive their step
//! ([`ToolContext::running`]).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use termide_agent_core::{
    CancelToken, Tool, ToolCall, ToolContext, ToolResultMessage, ToolText, ToolUpdate,
};

/// The tasks the agent handed over that run on without a call waiting for
/// them, as the app keeps them.
pub trait OwnTasks: Send + Sync {
    /// One line per task, its id first.
    fn list(&self, ctx: &ToolContext) -> Vec<String>;
    /// Stop the task `id`: what to tell the agent, or `None` when no task of
    /// its runs under that id.
    fn stop(&self, id: &str, ctx: &ToolContext) -> Option<String>;
}

/// The `jobs` tool.
pub struct JobsTool {
    tasks: Arc<dyn OwnTasks>,
}

impl JobsTool {
    #[must_use]
    pub fn new(tasks: Arc<dyn OwnTasks>) -> Self {
        Self { tasks }
    }
}

impl Tool for JobsTool {
    fn name(&self) -> &str {
        "jobs"
    }

    fn description(&self) -> &str {
        &ToolText::seed("jobs").description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "stop": {
                    "type": "string",
                    "description": "The id of one of them to stop, as the list or its hand-over gave it; leave it out to list them"
                }
            }
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("jobs").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("jobs").guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        let stop = call
            .arguments
            .get("stop")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty());
        if let Some(id) = stop {
            if let Some(text) = self.tasks.stop(id, ctx) {
                return ToolResultMessage::text(call, text);
            }
            let beside = ctx.running.as_ref().map(|running| &running.0);
            if let Some(running) = beside {
                let found = running.list().into_iter().find(|other| other.id == id);
                if let Some(other) = found.filter(|_| running.stop(id)) {
                    return ToolResultMessage::text(
                        call,
                        format!(
                            "Stopped {id} ({}); its result will not come.",
                            summary(&other.name, &other.arguments)
                        ),
                    );
                }
            }
            return ToolResultMessage::error(
                call,
                format!(
                    "nothing of yours runs under {id}: it is over already, or the id is not one \
                     of yours; call jobs without stop to see what runs"
                ),
            );
        }
        let mut lines = self.tasks.list(ctx);
        if let Some(running) = &ctx.running {
            lines.extend(running.0.list().into_iter().map(|other| {
                format!(
                    "- {} · call · {} · running {}",
                    other.id,
                    summary(&other.name, &other.arguments),
                    duration(other.elapsed)
                )
            }));
        }
        if lines.is_empty() {
            return ToolResultMessage::text(call, "Nothing of yours runs in the background.");
        }
        ToolResultMessage::text(call, lines.join("\n"))
    }
}

/// A call in a few words: its tool and what it is about.
fn summary(name: &str, arguments: &Value) -> String {
    let arg = |key: &str| arguments.get(key).and_then(Value::as_str).unwrap_or("");
    let about = match name {
        "bash" => arg("command").to_string(),
        "task" => format!("{}: {}", arg("agent"), arg("prompt")),
        _ => match arguments {
            Value::Object(map) if map.is_empty() => String::new(),
            _ => arguments.to_string(),
        },
    };
    let about = first_line(&about, 100);
    if about.is_empty() {
        name.to_string()
    } else {
        format!("{name} {about}")
    }
}

/// The first non-blank line of `text`, cut to `width` characters.
#[must_use]
pub fn first_line(text: &str, width: usize) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() > width {
        let cut: String = line.chars().take(width).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

/// How long something has run, as `1h2m`, `3m12s` or `40s`.
#[must_use]
pub fn duration(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match (secs / 3600, secs % 3600 / 60, secs % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m{s}s"),
        (h, m, _) => format!("{h}h{m}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use termide_agent_core::{CallsBeside, RunningCall, RunningCalls};

    struct Tasks;

    impl OwnTasks for Tasks {
        fn list(&self, _ctx: &ToolContext) -> Vec<String> {
            vec!["- t1 · background task · search: find it · running 5s".into()]
        }
        fn stop(&self, id: &str, _ctx: &ToolContext) -> Option<String> {
            (id == "t1").then(|| "stopped t1".to_string())
        }
    }

    #[derive(Default)]
    struct Beside {
        stopped: Mutex<Vec<String>>,
    }

    impl CallsBeside for Beside {
        fn list(&self) -> Vec<RunningCall> {
            vec![RunningCall {
                id: "toolu_1".into(),
                name: "bash".into(),
                arguments: json!({ "command": "cargo test\n--all" }),
                elapsed: Duration::from_secs(192),
            }]
        }
        fn stop(&self, id: &str) -> bool {
            self.stopped.lock().unwrap().push(id.to_string());
            id == "toolu_1"
        }
    }

    fn run(ctx: &ToolContext, args: Value) -> ToolResultMessage {
        let call = ToolCall {
            id: "j".into(),
            name: "jobs".into(),
            arguments: args,
            extra_content: None,
        };
        JobsTool::new(Arc::new(Tasks)).execute(&call, ctx, &mut |_| {}, &CancelToken::new())
    }

    #[test]
    fn it_lists_the_agents_tasks_and_calls_and_stops_one_by_id() {
        let beside = Arc::new(Beside::default());
        let ctx = ToolContext {
            running: Some(RunningCalls(beside.clone())),
            ..ToolContext::new("/tmp")
        };
        let listed = run(&ctx, json!({}));
        assert_eq!(
            listed.plain_text(),
            "- t1 · background task · search: find it · running 5s\n\
             - toolu_1 · call · bash cargo test · running 3m12s"
        );

        // A task of the panel's goes first; a call beside is stopped by the
        // id the agent knows it by.
        assert_eq!(
            run(&ctx, json!({ "stop": "t1" })).plain_text(),
            "stopped t1"
        );
        let stopped = run(&ctx, json!({ "stop": "toolu_1" }));
        assert!(!stopped.is_error);
        assert!(stopped.plain_text().contains("bash cargo test"));
        assert_eq!(*beside.stopped.lock().unwrap(), ["toolu_1"]);

        let unknown = run(&ctx, json!({ "stop": "nope" }));
        assert!(unknown.is_error);
        assert!(unknown
            .plain_text()
            .contains("nothing of yours runs under nope"));
    }

    #[test]
    fn without_calls_beside_it_lists_the_tasks_alone() {
        struct NoTasks;
        impl OwnTasks for NoTasks {
            fn list(&self, _ctx: &ToolContext) -> Vec<String> {
                Vec::new()
            }
            fn stop(&self, _id: &str, _ctx: &ToolContext) -> Option<String> {
                None
            }
        }
        let call = ToolCall {
            id: "j".into(),
            name: "jobs".into(),
            arguments: json!({}),
            extra_content: None,
        };
        let empty = JobsTool::new(Arc::new(NoTasks)).execute(
            &call,
            &ToolContext::new("/tmp"),
            &mut |_| {},
            &CancelToken::new(),
        );
        assert_eq!(
            empty.plain_text(),
            "Nothing of yours runs in the background."
        );
    }

    #[test]
    fn durations_and_lines_are_short() {
        assert_eq!(duration(Duration::from_secs(40)), "40s");
        assert_eq!(duration(Duration::from_secs(3720 + 5)), "1h2m");
        assert_eq!(first_line("\n  abc\nmore", 2), "ab…");
    }
}
