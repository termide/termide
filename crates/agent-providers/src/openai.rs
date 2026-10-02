//! OpenAI-compatible streaming chat completions over blocking HTTP.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::{json, Map, Value};
use termide_agent_core::{
    AssistantContent, AssistantMessage, CancelToken, Message, ModelInfo, Provider, Request,
    StopReason, StreamEvent, ThinkingLevel,
};

use crate::retry::{with_retries, Failure, RetryPolicy};
use crate::sse::Accumulator;

/// Vendor quirks, expressed as data.
#[derive(Debug, Clone, PartialEq)]
pub struct Compat {
    /// Name of the output-limit field: `max_tokens` (most servers) or
    /// `max_completion_tokens` (newer OpenAI models).
    pub max_tokens_field: String,
    /// How the server is asked for reasoning.
    pub reasoning: ReasoningParam,
    /// Echo previous `reasoning_content` back in assistant messages. Off by
    /// default: DeepSeek rejects it, most servers ignore it.
    pub send_reasoning: bool,
    /// Ask for prompt-processing progress while streaming (llama.cpp's
    /// `return_progress`). Off by default: OpenAI rejects unknown fields.
    pub prefill_progress: bool,
    /// Merged into every request body last; the escape hatch for anything
    /// not modelled here (`chat_template_kwargs`, `temperature`, ...).
    pub extra_body: Map<String, Value>,
}

impl Default for Compat {
    fn default() -> Self {
        Self {
            max_tokens_field: "max_tokens".into(),
            reasoning: ReasoningParam::None,
            send_reasoning: false,
            prefill_progress: false,
            extra_body: Map::new(),
        }
    }
}

/// The request field an OpenAI-compatible server takes a reasoning level in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReasoningParam {
    /// Nothing: the model reasons as the server has it.
    #[default]
    None,
    /// `reasoning_effort` (OpenAI, OpenRouter, Gemini, gpt-oss servers).
    Effort,
    /// `chat_template_kwargs.enable_thinking`, the on/off switch of the
    /// chat templates of Qwen3, GLM and DeepSeek on vLLM or llama.cpp.
    EnableThinking,
}

pub struct OpenAiCompatProvider {
    name: String,
    /// Base URL including the API prefix, e.g. `http://127.0.0.1:10000/v1`.
    base_url: String,
    api_key: Option<String>,
    pub compat: Compat,
    pub retry: RetryPolicy,
    /// Idle limit between two SSE reads; a stalled server fails the request.
    pub read_timeout: Duration,
    pub connect_timeout: Duration,
    agent: ureq::Agent,
}

impl OpenAiCompatProvider {
    #[must_use]
    pub fn new(name: impl Into<String>, base_url: impl Into<String>) -> Self {
        let read_timeout = Duration::from_secs(120);
        let connect_timeout = Duration::from_secs(15);
        Self {
            name: name.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: None,
            compat: Compat::default(),
            retry: RetryPolicy::default(),
            read_timeout,
            connect_timeout,
            agent: build_agent(connect_timeout, read_timeout),
        }
    }

    #[must_use]
    pub fn with_api_key(mut self, key: Option<String>) -> Self {
        self.api_key = key.filter(|k| !k.is_empty());
        self
    }

    #[must_use]
    pub fn with_compat(mut self, compat: Compat) -> Self {
        self.compat = compat;
        self
    }

    #[must_use]
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    #[must_use]
    pub fn with_timeouts(mut self, connect: Duration, read: Duration) -> Self {
        self.connect_timeout = connect;
        self.read_timeout = read;
        self.agent = build_agent(connect, read);
        self
    }

