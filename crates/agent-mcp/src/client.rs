//! JSON-RPC 2.0 over stdio, one line per message, as the MCP stdio
//! transport specifies. Blocking I/O on two threads (stdout, stderr) with
//! replies handed over channels, so a request can wait with a timeout and
//! notice a cancelled run.
//!
//! [`McpTransport`] is the pair of verbs every transport answers — one
//! request, one notification — with the handshake, `tools/list` and
//! `tools/call` written once against them, so [`HttpClient`] speaks the same
//! protocol over Streamable HTTP.
//!
//! What a server sends unasked reaches [`McpTransport::listen`]'s handler: a
//! notification such as `notifications/tools/list_changed`. A server's own
//! request is answered by the transport — `ping` with an empty result, as the
//! specification requires, anything else (roots, sampling, elicitation) with
//! "method not found", since termide offers none of them.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use termide_agent_core::{expand_env, CancelToken, McpServerConfig, McpTarget};

/// The protocol revision requested; servers answer with the one they speak.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// The most `tools/list` pages read from one server.
const MAX_TOOL_PAGES: usize = 100;

/// Receives a server's notifications: the method and its params. Called on
/// the transport's reader thread, so it must hand anything slow to a thread
/// of its own — a request made from inside it would wait for a reply that
/// thread is the one to deliver.
pub type OnMessage = Arc<dyn Fn(&str, &Value) + Send + Sync>;

/// The answer to a request a server sends us: `ping` is owed an empty
/// result, and nothing else is offered.
pub(crate) fn answer_server_request(id: &Value, method: &str) -> Value {
    if method == "ping" {
        json!({ "jsonrpc": "2.0", "id": id, "result": {} })
    } else {
        log::debug!("mcp: declining server request {method}");
        json!({
            "jsonrpc": "2.0", "id": id,
            "error": { "code": -32601, "message": "method not supported by termide" }
        })
    }
}

/// A tool as `tools/list` describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// The server's `annotations.readOnlyHint`: it says the tool changes
    /// nothing. Absent means no.
    pub read_only: bool,
}

/// What it takes to be a server termide can talk to: send a request and get
/// its answer, send a notification and expect nothing back. Everything above
/// that — the handshake, the tool list, a tool call — is the same on either
/// transport and lives here.
pub trait McpTransport: Send + Sync {
    /// Send a request and wait for its reply, up to the transport's timeout,
    /// giving up early when `cancel` is set.
    ///
    /// # Errors
    ///
    /// The reason there is no answer: the transport failed, the server
    /// answered with a JSON-RPC error, the wait ran out, or the run was
    /// cancelled.
    fn request(
        &self,
        method: &str,
        params: Value,
        cancel: Option<&CancelToken>,
    ) -> Result<Value, String>;

    /// Send a notification; nothing comes back.
    ///
    /// # Errors
    ///
    /// When the message cannot be sent.
    fn notify(&self, method: &str, params: Value) -> Result<(), String>;

    /// Hand the server's notifications to `on_message` from now on. Over
    /// stdio they arrive on the pipe already being read; over HTTP this opens
    /// the stream a server pushes them on. Called once, after the handshake.
    fn listen(&self, on_message: OnMessage);

    /// Whether the server is gone for good (a process whose output ended):
    /// nothing sent to it will be answered.
    fn stopped(&self) -> bool {
        false
    }

