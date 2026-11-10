//! `fork`: hand part of the work over to a copy of the session in a panel of
//! its own. The copy carries the conversation, compacted on `focus` when the
//! call names one, and takes `prompt` as its first request; from then on it
//! is the user's session like any other, and nothing comes back from it. Like
//! `send_message`, the tool holds no panel machinery: the app supplies the
//! closure that copies the session and opens the panel.

use std::sync::Arc;

use serde_json::{json, Value};
use termide_agent_core::{
    CancelToken, Tool, ToolCall, ToolContext, ToolResultMessage, ToolText, ToolUpdate,
};

use crate::args::required_str;

/// What a `fork` call asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkRequest<'a> {
    /// The fork's first request.
    pub prompt: &'a str,
    /// What the copy is compacted on before it, if anything.
    pub focus: Option<&'a str>,
    /// The short name the copy takes, shown as its panel's title, if the
    /// call gave one.
    pub title: Option<&'a str>,
    /// The id of the `fork` call, which the copy's log answers in place of
    /// the call this session goes on running.
    pub call: &'a str,
}

/// Forks the session of the context's panel: what to tell the model once the
/// fork's panel is open, or why it was not.
pub type ForkSession =
    Arc<dyn Fn(&ToolContext, &ForkRequest) -> Result<String, String> + Send + Sync>;

/// The `fork` tool.
pub struct ForkTool {
    fork: ForkSession,
}

impl ForkTool {
    #[must_use]
    pub fn new(fork: ForkSession) -> Self {
        Self { fork }
    }
}

impl Tool for ForkTool {
    fn name(&self) -> &str {
        "fork"
    }

    fn description(&self) -> &str {
        &ToolText::seed("fork").description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The fork's first request: its part of the work, and what stays with this session"
                },
                "title": {
                    "type": "string",
                    "description": "A short name for the fork, a few words on its part of the work: the title of its panel and its name among the sessions; leave it out to name it after the first line of `prompt`"
                },
                "focus": {
                    "type": "string",
                    "description": "What the copy keeps in full when it is compacted before the request; leave it out to keep the whole conversation"
                }
            },
            "required": ["prompt"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("fork").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("fork").guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        let prompt = match required_str(call, "prompt") {
            Ok(prompt) if !prompt.trim().is_empty() => prompt,
            Ok(_) => return ToolResultMessage::error(call, "`prompt` is empty"),
            Err(message) => return ToolResultMessage::error(call, message),
        };
        let optional = |name: &str| {
            call.arguments
                .get(name)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        };
        let request = ForkRequest {
            prompt,
            focus: optional("focus"),
            title: optional("title"),
            call: &call.id,
        };
        match (self.fork)(ctx, &request) {
            Ok(report) => ToolResultMessage::text(call, report),
            Err(message) => ToolResultMessage::error(call, message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn run(tool: &ForkTool, args: Value) -> ToolResultMessage {
        tool.execute(
            &ToolCall {
                id: "f1".into(),
                name: "fork".into(),
                arguments: args,
                extra_content: None,
            },
            &ToolContext::new(std::env::temp_dir()),
            &mut |_| {},
            &CancelToken::new(),
        )
    }

    #[test]
    fn fork_checks_its_arguments_and_hands_them_over() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let tool = ForkTool::new(Arc::new(move |_, request: &ForkRequest| {
            log.lock().unwrap().push((
                request.prompt.to_string(),
                request.focus.map(str::to_string),
                request.title.map(str::to_string),
                request.call.to_string(),
            ));
            Ok("forked into 1a2b3c4d".into())
        }));
        let done = run(
            &tool,
            json!({
                "prompt": "migrate the logs",
                "focus": " the log format ",
                "title": " Log migration "
            }),
        );
        assert_eq!(done.plain_text(), "forked into 1a2b3c4d");
        // A blank focus keeps the whole conversation; a blank title is none.
        assert!(!run(&tool, json!({ "prompt": "x", "focus": " ", "title": "" })).is_error);
        assert!(run(&tool, json!({ "prompt": "  " })).is_error);
        assert!(run(&tool, json!({ "focus": "x" })).is_error);
        assert_eq!(
            *seen.lock().unwrap(),
            [
                (
                    "migrate the logs".to_string(),
                    Some("the log format".to_string()),
                    Some("Log migration".to_string()),
                    "f1".to_string()
                ),
                ("x".to_string(), None, None, "f1".to_string())
            ]
        );
    }

    #[test]
    fn a_refused_fork_is_an_error() {
        let tool = ForkTool::new(Arc::new(|_, _: &ForkRequest| Err("no panel".into())));
        let refused = run(&tool, json!({ "prompt": "x" }));
        assert!(refused.is_error);
        assert_eq!(refused.plain_text(), "no panel");
    }
}