    /// The JSON body for `request`, exposed for tests and debugging.
    #[must_use]
    pub fn build_body(&self, request: &Request<'_>) -> Value {
        let mut messages = Vec::new();
        if !request.system_prompt.is_empty() {
            messages.push(json!({ "role": "system", "content": request.system_prompt }));
        }
        for message in request.messages {
            messages.push(self.convert(message));
        }

        let mut body = Map::new();
        body.insert("model".into(), json!(request.model.id));
        body.insert("messages".into(), Value::Array(messages));
        body.insert("stream".into(), json!(true));
        body.insert("stream_options".into(), json!({ "include_usage": true }));
        if self.compat.prefill_progress {
            body.insert("return_progress".into(), json!(true));
        }
        // No limit configured: leave the reply's length to the model.
        if let Some(max_tokens) = request.model.max_tokens {
            body.insert(self.compat.max_tokens_field.clone(), json!(max_tokens));
        }
        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters,
                        }
                    })
                })
                .collect();
            body.insert("tools".into(), Value::Array(tools));
        }
        let levels = self.thinking_levels(&request.model.id);
        if !levels.is_empty() {
            let level = request.thinking.nearest(&levels);
            match self.compat.reasoning {
                ReasoningParam::None => {}
                ReasoningParam::Effort => {
                    // `Off` is offered only where the API spells it `none`.
                    let effort = match level {
                        ThinkingLevel::Off => "none",
                        level => level.label(),
                    };
                    body.insert("reasoning_effort".into(), json!(effort));
                }
                ReasoningParam::EnableThinking => {
                    body.insert(
                        "chat_template_kwargs".into(),
                        json!({ "enable_thinking": level != ThinkingLevel::Off }),
                    );
                }
            }
        }
        for (key, value) in &self.compat.extra_body {
            body.insert(key.clone(), value.clone());
        }
        Value::Object(body)
    }

    fn convert(&self, message: &Message) -> Value {
        match message {
            Message::User(user) => json!({ "role": "user", "content": user.model_text() }),
            Message::ToolResult(result) => json!({
                "role": "tool",
                "tool_call_id": result.tool_call_id,
                "content": result.plain_text(),
            }),
            Message::Assistant(assistant) => {
                let mut object = Map::new();
                object.insert("role".into(), json!("assistant"));
                let text = assistant.plain_text();
                object.insert(
                    "content".into(),
                    if text.is_empty() {
                        Value::Null
                    } else {
                        json!(text)
                    },
                );
                let calls: Vec<Value> = assistant
                    .tool_calls()
                    .map(|call| {
                        let arguments = match &call.arguments {
                            Value::String(raw) => raw.clone(),
                            other => other.to_string(),
                        };
                        let mut value = json!({
                            "id": call.id,
                            "type": "function",
                            "function": { "name": call.name, "arguments": arguments }
                        });
                        if let Some(extra) = &call.extra_content {
                            value["extra_content"] = extra.clone();
                        }
                        value
                    })
                    .collect();
                if !calls.is_empty() {
                    object.insert("tool_calls".into(), Value::Array(calls));
                }
                if self.compat.send_reasoning {
                    let reasoning: String = assistant
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            AssistantContent::Thinking { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    if !reasoning.is_empty() {
                        object.insert("reasoning_content".into(), json!(reasoning));
                    }
                }
                Value::Object(object)
            }
        }
    }

    fn attempt(
        &self,
        body: &str,
        model: &str,
        on_event: &mut dyn FnMut(StreamEvent),
        cancel: &CancelToken,
    ) -> Result<AssistantMessage, Failure> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut http = self
            .agent
            .post(&url)
            .set("Content-Type", "application/json")
            .set("Accept", "text/event-stream");
        if let Some(key) = &self.api_key {
            http = http.set("Authorization", &format!("Bearer {key}"));
        }

        let response = match http.send_string(body) {
            Ok(response) => response,
            Err(ureq::Error::Status(code, response)) => {
                let text = response.into_string().unwrap_or_default();
                return Err(Failure {
                    message: format!("HTTP {code}: {}", error_text(&text)),
                    retryable: matches!(code, 408 | 409 | 425 | 429 | 500..=599),
                });
            }
            Err(ureq::Error::Transport(transport)) => {
                return Err(Failure {
                    message: format!("transport error: {transport}"),
                    retryable: true,
                });
            }
        };

        let mut acc = Accumulator::default();
        let mut reader = BufReader::new(response.into_reader());
        let mut line = String::new();
        loop {
            if cancel.is_cancelled() {
                return Ok(acc.finish(&self.name, model, Some(StopReason::Aborted), on_event));
            }
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                // Data that arrives after an abort is dropped, not appended.
                Ok(_) if cancel.is_cancelled() => {
                    return Ok(acc.finish(&self.name, model, Some(StopReason::Aborted), on_event));
                }
                Ok(_) => {}
                Err(error) => {
                    if acc.received_content {
                        return Ok(failed_after_content(
                            acc,
                            &self.name,
                            model,
                            format!("stream interrupted: {error}"),
                            on_event,
                        ));
                    }
                    return Err(Failure {
                        message: format!("stream error: {error}"),
                        retryable: true,
                    });
                }
            }
            let Some(payload) = Accumulator::payload(line.trim_end()) else {
                continue;
            };
            let Some(data) = payload else {
                break;
            };
            match serde_json::from_str::<Value>(data) {
                Ok(chunk) => {
                    if let Some(error) = chunk.get("error") {
                        let message = error_text(&error.to_string());
                        if acc.received_content {
                            return Ok(failed_after_content(
                                acc, &self.name, model, message, on_event,
                            ));
                        }
                        return Err(Failure {
                            message,
                            retryable: false,
                        });
                    }
                    acc.feed(&chunk, on_event);
                }
                Err(error) => log::warn!("skipping malformed SSE chunk ({error}): {data}"),
            }
        }
        Ok(acc.finish(&self.name, model, None, on_event))
    }
}

