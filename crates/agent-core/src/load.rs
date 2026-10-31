//! What a conversation's context is made of, estimated at characters over
//! four: what each tool's results (a skill's text under `skill:<name>`)
//! brought into it and how much of them later model calls read again, and
//! what the context holds now.
//!
//! The log's branch is replayed as [`Session::context_messages_with`] builds
//! the context — summaries, pruning and fitting included — and every model
//! call (an assistant message) is taken to have read all that stood before
//! it. An external agent logs a turn, not each of its calls, so what its
//! calls read again comes out low.

use std::collections::{BTreeMap, HashMap};

use crate::compaction::{message_chars, CompactionPrompts};
use crate::fit::fit;
use crate::message::{Message, ToolCall};
use crate::prune::prune_to_decisions;
use crate::session::{EntryKind, Session};

/// One tool's results in the context, in estimated tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolLoad {
    /// Put into the context, each result once.
    pub brought: u64,
    /// Read again by the model calls after them while they stayed.
    pub carried: u64,
}

/// What the context holds now, in estimated tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextNow {
    /// The summary standing for what was compacted.
    pub summary: u64,
    /// The user's and the model's words, reasoning and calls.
    pub conversation: u64,
    /// Skills' texts the `skill` tool loaded.
    pub skills: u64,
    /// The other tools' results.
    pub tool_output: u64,
}

/// See the module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextLoad {
    /// By tool name, `skill:<name>` for a skill.
    pub tools: BTreeMap<String, ToolLoad>,
    pub now: ContextNow,
}

/// What a tool result is told by: its tool, or the skill it loaded.
fn label_of(call: &ToolCall) -> String {
    match call.arguments.get("name").and_then(|name| name.as_str()) {
        Some(skill) if call.name == "skill" => format!("skill:{skill}"),
        _ => call.name.clone(),
    }
}

fn tokens(message: &Message) -> u64 {
    (message_chars(message) / 4) as u64
}

impl ContextLoad {
    /// Replay `session`'s branch, its summaries worded by `prompts`.
    #[must_use]
    pub fn of(session: &Session, prompts: &CompactionPrompts) -> Self {
        let mut load = Self::default();
        // The label of each call made so far, by its id.
        let mut labels: HashMap<String, String> = HashMap::new();
        let mut messages: Vec<Message> = Vec::new();
        // Each message's tool label (none for words) and size, as it is now.
        let mut sizes: Vec<(Option<String>, u64)> = Vec::new();
        let size_of = |labels: &HashMap<String, String>, message: &Message| {
            let label = match message {
                Message::ToolResult(result) => Some(
                    labels
                        .get(&result.tool_call_id)
                        .cloned()
                        .unwrap_or_else(|| result.tool_name.clone()),
                ),
                _ => None,
            };
            (label, tokens(message))
        };
        for entry in session.branch() {
            match &entry.kind {
                EntryKind::Message { message, .. } => {
                    if let Message::Assistant(reply) = message {
                        for (label, size) in &sizes {
                            if let Some(label) = label {
                                load.tools.entry(label.clone()).or_default().carried += size;
                            }
                        }
                        for call in reply.tool_calls() {
                            labels.insert(call.id.clone(), label_of(call));
                        }
                    }
                    let (label, size) = size_of(&labels, message);
                    if let Some(label) = &label {
                        load.tools.entry(label.clone()).or_default().brought += size;
                    }
                    messages.push(message.clone());
                    sizes.push((label, size));
                    continue;
                }
                EntryKind::Compaction {
                    summary, keep_last, ..
                } => {
                    let tail = messages.split_off(messages.len().saturating_sub(*keep_last));
                    messages = vec![prompts.summary_message(summary)];
                    messages.extend(tail);
                }
                EntryKind::Pruned => messages = prune_to_decisions(&messages),
                EntryKind::Fitted {
                    budget, keep_tail, ..
                } => messages = fit(&messages, *budget, *keep_tail).messages,
                _ => continue,
            }
            sizes = messages.iter().map(|m| size_of(&labels, m)).collect();
        }
        for (message, (label, size)) in messages.iter().zip(&sizes) {
            let now = &mut load.now;
            match (message, label) {
                (_, Some(label)) if label.starts_with("skill:") => now.skills += size,
                (_, Some(_)) => now.tool_output += size,
                (Message::User(user), None) if prompts.is_summary(&user.plain_text()) => {
                    now.summary += size;
                }
                (_, None) => now.conversation += size,
            }
        }
        load
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::test_support::text_reply;
    use crate::message::{AssistantContent, ToolResultMessage, UserMessage};

    fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
            extra_content: None,
        }
    }

    fn asks(call: &ToolCall) -> Message {
        let mut reply = text_reply("");
        reply.content = vec![AssistantContent::ToolCall(call.clone())];
        Message::Assistant(reply)
    }

    fn answer(call: &ToolCall, chars: usize) -> Message {
        Message::ToolResult(ToolResultMessage::text(call, "x".repeat(chars)))
    }

    /// Each result counts once where it came in and again with every model
    /// call while it stays; a summary ends that, and what the context holds
    /// now is told apart by kind.
    #[test]
    fn results_count_where_they_came_in_and_while_they_stayed() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        let skill = call("c1", "skill", serde_json::json!({ "name": "review" }));
        let read = call("c2", "read", serde_json::json!({ "path": "a.rs" }));
        session
            .append_message(&Message::User(UserMessage::text("go")))
            .unwrap();
        session.append_message(&asks(&skill)).unwrap();
        session.append_message(&answer(&skill, 400)).unwrap();
        session.append_message(&asks(&read)).unwrap();
        session.append_message(&answer(&read, 800)).unwrap();
        session
            .append_message(&Message::Assistant(text_reply("done")))
            .unwrap();
        let load = ContextLoad::of(&session, &CompactionPrompts::default());
        // The skill's 100 tokens were read by two calls after it, the read's
        // 200 by one.
        assert_eq!(
            load.tools.get("skill:review"),
            Some(&ToolLoad {
                brought: 100,
                carried: 200,
            })
        );
        assert_eq!(
            load.tools.get("read"),
            Some(&ToolLoad {
                brought: 200,
                carried: 200,
            })
        );
        assert_eq!((load.now.skills, load.now.tool_output), (100, 200));
        assert_eq!(load.now.summary, 0);

        // Summarised with nothing kept: the results leave the context and
        // later calls read only the summary.
        session
            .append_compaction_with(&"s".repeat(40), 1_000, 0, Some(10), None)
            .unwrap();
        session
            .append_message(&Message::User(UserMessage::text("more")))
            .unwrap();
        session
            .append_message(&Message::Assistant(text_reply("ok")))
            .unwrap();
        let load = ContextLoad::of(&session, &CompactionPrompts::default());
        assert_eq!(load.tools["read"].carried, 200);
        assert_eq!((load.now.skills, load.now.tool_output), (0, 0));
        assert!(load.now.summary > 0);
    }
}
