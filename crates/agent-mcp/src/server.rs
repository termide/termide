//! MCP server of termide: its own tools, served to an external agent, so the
//! agent's model calls them and termide runs them — under its hooks, with its
//! permission decisions and its undo checkpoints — rather than the agent's
//! own tools running out of termide's sight.
//!
//! The transport is MCP's Streamable HTTP in its simplest form, on the
//! loopback only: every JSON-RPC message is a `POST`, a request gets its
//! response as the body, a notification gets `202`. There is no server
//! stream (`GET` is refused, which the protocol allows). A bearer token,
//! handed to the agent with the URL, keeps other local processes out.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde_json::{json, Value};
use termide_agent_core::{execute_tool, CancelToken, Hooks, ToolCall, ToolRegistry};

use crate::client::PROTOCOL_VERSION;

/// The name the agent knows the server by; its tools reach the model as
/// `mcp__termide__<tool>`.
pub const SERVER_NAME: &str = "termide";

/// The largest request body read; a tool call's arguments are far smaller.
const MAX_BODY: usize = 16 * 1024 * 1024;

struct Shared {
    tools: ToolRegistry,
    /// One call at a time goes through the hooks: they may block on a
    /// permission card, and the panel answers one question at a time.
    hooks: Mutex<Box<dyn Hooks + Send>>,
    cwd: PathBuf,
    token: String,
    /// The cancel token of each call in flight, by its JSON-RPC id, for
    /// `notifications/cancelled`.
    running: Mutex<HashMap<String, CancelToken>>,
    stop: AtomicBool,
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
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            tools,
            hooks: Mutex::new(hooks),
            cwd,
            token: new_token(),
            running: Mutex::new(HashMap::new()),
            stop: AtomicBool::new(false),
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
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        for cancel in self.shared.running.lock().unwrap().values() {
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
    if request.method != "POST" {
        return respond(&mut stream, "405 Method Not Allowed", "");
    }
    let Ok(message) = serde_json::from_slice::<Value>(&request.body) else {
        let error = rpc_error(Value::Null, -32700, "parse error");
        return respond(&mut stream, "400 Bad Request", &error.to_string());
    };
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
            if let Some(cancel) = shared.running.lock().unwrap().get(&request) {
                cancel.cancel();
            }
        }
        return None;
    };
    let params = &message["params"];
    let result = match method {
        "initialize" => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        }),
        "ping" => json!({}),
        "tools/list" => {
            let tools: Vec<Value> = shared
                .tools
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
        "tools/call" => call_tool(shared, &id, params),
        _ => return Some(rpc_error(id, -32601, "method not found")),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// Run a `tools/call` the way the built-in loop runs a call.
fn call_tool(shared: &Shared, id: &Value, params: &Value) -> Value {
    let call = ToolCall {
        id: format!("mcp-{}", id.to_string().trim_matches('"')),
        name: params["name"].as_str().unwrap_or("").to_string(),
        arguments: match &params["arguments"] {
            Value::Object(_) => params["arguments"].clone(),
            _ => json!({}),
        },
    };
    let cancel = CancelToken::new();
    let key = id.to_string();
    shared
        .running
        .lock()
        .unwrap()
        .insert(key.clone(), cancel.clone());
    let result = {
        let mut hooks = shared.hooks.lock().unwrap();
        let result = execute_tool(
            &shared.tools,
            &call,
            hooks.as_mut(),
            &shared.cwd,
            &cancel,
            &mut |_| {},
        );
        hooks.after_tool_call(&call, result)
    };
    shared.running.lock().unwrap().remove(&key);
    json!({
        "content": [{ "type": "text", "text": result.plain_text() }],
        "isError": result.is_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use termide_agent_core::{Tool, ToolContext, ToolDecision, ToolResultMessage, ToolUpdate};

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
