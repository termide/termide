//! A conversation retold as text, for an external agent that joins it
//! without a session of its own to resume.
//!
//! The recap keeps what [`crate::prune::prune_to_decisions`] keeps — the
//! user's messages, the questions put to the user with their answers, the
//! skills loaded and the answers that closed each run — so nothing is
//! reworded, and leaves out the exploration, which the agent can redo. It
//! goes before the first request as context, never into the session log.

use crate::message::{AssistantContent, Message};
use crate::prune::prune_to_decisions;

/// The most of the recap an agent is sent, in characters: the newest
/// exchanges are kept whole and older ones dropped once it is reached.
pub const RECAP_LIMIT: usize = 100_000;

const LEAD: &str = "You are joining a conversation that began before this session, so its \
earlier part is retold here. Only the requests, the questions asked with their answers, \
the skills loaded and the answers that ended each run are kept; file contents, commands \
and their output are left out, so re-read anything before relying on it. The request to \
answer now follows after this history.";

/// The recap of `messages` within `limit` characters, `None` when they hold
/// nothing to retell.
#[must_use]
pub fn recap(messages: &[Message], limit: usize) -> Option<String> {
    let pruned = prune_to_decisions(messages);
    let parts: Vec<String> = pruned.iter().filter_map(retell).collect();
    // Newest first, until the next one would not fit.
    let mut kept = Vec::new();
    let mut size = 0;
    for part in parts.iter().rev() {
        size += part.len() + 1;
        if size > limit && !kept.is_empty() {
            break;
        }
        kept.push(part.as_str());
    }
    if kept.is_empty() {
        return None;
    }
    let left_out = parts.len() - kept.len();
    kept.reverse();
    let mut text = format!("<conversation_history>\n{LEAD}\n");
    if left_out > 0 {
        text.push_str(&format!(
            "\n({left_out} earlier entries are left out for length.)\n"
        ));
    }
    for part in kept {
        text.push('\n');
        text.push_str(part);
        text.push('\n');
    }
    text.push_str("</conversation_history>");
    Some(text)
}

/// One message of the pruned history as a tagged block.
fn retell(message: &Message) -> Option<String> {
    let block = |tag: &str, body: &str| {
        let body = body.trim();
        (!body.is_empty()).then(|| format!("<{tag}>\n{body}\n</{tag}>"))
    };
    match message {
        Message::User(user) => block("user", &user.plain_text()),
        Message::Assistant(assistant) => {
            let blocks: Vec<String> = assistant
                .content
                .iter()
                .filter_map(|content| match content {
                    AssistantContent::Text { text } => block("assistant", text),
                    AssistantContent::ToolCall(call) if call.name == "skill" => {
                        let name = call.arguments["name"].as_str().unwrap_or_default();
                        block("skill_loaded", name)
                    }
                    AssistantContent::ToolCall(call) => {
                        block("question_asked", &call.arguments.to_string())
                    }
                    AssistantContent::Thinking { .. } => None,
                })
                .collect();
            (!blocks.is_empty()).then(|| blocks.join("\n"))
        }
        // A skill's instructions are reloaded on demand; a question's
        // answer is the decision itself.
        Message::ToolResult(result) if result.tool_name == "skill" => None,
        Message::ToolResult(result) => {
            let tag = if result.is_error {
                "question_declined"
            } else {
                "question_answered"
            };
            block(tag, &result.plain_text())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{StopReason, ToolCall, ToolResultMessage, Usage, UserMessage};
    use serde_json::json;

    fn assistant(content: Vec<AssistantContent>) -> Message {
        Message::Assistant(crate::message::AssistantMessage {
            content,
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            provider: "fake".into(),
            model: "m".into(),
            error_message: None,
            timestamp: 0,
        })
    }

    fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
            extra_content: None,
        }
    }

    #[test]
    fn decisions_are_retold_and_exploration_left_out() {
        let read = call("r1", "read", json!({ "path": "secret.rs" }));
        let skill = call("s1", "skill", json!({ "name": "commit" }));
        let question = call("q1", "question", json!({ "question": "Which crate?" }));
        let history = vec![
            Message::User(UserMessage::text("plan the change")),
            assistant(vec![
                AssistantContent::thinking("look around"),
                AssistantContent::Text {
                    text: "Reading.".into(),
                },
                AssistantContent::ToolCall(read.clone()),
                AssistantContent::ToolCall(skill.clone()),
            ]),
            Message::ToolResult(ToolResultMessage::text(&read, "file body")),
            Message::ToolResult(ToolResultMessage::text(&skill, "skill body")),
            assistant(vec![AssistantContent::ToolCall(question.clone())]),
            Message::ToolResult(ToolResultMessage::text(&question, "core")),
            assistant(vec![AssistantContent::Text {
                text: "The plan.".into(),
            }]),
        ];
        let text = recap(&history, RECAP_LIMIT).unwrap();
        assert!(text.starts_with("<conversation_history>\n"));
        assert!(text.ends_with("</conversation_history>"));
        for kept in [
            "<user>\nplan the change\n</user>",
            "<skill_loaded>\ncommit\n</skill_loaded>",
            "Which crate?",
            "<question_answered>\ncore\n</question_answered>",
            "<assistant>\nThe plan.\n</assistant>",
        ] {
            assert!(text.contains(kept), "{kept} missing from {text}");
        }
        for gone in [
            "look around",
            "Reading.",
            "secret.rs",
            "file body",
            "skill body",
        ] {
            assert!(!text.contains(gone), "{gone} kept in {text}");
        }
    }

    #[test]
    fn nothing_to_retell_is_no_recap() {
        assert_eq!(recap(&[], RECAP_LIMIT), None);
        let empty = vec![Message::User(UserMessage::text("  "))];
        assert_eq!(recap(&empty, RECAP_LIMIT), None);
    }

    #[test]
    fn the_newest_entries_are_kept_when_the_history_is_long() {
        let history: Vec<Message> = (0..10)
            .map(|i| Message::User(UserMessage::text(format!("request {i} {}", "x".repeat(50)))))
            .collect();
        let text = recap(&history, 200).unwrap();
        assert!(text.contains("request 9"));
        assert!(!text.contains("request 0"));
        assert!(text.contains("earlier entries are left out"));
        // One entry over the limit still goes, alone.
        let text = recap(&history[..1], 10).unwrap();
        assert!(text.contains("request 0"));
    }
}
