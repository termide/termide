//! Context compaction: when the transcript nears the model's window, the
//! older part is replaced by a model-written summary and the recent tail is
//! kept verbatim.
//!
//! Triggers, as in pi, Claude Code and Codex: a token threshold checked
//! before each model call, and an overflow error from the provider, after
//! which the call is retried once. Token counts come from the last reported
//! usage plus a characters-over-four estimate for what was appended since.

use serde::{Deserialize, Serialize};

use crate::message::{AssistantContent, Message, ToolResultContent, UserContent, UserMessage};

/// A field left out of `[ai.compaction]` keeps its default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactionPolicy {
    pub enabled: bool,
    /// Compact when the context exceeds `context_window - reserve_tokens`.
    pub reserve_tokens: u64,
    /// Token budget of the most recent messages kept verbatim; capped at a
    /// quarter of the window.
    pub keep_recent_tokens: u64,
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            reserve_tokens: 16_384,
            keep_recent_tokens: 4_096,
        }
    }
}

/// Summaries shorter than this are treated as a failed compaction: a small
/// model asked to summarise almost nothing tends to answer `{}` or `None`.
pub const MIN_SUMMARY_CHARS: usize = 40;

/// Why a compaction ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionReason {
    Threshold,
    Overflow,
    Manual,
}

/// The seed of `ai/system/compact.md`: instructions of the summarisation
/// call, with the closing user turn in its front matter.
pub const SEED_COMPACT: &str = include_str!("../assets/system/compact.md");
/// The seed of `ai/system/compacted.md`: how the summary is worded when it
/// stands in for the summarised part of the transcript.
pub const SEED_COMPACTED: &str = include_str!("../assets/system/compacted.md");

/// The texts of a compaction, read from the `system/` files of the agent
/// directory so they can be seen and changed like the system prompt. No
/// prompt text lives in code: the seeds above are data files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionPrompts {
    /// System prompt of the summarisation call; `{{focus}}` takes the words
    /// the user gave to `/compact`, or disappears.
    pub instructions: String,
    /// The user turn that closes the summarisation request.
    pub request: String,
    /// The message that carries the summary back into the context;
    /// `{{summary}}` is replaced by the model's text.
    pub wrapper: String,
}

impl Default for CompactionPrompts {
    fn default() -> Self {
        Self::from_files(SEED_COMPACT, SEED_COMPACTED)
    }
}

impl CompactionPrompts {
    /// Parse `compact.md` (front matter `request:` plus the instructions)
    /// and `compacted.md` (the wrapper).
    #[must_use]
    pub fn from_files(compact: &str, compacted: &str) -> Self {
        let (fields, body) = crate::layers::split_front_matter(compact);
        Self {
            instructions: body.trim().to_string(),
            request: fields.get("request").cloned().unwrap_or_default(),
            wrapper: compacted.trim().to_string(),
        }
    }

    /// The system prompt of the summarisation call, with `focus` in place
    /// of `{{focus}}` when there is one.
    #[must_use]
    pub fn system_prompt(&self, focus: Option<&str>) -> String {
        let focus = focus.map(str::trim).filter(|f| !f.is_empty());
        let text = self.instructions.replace(
            "{{focus}}",
            &focus.map_or(String::new(), |f| format!("Focus on: {f}")),
        );
        let mut text = text.trim_end().to_string();
        while text.contains("\n\n\n") {
            text = text.replace("\n\n\n", "\n\n");
        }
        text
    }

    /// The transcript message that stands in for the summarised part.
    #[must_use]
    pub fn summary_message(&self, summary: &str) -> Message {
        Message::User(UserMessage::text(
            self.wrapper.replace("{{summary}}", summary.trim()),
        ))
    }

    /// Whether `text` is a summary this wrapper produced: it opens with the
    /// wrapper's words before `{{summary}}`. A wrapper that opens with the
    /// summary itself cannot be told, and no text counts.
    #[must_use]
    pub fn is_summary(&self, text: &str) -> bool {
        let lead = self
            .wrapper
            .split("{{summary}}")
            .next()
            .unwrap_or("")
            .trim();
        !lead.is_empty() && text.trim_start().starts_with(lead)
    }
}

