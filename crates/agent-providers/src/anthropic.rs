//! Anthropic Messages API over blocking HTTP.
//!
//! A native provider for an Anthropic subscription, beside the
//! OpenAI-compatible one that already covers omlx, OpenAI and OpenRouter. The
//! wire shape is different enough — a top-level `system`, content-block
//! messages, tool results carried inside a user turn, a distinct SSE event
//! stream — to warrant its own provider rather than another `Compat` dialect.

use std::collections::HashMap;
use std::sync::PoisonError;
use std::time::Duration;

use serde_json::{json, Map, Value};
use termide_agent_core::{
    now_millis, AssistantContent, AssistantMessage, Attribution, CancelToken, Message, ModelInfo,
    Provider, Request, StopReason, StreamEvent, ThinkingLevel, ToolCall, Usage,
};

use crate::attribution;
use crate::openai::{learn_refusals, refusals_of, Refusals};
use crate::retry::{with_retries, Failure, RetryPolicy};
use crate::sse::UNFINISHED_STREAM;

/// The Messages API version pinned in the request header.
const API_VERSION: &str = "2023-06-01";
/// The public API root; overridable for a gateway.
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

pub struct AnthropicProvider {
    name: String,
    base_url: String,
    api_key: Option<String>,
    /// How to identify termide to this endpoint, when it files usage by app
    /// and the user asked for it; `None` sends no such header.
    attribution: Option<Attribution>,
    /// The optional fields the endpoint refused (a gateway that takes no
    /// `cache_control`), shared by every provider talking to it.
    refused: Refusals,
    pub retry: RetryPolicy,
    pub read_timeout: Duration,
    pub connect_timeout: Duration,
    agent: ureq::Agent,
}

impl AnthropicProvider {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_base_url(name, DEFAULT_BASE_URL)
    }

    #[must_use]
    pub fn with_base_url(name: impl Into<String>, base_url: impl Into<String>) -> Self {
        let connect_timeout = Duration::from_secs(15);
        let read_timeout = Duration::from_secs(120);
        let base = base_url.into();
        let base = base.trim_end_matches('/');
        let base = base
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();
        let base_url = if base.is_empty() {
            DEFAULT_BASE_URL.to_string()
        } else {
            base
        };
        Self {
            name: name.into(),
            refused: refusals_of(&base_url),
            base_url,
            api_key: None,
            attribution: None,
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

    /// Identify termide to the endpoint with `attribution`'s headers; `None`
    /// leaves the request unattributed, as Anthropic's own API sees it.
    #[must_use]
    pub fn with_attribution(mut self, attribution: Option<Attribution>) -> Self {
        self.attribution = attribution;
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
        self.body(request, true)
    }

    /// The body, replaying the current turn's signed reasoning when
    /// `replay_thinking` (the retry after a rejected signature goes without).
    fn body(&self, request: &Request<'_>, replay_thinking: bool) -> Value {
        let mut body = Map::new();
        body.insert("model".into(), json!(request.model.id));
        // The Messages API requires a bound, so an unlimited turn sends a
        // generous one of its own.
        let max_tokens = request.model.max_tokens.unwrap_or(UNLIMITED_MAX_TOKENS);
        body.insert("max_tokens".into(), json!(max_tokens));
        body.insert("stream".into(), json!(true));
        // Automatic prompt caching: the conversation up to its last block is
        // cached, and the breakpoint moves along as it grows, so each step of
        // a run reads the steps before it from the cache. The API caches
        // nothing unasked.
        if !self.refuses(CACHE_CONTROL) {
            body.insert(CACHE_CONTROL.into(), json!({ "type": "ephemeral" }));
        }
        if !request.system_prompt.is_empty() {
            body.insert("system".into(), json!(request.system_prompt));
        }
        let style = thinking_style(&request.model.id);
        let level = request.thinking.nearest(&style_levels(style));
        // A field the endpoint refused stays out; without `thinking` the
        // model does not reason, and its earlier reasoning is not replayed.
        let thinking = thinking_param(style, level, max_tokens, request.model.reasoning_budget)
            .and_then(|(on, param)| {
                let param: Vec<_> = param
                    .into_iter()
                    .filter(|(key, _)| !self.refuses(key))
                    .collect();
                let on = on && param.iter().any(|(key, _)| key == "thinking");
                (!param.is_empty()).then_some((on, param))
            });
        let replay = replay_thinking && thinking.as_ref().is_some_and(|(on, _)| *on);
        let replay_model = replay.then_some(request.model.id.as_str());
        body.insert(
            "messages".into(),
            json!(convert_messages(request.messages, replay_model)),
        );
        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.parameters,
                    })
                })
                .collect();
            body.insert("tools".into(), Value::Array(tools));
        }
        if let Some((_, param)) = thinking {
            for (key, value) in param {
                body.insert(key, value);
            }
        }
        Value::Object(body)
    }

    fn attempt(
        &self,
        body: &str,
        model: &str,
        on_event: &mut dyn FnMut(StreamEvent),
        cancel: &CancelToken,
    ) -> Result<AssistantMessage, Failure> {
        use std::io::{BufRead, BufReader};

        let url = format!("{}/v1/messages", self.base_url);
        let mut http = self
            .agent
            .post(&url)
            .set("content-type", "application/json")
            .set("accept", "text/event-stream")
            .set("anthropic-version", API_VERSION);
        if let Some(key) = &self.api_key {
            http = http.set("x-api-key", key);
        }
        let http = attribution::apply(http, self.attribution);
        let response = match http.send_string(body) {
            Ok(response) => {
                if let Some(message) = crate::retry::redirect(&response) {
                    return Err(Failure::http(response.status(), "", None, message));
                }
                response
            }
            Err(ureq::Error::Status(code, response)) => {
                let retry_after = response.header("retry-after").map(str::to_string);
                let text = response.into_string().unwrap_or_default();
                return Err(Failure::http(
                    code,
                    &text,
                    retry_after.as_deref(),
                    error_text(&text),
                ));
            }
            Err(ureq::Error::Transport(transport)) => {
                return Err(Failure::transport(format!("transport error: {transport}")));
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
                Ok(_) if cancel.is_cancelled() => {
                    return Ok(acc.finish(&self.name, model, Some(StopReason::Aborted), on_event));
                }
                Ok(_) => {}
                Err(error) => {
                    if acc.received_content {
                        let mut partial =
                            acc.finish(&self.name, model, Some(StopReason::Error), on_event);
                        partial.error_message = Some(format!("stream interrupted: {error}"));
                        return Ok(partial);
                    }
                    return Err(Failure::transport(format!("stream error: {error}")));
                }
            }
            let Some(data) = sse_data(line.trim_end()) else {
                continue;
            };
            match serde_json::from_str::<Value>(data) {
                Ok(event) => {
                    if event.get("type").and_then(Value::as_str) == Some("error") {
                        let message = error_text(&event.to_string());
                        if acc.received_content {
                            let mut partial =
                                acc.finish(&self.name, model, Some(StopReason::Error), on_event);
                            partial.error_message = Some(message);
                            return Ok(partial);
                        }
                        return Err(Failure::in_stream(&event.to_string(), message));
                    }
                    acc.feed(&event, on_event);
                }
                Err(error) => log::warn!("skipping malformed SSE event ({error}): {data}"),
            }
        }
        // A stream that closed before its end broke off: asked again while
        // nothing came, an error after that.
        if !acc.ended() && !acc.received_content {
            return Err(Failure::transport(UNFINISHED_STREAM.to_string()));
        }
        Ok(acc.finish(&self.name, model, None, on_event))
    }
}

impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn endpoint(&self) -> Option<String> {
        let rest = self
            .base_url
            .split_once("://")
            .map_or(self.base_url.as_str(), |(_, rest)| rest);
        let host = rest.split('/').next().unwrap_or("");
        (!host.is_empty()).then(|| host.to_string())
    }

    /// `GET /v1/models`. Anthropic does not report a context window there, so
    /// the picker keeps whatever is configured.
    fn list_models(&self) -> Result<Vec<ModelInfo>, String> {
        let url = format!("{}/v1/models?limit=1000", self.base_url);
        let mut http = self
            .agent
            .get(&url)
            .set("accept", "application/json")
            .set("anthropic-version", API_VERSION);
        if let Some(key) = &self.api_key {
            http = http.set("x-api-key", key);
        }
        let http = attribution::apply(http, self.attribution);
        let body = match http.call() {
            Ok(response) => {
                if let Some(message) = crate::retry::redirect(&response) {
                    return Err(message);
                }
                response
                    .into_string()
                    .map_err(|error| format!("cannot read the model list: {error}"))?
            }
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

    fn thinking_levels(&self, model: &str) -> Vec<ThinkingLevel> {
        style_levels(thinking_style(model))
    }

    fn stream(
        &self,
        request: &Request<'_>,
        on_event: &mut dyn FnMut(StreamEvent),
        cancel: &CancelToken,
    ) -> AssistantMessage {
        let model = request.model.id.as_str();
        let mut replay = true;
        let mut current = self.body(request, replay);
        with_retries(
            &self.name,
            model,
            self.retry,
            cancel,
            on_event,
            |on_event| loop {
                match self.attempt(&current.to_string(), model, on_event, cancel) {
                    // A replayed block is refused when the conversation
                    // before it changed (a compaction, a rebuilt system
                    // prompt): the documented recovery is to send the
                    // history once more without its reasoning.
                    Err(failure) if replay && is_signature_rejection(&failure.message) => {
                        log::warn!("{model} refused the replayed reasoning; retrying without it");
                        replay = false;
                    }
                    // A gateway that refuses an optional field (no
                    // `cache_control`, an older thinking shape) is asked
                    // again without it, and never again after.
                    Err(failure) => {
                        if !learn_refusals(
                            &self.refused,
                            &failure,
                            &current,
                            &OPTIONAL_FIELDS,
                            model,
                        ) {
                            return Err(failure);
                        }
                    }
                    reply => return reply,
                }
                current = self.body(request, replay);
            },
        )
    }
}

/// An image block, as a user message and a tool result both carry it.
fn image_block((media_type, data): (&str, &str)) -> Value {
    json!({
        "type": "image",
        "source": { "type": "base64", "media_type": media_type, "data": data },
    })
}

/// Turn the transcript into Anthropic messages: assistant blocks as they are,
/// and tool results gathered into the user turn that must carry them, with
/// consecutive results merged so each assistant tool-use turn is answered by
/// exactly one following user turn.
///
/// Reasoning goes back only for `replay_model`'s own blocks in the turn still
/// in progress (after the last user message), where a tool round needs it:
/// dropping the earlier ones is a leading run, which the API allows, and
/// keeps a compacted or re-prompted history valid.
fn convert_messages(messages: &[Message], replay_model: Option<&str>) -> Vec<Value> {
    let turn_start = messages
        .iter()
        .rposition(|message| matches!(message, Message::User(_)))
        .map_or(0, |index| index + 1);
    let mut out: Vec<Value> = Vec::new();
    let mut pending_results: Vec<Value> = Vec::new();
    let flush = |out: &mut Vec<Value>, results: &mut Vec<Value>| {
        if !results.is_empty() {
            out.push(json!({ "role": "user", "content": std::mem::take(results) }));
        }
    };
    for (index, message) in messages.iter().enumerate() {
        match message {
            Message::ToolResult(result) => {
                // Plain text stays a string; a result with images needs the
                // list of blocks, which `tool_result` takes images in.
                let content = if result.images().next().is_none() {
                    json!(result.plain_text())
                } else {
                    let mut blocks = vec![json!({ "type": "text", "text": result.plain_text() })];
                    blocks.extend(result.images().map(image_block));
                    Value::Array(blocks)
                };
                pending_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": result.tool_call_id,
                    "content": content,
                    "is_error": result.is_error,
                }));
            }
            Message::User(user) => {
                flush(&mut out, &mut pending_results);
                let mut content = vec![json!({ "type": "text", "text": user.model_text() })];
                content.extend(user.images().map(image_block));
                out.push(json!({ "role": "user", "content": content }));
            }
            Message::Assistant(assistant) => {
                flush(&mut out, &mut pending_results);
                let replay = index >= turn_start && replay_model == Some(assistant.model.as_str());
                let mut blocks: Vec<Value> = Vec::new();
                for block in &assistant.content {
                    match block {
                        AssistantContent::Thinking {
                            text,
                            signature: Some(signature),
                            redacted,
                        } if replay => {
                            blocks.push(if *redacted {
                                json!({ "type": "redacted_thinking", "data": signature })
                            } else {
                                json!({ "type": "thinking", "thinking": text, "signature": signature })
                            });
                        }
                        // The API rejects a text block of whitespace only.
                        AssistantContent::Text { text } if !text.trim().is_empty() => {
                            blocks.push(json!({ "type": "text", "text": text }));
                        }
                        AssistantContent::ToolCall(call) => {
                            blocks.push(json!({
                                "type": "tool_use",
                                "id": call.id,
                                "name": call.name,
                                // The API takes only an object; the text of a
                                // malformed call stays in the log.
                                "input": if call.arguments.is_object() { call.arguments.clone() } else { json!({}) },
                            }));
                        }
                        // Unsigned or earlier reasoning is not the API's to read.
                        _ => {}
                    }
                }
                if !blocks.is_empty() {
                    out.push(json!({ "role": "assistant", "content": blocks }));
                }
            }
        }
    }
    flush(&mut out, &mut pending_results);
    out
}