impl Provider for OpenAiCompatProvider {
    fn name(&self) -> &str {
        &self.name
    }

    /// `GET /models`, which every OpenAI-compatible server answers with the
    /// ids it serves. One attempt, no retries: a picker is interactive.
    /// The host (and port) of the base URL: `127.0.0.1:10000`,
    /// `openrouter.ai`.
    fn endpoint(&self) -> Option<String> {
        let rest = self
            .base_url
            .split_once("://")
            .map_or(self.base_url.as_str(), |(_, rest)| rest);
        let host = rest.split('/').next().unwrap_or("");
        (!host.is_empty()).then(|| host.to_string())
    }

    fn thinking_levels(&self, model: &str) -> Vec<ThinkingLevel> {
        match self.compat.reasoning {
            ReasoningParam::None => Vec::new(),
            ReasoningParam::Effort => effort_levels(model),
            ReasoningParam::EnableThinking => vec![ThinkingLevel::Off, ThinkingLevel::High],
        }
    }

    fn list_models(&self) -> Result<Vec<ModelInfo>, String> {
        let url = format!("{}/models", self.base_url);
        let mut http = self.agent.get(&url).set("Accept", "application/json");
        if let Some(key) = &self.api_key {
            http = http.set("Authorization", &format!("Bearer {key}"));
        }
        let body = match http.call() {
            Ok(response) => response
                .into_string()
                .map_err(|error| format!("cannot read the model list: {error}"))?,
            Err(ureq::Error::Status(code, response)) => {
                let text = response.into_string().unwrap_or_default();
                return Err(format!("HTTP {code}: {}", error_text(&text)));
            }
            Err(ureq::Error::Transport(transport)) => {
                return Err(format!("transport error: {transport}"));
            }
        };
        parse_model_list(&body)
    }

    fn stream(
        &self,
        request: &Request<'_>,
        on_event: &mut dyn FnMut(StreamEvent),
        cancel: &CancelToken,
    ) -> AssistantMessage {
        let body = self.build_body(request).to_string();
        let model = request.model.id.as_str();
        with_retries(
            &self.name,
            model,
            self.retry,
            cancel,
            on_event,
            |on_event| self.attempt(&body, model, on_event, cancel),
        )
    }
}

fn failed_after_content(
    acc: Accumulator,
    provider: &str,
    model: &str,
    message: String,
    on_event: &mut dyn FnMut(StreamEvent),
) -> AssistantMessage {
    let mut partial = acc.finish(provider, model, Some(StopReason::Error), on_event);
    partial.error_message = Some(message);
    partial
}

