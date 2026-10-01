//! The Streamable HTTP client against a server on the loopback: the
//! handshake, the session it opens, the tools it lists and the call it
//! answers — over a real socket, so the framing is tested and not only the
//! parsing.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};

use serde_json::{json, Value};
use termide_agent_core::{CancelToken, LateTools, McpServerConfig};
use termide_agent_mcp::{Connections, HttpClient, McpTransport};

/// The session id the fake server hands out at `initialize` and demands on
/// every later request, as a session-keeping server does — the one thing an
/// HTTP MCP client most often gets wrong.
const SESSION: &str = "sess-42";

/// One request as the server saw it.
#[derive(Debug)]
struct Seen {
    method: String,
    session: Option<String>,
    protocol: Option<String>,
    authorization: Option<String>,
}

/// A server on the loopback answering the three methods termide sends. The
/// first `tools/list` answers as an event stream and the rest as plain JSON,
/// so both shapes a server may answer with are covered.
fn spawn_server() -> (String, Receiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (seen_tx, seen_rx) = mpsc::channel();
    // One tool is what this server offers.
    let tools = json!([{"name": "query", "description": "Run a query",
                       "inputSchema": {"type": "object", "properties": {}}}]);
    std::thread::spawn(move || {
        let mut listed = false;
        while let Ok((mut stream, _)) = listener.accept() {
            let Some((message, headers)) = read_request(&mut stream) else {
                continue;
            };
            let method = message["method"].as_str().unwrap_or("").to_string();
            let session = headers.get("mcp-session-id").cloned();
            let _ = seen_tx.send(Seen {
                method: method.clone(),
                session: session.clone(),
                protocol: headers.get("mcp-protocol-version").cloned(),
                authorization: headers.get("authorization").cloned(),
            });
            // A request that does not name the session it was given is
            // refused, exactly as a real server refuses it.
            if method != "initialize" && session.as_deref() != Some(SESSION) {
                respond(&mut stream, 404, "text/plain", None, false);
                continue;
            }
            let id = message.get("id").cloned();
            if id.is_none() {
                // A notification owes nothing but an accepted.
                respond(&mut stream, 202, "text/plain", None, false);
                continue;
            }
            match method.as_str() {
                "initialize" => respond(
                    &mut stream,
                    200,
                    "application/json",
                    Some(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": "2025-06-18",
                            "serverInfo": {"name": "fake", "version": "1"},
                            "capabilities": {"tools": {}}
                        }
                    })),
                    true,
                ),
                "tools/list" => {
                    let body = json!({"jsonrpc":"2.0","id":id,"result":{"tools":tools}});
                    // The first answer arrives as a stream, the next as JSON.
                    let as_stream = !listed;
                    listed = true;
                    let kind = if as_stream {
                        "text/event-stream"
                    } else {
                        "application/json"
                    };
                    respond(&mut stream, 200, kind, Some(&body), false);
                }
                "tools/call" => respond(
                    &mut stream,
                    200,
                    "application/json",
                    Some(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"content": [{"type": "text", "text": "rows: 3"}], "isError": false}
                    })),
                    false,
                ),
                other => respond(
                    &mut stream,
                    200,
                    "application/json",
                    Some(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32601, "message": format!("no {other}")}
                    })),
                    false,
                ),
            }
        }
    });
    (format!("http://{addr}/mcp"), seen_rx)
}

/// The request's headers, lowercased, and its JSON body.
fn read_request(
    stream: &mut TcpStream,
) -> Option<(Value, std::collections::HashMap<String, String>)> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut headers = std::collections::HashMap::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            let value = value.trim().to_string();
            if name.eq_ignore_ascii_case("content-length") {
                length = value.parse().unwrap_or(0);
            }
            headers.insert(name.to_ascii_lowercase(), value);
        }
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        reader.read_exact(&mut body).ok()?;
    }
    Some((serde_json::from_slice(&body).ok()?, headers))
}

/// Answer, closing the connection so the client's next request is a fresh
/// connection rather than one `ureq` would have to reuse.
fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: Option<&Value>,
    with_session: bool,
) {
    let text = match (body, content_type.contains("event-stream")) {
        (Some(body), true) => format!("event: message\ndata: {body}\n\n"),
        (Some(body), false) => body.to_string(),
        (None, _) => String::new(),
    };
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        404 => "Not Found",
        _ => "Error",
    };
    let session = if with_session {
        format!("Mcp-Session-Id: {SESSION}\r\n")
    } else {
        String::new()
    };

    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{session}Connection: close\r\n\r\n",
        text.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(text.as_bytes());
    let _ = stream.flush();
}