/// The output bound sent when none is configured: the Messages API requires
/// one. Claude 4 models accept at least this many output tokens; an older
/// model with a lower ceiling needs an explicit limit.
const UNLIMITED_MAX_TOKENS: u64 = 32_000;

/// How a model asks for reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThinkingStyle {
    /// `thinking: {type: "adaptive"}` with `output_config.effort` (Claude 4.6
    /// and later). `off` is the thinking type that stops it, when the model
    /// has one; `xhigh` whether it knows that effort; `summarized` whether
    /// its reasoning is omitted unless asked for.
    Adaptive {
        off: Option<&'static str>,
        xhigh: bool,
        summarized: bool,
    },
    /// `thinking: {type: "enabled", budget_tokens}`: Claude 4.5 and older,
    /// and what gateways serving other models accept.
    Budget,
}

/// The style of `model`, from its id. Unknown Claude ids are taken for newer
/// ones, which think adaptively; ids that are not Claude's for a gateway's.
fn thinking_style(model: &str) -> ThinkingStyle {
    let id = model.to_ascii_lowercase();
    let Some(start) = id.find("claude-") else {
        return ThinkingStyle::Budget;
    };
    let name = &id[start + "claude-".len()..];
    let is = |family: &str| {
        name.strip_prefix(family)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '@', ':']))
    };
    let adaptive = |off, xhigh, summarized| ThinkingStyle::Adaptive {
        off,
        xhigh,
        summarized,
    };
    if is("opus-5-5") || is("fable-5") || is("mythos-5") {
        adaptive(None, true, true)
    } else if is("sonnet-5-5") {
        adaptive(Some("between_tools"), true, true)
    } else if is("opus-5") || is("sonnet-5") || is("opus-4-8") || is("opus-4-7") {
        adaptive(Some("disabled"), true, true)
    } else if is("opus-4-6") || is("sonnet-4-6") {
        adaptive(Some("disabled"), false, false)
    } else if ["opus-4", "sonnet-4", "haiku", "3", "2", "instant"]
        .iter()
        .any(|old| name.starts_with(old))
    {
        ThinkingStyle::Budget
    } else {
        adaptive(None, true, true)
    }
}

/// The levels a style offers, lowest first.
fn style_levels(style: ThinkingStyle) -> Vec<ThinkingLevel> {
    use ThinkingLevel::{High, Low, Max, Medium, Minimal, Off, XHigh};
    match style {
        ThinkingStyle::Budget => vec![Off, Minimal, Low, Medium, High],
        ThinkingStyle::Adaptive { off, xhigh, .. } => {
            let mut levels = Vec::new();
            if off.is_some() {
                levels.push(Off);
            }
            levels.extend([Low, Medium, High]);
            if xhigh {
                levels.push(XHigh);
            }
            levels.push(Max);
            levels
        }
    }
}

/// The body fields asking for `level` (already one the style offers), and
/// whether the model reasons with them; `None` when nothing is sent.
fn thinking_param(
    style: ThinkingStyle,
    level: ThinkingLevel,
    max_tokens: u64,
    reasoning_budget: Option<u64>,
) -> Option<(bool, Vec<(String, Value)>)> {
    match style {
        ThinkingStyle::Budget => {
            let budget = thinking_budget(level, max_tokens, reasoning_budget)?;
            Some((
                true,
                vec![(
                    "thinking".into(),
                    json!({ "type": "enabled", "budget_tokens": budget }),
                )],
            ))
        }
        ThinkingStyle::Adaptive {
            off, summarized, ..
        } => {
            if level == ThinkingLevel::Off {
                // The effort is left at its default: the off types are
                // refused above `high`.
                let off = off?;
                return Some((false, vec![("thinking".into(), json!({ "type": off }))]));
            }
            // These models omit the reasoning text unless asked for its summary.
            let thinking = if summarized {
                json!({ "type": "adaptive", "display": "summarized" })
            } else {
                json!({ "type": "adaptive" })
            };
            Some((
                true,
                vec![
                    ("thinking".into(), thinking),
                    ("output_config".into(), json!({ "effort": level.label() })),
                ],
            ))
        }
    }
}

/// The request field that asks for prompt caching.
const CACHE_CONTROL: &str = "cache_control";

/// The fields termide adds to a request beyond what the Messages API
/// requires, any of which a gateway may refuse. One that names it in a 400
/// or 422 gets the request again without it, and never again after.
const OPTIONAL_FIELDS: [&str; 3] = [CACHE_CONTROL, "thinking", "output_config"];

impl AnthropicProvider {
    fn refuses(&self, field: &str) -> bool {
        self.refused
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(field)
    }
}

/// Whether an error is the API refusing a replayed reasoning block.
fn is_signature_rejection(message: &str) -> bool {
    message.starts_with("HTTP 400") && message.contains("signature")
}

