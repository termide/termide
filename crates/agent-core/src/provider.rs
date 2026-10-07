//! The model provider contract.
//!
//! A provider turns a [`Request`] into one streamed [`AssistantMessage`].
//! Streaming deltas go to a callback so the UI can paint text as it arrives;
//! the returned message is authoritative and is what enters the transcript.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cancel::CancelToken;
use crate::message::{AssistantMessage, Message, StopReason, UserMessage};

/// How much reasoning effort to request from a model that supports it.
///
/// A provider-neutral scale: each provider maps it onto its own parameter
/// (an effort name, a token budget, an on/off switch), and a model offers
/// only the levels its API accepts ([`Provider::thinking_levels`]).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    #[default]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
    Max,
}

impl ThinkingLevel {
    pub const ALL: [Self; 7] = [
        Self::Off,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    /// The name used in the config, the session log and the status bar.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|level| level.label().eq_ignore_ascii_case(name.trim()))
    }

    /// The level of `offered` that best stands for `self`: itself when
    /// offered, else the highest one below it, else the lowest one above.
    /// Reasoning asked for never falls to `Off` (an on/off model answers
    /// any level with on), and a model that cannot stop thinking answers
    /// `Off` with its least. `Off` when nothing is offered.
    #[must_use]
    pub fn nearest(self, offered: &[Self]) -> Self {
        if self == Self::Off {
            return offered.iter().copied().min().unwrap_or(Self::Off);
        }
        let on = || offered.iter().copied().filter(|level| *level != Self::Off);
        on().filter(|level| *level <= self)
            .max()
            .or_else(|| on().min())
            .unwrap_or(Self::Off)
    }
}

/// A model as configured by the user, independent of the provider wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSpec {
    /// Provider name the model is served by, e.g. `"openai-compatible"`.
    pub provider: String,
    /// Model id as the provider expects it.
    pub id: String,
    /// Context window in tokens; drives compaction thresholds.
    pub context_window: u64,
    /// Upper bound for output tokens per response; `None` leaves the length
    /// to the model (a provider whose API requires a bound sends its own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// The reasoning level asked for; the provider falls back to the nearest
    /// one the model accepts.
    #[serde(default)]
    pub thinking: ThinkingLevel,
}

/// One entry of a provider's model list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// Model id as the provider expects it.
    pub id: String,
    /// Context window in tokens when the endpoint reports it (vLLM and omlx
    /// do as `max_model_len`; Ollama and llama.cpp do not).
    pub context_window: Option<u64>,
}

/// Tool description in the shape model APIs expect: name, description and a
/// JSON Schema for the arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Everything a provider needs for one model call.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub model: &'a ModelSpec,
    pub system_prompt: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
    pub thinking: ThinkingLevel,
}

/// Incremental piece of an assistant message, for live rendering only.
///
/// Consumers must not rebuild the message from deltas; the provider returns
/// the complete [`AssistantMessage`] when the stream ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    TextDelta(String),
    ThinkingDelta(String),
    ToolCallStart {
        id: String,
        name: String,
    },
    /// A fragment of the raw JSON arguments for the call with this id.
    ToolCallDelta {
        id: String,
        arguments: String,
    },
    ToolCallEnd {
        id: String,
    },
    /// How far the server has read the prompt, from a server that reports
    /// it (llama.cpp's `prompt_progress`): `processed` of `total` tokens,
    /// `cached` of them served from its cache.
    PrefillProgress {
        processed: u64,
        total: u64,
        cached: u64,
    },
    /// The request waits for a free slot of its connection, with `ahead`
    /// others waiting before it; sent again whenever that number changes.
    Queued {
        ahead: usize,
    },
    /// The request that was [`Self::Queued`] got its slot and is sent now:
    /// the model's reading starts here, not when it began to wait.
    Admitted,
    /// The request failed before any content arrived and will be retried
    /// after `delay_ms`.
    Retry {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        error: String,
    },
}

/// A model backend.
///
/// `stream` never fails: transport errors, HTTP errors and cancellation are
/// reported through [`AssistantMessage::stop_reason`] and
/// [`AssistantMessage::error_message`]. Implementations poll `cancel` between
/// chunks and return a message with [`crate::StopReason::Aborted`] when it is
/// set.
pub trait Provider: Send + Sync {
    /// Stable provider name, recorded on every assistant message.
    fn name(&self) -> &str;

