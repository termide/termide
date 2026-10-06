//! Clearing a plan's exploration from the model's context.
//!
//! When the user carries a plan out "from a clean context", the history the
//! model sees keeps what was decided and drops how it was found out: the
//! user's messages, the questions put to the user with their answers, the
//! skills loaded, and every answer the agent finished a run with (the plan
//! among them). File reads, commands, web pages, searches and delegated
//! reports go. Nothing is summarised, so no decision is reworded.
//!
//! The same function runs on the live history and when a session log is
//! replayed past its [`crate::session::EntryKind::Pruned`] entry, so both
//! agree by construction.

use std::collections::HashSet;

use crate::message::{AssistantContent, AssistantMessage, Message, Usage};

/// Tools whose calls and results survive the pruning: a question records a
/// decision, a skill the instructions the work follows.
const KEPT_TOOLS: [&str; 2] = ["question", "skill"];

/// `messages` with the exploration left out. Every call kept keeps its
/// result and every result kept its call, so the history stays valid for
/// any provider. The kept assistant messages lose their usage, so the next
/// context estimate does not count what was removed.
#[must_use]
pub fn prune_to_decisions(messages: &[Message]) -> Vec<Message> {
    prune_by(messages, |message| message)
        .into_iter()
        .map(|(_, message)| message)
        .collect()
}

