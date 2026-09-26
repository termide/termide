//! Transcript types shared by providers, tools, the loop and the UI.
//!
//! The shapes are serde-friendly on purpose: the same structs are appended
//! to the session log and sent (after conversion) to a model API. Content is
//! a list of typed blocks so images or other block kinds can be added later
//! without changing the message envelope.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Milliseconds since the Unix epoch, used to timestamp transcript entries.
#[must_use]
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Why the model stopped producing the assistant message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Natural end of the answer.
    Stop,
    /// The output token limit was hit; tool call arguments may be truncated.
    Length,
    /// The model asked for tool calls and is waiting for their results.
    ToolUse,
    /// The request failed; see [`AssistantMessage::error_message`].
    Error,
    /// The run was cancelled through a [`crate::CancelToken`].
    Aborted,
}

/// Token accounting for one assistant message.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
}

impl Usage {
    /// The prompt tokens not served from the cache, billed at the full
    /// input price or above: the uncached input and what was written to the
    /// cache.
    #[must_use]
    pub fn uncached(&self) -> u64 {
        self.input.saturating_add(self.cache_write)
    }

    /// Every token that counted against the context window.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }
}

/// A block inside a user message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserContent {
    Text { text: String },
}

/// A tool invocation requested by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-assigned id echoed back in the matching [`ToolResultMessage`].
    pub id: String,
    pub name: String,
    /// Parsed JSON arguments; an object for well-formed calls.
    pub arguments: Value,
}

/// A block inside an assistant message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContent {
    Text { text: String },
    Thinking { text: String },
    ToolCall(ToolCall),
}

/// A block inside a tool result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultContent {
    Text { text: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: Vec<UserContent>,
    pub timestamp: u64,
}

impl UserMessage {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![UserContent::Text { text: text.into() }],
            timestamp: now_millis(),
        }
    }

    /// `messages` as one, their texts joined by a blank line, or `None` for
    /// none: messages queued one after another are usually one thought
    /// written in pieces.
    #[must_use]
    pub fn merge(messages: Vec<UserMessage>) -> Option<UserMessage> {
        if messages.len() <= 1 {
            return messages.into_iter().next();
        }
        let text = messages
            .iter()
            .map(UserMessage::plain_text)
            .collect::<Vec<_>>()
            .join("\n\n");
        Some(UserMessage::text(text))
    }

    /// Concatenated text blocks.
    #[must_use]
    pub fn plain_text(&self) -> String {
        self.content
            .iter()
            .map(|block| match block {
                UserContent::Text { text } => text.as_str(),
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub content: Vec<AssistantContent>,
    pub stop_reason: StopReason,
    #[serde(default)]
    pub usage: Usage,
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub timestamp: u64,
}

impl AssistantMessage {
    /// A message that carries no content, only a failure. Providers return
    /// this instead of an `Err`, so the loop sees a uniform result.
    #[must_use]
    pub fn failed(
        provider: impl Into<String>,
        model: impl Into<String>,
        stop_reason: StopReason,
        error_message: impl Into<String>,
    ) -> Self {
        Self {
            content: Vec::new(),
            stop_reason,
            usage: Usage::default(),
            provider: provider.into(),
            model: model.into(),
            error_message: Some(error_message.into()),
            timestamp: now_millis(),
        }
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = &ToolCall> {
        self.content.iter().filter_map(|block| match block {
            AssistantContent::ToolCall(call) => Some(call),
            _ => None,
        })
    }

    /// Concatenated text blocks, thinking excluded.
    #[must_use]
    pub fn plain_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                AssistantContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Concatenated thinking blocks, so a reopened conversation can show the
    /// reasoning it recorded.
    #[must_use]
    pub fn thinking_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                AssistantContent::Thinking { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: Vec<ToolResultContent>,
    #[serde(default)]
    pub is_error: bool,
    /// Structured data for the UI (a diff, a truncation report); never sent
    /// to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    pub timestamp: u64,
}

impl ToolResultMessage {
    #[must_use]
    pub fn text(call: &ToolCall, text: impl Into<String>) -> Self {
        Self {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            content: vec![ToolResultContent::Text { text: text.into() }],
            is_error: false,
            details: None,
            timestamp: now_millis(),
        }
    }

    #[must_use]
    pub fn error(call: &ToolCall, text: impl Into<String>) -> Self {
        Self {
            is_error: true,
            ..Self::text(call, text)
        }
    }

    #[must_use]
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Concatenated text blocks.
    #[must_use]
    pub fn plain_text(&self) -> String {
        self.content
            .iter()
            .map(|block| match block {
                ToolResultContent::Text { text } => text.as_str(),
            })
            .collect()
    }
}

/// One transcript entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn message_round_trips_through_json_with_role_tag() {
        let assistant = AssistantMessage {
            content: vec![
                AssistantContent::Thinking {
                    text: "plan".into(),
                },
                AssistantContent::Text {
                    text: "hello".into(),
                },
                AssistantContent::ToolCall(ToolCall {
                    id: "c1".into(),
                    name: "read".into(),
                    arguments: json!({ "path": "Cargo.toml" }),
                }),
            ],
            stop_reason: StopReason::ToolUse,
            usage: Usage {
                input: 10,
                output: 5,
                cache_read: 0,
                cache_write: 0,
            },
            provider: "fake".into(),
            model: "m".into(),
            error_message: None,
            timestamp: 1,
        };
        let messages = vec![
            Message::User(UserMessage {
                content: vec![UserContent::Text { text: "hi".into() }],
                timestamp: 0,
            }),
            Message::Assistant(assistant.clone()),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "c1".into(),
                tool_name: "read".into(),
                content: vec![ToolResultContent::Text { text: "ok".into() }],
                is_error: false,
                details: None,
                timestamp: 2,
            }),
        ];

        let encoded = serde_json::to_string(&messages).unwrap();
        assert!(encoded.contains("\"role\":\"assistant\""));
        assert!(encoded.contains("\"type\":\"tool_call\""));
        assert!(encoded.contains("\"stop_reason\":\"tool_use\""));
        assert!(!encoded.contains("error_message"));

        let decoded: Vec<Message> = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, messages);
        assert_eq!(assistant.plain_text(), "hello");
        assert_eq!(assistant.tool_calls().count(), 1);
    }

    #[test]
    fn error_result_flags_and_copies_call_identity() {
        let call = ToolCall {
            id: "id-7".into(),
            name: "bash".into(),
            arguments: json!({}),
        };
        let result = ToolResultMessage::error(&call, "boom");
        assert!(result.is_error);
        assert_eq!(result.tool_call_id, "id-7");
        assert_eq!(result.tool_name, "bash");
        assert_eq!(result.plain_text(), "boom");
    }
}
