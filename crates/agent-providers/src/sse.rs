//! Accumulates OpenAI-style `chat.completion.chunk` deltas into one
//! [`AssistantMessage`], emitting live [`StreamEvent`]s on the way.
//!
//! Pure and transport-free so the exact wire transcripts of real servers can
//! be replayed in tests.

use serde_json::Value;
use termide_agent_core::{
    now_millis, AssistantContent, AssistantMessage, StopReason, StreamEvent, ToolCall, Usage,
};

#[derive(Debug, Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
    extra_content: Option<Value>,
    announced: bool,
}

/// Builds the assistant message from a chunk stream.
#[derive(Debug, Default)]
pub struct Accumulator {
    text: String,
    reasoning: String,
    calls: Vec<PartialCall>,
    finish_reason: Option<String>,
    usage: Usage,
    /// Whether any content delta has been delivered; a failed request can
    /// only be retried transparently before this flips.
    pub received_content: bool,
}

impl Accumulator {
    /// Strip the SSE framing from one line. `None` for comments, blank lines
    /// and other fields; `Some(None)` for the `[DONE]` sentinel.
    pub fn payload(line: &str) -> Option<Option<&str>> {
        let data = line.strip_prefix("data:")?.trim_start();
        if data == "[DONE]" {
            return Some(None);
        }
        (!data.is_empty()).then_some(Some(data))
    }

    /// Fold one parsed chunk into the message.
    pub fn feed(&mut self, chunk: &Value, on_event: &mut dyn FnMut(StreamEvent)) {
        if let Some(usage) = chunk.get("usage").filter(|u| !u.is_null()) {
            self.usage = parse_usage(usage);
        }
        // llama.cpp's prompt-processing progress (asked for with
        // `return_progress`) rides on chunks before the first token; it is not
        // content, so a failure after it can still be retried.
        if let Some(progress) = chunk.get("prompt_progress").filter(|p| p.is_object()) {
            let field = |key| progress.get(key).and_then(Value::as_u64).unwrap_or(0);
            let total = field("total");
            if total > 0 {
                on_event(StreamEvent::PrefillProgress {
                    processed: field("processed").min(total),
                    total,
                    cached: field("cache").min(total),
                });
            }
        }
        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return;
        };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_string());
        }
        let Some(delta) = choice.get("delta") else {
            return;
        };

        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            if !text.is_empty() {
                self.received_content = true;
                self.text.push_str(text);
                on_event(StreamEvent::TextDelta(text.to_string()));
            }
        }
        for key in ["reasoning_content", "reasoning"] {
            if let Some(text) = delta.get(key).and_then(Value::as_str) {
                if !text.is_empty() {
                    self.received_content = true;
                    self.reasoning.push_str(text);
                    on_event(StreamEvent::ThinkingDelta(text.to_string()));
                }
            }
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                self.feed_tool_call(call, on_event);
            }
        }
    }

    fn feed_tool_call(&mut self, call: &Value, on_event: &mut dyn FnMut(StreamEvent)) {
        self.received_content = true;
        let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        while self.calls.len() <= index {
            self.calls.push(PartialCall::default());
        }
        let partial = &mut self.calls[index];
        if let Some(id) = call.get("id").and_then(Value::as_str) {
            if !id.is_empty() {
                partial.id = id.to_string();
            }
        }
        // Gemini's thought signature, sent with the call once and wanted back.
        if let Some(extra) = call.get("extra_content").filter(|v| !v.is_null()) {
            partial.extra_content = Some(extra.clone());
        }
        let function = call.get("function");
        if let Some(name) = function.and_then(|f| f.get("name")).and_then(Value::as_str) {
            if !name.is_empty() {
                partial.name = name.to_string();
            }
        }
        if !partial.announced && !partial.name.is_empty() {
            if partial.id.is_empty() {
                partial.id = format!("call_{index}");
            }
            partial.announced = true;
            on_event(StreamEvent::ToolCallStart {
                id: partial.id.clone(),
                name: partial.name.clone(),
            });
        }
        if let Some(fragment) = function
            .and_then(|f| f.get("arguments"))
            .and_then(Value::as_str)
        {
            if !fragment.is_empty() {
                partial.arguments.push_str(fragment);
                if partial.announced {
                    on_event(StreamEvent::ToolCallDelta {
                        id: partial.id.clone(),
                        arguments: fragment.to_string(),
                    });
                }
            }
        }
    }

    /// Close the stream. `stop_override` forces the stop reason (abort).
    pub fn finish(
        self,
        provider: &str,
        model: &str,
        stop_override: Option<StopReason>,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> AssistantMessage {
        let mut content = Vec::new();
        if !self.reasoning.is_empty() {
            content.push(AssistantContent::thinking(self.reasoning));
        }
        if !self.text.is_empty() {
            content.push(AssistantContent::Text { text: self.text });
        }
        let mut has_calls = false;
        for (index, partial) in self.calls.into_iter().enumerate() {
            if partial.name.is_empty() {
                continue;
            }
            has_calls = true;
            let id = if partial.id.is_empty() {
                format!("call_{index}")
            } else {
                partial.id
            };
            on_event(StreamEvent::ToolCallEnd { id: id.clone() });
            content.push(AssistantContent::ToolCall(ToolCall {
                id,
                name: partial.name,
                arguments: parse_arguments(&partial.arguments),
                extra_content: partial.extra_content,
            }));
        }

        let stop_reason = stop_override.unwrap_or(match self.finish_reason.as_deref() {
            Some("length") => StopReason::Length,
            Some("tool_calls") | Some("function_call") => StopReason::ToolUse,
            _ if has_calls => StopReason::ToolUse,
            _ => StopReason::Stop,
        });

        AssistantMessage {
            content,
            stop_reason,
            usage: self.usage,
            provider: provider.to_string(),
            model: model.to_string(),
            error_message: None,
            failure: None,
            timestamp: now_millis(),
        }
    }
}

