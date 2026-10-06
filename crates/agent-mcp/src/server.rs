//! MCP server of termide: its own tools, served to an external agent, so the
//! agent's model calls them and termide runs them — under its hooks, with its
//! permission decisions and its undo checkpoints — rather than the agent's
//! own tools running out of termide's sight.
//!
//! The transport is MCP's Streamable HTTP in its simplest form, on the
//! loopback only: every JSON-RPC message is a `POST`, a request gets its
//! response as the body, a notification gets `202`. A `GET` opens the
//! standalone server stream, which carries `notifications/tools/list_changed`
//! whenever [`McpServer::set_tools`] changes the set, so the agent lists the
//! tools again: an MCP server of termide's that connects after the agent's
//! session started still reaches it. A bearer token, handed to the agent with
//! the URL, keeps other local processes out.
//!
//! A tool call is answered as a server-sent event stream when the client
//! accepts one: the headers go out at once and a comment line every few
//! seconds, so a call that runs for many minutes — a subagent, a long build,
//! a permission card the user has not got to — does not trip the client's
//! idle timeout. The same writes notice a client that went away, and the
//! call is then cancelled, taking down a question asked on its behalf.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};
use termide_agent_core::{
    judge_tool_call, run_judged_call, CancelToken, Hooks, Judgment, ToolCall, ToolContext,
    ToolRegistry,
};

use crate::client::PROTOCOL_VERSION;

/// The name the agent knows the server by; its tools reach the model as
/// `mcp__termide__<tool>`.
pub const SERVER_NAME: &str = "termide";

/// The largest request body read; a tool call's arguments are far smaller.
const MAX_BODY: usize = 16 * 1024 * 1024;

/// How often a streamed tool call shows the client it is alive. Clients
/// drop a request that has been silent for minutes (five in Bun's `fetch`).
const KEEPALIVE: Duration = Duration::from_secs(10);

struct Shared {
    tools: RwLock<ToolRegistry>,
    /// Bumped by every change of `tools`; the server streams wait on it.
    generation: Mutex<u64>,
    changed: Condvar,
    /// One call at a time is judged by the hooks: they may block on a
    /// permission card, and the panel answers one question at a time. The
    /// tools themselves run outside, so a long call holds up no other.
    hooks: Mutex<Box<dyn Hooks + Send>>,
    cwd: PathBuf,
    token: String,
    /// The cancel token of each call in flight, by its JSON-RPC id, for
    /// `notifications/cancelled`.
    running: Mutex<HashMap<String, CancelToken>>,
    stop: AtomicBool,
    /// [`KEEPALIVE`]; shorter in tests.
    keepalive: Duration,
}

/// A running server; dropping it stops it.
pub struct McpServer {
    shared: Arc<Shared>,
    addr: SocketAddr,
    thread: Option<JoinHandle<()>>,
}

impl McpServer {
    /// Serve `tools` on a free loopback port. Each call runs through `hooks`
    /// in `cwd`, as the built-in loop would run it.
    pub fn start(
        tools: ToolRegistry,
        hooks: Box<dyn Hooks + Send>,
        cwd: PathBuf,
    ) -> std::io::Result<Self> {
        Self::start_with(tools, hooks, cwd, KEEPALIVE)
    }