    /// The `initialize` handshake; returns the server's declared name.
    fn initialize(&self) -> Result<String, String> {
        let result = self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "termide", "version": env!("CARGO_PKG_VERSION") }
            }),
            None,
        )?;
        self.notify("notifications/initialized", json!({}))?;
        Ok(result["serverInfo"]["name"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    /// Every tool the server offers, following `nextCursor` pages, up to
    /// [`MAX_TOOL_PAGES`] and not round a cursor met before: a server that
    /// pages in a circle would hold the connection forever.
    fn list_tools(&self) -> Result<Vec<McpToolInfo>, String> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..MAX_TOOL_PAGES {
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let result = self.request("tools/list", params, None)?;
            for tool in result["tools"].as_array().into_iter().flatten() {
                let Some(name) = tool["name"].as_str() else {
                    continue;
                };
                tools.push(McpToolInfo {
                    name: name.to_string(),
                    description: tool["description"].as_str().unwrap_or("").to_string(),
                    input_schema: tool
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
                    read_only: tool["annotations"]["readOnlyHint"].as_bool() == Some(true),
                });
            }
            match result["nextCursor"].as_str() {
                Some(next) if !next.is_empty() => {
                    if !seen.insert(next.to_string()) {
                        log::warn!("mcp: tools/list came back to cursor {next}; stopping there");
                        return Ok(tools);
                    }
                    cursor = Some(next.to_string());
                }
                _ => return Ok(tools),
            }
        }
        log::warn!("mcp: tools/list still had pages after {MAX_TOOL_PAGES}; stopping there");
        Ok(tools)
    }

    /// Call a tool; the text of the result's content blocks and whether the
    /// server flagged it as an error.
    fn call_tool(
        &self,
        name: &str,
        arguments: &Value,
        cancel: &CancelToken,
    ) -> Result<(String, bool), String> {
        let arguments = if arguments.is_object() {
            arguments.clone()
        } else {
            json!({})
        };
        let result = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
            Some(cancel),
        )?;
        let mut text = String::new();
        for block in result["content"].as_array().into_iter().flatten() {
            let piece = match block["type"].as_str() {
                Some("text") => block["text"].as_str().unwrap_or("").to_string(),
                Some("image") => format!(
                    "[image {}]",
                    block["mimeType"].as_str().unwrap_or("of unknown type")
                ),
                Some("resource") => block["resource"]["text"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        format!(
                            "[resource {}]",
                            block["resource"]["uri"].as_str().unwrap_or("")
                        )
                    }),
                other => format!("[{} content]", other.unwrap_or("unknown")),
            };
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&piece);
        }
        Ok((text, result["isError"].as_bool().unwrap_or(false)))
    }
}

type Pending = Arc<Mutex<HashMap<u64, Sender<Result<Value, String>>>>>;
type SharedHandler = Arc<Mutex<Option<OnMessage>>>;

/// Lines for the server's stdin, written on a thread of their own: a server
/// that stops reading fills the pipe, and a write made by the caller would
/// then hold that request, and every one after it, past any timeout. `None`
/// closes stdin.
type Outbox = Sender<Option<Vec<u8>>>;

fn start_writer(mut writer: impl Write + Send + 'static) -> Outbox {
    let (outbox, lines) = mpsc::channel::<Option<Vec<u8>>>();
    let spawned = std::thread::Builder::new()
        .name("termide-mcp-stdin".into())
        .spawn(move || {
            while let Ok(Some(line)) = lines.recv() {
                if let Err(error) = writer.write_all(&line).and_then(|()| writer.flush()) {
                    log::debug!("mcp: cannot write to the server: {error}");
                    return;
                }
            }
        });
    if let Err(error) = spawned {
        log::warn!("mcp: cannot start the writer thread: {error}");
    }
    outbox
}

fn post(outbox: &Outbox, message: &Value) -> Result<(), String> {
    let mut line = message.to_string().into_bytes();
    line.push(b'\n');
    outbox
        .send(Some(line))
        .map_err(|_| "cannot write to the server: it stopped reading".to_string())
}

pub struct McpClient {
    writer: Outbox,
    pending: Pending,
    on_message: SharedHandler,
    next_id: AtomicU64,
    timeout: Duration,
    child: Mutex<Option<Child>>,
    /// Set by the reader when the server's output ends, under the `pending`
    /// lock, so a request either sees it or is cleared with the others.
    stopped: Arc<AtomicBool>,
}

/// What a request to a server whose output has ended fails with.
pub(crate) const STOPPED: &str = "the server has stopped; /mcp reload starts it again";