fn build_agent(connect: Duration, read: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(connect)
        .timeout_read(read)
        .user_agent(concat!("termide-agent/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// The `reasoning_effort` values `model` takes, as levels: OpenAI's own
/// models by family (a gateway's `openai/` prefix aside), the three common
/// ones for anything else.
fn effort_levels(model: &str) -> Vec<ThinkingLevel> {
    use ThinkingLevel::{High, Low, Medium, Minimal, Off, XHigh};
    let id = model.to_ascii_lowercase();
    let id = id.rsplit('/').next().unwrap_or(&id);
    let codex = id.contains("codex");
    if let Some(rest) = id.strip_prefix("gpt-5") {
        // `gpt-5`, `gpt-5-mini`: the first generation; `gpt-5.N` after it.
        let minor = rest
            .strip_prefix('.')
            .map(|rest| {
                rest.split(|c: char| !c.is_ascii_digit())
                    .next()
                    .unwrap_or("")
            })
            .and_then(|digits| digits.parse::<u32>().ok());
        return match minor {
            None if codex => vec![Low, Medium, High],
            None => vec![Minimal, Low, Medium, High],
            Some(1) if codex => vec![Low, Medium, High],
            Some(1) => vec![Off, Low, Medium, High],
            Some(_) if codex => vec![Low, Medium, High, XHigh],
            Some(_) if id.contains("pro") => vec![Medium, High, XHigh],
            Some(_) => vec![Off, Low, Medium, High, XHigh],
        };
    }
    if id.starts_with("gpt-6") {
        return if id.contains("pro") {
            vec![Medium, High, XHigh]
        } else {
            vec![Off, Low, Medium, High, XHigh]
        };
    }
    vec![Low, Medium, High]
}

/// Models from a `GET /models` body: OpenAI's `{"data": [{"id": …}]}`, or a
/// bare array of objects or strings as some servers return. Sorted by id,
/// duplicates dropped. `max_model_len` (vLLM, omlx) becomes the context
/// window when present.
fn parse_model_list(body: &str) -> Result<Vec<ModelInfo>, String> {
    let value: Value =
        serde_json::from_str(body).map_err(|error| format!("model list is not JSON: {error}"))?;
    let items = value
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .ok_or_else(|| "model list has no `data` array".to_string())?;
    let mut models: Vec<ModelInfo> = items
        .iter()
        .filter_map(|item| {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| item.as_str())?;
            Some(ModelInfo {
                id: id.to_string(),
                context_window: item
                    .get("max_model_len")
                    .and_then(Value::as_u64)
                    .filter(|len| *len > 0),
            })
        })
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);
    Ok(models)
}

/// Pull the human-readable message out of an error body when it is the usual
/// `{"error": {"message": ...}}` envelope; otherwise return the body trimmed.
fn error_text(body: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|v| v.get("error").unwrap_or(v).get("message"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let text = message.unwrap_or_else(|| body.trim().to_string());
    if text.is_empty() {
        "no error body".to_string()
    } else {
        text.chars().take(500).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    use termide_agent_core::{
        AssistantContent, ModelSpec, ToolCall, ToolResultMessage, ToolSpec, Usage, UserMessage,
    };

    use super::*;

    fn model() -> ModelSpec {
        ModelSpec {
            provider: "test".into(),
            id: "qwen".into(),
            context_window: 32_000,
            max_tokens: Some(512),
            thinking: ThinkingLevel::Off,
        }
    }

    /// One-shot HTTP responder: each entry answers one request in order and
    /// records the request bodies.
    fn serve(responses: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let seen = bodies.clone();
        std::thread::spawn(move || {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let body = read_request(&mut stream);
                seen.lock().unwrap().push(body);
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        (url, bodies)
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut raw = Vec::new();
        let mut buffer = [0u8; 4096];
        let (header_end, content_length) = loop {
            let n = stream.read(&mut buffer).unwrap();
            raw.extend_from_slice(&buffer[..n]);
            let text = String::from_utf8_lossy(&raw).into_owned();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text
                    .lines()
                    .find_map(|l| l.strip_prefix("Content-Length: "))
                    .map(|v| v.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                break (end + 4, length);
            }
        };
        while raw.len() < header_end + content_length {
            let n = stream.read(&mut buffer).unwrap();
            raw.extend_from_slice(&buffer[..n]);
        }
        String::from_utf8_lossy(&raw[header_end..header_end + content_length]).into_owned()
    }

    fn sse(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}"
        )
    }

    fn status(code: u16, body: &str) -> String {
        format!(
            "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn provider(url: &str) -> OpenAiCompatProvider {
        OpenAiCompatProvider::new("test", url)
            .with_api_key(Some("secret".into()))
            .with_retry(RetryPolicy {
                max_attempts: 3,
                base_delay: Duration::from_millis(10),
            })
    }

    #[test]
    fn an_unlimited_turn_sends_no_output_bound() {
        let provider = OpenAiCompatProvider::new("p", "http://x/v1/");
        let model = ModelSpec {
            max_tokens: None,
            ..model()
        };
        let messages = vec![Message::User(UserMessage::text("hi"))];
        let request = Request {
            model: &model,
            system_prompt: "",
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::Off,
        };
        let body = provider.build_body(&request);
        assert!(body.get("max_tokens").is_none(), "{body}");
    }

    #[test]
    fn body_has_openai_shape_and_honours_compat() {
        let mut compat = Compat {
            reasoning: ReasoningParam::Effort,
            send_reasoning: true,
            prefill_progress: true,
            ..Compat::default()
        };
        compat.extra_body.insert("temperature".into(), json!(0.2));
        let provider = OpenAiCompatProvider::new("p", "http://x/v1/").with_compat(compat);
        let call = ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: json!({ "path": "a" }),
            extra_content: None,
        };
        let messages = vec![
            Message::User(UserMessage::text("hi")),
            Message::Assistant(AssistantMessage {
                content: vec![
                    AssistantContent::thinking("hmm"),
                    AssistantContent::ToolCall(call.clone()),
                ],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
                provider: "p".into(),
                model: "m".into(),
                error_message: None,
                timestamp: 0,
            }),
            Message::ToolResult(ToolResultMessage::text(&call, "contents")),
        ];
        let tools = vec![ToolSpec {
            name: "read".into(),
            description: "Read".into(),
            parameters: json!({ "type": "object" }),
        }];
        let request = Request {
            model: &model(),
            system_prompt: "You are terse.",
            messages: &messages,
            tools: &tools,
            thinking: ThinkingLevel::High,
        };
        let body = provider.build_body(&request);

        assert_eq!(body["model"], "qwen");
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_tokens"], 512);
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["temperature"], 0.2);
        assert_eq!(body["return_progress"], true);
        assert_eq!(body["tools"][0]["function"]["name"], "read");
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1], json!({ "role": "user", "content": "hi" }));
        assert_eq!(messages[2]["role"], "assistant");
        assert!(messages[2]["content"].is_null());
        assert_eq!(messages[2]["reasoning_content"], "hmm");
        assert_eq!(
            messages[2]["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"a\"}"
        );
        assert_eq!(
            messages[3],
            json!({ "role": "tool", "tool_call_id": "c1", "content": "contents" })
        );

        let plain = OpenAiCompatProvider::new("p", "http://x/v1").build_body(&request);
        assert!(plain.get("return_progress").is_none());
        assert!(plain.get("reasoning_effort").is_none());
        assert!(plain["messages"][2].get("reasoning_content").is_none());
    }

    #[test]
    fn openai_models_offer_their_own_effort_values() {
        use ThinkingLevel::{High, Low, Medium, Minimal, Off, XHigh};
        let provider = OpenAiCompatProvider::new("p", "http://x/v1").with_compat(Compat {
            reasoning: ReasoningParam::Effort,
            ..Compat::default()
        });
        let levels = |id: &str| provider.thinking_levels(id);
        assert_eq!(levels("gpt-5-mini"), vec![Minimal, Low, Medium, High]);
        assert_eq!(levels("gpt-5.1"), vec![Off, Low, Medium, High]);
        assert_eq!(levels("gpt-5.1-codex"), vec![Low, Medium, High]);
        assert_eq!(
            levels("openai/gpt-5.4"),
            vec![Off, Low, Medium, High, XHigh]
        );
        assert_eq!(levels("gpt-5.5-pro"), vec![Medium, High, XHigh]);
        assert_eq!(levels("gpt-5.2-codex"), vec![Low, Medium, High, XHigh]);
        assert_eq!(levels("o4-mini"), vec![Low, Medium, High]);
        assert_eq!(levels("gpt-oss-120b"), vec![Low, Medium, High]);

        let messages = vec![Message::User(UserMessage::text("hi"))];
        let body = |id: &str, thinking| {
            provider.build_body(&Request {
                model: &ModelSpec {
                    id: id.into(),
                    ..model()
                },
                system_prompt: "",
                messages: &messages,
                tools: &[],
                thinking,
            })
        };
        assert_eq!(
            body("gpt-5.4", ThinkingLevel::Off)["reasoning_effort"],
            "none"
        );
        // Where off is not a value, the least is sent; max falls to xhigh.
        assert_eq!(body("o3", ThinkingLevel::Off)["reasoning_effort"], "low");
        assert_eq!(
            body("gpt-5.4", ThinkingLevel::Max)["reasoning_effort"],
            "xhigh"
        );
    }

    #[test]
    fn a_chat_template_switch_turns_reasoning_on_or_off() {
        let provider = OpenAiCompatProvider::new("p", "http://x/v1").with_compat(Compat {
            reasoning: ReasoningParam::EnableThinking,
            ..Compat::default()
        });
        assert_eq!(
            provider.thinking_levels("qwen"),
            vec![ThinkingLevel::Off, ThinkingLevel::High]
        );
        let messages = vec![Message::User(UserMessage::text("hi"))];
        let body = |thinking| {
            provider.build_body(&Request {
                model: &model(),
                system_prompt: "",
                messages: &messages,
                tools: &[],
                thinking,
            })
        };
        assert_eq!(
            body(ThinkingLevel::Medium)["chat_template_kwargs"],
            json!({ "enable_thinking": true })
        );
        assert_eq!(
            body(ThinkingLevel::Off)["chat_template_kwargs"],
            json!({ "enable_thinking": false })
        );
        let silent = OpenAiCompatProvider::new("p", "http://x/v1");
        assert!(silent.thinking_levels("qwen").is_empty());
    }

    #[test]
    fn a_calls_extra_content_goes_back_with_it() {
        // Gemini's OpenAI-compatible endpoint signs a call in `extra_content`
        // and refuses the next request unless the call carries it again.
        let transcript = r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"extra_content":{"google":{"thought_signature":"sig"}},"function":{"arguments":"{\"path\":\"x\"}","name":"read"},"id":"function-call-1","type":"function"}]},"finish_reason":"tool_calls"}]}

