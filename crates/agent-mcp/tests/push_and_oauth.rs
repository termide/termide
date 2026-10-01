//! What a `url` server does after the handshake: push a change of tools on
//! its GET stream, refuse a client that has not signed in, and let one in
//! that went through OAuth — against a server on the loopback that keeps a
//! connection per request, as a real one does, so the stream stays open
//! while requests come and go.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use termide_agent_core::{
    CancelToken, LateTools, McpServerConfig, McpServerState, McpSignIn, McpStatus, Tool,
};
use termide_agent_mcp::{Connections, TokenStore};

const SESSION: &str = "sess-7";

/// One request as it arrived.
struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

#[derive(Default)]
struct State {
    /// How many tools `tools/list` answers with.
    tools: usize,
    /// The open GET streams, to push on.
    streams: Vec<TcpStream>,
    /// Whether `/mcp` wants a bearer token.
    oauth: bool,
    /// Authorization codes issued: code → PKCE challenge.
    codes: HashMap<String, String>,
    access: Vec<String>,
    refresh: Vec<String>,
    issued: usize,
    /// How many times the token endpoint renewed a token.
    renewals: usize,
}

struct Fake {
    base: String,
    state: Arc<Mutex<State>>,
    gets: Arc<AtomicUsize>,
}

impl Fake {
    fn start(oauth: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            tools: 1,
            oauth,
            ..State::default()
        }));
        let gets = Arc::new(AtomicUsize::new(0));
        let (server_state, server_gets, server_base) =
            (Arc::clone(&state), Arc::clone(&gets), base.clone());
        std::thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                let state = Arc::clone(&server_state);
                let gets = Arc::clone(&server_gets);
                let base = server_base.clone();
                std::thread::spawn(move || serve(stream, &state, &gets, &base));
            }
        });
        Self { base, state, gets }
    }

    fn url(&self) -> String {
        format!("{}/mcp", self.base)
    }

    /// Change the tools and say so on every open stream.
    fn change_tools(&self, count: usize) {
        let mut state = self.state.lock().unwrap();
        state.tools = count;
        let event = format!(
            "id: 1\ndata: {}\n\n",
            json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
        );
        state
            .streams
            .retain_mut(|stream| stream.write_all(event.as_bytes()).is_ok());
    }

    /// Every access token stops working, as when they expire.
    fn expire_tokens(&self) {
        self.state.lock().unwrap().access.clear();
    }
}

fn serve(mut stream: TcpStream, state: &Mutex<State>, gets: &AtomicUsize, base: &str) {
    let Some(request) = read_request(&stream) else {
        return;
    };
    let authorized = {
        let state = state.lock().unwrap();
        !state.oauth
            || request
                .headers
                .get("authorization")
                .and_then(|value| value.strip_prefix("Bearer "))
                .is_some_and(|token| state.access.iter().any(|t| t == token))
    };
    match (request.method.as_str(), request.path.as_str()) {
        (_, "/mcp") if !authorized => {
            let challenge = format!(
                "Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource/mcp\""
            );
            respond(
                &mut stream,
                401,
                "text/plain",
                "",
                &[("WWW-Authenticate", &challenge)],
            );
        }
        ("GET", "/mcp") => {
            gets.fetch_add(1, Ordering::SeqCst);
            if request.headers.get("mcp-session-id").map(String::as_str) != Some(SESSION) {
                respond(&mut stream, 404, "text/plain", "", &[]);
                return;
            }
            // Held open: what is pushed is written to it later.
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n";
            if stream.write_all(head.as_bytes()).is_ok() {
                state.lock().unwrap().streams.push(stream);
            }
        }
        ("DELETE", "/mcp") => respond(&mut stream, 200, "text/plain", "", &[]),
        ("POST", "/mcp") => {
            let message: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let Some(id) = message.get("id").cloned() else {
                respond(&mut stream, 202, "text/plain", "", &[]);
                return;
            };
            let result = match message["method"].as_str().unwrap_or("") {
                "initialize" => json!({
                    "protocolVersion": "2025-06-18",
                    "serverInfo": {"name": "fake", "version": "1"},
                    "capabilities": {"tools": {"listChanged": true}}
                }),
                "tools/list" => {
                    let count = state.lock().unwrap().tools;
                    let tools: Vec<Value> = (0..count)
                        .map(
                            |i| json!({"name": format!("t{i}"), "inputSchema": {"type": "object"}}),
                        )
                        .collect();
                    json!({ "tools": tools })
                }
                "tools/call" => json!({"content": [{"type": "text", "text": "done"}]}),
                _ => json!({}),
            };
            let body = json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
            respond(
                &mut stream,
                200,
                "application/json",
                &body,
                &[("Mcp-Session-Id", SESSION)],
            );
        }
        ("GET", "/.well-known/oauth-protected-resource/mcp") => {
            let body = json!({"resource": format!("{base}/mcp"), "authorization_servers": [base]});
            respond(&mut stream, 200, "application/json", &body.to_string(), &[]);
        }
        ("GET", "/.well-known/oauth-authorization-server") => {
            let body = json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "registration_endpoint": format!("{base}/register"),
                "code_challenge_methods_supported": ["S256"],
            });
            respond(&mut stream, 200, "application/json", &body.to_string(), &[]);
        }
        ("POST", "/register") => {
            let asked: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(asked["token_endpoint_auth_method"], "none");
            let body =
                json!({"client_id": "termide-client", "redirect_uris": asked["redirect_uris"]});
            respond(&mut stream, 201, "application/json", &body.to_string(), &[]);
        }
        // The user approves at once: back to the client with a code.
        ("GET", "/authorize") => {
            let q = &request.query;
            assert_eq!(q["response_type"], "code");
            assert_eq!(q["client_id"], "termide-client");
            assert_eq!(q["code_challenge_method"], "S256");
            assert_eq!(q["resource"], format!("{base}/mcp"));
            let code = "code-1".to_string();
            state
                .lock()
                .unwrap()
                .codes
                .insert(code.clone(), q["code_challenge"].clone());
            let location = format!("{}?code={code}&state={}", q["redirect_uri"], q["state"]);
            respond(
                &mut stream,
                302,
                "text/plain",
                "",
                &[("Location", &location)],
            );
        }
        ("POST", "/token") => {
            let form: HashMap<String, String> = url::form_urlencoded::parse(&request.body)
                .into_owned()
                .collect();
            let mut state = state.lock().unwrap();
            let valid = match form["grant_type"].as_str() {
                "authorization_code" => {
                    let challenge = state.codes.remove(&form["code"]);
                    let derived = base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .encode(Sha256::digest(form["code_verifier"].as_bytes()));
                    challenge == Some(derived)
                }
                "refresh_token" => {
                    let known = state.refresh.contains(&form["refresh_token"]);
                    state.renewals += usize::from(known);
                    known
                }
                _ => false,
            };
            if !valid {
                respond(
                    &mut stream,
                    400,
                    "application/json",
                    r#"{"error":"invalid_grant"}"#,
                    &[],
                );
                return;
            }
            state.issued += 1;
            let (access, refresh) = (
                format!("at-{}", state.issued),
                format!("rt-{}", state.issued),
            );
            state.access.push(access.clone());
            state.refresh.push(refresh.clone());
            let body = json!({"access_token": access, "token_type": "Bearer",
                              "refresh_token": refresh, "expires_in": 3600});
            respond(&mut stream, 200, "application/json", &body.to_string(), &[]);
        }
        _ => respond(&mut stream, 404, "text/plain", "", &[]),
    }
}