    fn start_with(
        tools: ToolRegistry,
        hooks: Box<dyn Hooks + Send>,
        cwd: PathBuf,
        keepalive: Duration,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            tools: RwLock::new(tools),
            generation: Mutex::new(0),
            changed: Condvar::new(),
            hooks: Mutex::new(hooks),
            cwd,
            token: new_token(),
            running: Mutex::new(HashMap::new()),
            stop: AtomicBool::new(false),
            keepalive,
        });
        let serving = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("termide-mcp-server".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if serving.stop.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let shared = Arc::clone(&serving);
                    // A tool call can take minutes; others must not wait on it.
                    std::thread::spawn(move || {
                        if let Err(error) = serve(&shared, stream) {
                            log::debug!("mcp server: {error}");
                        }
                    });
                }
            })?;
        Ok(Self {
            shared,
            addr,
            thread: Some(thread),
        })
    }

    /// Where the agent reaches the server.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    /// The bearer token the agent sends in `Authorization`.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.shared.token
    }

    /// Serve `tools` from now on, and tell the agent the list changed. A call
    /// already running keeps the tool it started with.
    pub fn set_tools(&self, tools: ToolRegistry) {
        *self
            .shared
            .tools
            .write()
            .unwrap_or_else(PoisonError::into_inner) = tools;
        *self
            .shared
            .generation
            .lock()
            .unwrap_or_else(PoisonError::into_inner) += 1;
        self.shared.changed.notify_all();
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        // Wake the server streams so they see the flag and close.
        drop(
            self.shared
                .generation
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        self.shared.changed.notify_all();
        for cancel in self
            .shared
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            cancel.cancel();
        }
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect(self.addr);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A token no other local process can guess: the process's random hashing
/// keys, mixed with the time.
fn new_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    (0..2)
        .map(|round| {
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_u128(nanos);
            hasher.write_u32(round);
            format!("{:016x}", hasher.finish())
        })
        .collect()
}

/// One HTTP request.
struct Request {
    method: String,
    path: String,
    authorization: Option<String>,
    /// Whether the client takes the answer as an event stream.
    accepts_stream: bool,
    body: Vec<u8>,
}