/// Rough token count of messages with no usage data: characters over four.
#[must_use]
pub fn estimate_tokens(messages: &[Message]) -> u64 {
    let chars: usize = messages.iter().map(message_chars).sum();
    (chars / 4) as u64
}

fn message_chars(message: &Message) -> usize {
    match message {
        Message::User(user) => user
            .content
            .iter()
            .map(|block| match block {
                UserContent::Text { text } => text.chars().count(),
            })
            .sum(),
        Message::Assistant(assistant) => assistant
            .content
            .iter()
            .map(|block| match block {
                AssistantContent::Text { text } | AssistantContent::Thinking { text, .. } => {
                    text.chars().count()
                }
                AssistantContent::ToolCall(call) => {
                    call.name.len() + call.arguments.to_string().chars().count()
                }
            })
            .sum(),
        Message::ToolResult(result) => result
            .content
            .iter()
            .map(|block| match block {
                ToolResultContent::Text { text } => text.chars().count(),
            })
            .sum(),
    }
}

/// Best available size of the current context: the last reported usage plus
/// an estimate for everything appended after that message.
#[must_use]
pub fn context_tokens(messages: &[Message]) -> u64 {
    let last_usage = messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, m)| match m {
            Message::Assistant(a) if a.usage.total() > 0 => Some((index, a.usage.total())),
            _ => None,
        });
    match last_usage {
        Some((index, total)) => total + estimate_tokens(&messages[index + 1..]),
        None => estimate_tokens(messages),
    }
}

#[must_use]
pub fn should_compact(
    messages: &[Message],
    context_window: u64,
    policy: &CompactionPolicy,
) -> bool {
    // The reserve never eats more than a quarter of a small window, so a
    // model with a short context still gets to work before compacting.
    let reserve = policy.reserve_tokens.min(context_window / 4);
    policy.enabled
        && messages.len() >= 2
        && context_tokens(messages) > context_window.saturating_sub(reserve)
}

/// Index where the kept tail starts: the most recent messages that fit
/// `keep_tokens`, the last message always included. Never splits a tool
/// call from its results, and always leaves something to summarise when
/// there are at least two messages.
#[must_use]
pub fn split_point(messages: &[Message], keep_tokens: u64) -> usize {
    let len = messages.len();
    if len < 2 {
        return 0;
    }
    let mut start = len - 1;
    let mut chars = message_chars(&messages[start]);
    while start > 0 {
        let next = chars + message_chars(&messages[start - 1]);
        if (next / 4) as u64 > keep_tokens {
            break;
        }
        chars = next;
        start -= 1;
    }
    if start == 0 {
        // Everything fits the tail budget; keep only the last message so the
        // compaction still frees space.
        start = len - 1;
    }
    while start > 0 && matches!(messages[start], Message::ToolResult(_)) {
        start -= 1;
    }
    start
}