fn parse_arguments(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return Value::Object(Default::default());
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(value @ Value::Object(_)) => value,
        Ok(other) => {
            log::warn!("tool call arguments are not a JSON object: {other}");
            Value::String(raw.to_string())
        }
        Err(error) => {
            log::warn!("tool call arguments are not valid JSON ({error}): {raw}");
            Value::String(raw.to_string())
        }
    }
}

fn parse_usage(usage: &Value) -> Usage {
    let get = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let cache_read = usage
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Usage {
        input: get("prompt_tokens").saturating_sub(cache_read),
        output: get("completion_tokens"),
        cache_read,
        cache_write: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim transcript from the omlx server (Qwen3.8 27B) on 2026-09-17.
    const OMLX_TOOL_CALL: &str = r#"data: {"id":"chatcmpl-92372d6f","object":"chat.completion.chunk","created":0,"model":"keepalive","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}

data: {"id":"chatcmpl-92372d6f","object":"chat.completion.chunk","created":1789654796,"model":"Qwen3.8-27B-MTPLX-Optimized-Quality","choices":[{"index":0,"delta":{"role":"assistant"}}]}

data: {"id":"chatcmpl-92372d6f","object":"chat.completion.chunk","created":1789654797,"model":"Qwen3.8-27B-MTPLX-Optimized-Quality","choices":[{"index":0,"delta":{"reasoning_content":"\nThe user is asking to use the tool to read the"}}]}

data: {"id":"chatcmpl-92372d6f","object":"chat.completion.chunk","created":1789654797,"model":"Qwen3.8-27B-MTPLX-Optimized-Quality","choices":[{"index":0,"delta":{"reasoning_content":" file README.md.\n"}}]}

data: {"id":"chatcmpl-92372d6f","object":"chat.completion.chunk","created":1789654798,"model":"Qwen3.8-27B-MTPLX-Optimized-Quality","choices":[{"index":0,"delta":{"content":"\n\n"}}]}

data: {"id":"chatcmpl-92372d6f","object":"chat.completion.chunk","created":1789654798,"model":"Qwen3.8-27B-MTPLX-Optimized-Quality","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_d2bac9fb","type":"function","function":{"name":"read","arguments":"{\"path\": \"README.md\"}"}}]}}]}