/// The thinking budget for a level, always leaving room for the answer,
/// and no more than the connection's `reasoning_budget`: the server then
/// holds the model to it, which a count of the streamed text cannot do
/// for reasoning it summarises.
fn thinking_budget(
    level: ThinkingLevel,
    max_tokens: u64,
    reasoning_budget: Option<u64>,
) -> Option<u64> {
    let budget = match level {
        ThinkingLevel::Off => return None,
        ThinkingLevel::Minimal => 1024,
        ThinkingLevel::Low => 4096,
        ThinkingLevel::Medium => 8192,
        ThinkingLevel::High | ThinkingLevel::XHigh | ThinkingLevel::Max => 16384,
    };
    // The API requires max_tokens to exceed the budget; keep a quarter for
    // the reply at least, and never go below the 1024 floor.
    let cap = max_tokens.saturating_mul(3) / 4;
    let budget = reasoning_budget.map_or(budget, |limit| budget.min(limit));
    Some(budget.min(cap.max(1024)).max(1024))
}

/// One SSE `data:` line's payload; `None` for `event:`, comments and blanks.
fn sse_data(line: &str) -> Option<&str> {
    let data = line.strip_prefix("data:")?.trim_start();
    (!data.is_empty()).then_some(data)
}

#[derive(Debug, Default)]
struct PartialBlock {
    kind: String,
    id: String,
    name: String,
    text: String,
    input: String,
    /// A thinking block's signature, or a redacted one's data.
    signature: String,
}

/// Builds the assistant message from the Messages API event stream, keyed by
/// content-block index.
#[derive(Debug, Default)]
struct Accumulator {
    blocks: HashMap<usize, PartialBlock>,
    order: Vec<usize>,
    stop_reason: Option<String>,
    /// Whether `message_stop` arrived.
    stopped: bool,
    usage: Usage,
    received_content: bool,
}

impl Accumulator {
    /// Whether the stream said the answer was complete, with `message_stop`
    /// or the stop reason before it. A connection that closes before either
    /// cut it short.
    fn ended(&self) -> bool {
        self.stopped || self.stop_reason.is_some()
    }