fn read_request(reader: &mut impl BufRead) -> std::io::Result<Option<Request>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut length = 0usize;
    let mut authorization = None;
    let mut accepts_stream = false;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            return Ok(None);
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().unwrap_or(0),
            "authorization" => authorization = Some(value.trim().to_string()),
            "accept" => accepts_stream |= value.contains("text/event-stream"),
            _ => {}
        }
    }
    if length > MAX_BODY {
        return Err(std::io::Error::other("request body too large"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Some(Request {
        method,
        path,
        authorization,
        accepts_stream,
        body,
    }))
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    let content_type = if body.is_empty() {
        String::new()
    } else {
        "Content-Type: application/json\r\n".to_string()
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\n{content_type}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

/// Answer one connection: one request, then close.
fn serve(shared: &Shared, mut stream: TcpStream) -> std::io::Result<()> {
    if shared.stop.load(Ordering::Acquire) {
        return Ok(());
    }
    let mut reader = BufReader::new(stream.try_clone()?);
    let Some(request) = read_request(&mut reader)? else {
        return Ok(());
    };
    if request.path.split('?').next() != Some("/mcp") {
        return respond(&mut stream, "404 Not Found", "");
    }
    let expected = format!("Bearer {}", shared.token);
    if request.authorization.as_deref() != Some(expected.as_str()) {
        return respond(&mut stream, "401 Unauthorized", "");
    }
    if request.method == "GET" && request.accepts_stream {
        return server_stream(shared, stream);
    }
    if request.method != "POST" {
        return respond(&mut stream, "405 Method Not Allowed", "");
    }
    let Ok(message) = serde_json::from_slice::<Value>(&request.body) else {
        let error = rpc_error(Value::Null, -32700, "parse error");
        return respond(&mut stream, "400 Bad Request", &error.to_string());
    };
    if request.accepts_stream && message["method"] == "tools/call" && message.get("id").is_some() {
        return stream_tool_call(shared, stream, &message);
    }
    let reply = match message {
        Value::Array(batch) => {
            let replies: Vec<Value> = batch.iter().filter_map(|m| handle(shared, m)).collect();
            (!replies.is_empty()).then_some(Value::Array(replies))
        }
        message => handle(shared, &message),
    };
    match reply {
        Some(reply) => respond(&mut stream, "200 OK", &reply.to_string()),
        None => respond(&mut stream, "202 Accepted", ""),
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The answer to one JSON-RPC message; `None` for a notification.
fn handle(shared: &Shared, message: &Value) -> Option<Value> {
    let method = message["method"].as_str().unwrap_or("");
    let Some(id) = message.get("id").cloned() else {
        if method == "notifications/cancelled" {
            let request = message["params"]["requestId"].to_string();
            if let Some(cancel) = shared
                .running
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&request)
            {
                cancel.cancel();
            }
        }
        return None;
    };
    let params = &message["params"];
    let result = match method {
        "initialize" => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
            "capabilities": { "tools": { "listChanged": true } },
            "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        }),
        "ping" => json!({}),
        "tools/list" => {
            let tools: Vec<Value> = shared
                .tools
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .specs()
                .into_iter()
                .map(|spec| {
                    json!({
                        "name": spec.name,
                        "description": spec.description,
                        "inputSchema": spec.parameters,
                    })
                })
                .collect();
            json!({ "tools": tools })
        }
        "tools/call" => call_tool(shared, &id, params, CancelToken::new()),
        _ => return Some(rpc_error(id, -32601, "method not found")),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// Answer a `tools/call` as an event stream: the headers at once, a progress
/// notification (or, with no progress token, a comment) every [`KEEPALIVE`]
/// while the call runs, then the response as one event.
/// A write that fails, or a read that finds the connection closed, means the
/// client gave up: the call is cancelled, and whatever it waits on with it.
fn stream_tool_call(
    shared: &Shared,
    mut stream: TcpStream,
    message: &Value,
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n"
    )?;
    stream.flush()?;
    let id = message["id"].clone();
    let cancel = CancelToken::new();
    let (done, result) = mpsc::channel();
    std::thread::scope(|scope| {
        let call_cancel = cancel.clone();
        let id = &id;
        scope.spawn(move || {
            let result = call_tool(shared, id, &message["params"], call_cancel);
            let _ = done.send(result);
        });
        // A comment keeps the connection open, but Claude Code also aborts a
        // call that sends "no response or progress" for five minutes, and only
        // a progress notification counts as progress there. The client asks
        // for them by naming a token; one that names none gets the comment.
        let progress_token = message["params"]["_meta"]
            .get("progressToken")
            .filter(|token| token.is_string() || token.is_number())
            .cloned();
        let mut ticks = 0u64;
        let mut alive = true;
        let result = loop {
            match result.recv_timeout(shared.keepalive) {
                Ok(result) => break Some(result),
                Err(RecvTimeoutError::Disconnected) => break None,
                Err(RecvTimeoutError::Timeout) if alive => {
                    ticks += 1;
                    let beat = match &progress_token {
                        // `progress` must grow with each notification.
                        Some(token) => {
                            let note = json!({
                                "jsonrpc": "2.0",
                                "method": "notifications/progress",
                                "params": { "progressToken": token, "progress": ticks },
                            });
                            format!("event: message\ndata: {note}\n\n")
                        }
                        None => ": keepalive\n\n".to_string(),
                    };
                    alive = !client_gone(&stream)
                        && stream
                            .write_all(beat.as_bytes())
                            .and_then(|()| stream.flush())
                            .is_ok();
                    if !alive {
                        cancel.cancel();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        };
        let Some(result) = result.filter(|_| alive) else {
            return Ok(());
        };
        let reply = json!({ "jsonrpc": "2.0", "id": id, "result": result });
        write!(stream, "event: message\ndata: {reply}\n\n")?;
        stream.flush()
    })
}

/// The standalone server stream a `GET` opens: a
/// `notifications/tools/list_changed` for every change of the tools since it
/// started, a comment every [`KEEPALIVE`] otherwise, until the client goes or
/// the server stops.
fn server_stream(shared: &Shared, mut stream: TcpStream) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n"
    )?;
    stream.flush()?;
    let lock = || {
        shared
            .generation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    };
    // A change between the agent's first `tools/list` and this stream would
    // be lost, so a stream opened after any change starts with the
    // notification: one listing too many costs nothing.
    let mut seen = 0;
    loop {
        let (generation, _) = shared
            .changed
            .wait_timeout_while(lock(), shared.keepalive, |generation| {
                *generation == seen && !shared.stop.load(Ordering::Acquire)
            })
            .unwrap_or_else(PoisonError::into_inner);
        let now = *generation;
        drop(generation);
        if shared.stop.load(Ordering::Acquire) || client_gone(&stream) {
            return Ok(());
        }
        let event = if now == seen {
            ": keepalive\n\n".to_string()
        } else {
            seen = now;
            let note = json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" });
            format!("event: message\ndata: {note}\n\n")
        };
        stream.write_all(event.as_bytes())?;
        stream.flush()?;
    }
}

/// Whether the client closed its end: a read that would otherwise wait
/// returns end of stream at once. The client sends nothing more on a
/// request's connection, so there is nothing to read past.
fn client_gone(stream: &TcpStream) -> bool {
    if stream.set_nonblocking(true).is_err() {
        return false;
    }
    let gone = matches!(stream.peek(&mut [0u8; 1]), Ok(0));
    let _ = stream.set_nonblocking(false);
    gone
}

/// Run a `tools/call` the way the built-in loop runs a call; `cancel` stops
/// it, and is cancelled too by a `notifications/cancelled` for its id.
fn call_tool(shared: &Shared, id: &Value, params: &Value, cancel: CancelToken) -> Value {
    let call = ToolCall {
        id: format!("mcp-{}", id.to_string().trim_matches('"')),
        name: params["name"].as_str().unwrap_or("").to_string(),
        arguments: match &params["arguments"] {
            Value::Object(_) => params["arguments"].clone(),
            _ => json!({}),
        },
        extra_content: None,
    };
    let key = id.to_string();
    shared
        .running
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key.clone(), cancel.clone());
    let ctx = ToolContext {
        withdrawn: Some(cancel.clone()),
        ..ToolContext::new(shared.cwd.clone())
    };
    let hooks = || shared.hooks.lock().unwrap_or_else(PoisonError::into_inner);
    // The set as it stands now: a change while the call runs leaves it be.
    let tools = shared
        .tools
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let judged = judge_tool_call(&tools, &call, hooks().as_mut(), &ctx, &cancel);
    let result = match judged {
        Judgment::Run(judged) => run_judged_call(&tools, judged, &ctx, &cancel, &mut |_| {}),
        Judgment::Done(result) => result,
    };
    let result = hooks().after_tool_call(&call, result);
    shared
        .running
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&key);
    json!({
        "content": [{ "type": "text", "text": result.plain_text() }],
        "isError": result.is_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use termide_agent_core::{Tool, ToolDecision, ToolResultMessage, ToolUpdate};

    struct Echo;

    impl Tool for Echo {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Say it back"
        }
        fn parameters(&self) -> Value {
            json!({ "type": "object", "properties": { "text": { "type": "string" } } })
        }
        fn execute(
            &self,
            call: &ToolCall,
            _ctx: &ToolContext,
            _on_update: &mut dyn FnMut(ToolUpdate),
            _cancel: &CancelToken,
        ) -> ToolResultMessage {
            ToolResultMessage::text(call, call.arguments["text"].as_str().unwrap_or(""))
        }
    }

    /// Refuses a call that says "secret".
    struct Guard;

    impl Hooks for Guard {
        fn before_tool_call(&mut self, call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
            if call.arguments["text"] == "secret" {
                ToolDecision::Block {
                    reason: "not that".into(),
                }
            } else {
                ToolDecision::Allow
            }
        }
    }

    fn post(server: &McpServer, token: &str, body: &Value) -> (String, String) {
        let url = server.url();
        let addr = url
            .trim_start_matches("http://")
            .trim_end_matches("/mcp")
            .to_string();
        let mut stream = TcpStream::connect(addr).unwrap();
        let body = body.to_string();
        write!(
            stream,
            "POST /mcp HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        (head.lines().next().unwrap().to_string(), body.to_string())
    }

    /// Runs until its call is cancelled, and says so.
    struct UntilCancelled(Arc<AtomicBool>);

    impl Tool for UntilCancelled {
        fn name(&self) -> &str {
            "wait"
        }
        fn description(&self) -> &str {
            "Wait to be cancelled"
        }
        fn parameters(&self) -> Value {
            json!({ "type": "object" })
        }
        fn execute(
            &self,
            call: &ToolCall,
            _ctx: &ToolContext,
            _on_update: &mut dyn FnMut(ToolUpdate),
            cancel: &CancelToken,
        ) -> ToolResultMessage {
            while !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            self.0.store(true, Ordering::Release);
            ToolResultMessage::text(call, "cancelled")
        }
    }

    /// Send a streamed `tools/call` and return the open connection.
    fn post_streamed(server: &McpServer, body: &Value) -> TcpStream {
        let addr = server
            .url()
            .trim_start_matches("http://")
            .trim_end_matches("/mcp")
            .to_string();
        let mut stream = TcpStream::connect(addr).unwrap();
        let body = body.to_string();
        write!(
            stream,
            "POST /mcp HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {}\r\nAccept: application/json, text/event-stream\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            server.token(),
            body.len()
        )
        .unwrap();
        stream
    }

    fn server() -> McpServer {
        let mut tools = ToolRegistry::new();
        tools.insert(Arc::new(Echo));
        McpServer::start(tools, Box::new(Guard), PathBuf::from("/tmp")).unwrap()
    }

    #[test]
    fn tools_are_listed_and_called_through_the_hooks() {
        let server = server();
        let token = server.token().to_string();
        let (status, body) = post(
            &server,
            &token,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                     "params": { "protocolVersion": "2025-06-18" } }),
        );
        assert_eq!(status, "HTTP/1.1 200 OK");
        let init: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "termide");
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

        let (status, _) = post(
            &server,
            &token,
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        );
        assert_eq!(status, "HTTP/1.1 202 Accepted");

        let (_, body) = post(
            &server,
            &token,
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        );
        let list: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(list["result"]["tools"][0]["name"], "echo");
        assert_eq!(list["result"]["tools"][0]["inputSchema"]["type"], "object");

        let call = |text: &str| {
            let (_, body) = post(
                &server,
                &token,
                &json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                         "params": { "name": "echo", "arguments": { "text": text } } }),
            );
            serde_json::from_str::<Value>(&body).unwrap()["result"].clone()
        };
        let said = call("hello");
        assert_eq!(said["content"][0]["text"], "hello");
        assert_eq!(said["isError"], false);
        // The hooks judge the call, as they would in the built-in loop.
        let refused = call("secret");
        assert_eq!(refused["isError"], true);
        assert!(refused["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not that"));
    }

    /// A client that takes event streams gets the call's answer as one,
    /// after keepalive comments while it runs.
    #[test]
    fn a_tool_call_is_answered_as_an_event_stream() {
        let mut tools = ToolRegistry::new();
        tools.insert(Arc::new(Echo));
        let server = McpServer::start_with(
            tools,
            Box::new(Guard),
            PathBuf::from("/tmp"),
            Duration::from_millis(20),
        )
        .unwrap();
        let mut stream = post_streamed(
            &server,
            &json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                     "params": { "name": "echo", "arguments": { "text": "hi" } } }),
        );
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
        assert!(head.contains("Content-Type: text/event-stream"), "{head}");
        let data = body
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("an event with the response");
        let reply: Value = serde_json::from_str(data).unwrap();
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["result"]["content"][0]["text"], "hi");
    }

    /// A call whose client went away is cancelled, so it does not hold the
    /// hooks — and every call after it — for good.
    #[test]
    fn a_call_whose_client_went_away_is_cancelled() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut tools = ToolRegistry::new();
        tools.insert(Arc::new(UntilCancelled(Arc::clone(&cancelled))));
        let server = McpServer::start_with(
            tools,
            Box::new(Guard),
            PathBuf::from("/tmp"),
            Duration::from_millis(20),
        )
        .unwrap();
        let mut stream = post_streamed(
            &server,
            &json!({ "jsonrpc": "2.0", "id": 8, "method": "tools/call",
                     "params": { "name": "wait", "arguments": {} } }),
        );
        let mut head = [0u8; 15];
        stream.read_exact(&mut head).unwrap();
        assert_eq!(&head, b"HTTP/1.1 200 OK");
        drop(stream);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !cancelled.load(Ordering::Acquire) {
            assert!(std::time::Instant::now() < deadline, "the call ran on");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A call that names a progress token hears growing progress while it
    /// runs: Claude Code aborts one silent for five minutes, and a keepalive
    /// comment is not progress to it.
    #[test]
    fn a_long_call_reports_progress_to_its_token() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut tools = ToolRegistry::new();
        tools.insert(Arc::new(UntilCancelled(Arc::clone(&cancelled))));
        let server = McpServer::start_with(
            tools,
            Box::new(Guard),
            PathBuf::from("/tmp"),
            Duration::from_millis(20),
        )
        .unwrap();
        let stream = post_streamed(
            &server,
            &json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/call",
                     "params": { "name": "wait", "arguments": {},
                                 "_meta": { "progressToken": 9 } } }),
        );
        let mut progress = Vec::new();
        for line in BufReader::new(&stream).lines() {
            let line = line.unwrap();
            assert_ne!(line, ": keepalive", "a comment instead of progress");
            if let Some(data) = line.strip_prefix("data: ") {
                let note: Value = serde_json::from_str(data).unwrap();
                assert_eq!(note["method"], "notifications/progress");
                assert_eq!(note["params"]["progressToken"], 9);
                progress.push(note["params"]["progress"].as_u64().unwrap());
                if progress.len() == 2 {
                    break;
                }
            }
        }
        assert!(progress[0] < progress[1], "{progress:?}");
        assert!(!cancelled.load(Ordering::Acquire), "the call still runs");
        drop(stream);
    }

    /// A long call holds up no other: the hooks judge one call at a time,
    /// but the tools run side by side.
    #[test]
    fn a_long_call_holds_up_no_other() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut tools = ToolRegistry::new();
        tools.insert(Arc::new(Echo));
        tools.insert(Arc::new(UntilCancelled(Arc::clone(&cancelled))));
        let server = McpServer::start_with(
            tools,
            Box::new(Guard),
            PathBuf::from("/tmp"),
            Duration::from_millis(20),
        )
        .unwrap();
        let mut long = post_streamed(
            &server,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                     "params": { "name": "wait", "arguments": {} } }),
        );
        let mut head = [0u8; 15];
        long.read_exact(&mut head).unwrap();
        let (_, body) = post(
            &server,
            server.token(),
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                     "params": { "name": "echo", "arguments": { "text": "meanwhile" } } }),
        );
        let reply: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(reply["result"]["content"][0]["text"], "meanwhile");
        assert!(
            !cancelled.load(Ordering::Acquire),
            "the long call still runs"
        );
        drop(long);
    }

    #[test]
    fn only_the_holder_of_the_token_is_served() {
        let server = server();
        let (status, _) = post(
            &server,
            "wrong",
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        );
        assert_eq!(status, "HTTP/1.1 401 Unauthorized");
        assert!(server.url().starts_with("http://127.0.0.1:"));
        assert_eq!(server.token().len(), 32);
    }
}
