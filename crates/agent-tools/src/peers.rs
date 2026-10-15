//! `agents` and `send_message`: agents in different panels talking to one
//! another.
//!
//! `agents` lists the agent panels open in the project; `send_message` hands
//! one of them a message, which reaches its agent as another agent's words —
//! never the user's — and starts a turn there or joins the one under way.
//! An answer comes back the same way, as a message of its own. Like `task`,
//! the tools hold no panel machinery: the app supplies closures that reach
//! the panels.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use termide_agent_core::{
    CancelToken, OriginKind, Tool, ToolCall, ToolContext, ToolResultMessage, ToolText, ToolUpdate,
};

use crate::args::required_str;

/// An agent panel as `agents` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    /// The panel's number, which `send_message` takes.
    pub panel: u64,
    pub agent: String,
    /// What it is doing: `idle`, `working`, `waiting` for the user,
    /// `delegate` of another agent's task, or `free` (nothing in it yet).
    pub state: &'static str,
    pub cwd: PathBuf,
    /// The panel the call comes from.
    pub own: bool,
}

/// Lists the agent panels for a call from the context's panel.
pub type PeerList = Arc<dyn Fn(&ToolContext) -> Result<Vec<PeerInfo>, String> + Send + Sync>;

/// Delivers a message: `(context, panel, kind, text)`; what to tell the
/// model once it is delivered, or why it was not.
pub type PeerSend =
    Arc<dyn Fn(&ToolContext, u64, OriginKind, &str) -> Result<String, String> + Send + Sync>;

/// `agents`: the agent panels open in the project.
pub struct AgentsTool {
    list: PeerList,
}

impl AgentsTool {
    #[must_use]
    pub fn new(list: PeerList) -> Self {
        Self { list }
    }
}

impl Tool for AgentsTool {
    fn name(&self) -> &str {
        "agents"
    }

    fn description(&self) -> &str {
        &ToolText::seed("agents").description
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("agents").snippet.as_deref()
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        match (self.list)(ctx) {
            Ok(peers) if peers.iter().all(|peer| peer.own) => {
                ToolResultMessage::text(call, "No other agent panel is open in this project.")
            }
            Ok(peers) => ToolResultMessage::text(call, render(&peers)),
            Err(message) => ToolResultMessage::error(call, message),
        }
    }
}

/// One line per panel: `3 · reviewer · idle · /work/app`, this one marked.
fn render(peers: &[PeerInfo]) -> String {
    peers
        .iter()
        .map(|peer| {
            let own = if peer.own { " (this panel)" } else { "" };
            format!(
                "{} · {} · {} · {}{own}",
                peer.panel,
                peer.agent,
                peer.state,
                peer.cwd.display()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `send_message`: a message to the agent in another panel.
pub struct SendMessageTool {
    send: PeerSend,
}

impl SendMessageTool {
    #[must_use]
    pub fn new(send: PeerSend) -> Self {
        Self { send }
    }
}

impl Tool for SendMessageTool {
    fn name(&self) -> &str {
        "send_message"
    }

    fn description(&self) -> &str {
        &ToolText::seed("send_message").description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "integer",
                    "description": "The number of the panel to write to, from `agents`"
                },
                "kind": {
                    "type": "string",
                    "enum": ["info", "question", "reply"],
                    "description": "What the message is"
                },
                "text": {
                    "type": "string",
                    "description": "The message, complete in itself: the other agent does not see this conversation"
                }
            },
            "required": ["to", "kind", "text"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("send_message").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("send_message").guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        let Some(to) = call.arguments.get("to").and_then(panel_number) else {
            return ToolResultMessage::error(
                call,
                "missing required argument `to`: a panel number from `agents`",
            );
        };
        let kind = match required_str(call, "kind") {
            Ok("info") => OriginKind::Info,
            Ok("question") => OriginKind::Question,
            Ok("reply") => OriginKind::Reply,
            Ok(other) => {
                return ToolResultMessage::error(
                    call,
                    format!("`kind` is info, question or reply, not {other}"),
                )
            }
            Err(message) => return ToolResultMessage::error(call, message),
        };
        let text = match required_str(call, "text") {
            Ok(text) if !text.trim().is_empty() => text,
            Ok(_) => return ToolResultMessage::error(call, "`text` is empty"),
            Err(message) => return ToolResultMessage::error(call, message),
        };
        match (self.send)(ctx, to, kind, text) {
            Ok(report) => ToolResultMessage::text(call, report),
            Err(message) => ToolResultMessage::error(call, message),
        }
    }
}

/// A panel number as a model writes it: a number, or one in a string.
fn panel_number(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: "c".into(),
            name: name.into(),
            arguments: args,
            extra_content: None,
        }
    }

    fn run(tool: &dyn Tool, args: Value) -> ToolResultMessage {
        tool.execute(
            &call(tool.name(), args),
            &ToolContext::new(std::env::temp_dir()),
            &mut |_| {},
            &CancelToken::new(),
        )
    }

    fn peer(panel: u64, agent: &str, own: bool) -> PeerInfo {
        PeerInfo {
            panel,
            agent: agent.into(),
            state: "idle",
            cwd: PathBuf::from("/work"),
            own,
        }
    }

    #[test]
    fn agents_lists_the_panels_this_one_marked() {
        let tool = AgentsTool::new(Arc::new(|_| {
            Ok(vec![peer(1, "default", true), peer(2, "reviewer", false)])
        }));
        assert_eq!(
            run(&tool, json!({})).plain_text(),
            "1 · default · idle · /work (this panel)\n2 · reviewer · idle · /work"
        );
        let alone = AgentsTool::new(Arc::new(|_| Ok(vec![peer(1, "default", true)])));
        assert!(run(&alone, json!({})).plain_text().contains("No other"));
    }

    #[test]
    fn send_message_checks_its_arguments_and_delivers() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&sent);
        let tool = SendMessageTool::new(Arc::new(move |_, to, kind, text: &str| {
            seen.lock().unwrap().push((to, kind, text.to_string()));
            if to == 9 {
                Err("no agent panel 9".into())
            } else {
                Ok(format!("delivered to panel {to}"))
            }
        }));
        let ok = run(
            &tool,
            json!({ "to": "2", "kind": "question", "text": "which crate?" }),
        );
        assert_eq!(ok.plain_text(), "delivered to panel 2");
        assert!(run(&tool, json!({ "to": 9, "kind": "info", "text": "x" })).is_error);
        assert!(run(&tool, json!({ "to": 2, "kind": "task", "text": "x" })).is_error);
        assert!(run(&tool, json!({ "to": 2, "kind": "info", "text": " " })).is_error);
        assert!(run(&tool, json!({ "kind": "info", "text": "x" })).is_error);
        assert_eq!(
            *sent.lock().unwrap(),
            [
                (2, OriginKind::Question, "which crate?".to_string()),
                (9, OriginKind::Info, "x".to_string())
            ]
        );
    }
}