    fn stream(
        &self,
        request: &Request<'_>,
        on_event: &mut dyn FnMut(StreamEvent),
        cancel: &CancelToken,
    ) -> AssistantMessage;

    /// Where the models are served, for the status line: a host and port, a
    /// gateway's name. `None` when there is nothing useful to show.
    fn endpoint(&self) -> Option<String> {
        None
    }

    /// The models the endpoint serves, for a picker. Blocking; call it off
    /// the UI thread. The default says the provider cannot enumerate them,
    /// and callers fall back to a typed id.
    fn list_models(&self) -> Result<Vec<ModelInfo>, String> {
        Err("this provider cannot list its models".to_string())
    }

    /// The reasoning levels `model` accepts, lowest first; empty when the
    /// provider has no way to ask for reasoning (the model decides alone).
    fn thinking_levels(&self, _model: &str) -> Vec<ThinkingLevel> {
        Vec::new()
    }
}

/// Ask `provider` one question: `system` as the system prompt, `user` as the
/// only turn, no tools and no reasoning. The reply's text comes back; a call
/// that fails, is aborted or answers nothing is an `Err` with the reason.
///
/// # Errors
///
/// When the call ends in an error or an abort, or the reply has no text.
pub fn one_shot(
    provider: &dyn Provider,
    model: &ModelSpec,
    system: &str,
    user: &str,
    cancel: &CancelToken,
) -> Result<String, String> {
    let messages = [Message::User(UserMessage::text(user))];
    let request = Request {
        model,
        system_prompt: system,
        messages: &messages,
        tools: &[],
        thinking: ThinkingLevel::Off,
    };
    let reply = provider.stream(&request, &mut |_| {}, cancel);
    if matches!(reply.stop_reason, StopReason::Error | StopReason::Aborted) {
        return Err(reply
            .error_message
            .unwrap_or_else(|| "the call did not finish".to_string()));
    }
    let text = reply.plain_text();
    if text.trim().is_empty() {
        return Err("the model answered nothing".to_string());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::ThinkingLevel::{self, High, Low, Max, Medium, Minimal, Off, XHigh};

    #[test]
    fn one_shot_returns_the_text_or_the_reason_it_failed() {
        use crate::agent::test_support::{text_reply, ScriptedProvider};
        let model = super::ModelSpec {
            provider: "scripted".into(),
            id: "m".into(),
            context_window: 8_000,
            max_tokens: None,
            thinking: Off,
        };
        let cancel = crate::CancelToken::new();
        let provider = ScriptedProvider::new(vec![text_reply("answer"), text_reply("  ")]);
        assert_eq!(
            super::one_shot(&provider, &model, "sys", "question", &cancel).as_deref(),
            Ok("answer")
        );
        let seen = provider.seen_requests();
        assert_eq!(seen[0].len(), 1);
        assert!(super::one_shot(&provider, &model, "sys", "q", &cancel).is_err());
        // The script is exhausted: the provider reports an error.
        assert!(super::one_shot(&provider, &model, "sys", "q", &cancel).is_err());
    }

    #[test]
    fn a_level_falls_to_the_nearest_one_offered() {
        let effort = [Low, Medium, High, XHigh, Max];
        assert_eq!(Medium.nearest(&effort), Medium);
        assert_eq!(Off.nearest(&effort), Low, "no off: the least");
        assert_eq!(Minimal.nearest(&effort), Low);
        let budget = [Off, Minimal, Low, Medium, High];
        assert_eq!(Max.nearest(&budget), High);
        assert_eq!(Off.nearest(&budget), Off);
        let switch = [Off, High];
        assert_eq!(Low.nearest(&switch), High, "any reasoning is on");
        assert_eq!(Off.nearest(&switch), Off);
        assert_eq!(High.nearest(&[]), Off);
    }

    #[test]
    fn levels_parse_from_their_labels() {
        for level in ThinkingLevel::ALL {
            assert_eq!(ThinkingLevel::parse(level.label()), Some(level));
            let json = serde_json::to_string(&level).unwrap();
            assert_eq!(json, format!("\"{}\"", level.label()));
        }
        assert_eq!(ThinkingLevel::parse(" XHigh "), Some(XHigh));
        assert_eq!(ThinkingLevel::parse("extreme"), None);
    }
}