fn read_request(stream: &TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = url::Url::parse(&format!("http://x{}", parts.next()?)).ok()?;
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let length = headers
        .get("content-length")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        path: target.path().to_string(),
        query: target.query_pairs().into_owned().collect(),
        headers,
        body,
    })
}

fn respond(stream: &mut TcpStream, status: u16, kind: &str, body: &str, extra: &[(&str, &str)]) {
    let mut head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn servers(name: &str, url: &str) -> BTreeMap<String, McpServerConfig> {
    [(
        name.to_string(),
        McpServerConfig {
            url: Some(url.to_string()),
            timeout_secs: 5,
            ..Default::default()
        },
    )]
    .into_iter()
    .collect()
}

fn next(events: &Receiver<LateTools>) -> LateTools {
    events
        .recv_timeout(Duration::from_secs(15))
        .expect("an event in time")
}

fn ready(event: LateTools) -> Vec<Arc<dyn Tool>> {
    match event {
        LateTools::Ready { tools, .. } => tools,
        LateTools::Failed { error, .. } => panic!("failed: {error}"),
        LateTools::NeedsLogin { .. } => panic!("asked for a sign-in"),
        LateTools::Gone { .. } => panic!("gone"),
        LateTools::LoginStarted { .. } => panic!("a sign-in started"),
    }
}

/// Wait until the server holds `count` push streams open.
fn wait_for_streams(fake: &Fake, count: usize) {
    for _ in 0..200 {
        if fake.state.lock().unwrap().streams.len() >= count {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("the client opened no push stream");
}

#[test]
fn a_change_pushed_on_the_stream_brings_the_new_tools() {
    let fake = Fake::start(false);
    let connections = Connections::new(servers("live", &fake.url()));
    let events = connections.subscribe();
    assert_eq!(ready(next(&events)).len(), 1);
    wait_for_streams(&fake, 1);
    assert!(fake.gets.load(Ordering::SeqCst) >= 1);

    fake.change_tools(3);
    let tools = ready(next(&events));
    let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
    assert_eq!(names, ["live__t0", "live__t1", "live__t2"]);
    assert_eq!(
        connections.status(),
        [McpServerState {
            name: "live".into(),
            status: McpStatus::Ready { tools: 3 },
            sign_in: McpSignIn::None,
        }]
    );
}

#[test]
fn a_reload_drops_what_left_and_connects_what_came() {
    let (first, second) = (Fake::start(false), Fake::start(false));
    let connections = Connections::new(servers("a", &first.url()));
    let events = connections.subscribe();
    assert_eq!(ready(next(&events)).len(), 1);

    // Unchanged and connected: kept, no event.
    let report = connections.reload(servers("a", &first.url()));
    assert_eq!(report.kept, ["a"]);
    assert!(report.started.is_empty() && report.removed.is_empty());

    // "a" leaves, "b" arrives.
    let report = connections.reload(servers("b", &second.url()));
    assert_eq!(
        (report.removed, report.started),
        (vec!["a".to_string()], vec!["b".to_string()])
    );
    let mut seen = vec![next(&events), next(&events)];
    seen.sort_by_key(|event| event.source().to_string());
    assert!(matches!(&seen[0], LateTools::Gone { source } if source == "a"));
    assert_eq!(ready(seen.remove(1))[0].name(), "b__t0");

    // One server again, unchanged or not: it connects anew and nothing else
    // moves; one no longer configured goes.
    let config = servers("b", &second.url());
    let report = connections.reconnect("b", &config).unwrap();
    assert_eq!(report.started, ["b"]);
    assert_eq!(ready(next(&events))[0].name(), "b__t0");
    assert!(connections.reconnect("nope", &config).is_err());
    let report = connections.reconnect("b", &BTreeMap::new()).unwrap();
    assert_eq!(report.removed, ["b"]);
    assert!(matches!(next(&events), LateTools::Gone { source } if source == "b"));
    assert!(connections.status().is_empty());
}

#[test]
fn a_server_behind_oauth_is_signed_in_to_and_renewed() {
    let fake = Fake::start(true);
    let dir = tempfile::tempdir().unwrap();
    let store_path = dir.path().join("mcp-auth.json");
    // The browser: follow the authorization address, which the fake
    // approves at once and sends back to the loopback.
    let opened = Arc::new(Mutex::new(Vec::new()));
    let browser = Arc::clone(&opened);
    let connections = Connections::with_opener(
        servers("plane", &fake.url()),
        TokenStore::new(store_path.clone()),
        move |url: &str| {
            browser.lock().unwrap().push(url.to_string());
            let url = url.to_string();
            std::thread::spawn(move || {
                let _ = ureq::get(&url).call();
            });
        },
    );
    let events = connections.subscribe();
    assert!(matches!(next(&events), LateTools::NeedsLogin { source } if source == "plane"));
    let state = &connections.status()[0];
    assert_eq!(
        (&state.status, state.sign_in),
        (&McpStatus::NeedsLogin, McpSignIn::SignedOut)
    );

    connections.login("plane").unwrap();
    assert!(connections.login("plane").is_err(), "one sign-in at a time");
    let LateTools::LoginStarted { url, .. } = next(&events) else {
        panic!("the sign-in did not start");
    };
    assert!(url.starts_with(&format!("{}/authorize?", fake.base)));
    let tools = ready(next(&events));
    assert_eq!(tools[0].name(), "plane__t0");
    // The browser was opened once, on the address the panel was shown.
    assert_eq!(*opened.lock().unwrap(), [url]);
    let kept = TokenStore::new(store_path.clone())
        .get(&fake.url())
        .expect("the grant is kept");
    assert_eq!(kept.client_id, "termide-client");
    assert_eq!(connections.status()[0].sign_in, McpSignIn::SignedIn);

    // The token lapses on the server: the next call renews it and goes on.
    fake.expire_tokens();
    let call = termide_agent_core::ToolCall {
        id: "1".into(),
        name: tools[0].name().to_string(),
        arguments: json!({}),
        extra_content: None,
    };
    let ctx = termide_agent_core::ToolContext::new(dir.path());
    let result = tools[0].execute(&call, &ctx, &mut |_| {}, &CancelToken::new());
    assert!(!result.is_error, "{result:?}");
    assert_eq!(fake.state.lock().unwrap().renewals, 1);
    assert_ne!(
        TokenStore::new(store_path.clone())
            .get(&fake.url())
            .unwrap()
            .access_token,
        kept.access_token
    );

    // Signed out, the server is back to refusing us.
    assert!(connections.logout("plane").unwrap());
    assert!(matches!(next(&events), LateTools::NeedsLogin { .. }));
    assert!(TokenStore::new(store_path).get(&fake.url()).is_none());
}

#[test]
fn a_new_panel_finds_the_sign_in_already_kept() {
    let fake = Fake::start(true);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp-auth.json");
    let first = Connections::with_opener(
        servers("s", &fake.url()),
        TokenStore::new(path.clone()),
        |url: &str| {
            let url = url.to_string();
            std::thread::spawn(move || {
                let _ = ureq::get(&url).call();
            });
        },
    );
    let events = first.subscribe();
    assert!(matches!(next(&events), LateTools::NeedsLogin { .. }));
    first.login("s").unwrap();
    assert!(matches!(next(&events), LateTools::LoginStarted { .. }));
    ready(next(&events));

    // Another panel: no browser this time.
    let second = Connections::with_opener(
        servers("s", &fake.url()),
        TokenStore::new(path),
        |_: &str| {
            panic!("no sign-in should be needed");
        },
    );
    assert_eq!(ready(next(&second.subscribe())).len(), 1);
}