data: [DONE]
"#;
        let (url, _) = serve(vec![sse(transcript)]);
        let provider = provider(&url);
        let mut messages = vec![Message::User(UserMessage::text("read x"))];
        let request = Request {
            model: &model(),
            system_prompt: "",
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::Off,
        };
        let message = provider.stream(&request, &mut |_| {}, &CancelToken::new());
        let extra = json!({ "google": { "thought_signature": "sig" } });
        let call = message.tool_calls().next().unwrap().clone();
        assert_eq!(call.extra_content.as_ref(), Some(&extra), "{message:?}");

        messages.push(Message::Assistant(message));
        messages.push(Message::ToolResult(ToolResultMessage::text(
            &call, "contents",
        )));
        let body = provider.build_body(&Request {
            model: &model(),
            system_prompt: "",
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::Off,
        });
        let sent = &body["messages"][1]["tool_calls"][0];
        assert_eq!(sent["extra_content"], extra);
        assert_eq!(sent["id"], "function-call-1");
    }

    #[test]
    fn streams_a_tool_call_over_http_with_auth_header() {
        let transcript = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Sure\"}}]}\n\n\
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c9\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"x\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n\
data: [DONE]\n";
        let (url, bodies) = serve(vec![sse(transcript)]);
        let provider = provider(&url);
        let messages = vec![Message::User(UserMessage::text("read x"))];
        let request = Request {
            model: &model(),
            system_prompt: "",
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::Off,
        };
        let mut events = Vec::new();
        let message = provider.stream(&request, &mut |e| events.push(e), &CancelToken::new());

        assert_eq!(message.stop_reason, StopReason::ToolUse, "{message:?}");
        assert_eq!(message.plain_text(), "Sure");
        assert_eq!(message.tool_calls().next().unwrap().arguments["path"], "x");
        assert_eq!(message.provider, "test");
        assert!(events.contains(&StreamEvent::TextDelta("Sure".into())));
        let body: Value = serde_json::from_str(&bodies.lock().unwrap()[0]).unwrap();
        assert_eq!(body["messages"][0]["content"], "read x");
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn transient_status_is_retried_then_succeeds() {
        let ok = sse("data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"back\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n");
        let (url, bodies) = serve(vec![
            status(503, "{\"error\":{\"message\":\"overloaded\"}}"),
            ok,
        ]);
        let provider = provider(&url);
        let messages = vec![Message::User(UserMessage::text("hi"))];
        let request = Request {
            model: &model(),
            system_prompt: "",
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::Off,
        };
        let mut events = Vec::new();
        let message = provider.stream(&request, &mut |e| events.push(e), &CancelToken::new());

        assert_eq!(message.stop_reason, StopReason::Stop, "{message:?}");
        assert_eq!(message.plain_text(), "back");
        assert_eq!(bodies.lock().unwrap().len(), 2);
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::Retry { attempt: 1, max_attempts: 3, error, .. } if error.contains("overloaded")
        )));
    }

    #[test]
    fn client_errors_are_not_retried_and_surface_the_message() {
        let (url, bodies) = serve(vec![status(
            400,
            "{\"error\":{\"message\":\"Model 'x' failed to load\"}}",
        )]);
        let provider = provider(&url);
        let messages = vec![Message::User(UserMessage::text("hi"))];
        let request = Request {
            model: &model(),
            system_prompt: "",
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::Off,
        };
        let message = provider.stream(&request, &mut |_| {}, &CancelToken::new());

        assert_eq!(message.stop_reason, StopReason::Error);
        assert!(message
            .error_message
            .as_deref()
            .unwrap()
            .contains("HTTP 400"));
        assert!(message
            .error_message
            .as_deref()
            .unwrap()
            .contains("failed to load"));
        assert_eq!(bodies.lock().unwrap().len(), 1);
    }

    #[test]
    fn connection_refused_exhausts_retries() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        drop(listener);
        let provider = provider(&url);
        let messages = vec![Message::User(UserMessage::text("hi"))];
        let request = Request {
            model: &model(),
            system_prompt: "",
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::Off,
        };
        let mut retries = 0;
        let message = provider.stream(
            &request,
            &mut |e| {
                if matches!(e, StreamEvent::Retry { .. }) {
                    retries += 1;
                }
            },
            &CancelToken::new(),
        );
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(retries, 2);
        assert!(message.error_message.unwrap().contains("transport error"));
    }

    #[test]
    fn cancellation_mid_stream_returns_partial_content_as_aborted() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let cancel = CancelToken::new();
        let canceller = cancel.clone();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request(&mut stream);
            let head =
                sse("data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"}}]}\n\n");
            stream.write_all(head.as_bytes()).unwrap();
            stream.flush().unwrap();
            std::thread::sleep(Duration::from_millis(100));
            canceller.cancel();
            // Keep the connection open long enough for the reader to notice.
            std::thread::sleep(Duration::from_millis(500));
            stream
                .write_all(
                    b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" more\"}}]}\n\n",
                )
                .unwrap();
        });
        let provider = provider(&url);
        let messages = vec![Message::User(UserMessage::text("hi"))];
        let request = Request {
            model: &model(),
            system_prompt: "",
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::Off,
        };
        let message = provider.stream(&request, &mut |_| {}, &cancel);
        assert_eq!(message.stop_reason, StopReason::Aborted);
        assert_eq!(message.plain_text(), "partial");
    }
    #[test]
    fn model_list_is_fetched_from_get_models() {
        let (url, _bodies) = serve(vec![status(
            200,
            r#"{"object":"list","data":[{"id":"qwen","object":"model","max_model_len":32000},{"id":"llama"},{"id":"qwen"}]}"#,
        )]);
        let models = provider(&url).list_models().unwrap();
        assert_eq!(
            models,
            vec![
                ModelInfo {
                    id: "llama".into(),
                    context_window: None
                },
                ModelInfo {
                    id: "qwen".into(),
                    context_window: Some(32_000)
                },
            ]
        );

        let (url, _bodies) = serve(vec![status(
            404,
            r#"{"error":{"message":"no such route"}}"#,
        )]);
        let error = provider(&url).list_models().unwrap_err();
        assert_eq!(error, "HTTP 404: no such route");
    }

    #[test]
    fn model_list_accepts_bare_arrays_and_rejects_other_shapes() {
        let ids: Vec<String> = parse_model_list(r#"[{"id":"b"},"a"]"#)
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
        assert!(parse_model_list(r#"{"models": []}"#)
            .unwrap_err()
            .contains("no `data` array"));
        assert!(parse_model_list("<html>").unwrap_err().contains("not JSON"));
    }
    #[test]
    fn the_endpoint_is_the_host_of_the_base_url() {
        assert_eq!(
            OpenAiCompatProvider::new("p", "http://127.0.0.1:10000/v1").endpoint(),
            Some("127.0.0.1:10000".to_string())
        );
        assert_eq!(
            OpenAiCompatProvider::new("p", "https://openrouter.ai/api/v1/").endpoint(),
            Some("openrouter.ai".to_string())
        );
    }
}