    fn feed(&mut self, event: &Value, on_event: &mut dyn FnMut(StreamEvent)) {
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(usage) = event.get("message").and_then(|m| m.get("usage")) {
                    self.usage.input = u64_at(usage, "input_tokens");
                    self.usage.cache_read = u64_at(usage, "cache_read_input_tokens");
                    self.usage.cache_write = u64_at(usage, "cache_creation_input_tokens");
                }
            }
            Some("content_block_start") => {
                let index = u64_at(event, "index") as usize;
                let block = event.get("content_block");
                let kind = block
                    .and_then(|b| b.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let mut partial = PartialBlock {
                    kind: kind.clone(),
                    ..PartialBlock::default()
                };
                if kind == "redacted_thinking" {
                    partial.signature = block
                        .and_then(|b| b.get("data"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                }
                if kind == "tool_use" {
                    partial.id = block
                        .and_then(|b| b.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    partial.name = block
                        .and_then(|b| b.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    self.received_content = true;
                    on_event(StreamEvent::ToolCallStart {
                        id: partial.id.clone(),
                        name: partial.name.clone(),
                    });
                }
                self.order.push(index);
                self.blocks.insert(index, partial);
            }
            Some("content_block_delta") => {
                let index = u64_at(event, "index") as usize;
                let Some(delta) = event.get("delta") else {
                    return;
                };
                let partial = self.blocks.entry(index).or_default();
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            self.received_content = true;
                            partial.text.push_str(text);
                            on_event(StreamEvent::TextDelta(text.to_string()));
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(text) = delta.get("thinking").and_then(Value::as_str) {
                            self.received_content = true;
                            partial.text.push_str(text);
                            on_event(StreamEvent::ThinkingDelta(text.to_string()));
                        }
                    }
                    Some("signature_delta") => {
                        if let Some(signature) = delta.get("signature").and_then(Value::as_str) {
                            partial.signature.push_str(signature);
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(fragment) = delta.get("partial_json").and_then(Value::as_str) {
                            partial.input.push_str(fragment);
                            on_event(StreamEvent::ToolCallDelta {
                                id: partial.id.clone(),
                                arguments: fragment.to_string(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                let index = u64_at(event, "index") as usize;
                if let Some(partial) = self.blocks.get(&index) {
                    if partial.kind == "tool_use" {
                        on_event(StreamEvent::ToolCallEnd {
                            id: partial.id.clone(),
                        });
                    }
                }
            }
            Some("message_delta") => {
                if let Some(reason) = event
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.stop_reason = Some(reason.to_string());
                }
                if let Some(usage) = event.get("usage") {
                    let output = u64_at(usage, "output_tokens");
                    if output > 0 {
                        self.usage.output = output;
                    }
                }
            }
            Some("message_stop") => self.stopped = true,
            _ => {}
        }
    }

    fn finish(
        self,
        provider: &str,
        model: &str,
        stop_override: Option<StopReason>,
        _on_event: &mut dyn FnMut(StreamEvent),
    ) -> AssistantMessage {
        let mut content = Vec::new();
        let mut has_calls = false;
        for index in &self.order {
            let Some(partial) = self.blocks.get(index) else {
                continue;
            };
            match partial.kind.as_str() {
                // Signed reasoning is kept even without text (its display
                // omitted): the next request of the turn hands it back.
                "thinking" if !partial.text.is_empty() || !partial.signature.is_empty() => {
                    content.push(AssistantContent::Thinking {
                        text: partial.text.clone(),
                        signature: (!partial.signature.is_empty())
                            .then(|| partial.signature.clone()),
                        redacted: false,
                    });
                }
                "redacted_thinking" if !partial.signature.is_empty() => {
                    content.push(AssistantContent::Thinking {
                        text: String::new(),
                        signature: Some(partial.signature.clone()),
                        redacted: true,
                    });
                }
                "text" if !partial.text.is_empty() => content.push(AssistantContent::Text {
                    text: partial.text.clone(),
                }),
                "tool_use" if !partial.name.is_empty() => {
                    has_calls = true;
                    content.push(AssistantContent::ToolCall(ToolCall {
                        id: partial.id.clone(),
                        name: partial.name.clone(),
                        arguments: parse_arguments(&partial.input),
                        extra_content: None,
                    }));
                }
                _ => {}
            }
        }
        // A refusal, or a context the model could not take, is no answer: it
        // ends as an error that says so — the latter one the loop compacts
        // the context after — not as a reply that merely stopped.
        // So is a stream that closed before its end: what came may be cut
        // anywhere, a call's arguments included.
        let (stop_reason, error_message) = match (stop_override, self.stop_reason.as_deref()) {
            (Some(reason), _) => (reason, None),
            (None, _) if !self.ended() => (StopReason::Error, Some(UNFINISHED_STREAM.to_string())),
            (None, Some("max_tokens")) => (StopReason::Length, None),
            (None, Some("tool_use")) => (StopReason::ToolUse, None),
            (None, Some("refusal")) => (
                StopReason::Error,
                Some("The model declined to answer (refusal).".to_string()),
            ),
            (None, Some("model_context_window_exceeded")) => (
                StopReason::Error,
                Some(
                    "The conversation exceeds the model's context window \
                     (model_context_window_exceeded)."
                        .to_string(),
                ),
            ),
            (None, _) if has_calls => (StopReason::ToolUse, None),
            (None, _) => (StopReason::Stop, None),
        };
        AssistantMessage {
            content,
            stop_reason,
            usage: self.usage,
            provider: provider.to_string(),
            model: model.to_string(),
            error_message,
            failure: None,
            timestamp: now_millis(),
        }
    }
}

fn u64_at(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Tool arguments: the accumulated JSON object, an empty one when the model
/// sent none (a no-argument tool), or the raw text when it is no JSON object,
/// which the tool call then reports to the model.
fn parse_arguments(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return json!({});
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(value @ Value::Object(_)) => value,
        _ => {
            log::warn!("tool call arguments are not a JSON object: {raw}");
            Value::String(raw.to_string())
        }
    }
}

/// The client of the API. It follows no redirect: one would carry the key
/// (`x-api-key`) to wherever the `Location` points.
fn build_agent(connect: Duration, read: Duration) -> ureq::Agent {
    termide_http::agent_builder()
        .redirects(0)
        .timeout_connect(connect)
        .timeout_read(read)
        .user_agent(concat!("termide-agent/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn parse_model_list(body: &str) -> Result<Vec<ModelInfo>, String> {
    let value: Value =
        serde_json::from_str(body).map_err(|error| format!("model list is not JSON: {error}"))?;
    let items = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "model list has no `data` array".to_string())?;
    let mut models: Vec<ModelInfo> = items
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_str)?;
            Some(ModelInfo {
                id: id.to_string(),
                name: None,
                context_window: None,
            })
        })
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);
    Ok(models)
}

/// The human-readable message from `{"error": {"message": ...}}`, else the
/// body trimmed.
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
    use super::*;
    use termide_agent_core::{
        AssistantMessage, ModelSpec, ToolCall, ToolResultMessage, ToolSpec, UserMessage,
    };

    /// A redirect is not followed: the key would go along to its target.
    #[test]
    fn a_redirect_does_not_carry_the_key_away() {
        use std::io::{Read as _, Write as _};
        let elsewhere = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        elsewhere.set_nonblocking(true).unwrap();
        let target = elsewhere.local_addr().unwrap();
        let api = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/models", api.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = api.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let reply = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{target}/steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(reply.as_bytes()).unwrap();
        });
        let agent = build_agent(Duration::from_secs(5), Duration::from_secs(5));
        let response = agent.get(&url).set("x-api-key", "secret").call().unwrap();
        server.join().unwrap();
        let message = crate::retry::redirect(&response).expect("a redirect");
        assert!(
            message.contains(&format!("http://{target}/steal")),
            "{message}"
        );
        assert!(elsewhere.accept().is_err(), "the redirect was followed");
    }

    fn model(id: &str) -> ModelSpec {
        ModelSpec {
            reasoning_budget: None,
            provider: "anthropic".into(),
            id: id.into(),
            context_window: 200_000,
            max_tokens: Some(4096),
            thinking: ThinkingLevel::Off,
        }
    }

    fn assistant(model: &str, content: Vec<AssistantContent>) -> Message {
        Message::Assistant(AssistantMessage {
            content,
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
            provider: "anthropic".into(),
            model: model.into(),
            error_message: None,
            failure: None,
            timestamp: 0,
        })
    }

    fn signed(text: &str, signature: &str) -> AssistantContent {
        AssistantContent::Thinking {
            text: text.into(),
            signature: Some(signature.into()),
            redacted: false,
        }
    }

    fn feed_all(events: &[Value]) -> (AssistantMessage, Vec<StreamEvent>) {
        let mut acc = Accumulator::default();
        let mut seen = Vec::new();
        for event in events {
            acc.feed(event, &mut |e| seen.push(e));
        }
        (
            acc.finish("anthropic", "claude-x", None, &mut |e| seen.push(e)),
            seen,
        )
    }

    #[test]
    fn the_conversation_is_cached_unless_the_endpoint_refuses_it() {
        use crate::openai::tests::{serve, sse, status};
        let done = sse(concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        ));
        let refusal = status(
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"cache_control: Extra inputs are not permitted"}}"#,
        );
        let (url, bodies) = serve(vec![refusal, done.clone(), done]);
        let model = model("claude-sonnet-5");
        let messages = vec![Message::User(UserMessage::text("hi"))];
        let request = Request {
            model: &model,
            system_prompt: "",
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::Off,
        };
        let cached = |body: &str| {
            serde_json::from_str::<Value>(body).unwrap()["cache_control"]
                == json!({ "type": "ephemeral" })
        };
        assert!(cached(
            &AnthropicProvider::new("anthropic")
                .build_body(&request)
                .to_string()
        ));

        let gateway = AnthropicProvider::with_base_url("gateway", url.clone());
        let message = gateway.stream(&request, &mut |_| {}, &CancelToken::new());
        assert!(
            message.error_message.is_none(),
            "{:?}",
            message.error_message
        );
        // The next provider on that endpoint does not ask again.
        AnthropicProvider::with_base_url("gateway", url).stream(
            &request,
            &mut |_| {},
            &CancelToken::new(),
        );
        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 3);
        assert!(cached(&bodies[0]));
        assert!(!cached(&bodies[1]) && !cached(&bodies[2]));
    }

    #[test]
    fn an_unlimited_turn_still_sends_the_bound_the_api_requires() {
        let provider = AnthropicProvider::new("anthropic").with_api_key(Some("k".into()));
        let model = ModelSpec {
            max_tokens: None,
            ..model("claude-sonnet-4-5")
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
        assert_eq!(body["max_tokens"], json!(UNLIMITED_MAX_TOKENS));
    }

    #[test]
    fn the_body_carries_system_tools_and_thinking_in_the_messages_shape() {
        let provider = AnthropicProvider::new("anthropic").with_api_key(Some("k".into()));
        let call = ToolCall {
            id: "tu_1".into(),
            name: "read".into(),
            arguments: json!({ "path": "a.rs" }),
            extra_content: None,
        };
        let messages = vec![
            Message::User(UserMessage::text("hi")),
            Message::Assistant(AssistantMessage {
                content: vec![
                    AssistantContent::thinking("hmm"),
                    AssistantContent::Text {
                        text: "reading".into(),
                    },
                    AssistantContent::ToolCall(call.clone()),
                ],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-5".into(),
                error_message: None,
                failure: None,
                timestamp: 0,
            }),
            Message::ToolResult(ToolResultMessage::text(&call, "contents")),
        ];
        let tools = vec![ToolSpec {
            name: "read".into(),
            description: "read a file".into(),
            parameters: json!({ "type": "object" }),
        }];
        let request = Request {
            model: &model("claude-sonnet-4-5"),
            system_prompt: "be terse",
            messages: &messages,
            tools: &tools,
            thinking: ThinkingLevel::Medium,
        };
        let body = provider.build_body(&request);
        assert_eq!(body["system"], json!("be terse"));
        assert_eq!(body["max_tokens"], json!(4096));
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["thinking"]["budget_tokens"], json!(3072)); // capped at 3/4 of max
                                                                    // The connection's reasoning budget caps it lower still, down to the
                                                                    // API's floor.
        for (limit, sent) in [(2000, 2000), (100, 1024)] {
            let capped = ModelSpec {
                reasoning_budget: Some(limit),
                ..model("claude-sonnet-4-5")
            };
            let body = provider.build_body(&Request {
                model: &capped,
                ..request
            });
            assert_eq!(body["thinking"]["budget_tokens"], json!(sent));
        }
        assert_eq!(
            body["tools"][0]["input_schema"],
            json!({ "type": "object" })
        );

        let wire = body["messages"].as_array().unwrap();
        // user, assistant (thinking dropped, text + tool_use), tool_result user
        assert_eq!(wire.len(), 3);
        assert_eq!(
            wire[0],
            json!({ "role": "user", "content": [{ "type": "text", "text": "hi" }] })
        );
        let blocks = wire[1]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2, "unsigned thinking is not replayed");
        assert_eq!(blocks[0], json!({ "type": "text", "text": "reading" }));
        assert_eq!(blocks[1]["type"], json!("tool_use"));
        assert_eq!(blocks[1]["input"], json!({ "path": "a.rs" }));
        assert_eq!(wire[2]["role"], json!("user"));
        assert_eq!(wire[2]["content"][0]["type"], json!("tool_result"));
        assert_eq!(wire[2]["content"][0]["tool_use_id"], json!("tu_1"));

        // No thinking when it is off.
        let plain = provider.build_body(&Request {
            thinking: ThinkingLevel::Off,
            ..request.clone()
        });
        assert!(plain.get("thinking").is_none());
    }

    #[test]
    fn each_family_offers_the_levels_its_api_accepts() {
        use ThinkingLevel::{High, Low, Max, Medium, Minimal, Off, XHigh};
        let provider = AnthropicProvider::new("anthropic");
        let levels = |id: &str| provider.thinking_levels(id);
        assert_eq!(
            levels("claude-opus-5-5"),
            vec![Low, Medium, High, XHigh, Max]
        );
        assert_eq!(
            levels("claude-fable-5-1"),
            vec![Low, Medium, High, XHigh, Max]
        );
        assert_eq!(
            levels("claude-sonnet-5-5"),
            vec![Off, Low, Medium, High, XHigh, Max]
        );
        assert_eq!(
            levels("claude-opus-5"),
            vec![Off, Low, Medium, High, XHigh, Max]
        );
        assert_eq!(levels("claude-opus-4-6"), vec![Off, Low, Medium, High, Max]);
        assert_eq!(
            levels("claude-haiku-4-5"),
            vec![Off, Minimal, Low, Medium, High]
        );
        assert_eq!(
            levels("claude-sonnet-4-5@20250929"),
            vec![Off, Minimal, Low, Medium, High]
        );
        // A gateway's own model takes the budget.
        assert_eq!(levels("glm-4.6"), vec![Off, Minimal, Low, Medium, High]);
        // A Bedrock id, and a Claude newer than the table.
        assert_eq!(
            levels("us.anthropic.claude-opus-4-8"),
            vec![Off, Low, Medium, High, XHigh, Max]
        );
        assert_eq!(levels("claude-opus-7"), vec![Low, Medium, High, XHigh, Max]);
    }

    #[test]
    fn an_adaptive_model_gets_its_effort_and_a_readable_summary() {
        let provider = AnthropicProvider::new("anthropic");
        let messages = vec![Message::User(UserMessage::text("hi"))];
        let body = |id: &str, thinking| {
            provider.build_body(&Request {
                model: &model(id),
                system_prompt: "",
                messages: &messages,
                tools: &[],
                thinking,
            })
        };

        let on = body("claude-opus-5-5", ThinkingLevel::XHigh);
        assert_eq!(
            on["thinking"],
            json!({ "type": "adaptive", "display": "summarized" })
        );
        assert_eq!(on["output_config"], json!({ "effort": "xhigh" }));

        // Opus 5.5 cannot stop thinking: off is its lowest effort.
        let off = body("claude-opus-5-5", ThinkingLevel::Off);
        assert_eq!(off["output_config"], json!({ "effort": "low" }));

        // Sonnet 5.5 stops between tools; others disable.
        let off = body("claude-sonnet-5-5", ThinkingLevel::Off);
        assert_eq!(off["thinking"], json!({ "type": "between_tools" }));
        assert!(off.get("output_config").is_none());
        let off = body("claude-opus-4-8", ThinkingLevel::Off);
        assert_eq!(off["thinking"], json!({ "type": "disabled" }));

        // 4.6 has no xhigh and summarizes on its own.
        let on = body("claude-sonnet-4-6", ThinkingLevel::XHigh);
        assert_eq!(on["thinking"], json!({ "type": "adaptive" }));
        assert_eq!(on["output_config"], json!({ "effort": "high" }));
    }

    #[test]
    fn signed_reasoning_goes_back_only_within_the_current_turn() {
        let provider = AnthropicProvider::new("anthropic");
        let call = ToolCall {
            id: "tu_1".into(),
            name: "read".into(),
            arguments: json!({}),
            extra_content: None,
        };
        let id = "claude-opus-5-5";
        let messages = vec![
            Message::User(UserMessage::text("first")),
            assistant(
                id,
                vec![
                    signed("earlier", "s0"),
                    AssistantContent::Text {
                        text: "done".into(),
                    },
                ],
            ),
            Message::User(UserMessage::text("second")),
            assistant(
                id,
                vec![
                    signed("now", "s1"),
                    AssistantContent::ToolCall(call.clone()),
                ],
            ),
            Message::ToolResult(ToolResultMessage::text(&call, "contents")),
            assistant(
                "claude-opus-5",
                vec![
                    AssistantContent::Thinking {
                        text: String::new(),
                        signature: Some("opaque".into()),
                        redacted: true,
                    },
                    AssistantContent::ToolCall(call.clone()),
                ],
            ),
        ];
        let request = Request {
            model: &model(id),
            system_prompt: "",
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::High,
        };
        let body = provider.build_body(&request);
        let wire = body["messages"].as_array().unwrap();
        assert_eq!(
            wire[1]["content"].as_array().unwrap().len(),
            1,
            "earlier turn"
        );
        assert_eq!(
            wire[3]["content"][0],
            json!({ "type": "thinking", "thinking": "now", "signature": "s1" })
        );
        // Another model's block is not this one's to read.
        assert_eq!(wire[5]["content"][0]["type"], json!("tool_use"));

        let stripped = provider.body(&request, false);
        assert_eq!(
            stripped["messages"][3]["content"][0]["type"],
            json!("tool_use")
        );
        assert!(is_signature_rejection(
            "HTTP 400: messages.3.content.0: Invalid `signature` in `thinking` block."
        ));
        assert!(!is_signature_rejection("HTTP 429: slow down"));
    }

    #[test]
    fn the_stream_keeps_a_blocks_signature_and_redacted_reasoning() {
        let events = vec![
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "abc" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "redacted_thinking", "data": "xyz" } }),
            json!({ "type": "content_block_stop", "index": 1 }),
            json!({ "type": "message_stop" }),
        ];
        let (message, _) = feed_all(&events);
        assert_eq!(
            message.content,
            vec![
                signed("", "abc"),
                AssistantContent::Thinking {
                    text: String::new(),
                    signature: Some("xyz".into()),
                    redacted: true,
                },
            ]
        );
    }

    #[test]
    fn the_event_stream_becomes_text_a_tool_call_and_usage() {
        let events = vec![
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 12, "cache_read_input_tokens": 3 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Hello" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "tool_use", "id": "tu_9", "name": "read" } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": "{\"path\":" } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": "\"a.rs\"}" } }),
            json!({ "type": "content_block_stop", "index": 1 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 7 } }),
            json!({ "type": "message_stop" }),
        ];
        let (message, seen) = feed_all(&events);
        assert_eq!(message.plain_text(), "Hello");
        assert_eq!(message.stop_reason, StopReason::ToolUse);
        assert_eq!(message.usage.input, 12);
        assert_eq!(message.usage.cache_read, 3);
        assert_eq!(message.usage.output, 7);
        let calls: Vec<&ToolCall> = message.tool_calls().collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "tu_9");
        assert_eq!(calls[0].arguments, json!({ "path": "a.rs" }));
        assert!(seen.contains(&StreamEvent::TextDelta("Hello".into())));
        assert!(seen.contains(&StreamEvent::ToolCallStart {
            id: "tu_9".into(),
            name: "read".into()
        }));
        assert!(seen.contains(&StreamEvent::ToolCallEnd { id: "tu_9".into() }));
    }

    #[test]
    fn a_refused_or_overflowing_answer_ends_as_an_error_that_says_so() {
        for (reason, says) in [
            ("refusal", "declined"),
            ("model_context_window_exceeded", "context window"),
        ] {
            let events = vec![
                json!({ "type": "message_start", "message": { "usage": { "input_tokens": 1 } } }),
                json!({ "type": "message_delta", "delta": { "stop_reason": reason }, "usage": { "output_tokens": 0 } }),
                json!({ "type": "message_stop" }),
            ];
            let (message, _) = feed_all(&events);
            assert_eq!(message.stop_reason, StopReason::Error, "{reason}");
            let error = message.error_message.unwrap_or_default();
            assert!(error.contains(says), "{reason}: {error}");
        }
    }

    #[test]
    fn a_stream_that_closes_before_its_end_is_unfinished() {
        let cut = vec![
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Half" } }),
        ];
        let (message, _) = feed_all(&cut);
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(message.error_message.as_deref(), Some(UNFINISHED_STREAM));
        assert_eq!(message.plain_text(), "Half");
    }

    /// Over HTTP: a stream that brought nothing is asked again, one cut
    /// after its first content ends as an error.
    #[test]
    fn an_unfinished_stream_is_retried_until_content_came() {
        use crate::openai::tests::{serve, sse};
        let empty = sse(
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\n",
        );
        let partial = sse(concat!(
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Half\"}}\n\n",
        ));
        let (url, bodies) = serve(vec![empty, partial]);
        let model = model("claude-sonnet-5");
        let messages = vec![Message::User(UserMessage::text("hi"))];
        let request = Request {
            model: &model,
            system_prompt: "",
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::Off,
        };
        let provider = AnthropicProvider::with_base_url("gw", url).with_retry(RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(1),
        });
        let message = provider.stream(&request, &mut |_| {}, &CancelToken::new());
        assert_eq!(bodies.lock().unwrap().len(), 2);
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(message.error_message.as_deref(), Some(UNFINISHED_STREAM));
        assert_eq!(message.plain_text(), "Half");
    }

    /// A gateway that refuses the adaptive thinking fields gets the request
    /// again without them, as one refusing `cache_control` does.
    #[test]
    fn a_refused_thinking_field_is_dropped_and_stays_dropped() {
        use crate::openai::tests::{serve, sse, status};
        let done = sse(concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        ));
        let refusal = status(
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"output_config: Extra inputs are not permitted"}}"#,
        );
        let (url, bodies) = serve(vec![refusal, done.clone(), done]);
        let model = model("claude-sonnet-5");
        let messages = vec![Message::User(UserMessage::text("hi"))];
        let request = Request {
            model: &model,
            system_prompt: "",
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::High,
        };
        let gateway = AnthropicProvider::with_base_url("gateway", url.clone());
        let message = gateway.stream(&request, &mut |_| {}, &CancelToken::new());
        assert!(message.error_message.is_none(), "{message:?}");
        AnthropicProvider::with_base_url("gateway", url).stream(
            &request,
            &mut |_| {},
            &CancelToken::new(),
        );
        let bodies: Vec<Value> = bodies
            .lock()
            .unwrap()
            .iter()
            .map(|b| serde_json::from_str(b).unwrap())
            .collect();
        assert_eq!(bodies.len(), 3);
        assert!(bodies[0].get("output_config").is_some());
        for body in &bodies[1..] {
            assert!(body.get("output_config").is_none(), "{body}");
            // The thinking itself was not refused and keeps going.
            assert_eq!(body["thinking"]["type"], "adaptive", "{body}");
        }
    }

    #[test]
    fn consecutive_tool_results_merge_into_one_user_turn() {
        let a = ToolCall {
            id: "u1".into(),
            name: "read".into(),
            arguments: json!({}),
            extra_content: None,
        };
        let b = ToolCall {
            id: "u2".into(),
            name: "read".into(),
            arguments: json!({}),
            extra_content: None,
        };
        let messages = vec![
            Message::ToolResult(ToolResultMessage::text(&a, "one")),
            Message::ToolResult(ToolResultMessage::text(&b, "two")),
        ];
        let wire = convert_messages(&messages, None);
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0]["role"], json!("user"));
        assert_eq!(wire[0]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn an_image_follows_the_text_as_a_base64_block() {
        let shot = UserMessage::text("see").with_images(vec![("image/png".into(), "QUJD".into())]);
        let wire = convert_messages(&[Message::User(shot)], None);
        assert_eq!(
            wire[0]["content"],
            json!([
                { "type": "text", "text": "see" },
                { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "QUJD" } },
            ])
        );
    }

    #[test]
    fn a_tool_result_with_an_image_goes_as_blocks() {
        let call = ToolCall {
            id: "u1".into(),
            name: "read".into(),
            arguments: json!({}),
            extra_content: None,
        };
        let shot = ToolResultMessage::text(&call, "Image shot.png")
            .with_images(vec![("image/png".into(), "QUJD".into())]);
        let wire = convert_messages(&[Message::ToolResult(shot)], None);
        assert_eq!(
            wire[0]["content"][0]["content"],
            json!([
                { "type": "text", "text": "Image shot.png" },
                { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "QUJD" } },
            ])
        );
        let plain = convert_messages(
            &[Message::ToolResult(ToolResultMessage::text(&call, "one"))],
            None,
        );
        assert_eq!(plain[0]["content"][0]["content"], "one");
    }

    #[test]
    fn base_url_normalises_and_endpoint_is_the_host() {
        let p = AnthropicProvider::with_base_url("anthropic", "https://api.anthropic.com/v1/");
        assert_eq!(p.endpoint().as_deref(), Some("api.anthropic.com"));
        let g = AnthropicProvider::with_base_url("anthropic", "https://gw.example/proxy");
        assert_eq!(g.endpoint().as_deref(), Some("gw.example"));
    }

    /// Arguments that are no JSON object are kept as their text, for the
    /// call to report, and replayed to the API as an empty object.
    #[test]
    fn malformed_arguments_are_kept_and_replayed_as_an_object() {
        assert_eq!(parse_arguments(""), json!({}));
        assert_eq!(parse_arguments("{\"a\":1}"), json!({ "a": 1 }));
        assert_eq!(parse_arguments("{\"a\":"), json!("{\"a\":"));
        assert_eq!(parse_arguments("[1]"), json!("[1]"));
        let call = ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: json!("{\"a\":"),
            extra_content: None,
        };
        let wire = convert_messages(
            &[Message::Assistant(AssistantMessage {
                content: vec![AssistantContent::ToolCall(call)],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
                provider: "anthropic".into(),
                model: "m".into(),
                error_message: None,
                failure: None,
                timestamp: 0,
            })],
            None,
        );
        assert_eq!(wire[0]["content"][0]["input"], json!({}));
    }

    /// The API rejects a whitespace-only text block, so one is left out,
    /// and a reply of nothing else is left out whole.
    #[test]
    fn a_whitespace_only_text_block_is_left_out() {
        let call = ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: json!({}),
            extra_content: None,
        };
        let reply = |content: Vec<AssistantContent>| {
            Message::Assistant(AssistantMessage {
                content,
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                provider: "anthropic".into(),
                model: "m".into(),
                error_message: None,
                failure: None,
                timestamp: 0,
            })
        };
        let wire = convert_messages(
            &[
                Message::User(UserMessage::text("hi")),
                reply(vec![
                    AssistantContent::Text {
                        text: "\n\n".into(),
                    },
                    AssistantContent::ToolCall(call),
                ]),
                reply(vec![AssistantContent::Text { text: " \n".into() }]),
            ],
            None,
        );
        assert_eq!(wire.len(), 2);
        let blocks = wire[1]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "tool_use");
    }
}
