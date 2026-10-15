//! A conversation retold as text, for an external agent that joins it
//! without a session of its own to resume.
//!
//! When the agent's window is known, the whole conversation goes, as a
//! record of what was said and done — requests, answers, reasoning, calls
//! and their output — cut down to fit half the window by
//! [`crate::fit::fit`] ([`handoff`]). Otherwise only the decisions go:
//!
//! The recap keeps what [`crate::prune::prune_to_decisions`] keeps — the
//! user's messages, the questions put to the user with their answers, the
//! skills loaded and the answers that closed each run — so nothing is
//! reworded, and leaves out the exploration, which the agent can redo. It
//! goes before the first request as context, never into the session log.

use crate::fit::fit;
use crate::message::{AssistantContent, Message, ToolResultContent};
use crate::prune::prune_to_decisions;

/// The most of the recap an agent is sent, in characters: the newest
/// exchanges are kept whole and older ones dropped once it is reached.
pub const RECAP_LIMIT: usize = 100_000;

const LEAD: &str = "You are joining a conversation that began before this session, so its \
earlier part is retold here. Only the requests, the questions asked with their answers, \
the skills loaded and the answers that ended each run are kept; file contents, commands \
and their output are left out, so re-read anything before relying on it. The request to \
answer now follows after this history.";

const RECORD_LEAD: &str = "You are joining a conversation that began before this session, so its \
earlier part follows as a record: the requests, the answers, the reasoning, the tool calls \
made and their output. Parts may be shortened or left out to fit, as marked. The calls in it \
already ran — do not run them again unless you need fresh output, and keep in mind that files \
may have changed since. The request to answer now follows after this history.";

/// The most of the record that is the most recent messages, kept whole
/// when the rest is cut down: a quarter.
const RECENT_SHARE: usize = 4;

/// The conversation as a record within `budget` characters, cut down by
/// [`fit`] as far as needed — what the agent gets when its window is
/// known; `None` when there is nothing to tell or it cannot be made to
/// fit, for the caller to fall back on [`recap`].
#[must_use]
pub fn handoff(messages: &[Message], budget: usize) -> Option<String> {
    // The tags take room the fitting does not count: fit to less, and
    // again to less still if the text comes out over.
    let mut room = budget.saturating_sub(RECORD_LEAD.len() + 200);
    for _ in 0..4 {
        let fitted = fit(messages, room, room / RECENT_SHARE);
        let text = record(&fitted.messages)?;
        if text.chars().count() <= budget {
            return fitted.fits.then_some(text);
        }
        room = room.saturating_sub(text.chars().count() - budget + room / 20);
    }
    None
}

/// `messages` as a tagged record, `None` when they hold nothing.
fn record(messages: &[Message]) -> Option<String> {
    let names: std::collections::HashMap<&str, &str> = messages
        .iter()
        .filter_map(|message| match message {
            Message::Assistant(assistant) => Some(assistant.tool_calls()),
            _ => None,
        })
        .flatten()
        .map(|call| (call.id.as_str(), call.name.as_str()))
        .collect();
    let block = |tag: &str, body: &str| {
        let body = body.trim();
        (!body.is_empty()).then(|| format!("<{tag}>\n{body}\n</{tag}>"))
    };
    let parts: Vec<String> = messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => block("user", &user.plain_text()),
            Message::Assistant(assistant) => {
                let blocks: Vec<String> = assistant
                    .content
                    .iter()
                    .filter_map(|content| match content {
                        AssistantContent::Text { text } => block("assistant", text),
                        AssistantContent::Thinking { text, .. } => block("reasoning", text),
                        AssistantContent::ToolCall(call) => Some(format!(
                            "<tool_call name=\"{}\">\n{}\n</tool_call>",
                            call.name, call.arguments
                        )),
                    })
                    .collect();
                (!blocks.is_empty()).then(|| blocks.join("\n"))
            }
            Message::ToolResult(result) => {
                let name = names
                    .get(result.tool_call_id.as_str())
                    .copied()
                    .unwrap_or(&result.tool_name);
                let text: Vec<&str> = result
                    .content
                    .iter()
                    .map(|block| match block {
                        ToolResultContent::Text { text } => text.as_str(),
                        ToolResultContent::Image { .. } => "[image]",
                    })
                    .collect();
                let error = if result.is_error {
                    " error=\"true\""
                } else {
                    ""
                };
                Some(format!(
                    "<tool_result name=\"{name}\"{error}>\n{}\n</tool_result>",
                    text.join("\n").trim()
                ))
            }
        })
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(format!(
        "<conversation_history>\n{RECORD_LEAD}\n\n{}\n</conversation_history>",
        parts.join("\n")
    ))
}

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
            failure: None,
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
    fn with_room_the_whole_conversation_goes_as_a_record() {
        let read = call("r1", "read", json!({ "path": "src/lib.rs" }));
        let history = vec![
            Message::User(UserMessage::text("plan the change")),
            assistant(vec![
                AssistantContent::thinking("look around"),
                AssistantContent::Text {
                    text: "Reading.".into(),
                },
                AssistantContent::ToolCall(read.clone()),
            ]),
            Message::ToolResult(ToolResultMessage::text(&read, "fn main() {}")),
            assistant(vec![AssistantContent::Text {
                text: "The plan.".into(),
            }]),
        ];
        let text = handoff(&history, 10_000).unwrap();
        for kept in [
            "<user>\nplan the change\n</user>",
            "<reasoning>\nlook around\n</reasoning>",
            "<assistant>\nReading.\n</assistant>",
            "<tool_call name=\"read\">\n{\"path\":\"src/lib.rs\"}\n</tool_call>",
            "<tool_result name=\"read\">\nfn main() {}\n</tool_result>",
            "<assistant>\nThe plan.\n</assistant>",
        ] {
            assert!(text.contains(kept), "{kept} missing from {text}");
        }
        assert!(text.starts_with("<conversation_history>"));
        assert!(text.ends_with("</conversation_history>"));
    }

    #[test]
    fn a_record_too_long_is_cut_down_to_its_budget() {
        let history: Vec<Message> = (0..30)
            .flat_map(|i| {
                let read = call(&format!("r{i}"), "read", json!({ "path": format!("f{i}") }));
                vec![
                    Message::User(UserMessage::text(format!("request {i}"))),
                    assistant(vec![
                        AssistantContent::thinking("t".repeat(2_000)),
                        AssistantContent::ToolCall(read.clone()),
                    ]),
                    Message::ToolResult(ToolResultMessage::text(&read, "o".repeat(5_000))),
                    assistant(vec![AssistantContent::Text {
                        text: format!("answer {i}"),
                    }]),
                ]
            })
            .collect();
        let text = handoff(&history, 20_000).unwrap();
        assert!(text.chars().count() <= 20_000, "{}", text.len());
        // The latest stays whole, the oldest output is left out.
        assert!(text.contains("request 29") && text.contains("answer 29"));
        assert!(text.contains(&"o".repeat(5_000)));
        assert!(text.contains("characters of output left out"));
        // Nothing could fit in too small a budget.
        assert_eq!(handoff(&history, 100), None);
        assert_eq!(handoff(&[], 10_000), None);
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