/// Whether a provider error says the request no longer fits the window.
#[must_use]
pub fn is_context_overflow_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    let mentions_context = lower.contains("context") || lower.contains("prompt");
    let mentions_size = [
        "length",
        "window",
        "too long",
        "too large",
        "exceed",
        "maximum",
        "too many tokens",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    (mentions_context && mentions_size) || lower.contains("context_length_exceeded")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_support::{text_reply, tool_reply};
    use crate::message::{StopReason, ToolCall, ToolResultMessage, Usage};
    use serde_json::json;

    fn user(text: &str) -> Message {
        Message::User(UserMessage::text(text))
    }

    #[test]
    fn tokens_use_last_usage_then_estimate() {
        let mut reply = text_reply("answer");
        reply.usage = Usage {
            input: 900,
            output: 100,
            cache_read: 0,
            cache_write: 0,
        };
        let messages = vec![user("q"), Message::Assistant(reply), user(&"x".repeat(400))];
        assert_eq!(context_tokens(&messages), 1000 + 100);
        assert_eq!(estimate_tokens(&[user(&"y".repeat(40))]), 10);

        let policy = CompactionPolicy {
            reserve_tokens: 100,
            ..Default::default()
        };
        assert!(should_compact(&messages, 1150, &policy));
        assert!(!should_compact(&messages, 1300, &policy));
        assert!(!should_compact(&messages[..1], 10, &policy));
        // A reserve larger than the window is capped at a quarter of it.
        let huge_reserve = CompactionPolicy {
            reserve_tokens: 1_000_000,
            ..Default::default()
        };
        assert!(!should_compact(&messages, 1600, &huge_reserve));
        assert!(should_compact(&messages, 1400, &huge_reserve));
    }

    #[test]
    fn split_keeps_tool_pairs_together_and_leaves_a_prefix() {
        let call = ToolCall {
            id: "c".into(),
            name: "read".into(),
            arguments: json!({}),
            extra_content: None,
        };
        let messages = vec![
            user("one"),
            Message::Assistant(text_reply("a1")),
            user("two"),
            Message::Assistant(tool_reply(
                vec![("c", "read", json!({}))],
                StopReason::ToolUse,
            )),
            Message::ToolResult(ToolResultMessage::text(&call, "r")),
            Message::Assistant(text_reply("a2")),
        ];
        // Sizes in chars: "one"=3, "a1"=2, "two"=3, call≈6, "r"=1, "a2"=2.
        // A 1-token budget keeps only "a2"; the tail may not start at the
        // tool result, so it moves back to the call.
        assert_eq!(split_point(&messages, 1), 3);
        // Two tokens (8 chars) cover "a2", "r" and the call (9 chars → 2).
        assert_eq!(split_point(&messages, 2), 3);
        // Three tokens reach back to "a1"; "one" would make 17 chars → 4.
        assert_eq!(split_point(&messages, 3), 1);
        // Everything fits: keep the last message only ("a2" is not a tool
        // result, so no adjustment).
        assert_eq!(split_point(&messages, 1_000), 5);
        assert_eq!(split_point(&messages[..2], 1_000), 1);
        assert_eq!(split_point(&messages[..1], 1), 0);
        let long = vec![
            user(&"x".repeat(400)),
            user(&"y".repeat(400)),
            user(&"z".repeat(40)),
        ];
        // 100 tokens keep "y" (100) and "z" (10) together? 110 > 100: only z.
        assert_eq!(split_point(&long, 100), 2);
        assert_eq!(split_point(&long, 120), 1);
    }

    #[test]
    fn overflow_errors_are_recognised() {
        assert!(is_context_overflow_error(
            "HTTP 400: This model's maximum context length is 32000 tokens"
        ));
        assert!(is_context_overflow_error(
            "prompt is too long: 210000 tokens > 200000 maximum"
        ));
        assert!(is_context_overflow_error("context_length_exceeded"));
        assert!(!is_context_overflow_error("HTTP 401: invalid api key"));
        assert!(!is_context_overflow_error("connection reset"));
    }

    #[test]
    fn prompts_come_from_the_files_and_take_a_focus() {
        let prompts = CompactionPrompts::default();
        assert!(prompts
            .instructions
            .starts_with("You are compacting a coding session."));
        assert_eq!(
            prompts.request,
            "Summarize the conversation above following the instructions."
        );
        let Message::User(u) = prompts.summary_message("  done things \n") else {
            panic!()
        };
        assert_eq!(
            u.plain_text(),
            "Summary of the earlier conversation (compacted):\n\ndone things\n\nContinue the task from this summary without repeating completed steps."
        );
        // Without a focus the placeholder line goes; with one it is filled.
        assert!(prompts
            .system_prompt(None)
            .ends_with("Plain text, no preamble."));
        assert!(prompts
            .system_prompt(Some(" the API changes "))
            .ends_with("Focus on: the API changes"));

        let custom = CompactionPrompts::from_files(
            "---\nrequest: Wrap it up.\n---\nBe brief.\n{{focus}}\n",
            "Earlier: {{summary}}",
        );
        assert_eq!(custom.request, "Wrap it up.");
        assert_eq!(custom.system_prompt(None), "Be brief.");
        let Message::User(u) = custom.summary_message("x") else {
            panic!()
        };
        assert_eq!(u.plain_text(), "Earlier: x");
    }
}