impl McpClient {
    /// Start the server process and speak to it over its stdin/stdout;
    /// stderr lines go to the log.
    pub fn spawn(name: &str, config: &McpServerConfig) -> Result<Self, String> {
        let (program, args) = match config.target()? {
            McpTarget::Stdio { command, args } => (command, args),
            McpTarget::Http { url } => return Err(format!("{name}: {url} is no process")),
        };
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &config.env {
            command.env(key, expand_env(value, |var| std::env::var(var).ok()));
        }
        if let Some(cwd) = &config.cwd {
            command.current_dir(cwd);
        }
        crate::process::own_group(&mut command);
        let mut child = command
            .spawn()
            .map_err(|error| format!("cannot start {program}: {error}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        if let Some(stderr) = child.stderr.take() {
            let server = name.to_string();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log::debug!("mcp {server}: {line}");
                }
            });
        }
        let client = Self::from_streams(stdout, stdin, Duration::from_secs(config.timeout_secs));
        *client.child.lock().unwrap_or_else(PoisonError::into_inner) = Some(child);
        Ok(client)
    }

    /// Speak over any pair of streams (tests, other transports).
    pub fn from_streams(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        timeout: Duration,
    ) -> Self {
        let writer = start_writer(writer);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let on_message: SharedHandler = Arc::new(Mutex::new(None));
        let reader_pending = Arc::clone(&pending);
        let reply_writer = writer.clone();
        let reader_handler = Arc::clone(&on_message);
        let stopped = Arc::new(AtomicBool::new(false));
        let reader_stopped = Arc::clone(&stopped);
        std::thread::spawn(move || {
            read_loop(reader, &reader_pending, &reply_writer, &reader_handler);
            // The server is gone: every waiting request learns it now, and
            // every later one at once rather than at its timeout.
            let mut pending = reader_pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            reader_stopped.store(true, Ordering::Relaxed);
            pending.clear();
        });
        Self {
            writer,
            pending,
            on_message,
            next_id: AtomicU64::new(1),
            timeout,
            child: Mutex::new(None),
            stopped,
        }
    }

    /// The `initialize` handshake is on [`McpTransport`]; the name it returns
    /// is what the panel shows. `request` and `notify` are the two verbs the
    /// pipes answer. Queued, never waited for (see [`Outbox`]).
    fn write(&self, message: &Value) -> Result<(), String> {
        post(&self.writer, message)
    }
}

impl McpTransport for McpClient {
    /// Write one line and wait for the reply carrying this id, up to the
    /// timeout, giving up early when `cancel` is set.
    fn request(
        &self,
        method: &str,
        params: Value,
        cancel: Option<&CancelToken>,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        {
            let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            if self.stopped.load(Ordering::Relaxed) {
                return Err(format!("{method}: {STOPPED}"));
            }
            pending.insert(id, tx);
        }
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if let Err(error) = self.write(&message) {
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
            return Err(error);
        }
        let outcome = wait(&rx, self.timeout, cancel);
        if let Err(error) = &outcome {
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
            if error.gave_up() {
                let _ = self.notify("notifications/cancelled", cancelled(id, error));
            }
        }
        outcome.map_err(|error| format!("{method}: {error}"))
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    fn listen(&self, on_message: OnMessage) {
        *self
            .on_message
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(on_message);
    }

    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }
}

impl Drop for McpClient {
    /// Close the server's stdin, which tells it to exit, and end its process
    /// group a moment later (see [`crate::process::retire`]).
    fn drop(&mut self) {
        let _ = self.writer.send(None);
        if let Some(child) = self
            .child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            crate::process::retire(child, crate::process::GRACE);
        }
    }
}

/// Why a request has no answer.
#[derive(Debug)]
pub(crate) enum WaitError {
    /// The server answered with an error, or the transport failed.
    Failed(String),
    /// The run was cancelled.
    Aborted,
    /// No answer within the timeout.
    TimedOut(Duration),
    /// The server went away.
    Closed,
}

impl WaitError {
    /// Whether we stopped waiting while the server may still be working on
    /// the request: it is then told so (`notifications/cancelled`).
    pub(crate) fn gave_up(&self) -> bool {
        matches!(self, Self::Aborted | Self::TimedOut(_))
    }
}

impl std::fmt::Display for WaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(error) => f.write_str(error),
            Self::Aborted => f.write_str("aborted"),
            Self::TimedOut(timeout) => write!(f, "no reply within {} s", timeout.as_secs()),
            Self::Closed => f.write_str("the server closed the connection"),
        }
    }
}

/// The params of `notifications/cancelled` for request `id`.
pub(crate) fn cancelled(id: u64, why: &WaitError) -> Value {
    json!({ "requestId": id, "reason": why.to_string() })
}

/// Wait for the reply on `rx` up to `timeout`, giving up early when
/// `cancel` is set.
pub(crate) fn wait(
    rx: &Receiver<Result<Value, String>>,
    timeout: Duration,
    cancel: Option<&CancelToken>,
) -> Result<Value, WaitError> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(WaitError::Aborted);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(WaitError::TimedOut(timeout));
        }
        match rx.recv_timeout(left.min(Duration::from_millis(50))) {
            Ok(reply) => return reply.map_err(WaitError::Failed),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(WaitError::Closed),
        }
    }
}