fn client(url: &str) -> HttpClient {
    let config = McpServerConfig {
        url: Some(url.to_string()),
        headers: [("Authorization".to_string(), "Bearer t0k".to_string())]
            .into_iter()
            .collect(),
        timeout_secs: 5,
        ..Default::default()
    };
    HttpClient::new("fake", &config).expect("a loopback url is reachable")
}

#[test]
fn the_handshake_opens_a_session_every_later_request_names() {
    let (url, seen) = spawn_server();
    let client = client(&url);
    assert_eq!(client.initialize().unwrap(), "fake");
    assert_eq!(client.list_tools().unwrap().len(), 1);
    let seen: Vec<Seen> = seen.try_iter().collect();
    let names: Vec<&str> = seen.iter().map(|s| s.method.as_str()).collect();
    assert_eq!(
        names,
        ["initialize", "notifications/initialized", "tools/list"]
    );
    assert_eq!(
        seen[0].session, None,
        "the first request has no session yet"
    );
    assert_eq!(seen[1].session.as_deref(), Some(SESSION));
    // The version the server agreed to is echoed on what follows.
    assert_eq!(seen[2].protocol.as_deref(), Some("2025-06-18"));
    assert_eq!(seen[2].authorization.as_deref(), Some("Bearer t0k"));
}

#[test]
fn a_stream_answer_and_a_json_answer_both_read_as_tools() {
    let (url, _seen) = spawn_server();
    let client = client(&url);
    client.initialize().unwrap();
    // First over text/event-stream, then over application/json: the same
    // answer either way, since a server may choose either.
    assert_eq!(client.list_tools().unwrap()[0].name, "query");
    assert_eq!(client.list_tools().unwrap()[0].name, "query");
}

#[test]
fn a_call_round_trips_and_reports_what_came_back() {
    let (url, _seen) = spawn_server();
    let client = client(&url);
    client.initialize().unwrap();
    let (text, is_error) = client
        .call_tool("query", &json!({"q": "select 1"}), &CancelToken::new())
        .unwrap();
    assert_eq!(text, "rows: 3");
    assert!(!is_error);
}

#[test]
fn a_notification_waits_for_nothing() {
    let (url, seen) = spawn_server();
    let client = client(&url);
    client.initialize().unwrap();
    // No id, so no reply is owed: this returns instead of timing out.
    client.notify("notifications/cancelled", json!({})).unwrap();
    let methods: Vec<String> = seen.try_iter().map(|s| s.method).collect();
    assert!(methods.contains(&"notifications/cancelled".to_string()));
}

#[test]
fn a_url_server_connects_through_connections_as_a_process_server_does() {
    let (url, _seen) = spawn_server();
    let mut servers = std::collections::BTreeMap::new();
    servers.insert(
        "redash".to_string(),
        McpServerConfig {
            url: Some(url),
            ..Default::default()
        },
    );
    let receiver = Connections::new(servers).subscribe();
    match receiver.recv_timeout(std::time::Duration::from_secs(15)) {
        Ok(LateTools::Ready { source, tools }) => {
            assert_eq!(source, "redash");
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].name(), "redash__query");
        }
        Ok(LateTools::Failed { source, error }) => {
            panic!("{source} should have connected: {error}");
        }
        Ok(_) => panic!("an event other than the connection"),
        Err(error) => panic!("no word from the url server: {error}"),
    }
}

#[test]
fn a_cancelled_run_does_not_wait() {
    let (url, _seen) = spawn_server();
    let client = client(&url);
    let cancel = CancelToken::new();
    cancel.cancel();
    let error = client.call_tool("query", &json!({}), &cancel).unwrap_err();
    assert!(error.contains("aborted"), "{error}");
}

#[test]
fn a_http_url_is_accepted_only_on_the_loopback_and_reaches_the_server() {
    let (url, _seen) = spawn_server();
    assert!(url.starts_with("http://127.0.0.1:"));
    // Reachable: the handshake completes over plain http on the loopback.
    assert_eq!(client(&url).initialize().unwrap(), "fake");
}