/// [`prune_to_decisions`] over items that carry a message: `message` reads
/// it out, and each kept message is returned beside the item it came from,
/// so a caller can keep its own metadata (a log's timestamps) alongside.
pub fn prune_by<'a, T>(
    items: &'a [T],
    message: impl Fn(&'a T) -> &'a Message,
) -> Vec<(&'a T, Message)> {
    let kept_calls: HashSet<&str> = items
        .iter()
        .filter_map(|item| match message(item) {
            Message::Assistant(assistant) => Some(assistant.tool_calls()),
            _ => None,
        })
        .flatten()
        .filter(|call| KEPT_TOOLS.contains(&call.name.as_str()))
        .map(|call| call.id.as_str())
        .collect();
    // A kept call whose result never came (a run cut short) is left out
    // with it: a call without a result is not a valid history.
    let answered: HashSet<&str> = items
        .iter()
        .filter_map(|item| match message(item) {
            Message::ToolResult(result) => Some(result.tool_call_id.as_str()),
            _ => None,
        })
        .filter(|id| kept_calls.contains(id))
        .collect();

    items
        .iter()
        .filter_map(|item| {
            let pruned = match message(item) {
                Message::User(_) => Some(message(item).clone()),
                Message::ToolResult(result) => answered
                    .contains(result.tool_call_id.as_str())
                    .then(|| message(item).clone()),
                Message::Assistant(assistant) => {
                    let has_calls = assistant.tool_calls().next().is_some();
                    let content: Vec<AssistantContent> = assistant
                        .content
                        .iter()
                        .filter(|block| match block {
                            // A run's closing answer keeps its words; text
                            // between tool calls is the exploration's.
                            AssistantContent::Text { text } => {
                                !has_calls && !text.trim().is_empty()
                            }
                            AssistantContent::Thinking { .. } => false,
                            AssistantContent::ToolCall(call) => answered.contains(call.id.as_str()),
                        })
                        .cloned()
                        .collect();
                    (!content.is_empty()).then(|| {
                        Message::Assistant(AssistantMessage {
                            content,
                            usage: Usage::default(),
                            error_message: assistant.error_message.clone(),
                            provider: assistant.provider.clone(),
                            model: assistant.model.clone(),
                            stop_reason: assistant.stop_reason,
                            timestamp: assistant.timestamp,
                        })
                    })
                }
            };
            pruned.map(|pruned| (item, pruned))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{StopReason, ToolCall, ToolResultMessage, UserContent, UserMessage};
    use serde_json::json;

    fn user(text: &str) -> Message {
        Message::User(UserMessage {
            content: vec![UserContent::Text { text: text.into() }],
            timestamp: 0,
            command: None,
            ran: None,
            ran_failed: false,
        })
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: json!({}),
            extra_content: None,
        }
    }

    fn assistant(content: Vec<AssistantContent>) -> Message {
        Message::Assistant(AssistantMessage {
            content,
            stop_reason: StopReason::ToolUse,
            usage: Usage {
                input: 1000,
                output: 10,
                cache_read: 0,
                cache_write: 0,
            },
            provider: "fake".into(),
            model: "m".into(),
            error_message: None,
            timestamp: 0,
        })
    }

    fn text(text: &str) -> AssistantContent {
        AssistantContent::Text { text: text.into() }
    }

    fn result(id: &str, name: &str, error: bool) -> Message {
        let call = call(id, name);
        Message::ToolResult(if error {
            ToolResultMessage::error(&call, "declined")
        } else {
            ToolResultMessage::text(&call, "output")
        })
    }

    /// The tool calls and results left, in order, as `name:id`.
    fn shape(messages: &[Message]) -> Vec<String> {
        messages
            .iter()
            .map(|message| match message {
                Message::User(user) => format!("user:{}", user.plain_text()),
                Message::Assistant(assistant) => {
                    let calls: Vec<String> = assistant
                        .tool_calls()
                        .map(|call| format!("{}:{}", call.name, call.id))
                        .collect();
                    if calls.is_empty() {
                        format!("answer:{}", assistant.plain_text())
                    } else {
                        format!("calls[{}]", calls.join(","))
                    }
                }
                Message::ToolResult(result) => {
                    format!("result:{}:{}", result.tool_name, result.tool_call_id)
                }
            })
            .collect()
    }

    #[test]
    fn exploration_goes_and_decisions_stay() {
        let history = vec![
            user("plan the change"),
            assistant(vec![
                AssistantContent::thinking("look around"),
                text("Let me read."),
                AssistantContent::ToolCall(call("r1", "read")),
                AssistantContent::ToolCall(call("s1", "skill")),
            ]),
            result("r1", "read", false),
            result("s1", "skill", false),
            assistant(vec![
                text("Two questions."),
                AssistantContent::ToolCall(call("b1", "bash")),
                AssistantContent::ToolCall(call("q1", "question")),
            ]),
            result("b1", "bash", false),
            result("q1", "question", false),
            assistant(vec![AssistantContent::ToolCall(call("q2", "question"))]),
            result("q2", "question", true),
            assistant(vec![AssistantContent::thinking("done"), text("The plan.")]),
        ];
        let pruned = prune_to_decisions(&history);
        assert_eq!(
            shape(&pruned),
            [
                "user:plan the change",
                "calls[skill:s1]",
                "result:skill:s1",
                "calls[question:q1]",
                "result:question:q1",
                "calls[question:q2]",
                "result:question:q2",
                "answer:The plan.",
            ]
        );
        for message in &pruned {
            if let Message::Assistant(assistant) = message {
                assert_eq!(assistant.usage, Usage::default());
                assert!(assistant
                    .content
                    .iter()
                    .all(|block| !matches!(block, AssistantContent::Thinking { .. })));
            }
        }
    }

    #[test]
    fn a_message_left_empty_goes_and_an_unanswered_call_with_it() {
        let history = vec![
            user("go"),
            assistant(vec![
                text("reading"),
                AssistantContent::ToolCall(call("r1", "read")),
            ]),
            result("r1", "read", false),
            // Cut short: the question was never answered.
            assistant(vec![AssistantContent::ToolCall(call("q1", "question"))]),
        ];
        assert_eq!(shape(&prune_to_decisions(&history)), ["user:go"]);
    }

    #[test]
    fn pruning_twice_changes_nothing() {
        let history = vec![
            user("go"),
            assistant(vec![AssistantContent::ToolCall(call("q1", "question"))]),
            result("q1", "question", false),
            assistant(vec![text("plan")]),
        ];
        let once = prune_to_decisions(&history);
        assert_eq!(prune_to_decisions(&once), once);
    }
}