data: {"id":"chatcmpl-92372d6f","object":"chat.completion.chunk","created":1789654798,"model":"Qwen3.8-27B-MTPLX-Optimized-Quality","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}

data: [DONE]
"#;

    fn replay(transcript: &str) -> (AssistantMessage, Vec<StreamEvent>) {
        let mut events = Vec::new();
        let mut acc = Accumulator::default();
        for line in transcript.lines() {
            match Accumulator::payload(line) {
                None => {}
                Some(None) => break,
                Some(Some(data)) => {
                    let chunk: Value = serde_json::from_str(data).unwrap();
                    acc.feed(&chunk, &mut |e| events.push(e));
                }
            }
        }
        let message = acc.finish("omlx", "qwen", None, &mut |e| events.push(e));
        (message, events)
    }

    #[test]
    fn omlx_tool_call_transcript_yields_thinking_text_and_call() {
        let (message, events) = replay(OMLX_TOOL_CALL);
        assert_eq!(message.stop_reason, StopReason::ToolUse);
        assert_eq!(message.content.len(), 3);
        assert!(
            matches!(&message.content[0], AssistantContent::Thinking { text, .. } if text.contains("README.md"))
        );
        assert!(matches!(&message.content[1], AssistantContent::Text { text } if text == "\n\n"));
        let call = message.tool_calls().next().unwrap();
        assert_eq!(call.id, "call_d2bac9fb");
        assert_eq!(call.name, "read");
        assert_eq!(call.arguments["path"], "README.md");
        assert!(events.contains(&StreamEvent::ToolCallStart {
            id: "call_d2bac9fb".into(),
            name: "read".into()
        }));
        assert!(events.contains(&StreamEvent::ToolCallEnd {
            id: "call_d2bac9fb".into()
        }));
        // The keepalive chunk with empty content produced no text event.
        assert!(!events.contains(&StreamEvent::TextDelta(String::new())));
    }

    #[test]
    fn llama_cpp_prompt_progress_is_reported_and_is_not_content() {
        // llama.cpp with `return_progress`: progress chunks with an empty
        // delta before the first token.
        let transcript = r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":null}],"prompt_progress":{"total":48000,"cache":30000,"processed":39000,"time_ms":5200.5}}
data: {"choices":[{"index":0,"delta":{},"finish_reason":null}],"prompt_progress":{"total":48000,"cache":30000,"processed":48000,"time_ms":9800.1}}
data: {"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":"stop"}]}
data: [DONE]
"#;
        let (_, events) = replay(transcript);
        let progress: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::PrefillProgress { .. }))
            .collect();
        assert_eq!(
            progress,
            vec![
                &StreamEvent::PrefillProgress {
                    processed: 39_000,
                    total: 48_000,
                    cached: 30_000
                },
                &StreamEvent::PrefillProgress {
                    processed: 48_000,
                    total: 48_000,
                    cached: 30_000
                },
            ]
        );

        let mut acc = Accumulator::default();
        let chunk: Value = serde_json::from_str(
            r#"{"choices":[],"prompt_progress":{"total":10,"cache":0,"processed":5}}"#,
        )
        .unwrap();
        acc.feed(&chunk, &mut |_| {});
        assert!(!acc.received_content);
    }

    #[test]
    fn fragmented_arguments_and_usage_are_assembled() {
        let transcript = r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"edit","arguments":""}}]}}]}
data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]}}]}
data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":" \"a.rs\"}"}}]}}]}
data: {"choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]}
data: {"choices":[],"usage":{"prompt_tokens":120,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":100}}}
data: [DONE]
"#;
        let (message, events) = replay(transcript);
        let call = message.tool_calls().next().unwrap();
        assert_eq!(call.arguments["path"], "a.rs");
        // "stop" with tool calls present still counts as tool use.
        assert_eq!(message.stop_reason, StopReason::ToolUse);
        assert_eq!(
            message.usage,
            Usage {
                input: 20,
                output: 7,
                cache_read: 100,
                cache_write: 0
            }
        );
        let deltas = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolCallDelta { .. }))
            .count();
        assert_eq!(deltas, 2);
    }

    #[test]
    fn length_stop_and_broken_arguments_are_preserved() {
        let transcript = r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"bash","arguments":"{\"command\": \"ls"}}]},"finish_reason":"length"}]}
data: [DONE]
"#;
        let (message, _) = replay(transcript);
        assert_eq!(message.stop_reason, StopReason::Length);
        let call = message.tool_calls().next().unwrap();
        assert_eq!(call.arguments, Value::String("{\"command\": \"ls".into()));
    }

    #[test]
    fn plain_text_answer_and_missing_ids() {
        let transcript = r#"data: {"choices":[{"index":0,"delta":{"content":"Hello"}}]}
data: {"choices":[{"index":0,"delta":{"content":" world"},"finish_reason":"stop"}]}
data: [DONE]
"#;
        let (message, _) = replay(transcript);
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(message.plain_text(), "Hello world");

        let no_id = r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"function":{"name":"read","arguments":"{}"}}]}}]}
"#;
        let (message, events) = replay(no_id);
        let call = message.tool_calls().next().unwrap();
        assert_eq!(call.id, "call_1");
        assert!(events
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolCallStart { id, .. } if id == "call_1")));
    }

    #[test]
    fn payload_framing() {
        assert_eq!(Accumulator::payload(": ping"), None);
        assert_eq!(Accumulator::payload(""), None);
        assert_eq!(Accumulator::payload("event: x"), None);
        assert_eq!(Accumulator::payload("data: [DONE]"), Some(None));
        assert_eq!(
            Accumulator::payload("data:{\"a\":1}"),
            Some(Some("{\"a\":1}"))
        );
    }
}
