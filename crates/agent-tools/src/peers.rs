//! `agents` and `send_message`: agents in different panels talking to one
//! another.
//!
//! `agents` lists the sessions open in the agent panels of the project;
//! `send_message` hands one of them a message, which reaches its agent as
//! another agent's words — never the user's — and starts a turn there or
//! joins the one under way. A message goes to a session, not a panel: the
//! panel it is open in may show another one by then, and the session may be
//! open in another panel or in none. An answer comes back the same way, as a
//! message of its own. Like `task`, the tools hold no panel machinery: the
//! app supplies closures that reach the panels.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use termide_agent_core::{
    CancelToken, OriginKind, Tool, ToolCall, ToolContext, ToolResultMessage, ToolText, ToolUpdate,
};

use crate::args::required_str;

/// A session open in an agent panel, as `agents` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    /// The session's id, which `send_message` takes.
    pub session: String,
    /// The panel it is open in, by its number, as the user sees it.
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

/// Delivers a message: `(context, session, kind, text)`; what to tell the
/// model once it is delivered, or why it was not.
pub type PeerSend =
    Arc<dyn Fn(&ToolContext, &str, OriginKind, &str) -> Result<String, String> + Send + Sync>;

/// `agents`: the sessions open in the agent panels of the project.
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

/// One line per session: `1a2b3c4d · reviewer · idle · /work/app · panel 3`,
/// this one marked.
fn render(peers: &[PeerInfo]) -> String {
    peers
        .iter()
        .map(|peer| {
            let own = if peer.own { " (this session)" } else { "" };
            format!(
                "{} · {} · {} · {} · panel {}{own}",
                peer.session,
                peer.agent,
                peer.state,
                peer.cwd.display(),
                peer.panel
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `send_message`: a message to the agent of another session.
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
                    "type": "string",
                    "description": "The id of the session to write to: from `agents`, or the `session` of the message you answer"
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
        let Some(to) = call.arguments.get("to").and_then(session_id) else {
            return ToolResultMessage::error(
                call,
                "missing required argument `to`: a session id from `agents`, or the `session` \
                 of the message you answer",
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
        match (self.send)(ctx, &to, kind, text) {
            Ok(report) => ToolResultMessage::text(call, report),
            Err(message) => ToolResultMessage::error(call, message),
        }
    }
}

/// A session id as a model writes it: a string, trimmed; one of digits
/// alone may come as a number.
fn session_id(value: &Value) -> Option<String> {
    let id = match value {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    (!id.is_empty()).then_some(id)
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
            session: format!("s{panel}"),
            panel,
            agent: agent.into(),
            state: "idle",
            cwd: PathBuf::from("/work"),
            own,
        }
    }

    #[test]
    fn agents_lists_the_sessions_this_one_marked() {
        let tool = AgentsTool::new(Arc::new(|_| {
            Ok(vec![peer(1, "default", true), peer(2, "reviewer", false)])
        }));
        assert_eq!(
            run(&tool, json!({})).plain_text(),
            "s1 · default · idle · /work · panel 1 (this session)\n\
             s2 · reviewer · idle · /work · panel 2"
        );
        let alone = AgentsTool::new(Arc::new(|_| Ok(vec![peer(1, "default", true)])));
        assert!(run(&alone, json!({})).plain_text().contains("No other"));
    }

    #[test]
    fn send_message_checks_its_arguments_and_delivers() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&sent);
        let tool = SendMessageTool::new(Arc::new(move |_, to: &str, kind, text: &str| {
            seen.lock()
                .unwrap()
                .push((to.to_string(), kind, text.to_string()));
            if to == "12345678" {
                Err("no session 12345678".into())
            } else {
                Ok(format!("delivered to {to}"))
            }
        }));
        let ok = run(
            &tool,
            json!({ "to": " 1a2b3c4d ", "kind": "question", "text": "which crate?" }),
        );
        assert_eq!(ok.plain_text(), "delivered to 1a2b3c4d");
        // An id of digits alone may come as a number.
        assert!(
            run(
                &tool,
                json!({ "to": 12345678, "kind": "info", "text": "x" })
            )
            .is_error
        );
        assert!(run(&tool, json!({ "to": "s2", "kind": "task", "text": "x" })).is_error);
        assert!(run(&tool, json!({ "to": "s2", "kind": "info", "text": " " })).is_error);
        assert!(run(&tool, json!({ "to": " ", "kind": "info", "text": "x" })).is_error);
        assert!(run(&tool, json!({ "kind": "info", "text": "x" })).is_error);
        assert_eq!(
            *sent.lock().unwrap(),
            [
                (
                    "1a2b3c4d".to_string(),
                    OriginKind::Question,
                    "which crate?".to_string()
                ),
                ("12345678".to_string(), OriginKind::Info, "x".to_string())
            ]
        );
    }
}