/// Deliver replies to their requests, answer a server's own requests (see
/// [`answer_server_request`]) and hand its notifications to the handler.
fn read_loop(reader: impl Read, pending: &Pending, writer: &Outbox, handler: &SharedHandler) {
    for line in BufReader::new(reader).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                log::debug!("mcp: skipping a line that is not JSON ({error})");
                continue;
            }
        };
        // A server's request may carry a string id; only our own replies are
        // known to be numbers.
        let has_id = message.get("id").is_some_and(|id| !id.is_null());
        match (
            has_id.then(|| message["id"].as_u64()),
            message.get("method"),
        ) {
            (Some(None), None) => {}
            (Some(Some(id)), None) => {
                let reply = if let Some(error) = message.get("error") {
                    Err(format!(
                        "{} (code {})",
                        error["message"].as_str().unwrap_or("error"),
                        error["code"]
                    ))
                } else {
                    Ok(message["result"].clone())
                };
                if let Some(tx) = pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&id)
                {
                    let _ = tx.send(reply);
                }
            }
            (Some(_), Some(method)) => {
                let reply = answer_server_request(&message["id"], method.as_str().unwrap_or(""));
                let _ = post(writer, &reply);
            }
            (None, Some(method)) => {
                let method = method.as_str().unwrap_or("");
                log::debug!("mcp: notification {method}");
                let handler = handler
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                if let Some(handler) = handler {
                    handler(method, &message["params"]);
                }
            }
            (None, None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::pipe;

    /// A server on the other end of two pipes: answers `initialize`,
    /// `tools/list` (two pages) and `tools/call`, and asks one question of
    /// its own to see it declined.
    fn fake_server() -> McpClient {
        let (to_server_rx, to_server_tx) = pipe().unwrap();
        let (from_server_rx, from_server_tx) = pipe().unwrap();
        std::thread::spawn(move || {
            let mut out = from_server_tx;
            let reply = |out: &mut std::io::PipeWriter, value: Value| {
                writeln!(out, "{value}").unwrap();
            };
            for line in BufReader::new(to_server_rx).lines().map_while(Result::ok) {
                let message: Value = serde_json::from_str(&line).unwrap();
                let id = message["id"].clone();
                match message["method"].as_str() {
                    Some("initialize") => {
                        assert_eq!(message["params"]["clientInfo"]["name"], "termide");
                        reply(
                            &mut out,
                            json!({ "jsonrpc": "2.0", "id": 99, "method": "roots/list" }),
                        );
                        reply(
                            &mut out,
                            json!({ "jsonrpc": "2.0", "id": id, "result": {
                            "protocolVersion": PROTOCOL_VERSION,
                            "serverInfo": { "name": "fake", "version": "0" } } }),
                        );
                    }
                    Some("notifications/initialized") => {}
                    Some("tools/list") => {
                        if message["params"]["cursor"].is_null() {
                            reply(
                                &mut out,
                                json!({ "jsonrpc": "2.0", "id": id, "result": {
                                "tools": [{ "name": "echo", "description": "Echo", "inputSchema": { "type": "object" },
                                    "annotations": { "readOnlyHint": true } }],
                                "nextCursor": "p2" } }),
                            );
                        } else {
                            reply(
                                &mut out,
                                json!({ "jsonrpc": "2.0", "id": id, "result": {
                                "tools": [{ "name": "fail" }] } }),
                            );
                        }
                    }
                    Some("tools/call") => {
                        let name = message["params"]["name"].as_str().unwrap();
                        if name == "echo" {
                            let text = message["params"]["arguments"]["text"].clone();
                            reply(
                                &mut out,
                                json!({ "jsonrpc": "2.0", "id": id, "result": {
                                "content": [{ "type": "text", "text": text }, { "type": "image", "mimeType": "image/png" }] } }),
                            );
                        } else if name == "fail" {
                            reply(
                                &mut out,
                                json!({ "jsonrpc": "2.0", "id": id, "result": {
                                "content": [{ "type": "text", "text": "boom" }], "isError": true } }),
                            );
                        } else {
                            reply(
                                &mut out,
                                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": "unknown tool" } }),
                            );
                        }
                    }
                    other => {
                        // The decline of our roots/list request arrives here as
                        // a reply with no method.
                        assert!(other.is_none(), "{message}");
                        assert_eq!(message["error"]["code"], -32601);
                    }
                }
            }
        });
        McpClient::from_streams(from_server_rx, to_server_tx, Duration::from_secs(5))
    }

    #[test]
    fn handshake_listing_and_calls_round_trip() {
        let client = fake_server();
        assert_eq!(client.initialize().unwrap(), "fake");
        let tools = client.list_tools().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["echo", "fail"]);
        assert_eq!(tools[1].input_schema["type"], "object");
        // Only a tool the server marks read-only is taken as one.
        assert!(tools[0].read_only);
        assert!(!tools[1].read_only);

        let cancel = CancelToken::new();
        let (text, is_error) = client
            .call_tool("echo", &json!({ "text": "hi" }), &cancel)
            .unwrap();
        assert_eq!(text, "hi\n[image image/png]");
        assert!(!is_error);
        let (text, is_error) = client.call_tool("fail", &json!({}), &cancel).unwrap();
        assert_eq!((text.as_str(), is_error), ("boom", true));
        let error = client.call_tool("nope", &json!({}), &cancel).unwrap_err();
        assert!(error.contains("unknown tool"), "{error}");

        cancel.cancel();
        let aborted = client.call_tool("echo", &json!({}), &cancel).unwrap_err();
        assert!(aborted.contains("aborted"), "{aborted}");
    }

    #[test]
    fn a_silent_server_times_out_and_a_closed_one_is_reported() {
        let (_keep_rx, to_server_tx) = pipe().unwrap();
        let (from_server_rx, from_server_tx) = pipe().unwrap();
        let client =
            McpClient::from_streams(from_server_rx, to_server_tx, Duration::from_millis(120));
        let error = client.request("ping", json!({}), None).unwrap_err();
        assert!(error.contains("no reply within"), "{error}");
        assert!(!client.stopped());
        drop(from_server_tx);
        std::thread::sleep(Duration::from_millis(50));
        let error = client.request("ping", json!({}), None).unwrap_err();
        assert!(
            error.contains("closed") || error.contains("no reply") || error.contains(STOPPED),
            "{error}"
        );
    }

    /// Once a server's output ends, it is known to be gone: a request fails
    /// at once instead of waiting out its timeout.
    #[test]
    fn a_server_whose_output_ended_fails_requests_at_once() {
        let (_keep_rx, to_server_tx) = pipe().unwrap();
        let (from_server_rx, from_server_tx) = pipe().unwrap();
        let client = McpClient::from_streams(from_server_rx, to_server_tx, Duration::from_secs(30));
        drop(from_server_tx);
        let started = Instant::now();
        while !client.stopped() {
            assert!(started.elapsed() < Duration::from_secs(5), "never noticed");
            std::thread::sleep(Duration::from_millis(10));
        }
        let started = Instant::now();
        let error = client.request("tools/call", json!({}), None).unwrap_err();
        assert_eq!(error, format!("tools/call: {STOPPED}"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    /// A server that stops reading its stdin fills the pipe; requests still
    /// end at their timeout instead of blocking in the write.
    #[test]
    fn a_server_that_reads_nothing_cannot_hold_requests() {
        let (_unread, to_server_tx) = pipe().unwrap();
        let (from_server_rx, _from_server_tx) = pipe().unwrap();
        let client =
            McpClient::from_streams(from_server_rx, to_server_tx, Duration::from_millis(100));
        let big = "x".repeat(1 << 20);
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            for _ in 0..3 {
                let error = client
                    .request("tools/call", json!({ "text": big }), None)
                    .unwrap_err();
                assert!(error.contains("no reply within"), "{error}");
            }
            let _ = done_tx.send(());
        });
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a request blocked on the full pipe");
    }

    /// A request given up on, by a cancel or the timeout, is cancelled on
    /// the server too.
    #[test]
    fn giving_up_on_a_request_tells_the_server() {
        let give_up = |timeout: Duration, cancel: Option<&CancelToken>| {
            let (to_server_rx, to_server_tx) = pipe().unwrap();
            let (from_server_rx, from_server_tx) = pipe().unwrap();
            let client = McpClient::from_streams(from_server_rx, to_server_tx, timeout);
            let error = client.request("tools/call", json!({}), cancel).unwrap_err();
            drop(client);
            drop(from_server_tx);
            let lines: Vec<Value> = BufReader::new(to_server_rx)
                .lines()
                .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
                .collect();
            (error, lines)
        };
        let (error, lines) = give_up(Duration::from_millis(100), None);
        assert!(error.contains("no reply within"), "{error}");
        assert_eq!(lines[1]["method"], "notifications/cancelled");
        assert_eq!(lines[1]["params"]["requestId"], 1);

        let cancel = CancelToken::new();
        let canceller = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            canceller.cancel();
        });
        let (error, lines) = give_up(Duration::from_secs(10), Some(&cancel));
        assert!(error.contains("aborted"), "{error}");
        assert_eq!(lines[1]["method"], "notifications/cancelled");
        assert_eq!(lines[1]["params"]["reason"], "aborted");
    }

    /// A server whose pages run in a circle, or on without end, is listed
    /// as far as it went rather than read forever.
    #[test]
    fn tool_pages_stop_at_a_repeated_cursor_and_at_the_cap() {
        struct Pages {
            next: fn(u64) -> Option<String>,
            calls: AtomicU64,
        }
        impl McpTransport for Pages {
            fn request(
                &self,
                _method: &str,
                _params: Value,
                _cancel: Option<&CancelToken>,
            ) -> Result<Value, String> {
                let page = self.calls.fetch_add(1, Ordering::Relaxed);
                Ok(json!({
                    "tools": [{ "name": format!("t{page}") }],
                    "nextCursor": (self.next)(page),
                }))
            }
            fn notify(&self, _method: &str, _params: Value) -> Result<(), String> {
                Ok(())
            }
            fn listen(&self, _on_message: OnMessage) {}
        }
        let circle = Pages {
            next: |page| Some(format!("c{}", page % 2)),
            calls: AtomicU64::new(0),
        };
        assert_eq!(circle.list_tools().unwrap().len(), 3);
        let endless = Pages {
            next: |page| Some(format!("c{page}")),
            calls: AtomicU64::new(0),
        };
        assert_eq!(endless.list_tools().unwrap().len(), MAX_TOOL_PAGES);
    }

    #[test]
    fn a_pushed_notification_reaches_the_handler_and_a_ping_is_answered() {
        let (to_server_rx, to_server_tx) = pipe().unwrap();
        let (from_server_rx, mut from_server_tx) = pipe().unwrap();
        let client = McpClient::from_streams(from_server_rx, to_server_tx, Duration::from_secs(5));
        let (seen_tx, seen_rx) = mpsc::channel();
        client.listen(Arc::new(move |method, _| {
            let _ = seen_tx.send(method.to_string());
        }));
        writeln!(
            from_server_tx,
            "{}",
            json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
        )
        .unwrap();
        // A server's request may carry a string id; the answer echoes it.
        writeln!(
            from_server_tx,
            "{}",
            json!({"jsonrpc": "2.0", "id": "p-1", "method": "ping"})
        )
        .unwrap();
        assert_eq!(
            seen_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "notifications/tools/list_changed"
        );
        let mut answer = String::new();
        BufReader::new(to_server_rx).read_line(&mut answer).unwrap();
        let answer: Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(answer["id"], "p-1");
        assert_eq!(answer["result"], json!({}));
    }

    #[cfg(unix)]
    #[test]
    fn a_real_process_over_stdio_answers() {
        let script = r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","serverInfo":{"name":"sh","version":"0"}}}\n' "$id";;
    *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"env","description":"Env","inputSchema":{"type":"object"}}]}}\n' "$id";;
    *'"tools/call"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"%s"}]}}\n' "$id" "$GREETING";;
  esac
done"#;
        let config = McpServerConfig {
            command: Some("sh".into()),
            args: vec!["-c".into(), script.into()],
            env: [("GREETING".to_string(), "hi-$USER_FOR_TEST".to_string())].into(),
            timeout_secs: 5,
            ..Default::default()
        };
        std::env::set_var("USER_FOR_TEST", "tester");
        let client = McpClient::spawn("sh", &config).unwrap();
        assert_eq!(client.initialize().unwrap(), "sh");
        assert_eq!(client.list_tools().unwrap()[0].name, "env");
        let (text, _) = client
            .call_tool("env", &json!({}), &CancelToken::new())
            .unwrap();
        assert_eq!(text, "hi-tester");
    }
}
