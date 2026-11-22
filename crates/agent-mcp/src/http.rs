//! MCP's Streamable HTTP transport: one POST per request, the answer in the
//! response body, the session in a header.
//!
//! The stdio client keeps a process and a pair of pipes; this keeps a URL and
//! an agent, and answers the same `request`/`notify` pair, so
//! [`McpTransport`](crate::client::McpTransport) and everything built on it —
//! the handshake, `tools/list`, the tools themselves — are written once.
//! Blocking `ureq` on the caller's thread, like the rest of the agent:
//! `Connections` already runs a thread per server, so nothing here needs to be
//! async and no tokio joins the workspace for it.
//!
//! The shape, as the 2025-06-18 revision defines it:
//!
//! - every request is a `POST` to the server's URL carrying one JSON-RPC
//!   message, offering `Accept: application/json, text/event-stream` because
//!   the server may answer either way;
//! - `initialize`'s answer may carry `Mcp-Session-Id`; every later request
//!   sends it back, since without it a session-keeping server answers 404;
//!   a 404 to a request that carried it means the server dropped the
//!   session, so a new one is opened with a fresh `initialize` and the
//!   request sent again in it;
//! - once the version is agreed, every request carries `MCP-Protocol-Version`;
//! - a notification is owed `202 Accepted` with no body, so there is nothing to
//!   parse and nothing to wait for;
//! - a `text/event-stream` answer is read until the event whose `id` is the
//!   request's; the server's own messages ahead of it are handled on the way;
//! - after the handshake a `GET` opens the stream the server pushes on —
//!   `notifications/tools/list_changed` above all — held on a thread of its
//!   own and opened again with `Last-Event-ID` when it drops; a server that
//!   offers none answers `405`.
//!
//! A server may answer `401` with a `WWW-Authenticate` naming an OAuth
//! authorization server. A static `Authorization` in `headers` is then
//! wrong, and reported as such; with none, the sign-in `/mcp login` kept for
//! the server is sent and renewed when it lapses (see [`crate::oauth`]).

use std::io::{BufRead, BufReader, Read};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use termide_agent_core::{expand_env, CancelToken, McpServerConfig};

use crate::client::{answer_server_request, cancelled, wait, McpTransport, OnMessage, WaitError};
use crate::oauth::{self, Grant, TokenStore};

/// The largest answer accepted. A tool's result is text a model reads, so
/// this sits far above any honest answer and only bounds a runaway server.
const MAX_BODY: u64 = 8 << 20;

/// A push stream silent this long is dropped and opened again: a connection
/// a proxy forgot looks exactly like a quiet server, and this is the only way
/// to tell them apart. It also bounds how long a stream outlives its client.
const STREAM_IDLE: Duration = Duration::from_secs(300);

/// The longest pause between attempts to reopen a push stream.
const STREAM_BACKOFF_MAX: Duration = Duration::from_secs(60);

/// A server at a URL, spoken to over Streamable HTTP.
pub struct HttpClient {
    inner: Arc<Inner>,
    /// Set when the client goes: the push stream stops at its next wake.
    stop: Arc<AtomicBool>,
}

/// What the requests and the push stream share.
struct Inner {
    name: String,
    url: String,
    /// Expanded once, at the start: a header is sent on every request, and
    /// reading the environment per request would let a change mid-session
    /// split a session across two keys.
    headers: Vec<(String, String)>,
    timeout: Duration,
    agent: ureq::Agent,
    /// The `Mcp-Session-Id` the server gave at `initialize`; empty until it
    /// does, and required on every request once it has.
    session: Mutex<String>,
    /// The protocol version the server agreed to, echoed as
    /// `MCP-Protocol-Version` from then on.
    protocol: Mutex<String>,
    next_id: AtomicU64,
    on_message: Mutex<Option<OnMessage>>,
    /// The OAuth sign-in the server is reached with; absent when `headers`
    /// carry a static `Authorization`, which is then the whole sign-in.
    auth: Option<Auth>,
    /// The server refused us and no kept sign-in could change its mind.
    needs_login: AtomicBool,
    /// The params of the `initialize` that opened the session, to open a
    /// new one with when the server drops it.
    init_params: Mutex<Option<Value>>,
    /// Held while a new session is opened, so requests that all find the
    /// old one gone open one between them, not one each.
    reopening: Mutex<()>,
}

struct Auth {
    store: Arc<TokenStore>,
    /// The grant in use; read from the store when absent, so a sign-in made
    /// elsewhere is picked up.
    grant: Mutex<Option<Grant>>,
}

/// A request that did not get an answer: refused for want of a sign-in, or
/// for any other reason.
enum PostError {
    Unauthorized(String),
    /// `404` to a request that carried this `Mcp-Session-Id`: the server
    /// dropped the session, and a new one is to be opened.
    SessionGone(String, String),
    Other(String),
}

impl PostError {
    fn into_message(self) -> String {
        match self {
            Self::Unauthorized(message) | Self::SessionGone(message, _) | Self::Other(message) => {
                message
            }
        }
    }
}

impl HttpClient {
    /// Prepare to speak to `config`'s URL with no OAuth sign-in. See
    /// [`HttpClient::with_tokens`].
    ///
    /// # Errors
    ///
    /// As for [`HttpClient::with_tokens`].
    pub fn new(name: &str, config: &McpServerConfig) -> Result<Self, String> {
        Self::with_tokens(name, config, None)
    }

    /// Prepare to speak to `config`'s URL. `$NAME` in a header value comes
    /// from termide's environment, so a token stays out of the file; headers
    /// are named in the log but never written to it. With `tokens`, a server
    /// with no static `Authorization` is sent the sign-in kept there, renewed
    /// when it lapses.
    ///
    /// # Errors
    ///
    /// When the config is not a URL server, when the URL's scheme is not
    /// `https` (or `http` on the loopback), or when a header name is not a
    /// token.
    pub fn with_tokens(
        name: &str,
        config: &McpServerConfig,
        tokens: Option<Arc<TokenStore>>,
    ) -> Result<Self, String> {
        let url = match config.target() {
            Ok(termide_agent_core::McpTarget::Http { url }) => url.to_string(),
            Ok(termide_agent_core::McpTarget::Stdio { .. }) => {
                return Err(format!("{name}: no url to reach"))
            }
            Err(reason) => return Err(format!("{name}: {reason}")),
        };
        check_url(&url)?;
        let headers = expand_headers(&config.headers)?;
        for (key, _) in &headers {
            // The name only: the value is the secret.
            log::debug!("mcp {name}: sends {key}");
        }
        let static_key = headers
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case("authorization"));
        let auth = tokens.filter(|_| !static_key).map(|store| Auth {
            grant: Mutex::new(store.get(&url)),
            store,
        });
        let timeout = Duration::from_secs(config.timeout_secs);
        Ok(Self {
            inner: Arc::new(Inner {
                name: name.to_string(),
                url,
                headers,
                timeout,
                agent: termide_http::agent_builder()
                    .timeout_connect(timeout)
                    .timeout_read(timeout)
                    .timeout_write(timeout)
                    .user_agent(concat!("termide-agent/", env!("CARGO_PKG_VERSION")))
                    .build(),
                session: Mutex::new(String::new()),
                protocol: Mutex::new(crate::client::PROTOCOL_VERSION.to_string()),
                next_id: AtomicU64::new(1),
                on_message: Mutex::new(None),
                auth,
                needs_login: AtomicBool::new(false),
                init_params: Mutex::new(None),
                reopening: Mutex::new(()),
            }),
            stop: Arc::new(AtomicBool::new(false)),
        })
    }

    /// The server refused us for want of a sign-in, and none is kept that it
    /// takes: `/mcp login` is what would let us in.
    #[must_use]
    pub fn needs_login(&self) -> bool {
        self.inner.needs_login.load(Ordering::Relaxed)
    }
}

impl Inner {
    /// The token to send, renewed first when it has lapsed. A renewal that
    /// fails forgets the grant: it will not work again.
    fn bearer(&self) -> Option<String> {
        let auth = self.auth.as_ref()?;
        let mut grant = auth.grant.lock().unwrap_or_else(PoisonError::into_inner);
        if grant.is_none() {
            *grant = auth.store.get(&self.url);
        }
        let lapsed = grant
            .as_ref()
            .is_some_and(|g| !g.is_fresh() && g.refresh_token.is_some());
        if lapsed {
            let renewed = grant
                .as_ref()
                .map(|g| oauth::refresh(&self.agent, &self.url, g));
            *grant = self.keep(auth, renewed);
        }
        grant.as_ref().map(|g| g.access_token.clone())
    }

    /// After a refusal of `sent`: renew the grant if that can help. `true`
    /// when a retry is worth making.
    fn renew(&self, sent: Option<&str>) -> bool {
        let Some(auth) = &self.auth else {
            return false;
        };
        let mut grant = auth.grant.lock().unwrap_or_else(PoisonError::into_inner);
        if grant.is_none() {
            *grant = auth.store.get(&self.url);
        }
        match grant.as_ref() {
            None => return false,
            // Renewed meanwhile by another request, or signed in elsewhere.
            Some(g) if Some(g.access_token.as_str()) != sent => return true,
            Some(g) if g.refresh_token.is_none() => return false,
            Some(_) => {}
        }
        let renewed = grant
            .as_ref()
            .map(|g| oauth::refresh(&self.agent, &self.url, g));
        *grant = self.keep(auth, renewed);
        grant.is_some()
    }

    /// File a renewal's outcome: the new grant kept, a refused one forgotten.
    fn keep(&self, auth: &Auth, renewed: Option<Result<Grant, String>>) -> Option<Grant> {
        match renewed? {
            Ok(fresh) => {
                if let Err(error) = auth.store.put(&self.url, &fresh) {
                    log::warn!("mcp {}: {error}", self.name);
                }
                Some(fresh)
            }
            Err(error) => {
                log::warn!("mcp {}: cannot renew the sign-in: {error}", self.name);
                if let Err(error) = auth.store.remove(&self.url) {
                    log::warn!("mcp {}: {error}", self.name);
                }
                None
            }
        }
    }

    /// The token held now, not renewed: for the goodbye of a closing client.
    fn held_bearer(&self) -> Option<String> {
        let auth = self.auth.as_ref()?;
        let grant = auth.grant.lock().unwrap_or_else(PoisonError::into_inner);
        grant.as_ref().map(|g| g.access_token.clone())
    }

    /// The headers every request carries: the configured ones, the sign-in,
    /// the session and the protocol version.
    fn decorate(&self, mut call: ureq::Request, bearer: Option<&str>) -> ureq::Request {
        for (key, value) in &self.headers {
            call = call.set(key, value);
        }
        if let Some(token) = bearer {
            call = call.set("Authorization", &format!("Bearer {token}"));
        }
        let session = self
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if !session.is_empty() {
            call = call.set("Mcp-Session-Id", &session);
        }
        let protocol = self
            .protocol
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        call.set("MCP-Protocol-Version", &protocol)
    }

    /// One POST of one message, and the JSON-RPC message it answers with;
    /// once more after a refusal a renewed sign-in may cure.
    fn post(&self, message: &Value) -> Result<Value, String> {
        self.post_until(message, &|| false)
    }

    /// [`Self::post`], dropping a streamed answer between events once
    /// `stop` says nobody waits for it any more.
    ///
    /// A `404` to a request that carried the session means the server
    /// dropped it: as the specification asks, a new session is opened with
    /// a fresh `initialize` and the request is sent once more in it.
    fn post_until(&self, message: &Value, stop: &dyn Fn() -> bool) -> Result<Value, String> {
        match self.post_signed_in(message, stop) {
            Err(PostError::SessionGone(error, gone)) => {
                let is_initialize =
                    message.get("method").and_then(Value::as_str) == Some("initialize");
                if is_initialize {
                    return Err(error);
                }
                if let Err(reason) = self.reopen(&gone, stop) {
                    return Err(format!(
                        "{error}; a new session could not be opened: {reason}"
                    ));
                }
                self.post_signed_in(message, stop)
                    .map_err(PostError::into_message)
            }
            other => other.map_err(PostError::into_message),
        }
    }

    /// One POST, once more after a refusal a renewed sign-in may cure.
    fn post_signed_in(&self, message: &Value, stop: &dyn Fn() -> bool) -> Result<Value, PostError> {
        let sent = self.bearer();
        let refused = match self.post_once(message, sent.as_deref(), stop) {
            Err(PostError::Unauthorized(error)) => error,
            other => return other,
        };
        if self.auth.is_none() {
            return Err(PostError::Other(refused));
        }
        if self.renew(sent.as_deref()) {
            match self.post_once(message, self.bearer().as_deref(), stop) {
                Err(PostError::Unauthorized(_)) => {}
                other => return other,
            }
        }
        self.needs_login.store(true, Ordering::Relaxed);
        Err(PostError::Other(format!(
            "HTTP 401: the server wants a sign-in; run /mcp login {}",
            self.name
        )))
    }

    /// Open a new session in place of `gone`, the one the server dropped:
    /// `initialize` again, with no session id, then `initialized`. Nothing
    /// to do when another request already opened one.
    fn reopen(&self, gone: &str, stop: &dyn Fn() -> bool) -> Result<(), String> {
        let _reopening = self
            .reopening
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        {
            let mut session = self.session.lock().unwrap_or_else(PoisonError::into_inner);
            if *session != gone {
                return Ok(());
            }
            session.clear();
        }
        let params = self
            .init_params
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or("the session was never opened")?;
        *self.protocol.lock().unwrap_or_else(PoisonError::into_inner) =
            crate::client::PROTOCOL_VERSION.to_string();
        log::info!(
            "mcp {}: the server dropped the session; opening a new one",
            self.name
        );
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let reply = self
            .post_signed_in(
                &json!({ "jsonrpc": "2.0", "id": id, "method": "initialize", "params": params }),
                stop,
            )
            .map_err(PostError::into_message)?;
        if reply.get("error").is_some() {
            return Err(jsonrpc_error(&reply));
        }
        self.agree_protocol(&reply);
        self.post_signed_in(
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized", "params": {} }),
            stop,
        )
        .map_err(PostError::into_message)?;
        Ok(())
    }

    /// Take the protocol version an `initialize` answer agrees to.
    fn agree_protocol(&self, reply: &Value) {
        if let Some(version) = reply["result"]["protocolVersion"].as_str() {
            *self.protocol.lock().unwrap_or_else(PoisonError::into_inner) = version.to_string();
        }
    }

    fn post_once(
        &self,
        message: &Value,
        bearer: Option<&str>,
        stop: &dyn Fn() -> bool,
    ) -> Result<Value, PostError> {
        let call = self
            .agent
            .post(&self.url)
            .set("Content-Type", "application/json")
            // Either answer is allowed, so both are offered.
            .set("Accept", "application/json, text/event-stream");
        let sent_session = self
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let response = match self
            .decorate(call, bearer)
            .send_string(&message.to_string())
        {
            Ok(response) => response,
            // A notification is owed 202 with no body; a 200 or 204 says the
            // same thing, and neither is an error.
            Err(ureq::Error::Status(200 | 202 | 204, _)) => return Ok(Value::Null),
            Err(ureq::Error::Status(401, response)) => {
                return Err(PostError::Unauthorized(http_error(401, &brief(response))))
            }
            Err(ureq::Error::Status(404, response)) if !sent_session.is_empty() => {
                return Err(PostError::SessionGone(
                    http_error(404, &brief(response)),
                    sent_session,
                ))
            }
            Err(ureq::Error::Status(code, response)) => {
                return Err(PostError::Other(http_error(code, &brief(response))))
            }
            Err(error) => {
                return Err(PostError::Other(format!(
                    "cannot reach {}: {error}",
                    self.url
                )))
            }
        };
        // The header is readable only before the body is consumed.
        if let Some(session) = response.header("Mcp-Session-Id") {
            *self.session.lock().unwrap_or_else(PoisonError::into_inner) = session.to_string();
        }
        let content_type = response.content_type().to_ascii_lowercase();
        let id = message.get("id").and_then(Value::as_u64);
        if content_type.contains("text/event-stream") {
            return reply_from_stream(response.into_reader(), id, stop, |other| {
                self.dispatch(&other);
            })
            .map_err(PostError::Other);
        }
        let text = read_capped(response.into_reader()).map_err(PostError::Other)?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|error| {
            PostError::Other(format!(
                "the server answered with that is not JSON: {error}"
            ))
        })
    }

    /// Tell the server we stopped waiting for request `id`, so it can stop
    /// working on it. Best effort, on a thread of its own: the caller has
    /// already given up on this server once.
    fn cancel_on_server(self: &Arc<Self>, id: u64, why: &WaitError) {
        let inner = Arc::clone(self);
        let message = json!({
            "jsonrpc": "2.0", "method": "notifications/cancelled", "params": cancelled(id, why)
        });
        let spawned = std::thread::Builder::new()
            .name(format!("termide-mcp-{}-cancel", self.name))
            .spawn(move || {
                if let Err(error) = inner.post(&message) {
                    log::debug!("mcp {}: cannot cancel request {id}: {error}", inner.name);
                }
            });
        if let Err(error) = spawned {
            log::debug!("mcp: cannot cancel request {id}: {error}");
        }
    }

    /// A message the server sent unasked: a request is answered, a
    /// notification handed to the listener. A reply nobody waits for is
    /// dropped.
    fn dispatch(&self, message: &Value) {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return;
        };
        if message.get("id").is_some_and(|id| !id.is_null()) {
            let answer = answer_server_request(&message["id"], method);
            if let Err(error) = self.post(&answer) {
                log::debug!("mcp {}: cannot answer {method}: {error}", self.name);
            }
            return;
        }
        log::debug!("mcp {}: notification {method}", self.name);
        let handler = self
            .on_message
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(handler) = handler {
            handler(method, &message["params"]);
        }
    }

    /// Hold the GET stream a server pushes its own messages on, and open it
    /// again when it drops, resuming from the last event seen, until `stop`.
    /// A server that offers none answers `405`, and that is the end of it.
    fn push_stream(&self, stop: &AtomicBool) {
        let agent = termide_http::agent_builder()
            .timeout_connect(self.timeout)
            .timeout_read(STREAM_IDLE)
            .user_agent(concat!("termide-agent/", env!("CARGO_PKG_VERSION")))
            .build();
        let mut last_event: Option<String> = None;
        let mut backoff = Duration::from_secs(1);
        let mut renewed = false;
        while !stop.load(Ordering::Relaxed) {
            let sent = self.bearer();
            let mut call = self
                .decorate(agent.get(&self.url), sent.as_deref())
                .set("Accept", "text/event-stream");
            if let Some(id) = &last_event {
                call = call.set("Last-Event-ID", id);
            }
            let opened = Instant::now();
            match call.call() {
                Ok(response) if response.content_type().contains("text/event-stream") => {
                    renewed = false;
                    let mut events = Events::new(response.into_reader());
                    events.last_id = last_event.take();
                    while let Ok(Some(data)) = events.next() {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Ok(message) = serde_json::from_str::<Value>(&data) {
                            self.dispatch(&message);
                        }
                    }
                    last_event = events.last_id;
                    if opened.elapsed() > Duration::from_secs(30) {
                        backoff = Duration::from_secs(1);
                    }
                }
                Ok(_) => {
                    log::debug!("mcp {}: GET answered with no stream", self.name);
                    return;
                }
                Err(ureq::Error::Status(405, _)) => {
                    log::debug!("mcp {}: the server pushes nothing", self.name);
                    return;
                }
                Err(ureq::Error::Status(401, _)) if !renewed && self.auth.is_some() => {
                    renewed = true;
                    if self.renew(sent.as_deref()) {
                        continue;
                    }
                    return;
                }
                Err(ureq::Error::Status(code @ (401 | 403), _)) => {
                    log::debug!("mcp {}: the push stream was refused ({code})", self.name);
                    return;
                }
                Err(error) => log::debug!("mcp {}: push stream: {error}", self.name),
            }
            let until = Instant::now() + backoff;
            while Instant::now() < until {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            backoff = (backoff * 2).min(STREAM_BACKOFF_MAX);
        }
    }
}

impl McpTransport for HttpClient {
    /// One POST, and the `result` out of the answer it comes back with. The
    /// session and the protocol version are learned inside `post`.
    ///
    /// The POST runs on a thread of its own, so the wait for it notices a
    /// cancelled run and ends at the timeout even while a streamed answer
    /// keeps trickling in; the thread drops the stream at its next event.
    fn request(
        &self,
        method: &str,
        params: Value,
        cancel: Option<&CancelToken>,
    ) -> Result<Value, String> {
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err("aborted".to_string());
        }
        let inner = &self.inner;
        let id = inner.next_id.fetch_add(1, Ordering::Relaxed);
        if method == "initialize" {
            *inner
                .init_params
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(params.clone());
        }
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let (tx, rx) = mpsc::channel();
        let worker = Arc::clone(inner);
        let token = cancel.cloned();
        let deadline = Instant::now() + inner.timeout;
        let spawned = std::thread::Builder::new()
            .name(format!("termide-mcp-{}-request", inner.name))
            .spawn(move || {
                let stop = || {
                    token.as_ref().is_some_and(CancelToken::is_cancelled)
                        || Instant::now() >= deadline
                };
                let _ = tx.send(worker.post_until(&message, &stop));
            });
        if let Err(error) = spawned {
            return Err(format!("{method}: cannot start the request: {error}"));
        }
        let outcome = match wait(&rx, inner.timeout, cancel) {
            // The thread can see the cancel, or the deadline, before the wait
            // does and end with an error of its own.
            Err(WaitError::Failed(_)) if cancel.is_some_and(CancelToken::is_cancelled) => {
                Err(WaitError::Aborted)
            }
            Err(WaitError::Failed(_)) if Instant::now() >= deadline => {
                Err(WaitError::TimedOut(inner.timeout))
            }
            other => other,
        };
        let reply = match outcome {
            Ok(reply) => reply,
            Err(WaitError::Failed(error)) => return Err(error),
            Err(error) => {
                if error.gave_up() {
                    inner.cancel_on_server(id, &error);
                }
                return Err(format!("{method}: {error}"));
            }
        };
        match &reply {
            Value::Null => Err(format!("{method}: the server answered with no result")),
            reply if reply.get("error").is_some() => Err(jsonrpc_error(reply)),
            reply => {
                if method == "initialize" {
                    inner.agree_protocol(reply);
                }
                Ok(reply.get("result").cloned().unwrap_or(Value::Null))
            }
        }
    }

    /// One POST with no id: no answer is owed, and one that arrives anyway is
    /// ignored rather than trusted.
    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.inner
            .post(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))?;
        Ok(())
    }

    /// Keep the handler and open the push stream on a thread of its own.
    fn listen(&self, on_message: OnMessage) {
        *self
            .inner
            .on_message
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(on_message);
        let inner = Arc::clone(&self.inner);
        let stop = Arc::clone(&self.stop);
        let spawned = std::thread::Builder::new()
            .name(format!("termide-mcp-{}-stream", inner.name))
            .spawn(move || inner.push_stream(&stop));
        if let Err(error) = spawned {
            log::warn!("mcp: cannot start the push stream: {error}");
        }
    }
}

impl Drop for HttpClient {
    /// Stop the push stream and tell the server the session is over, so it
    /// drops it now instead of holding it until its own timeout — which also
    /// closes the stream from its end. Best effort on purpose: this runs when
    /// a panel closes, the session may already be gone, and a server that
    /// answers nothing is no worse off than one that was never reached.
    ///
    /// The `DELETE` goes out on a thread of its own: a panel is usually
    /// dropped on the UI thread, and a round trip per server there froze the
    /// close for seconds with a few remote servers. A process that exits
    /// meanwhile cuts it short, which the server's own timeout covers.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if self
            .inner
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
        {
            return;
        }
        let inner = Arc::clone(&self.inner);
        let spawned = std::thread::Builder::new()
            .name(format!("termide-mcp-{}-close", inner.name))
            .spawn(move || {
                let agent = termide_http::agent_builder()
                    .timeout_connect(Duration::from_secs(2))
                    .timeout_read(Duration::from_secs(2))
                    .build();
                let _ = inner
                    .decorate(agent.delete(&inner.url), inner.held_bearer().as_deref())
                    .call();
            });
        if let Err(error) = spawned {
            log::warn!("mcp: cannot end the session: {error}");
        }
    }
}

/// The events of a `text/event-stream`, capped as a JSON answer is: a server
/// that pushes forever must not grow the process until it dies.
struct Events<R: Read> {
    reader: BufReader<std::io::Take<R>>,
    /// The `id:` of the last event, for `Last-Event-ID` on a reconnect.
    last_id: Option<String>,
}

impl<R: Read> Events<R> {
    fn new(stream: R) -> Self {
        Self {
            reader: BufReader::new(stream.take(MAX_BODY)),
            last_id: None,
        }
    }

    /// The next event's data, its `data:` lines joined; `None` at the end of
    /// the stream. `event:`, `retry:` and comments carry nothing for us.
    fn next(&mut self) -> Result<Option<String>, String> {
        let mut data = String::new();
        let mut line = String::new();
        loop {
            line.clear();
            if self
                .reader
                .read_line(&mut line)
                .map_err(|error| format!("cannot read the stream: {error}"))?
                == 0
            {
                return Ok(None);
            }
            let trimmed = line.trim_end_matches(['\n', '\r']);
            if trimmed.is_empty() {
                // A blank line closes an event; one with no data is nothing.
                if data.is_empty() {
                    continue;
                }
                return Ok(Some(data));
            }
            if let Some(payload) = trimmed.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(payload.strip_prefix(' ').unwrap_or(payload));
            } else if let Some(id) = trimmed.strip_prefix("id:") {
                self.last_id = Some(id.trim_start().to_string());
            }
        }
    }
}

/// Read an event stream until the event answering `id`, handing every other
/// message to `other`: a server may send its notifications, and requests of
/// its own, ahead of the answer. A stream that ends first has no answer.
/// `stop` is asked after each event: a stream may run on well past the point
/// where nobody waits for its answer.
fn reply_from_stream(
    stream: impl Read,
    id: Option<u64>,
    stop: &dyn Fn() -> bool,
    mut other: impl FnMut(Value),
) -> Result<Value, String> {
    let mut events = Events::new(stream);
    while let Some(data) = events.next()? {
        if stop() {
            return Err("nobody waits for the answer any more".to_string());
        }
        let Ok(message) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        let answers = message.get("method").is_none()
            && message
                .get("id")
                .and_then(Value::as_u64)
                .is_some_and(|reply_id| Some(reply_id) == id);
        if answers {
            return Ok(message);
        }
        other(message);
    }
    Err("the stream ended before the answer arrived".to_string())
}

/// A short reason out of an answer that is not the reply: the server's own
/// words when it has any.
fn brief(response: ureq::Response) -> String {
    let text = read_capped(response.into_reader()).unwrap_or_default();
    text.trim().chars().take(200).collect()
}

fn http_error(code: u16, detail: &str) -> String {
    let hint = match code {
        401 | 403 => " (the headers carry no key this server accepts)".to_string(),
        404 => " (nothing here, or its session was dropped)".to_string(),
        410 => " (the session is gone; a new panel opens a new one)".to_string(),
        429 => " (the server is asking us to slow down)".to_string(),
        _ => String::new(),
    };
    if detail.is_empty() {
        format!("HTTP {code}{hint}")
    } else {
        format!("HTTP {code}{hint}: {detail}")
    }
}

fn jsonrpc_error(reply: &Value) -> String {
    let error = &reply["error"];
    format!(
        "{} (code {})",
        error["message"].as_str().unwrap_or("error"),
        error["code"]
    )
}

/// Read an answer up to [`MAX_BODY`], so a server that streams forever cannot
/// grow the process until it dies. A TLS close without `close_notify` is
/// tolerated, as in the web tools: the bytes that arrived are the answer.
fn read_capped(body: impl Read) -> Result<String, String> {
    let mut buf = Vec::new();
    // One byte past the cap tells an answer over it from one that fits.
    if let Err(error) = body.take(MAX_BODY + 1).read_to_end(&mut buf) {
        if error.kind() != std::io::ErrorKind::UnexpectedEof {
            return Err(format!("cannot read the answer: {error}"));
        }
    }
    if buf.len() as u64 > MAX_BODY {
        return Err(format!("the answer is over {} MiB", MAX_BODY / (1 << 20)));
    }
    String::from_utf8(buf).map_err(|_| "the answer is not UTF-8".to_string())
}

/// Expand `$NAME` in each header value from termide's environment, refusing a
/// name that is not a token rather than sending a mangled one.
fn expand_headers(
    headers: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<(String, String)>, String> {
    headers
        .iter()
        .map(|(key, value)| {
            let key = key.trim();
            let valid = !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
            if !valid {
                return Err(format!("{key:?} is not a header name"));
            }
            Ok((
                key.to_string(),
                expand_env(value, |name| std::env::var(name).ok()),
            ))
        })
        .collect()
}

/// `https` anywhere, `http` only on the loopback: these servers carry API
/// keys, and one sent across a network in the clear is not a default to hand
/// out.
fn check_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|error| format!("{url} is not a URL: {error}"))?;
    let host = parsed.host_str().unwrap_or("");
    match parsed.scheme() {
        "https" => Ok(()),
        "http"
            if host == "localhost"
                || host == "127.0.0.1"
                || host == "::1"
                || host.starts_with("127.") =>
        {
            Ok(())
        }
        other => Err(format!(
            "{other}:// is refused: an MCP server over http sends its key in the clear"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_or_loopback_is_reached() {
        assert!(check_url("https://mcp.plane.so/http/mcp").is_ok());
        assert!(check_url("http://127.0.0.1:8080/mcp").is_ok());
        assert!(check_url("http://localhost:8080/mcp").is_ok());
        assert!(check_url("http://mcp.plane.so/mcp").is_err());
        assert!(check_url("ftp://host/mcp").is_err());
        assert!(check_url("not a url").is_err());
    }

    #[test]
    fn header_names_are_checked_and_values_expanded() {
        std::env::set_var("TERMIDE_TEST_TOKEN", "s3cret");
        let headers: std::collections::BTreeMap<String, String> = [
            (
                "Authorization".to_string(),
                "Bearer $TERMIDE_TEST_TOKEN".to_string(),
            ),
            ("X-Redash-API-Key".to_string(), "plain".to_string()),
        ]
        .into_iter()
        .collect();
        let expanded = expand_headers(&headers).unwrap();
        assert_eq!(
            expanded[0],
            ("Authorization".into(), "Bearer s3cret".into())
        );
        assert_eq!(expanded[1].1, "plain");
        let bad: std::collections::BTreeMap<String, String> =
            [("bad name".to_string(), "x".to_string())]
                .into_iter()
                .collect();
        assert!(expand_headers(&bad).is_err());
    }

    #[test]
    fn a_stream_is_read_until_the_answer_in_it() {
        let stream = b"event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\r\n\r\n\
                      event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\r\n\r\n";
        let reply = reply_from_stream(stream as &[u8], Some(7), &|| false, |_| {}).unwrap();
        assert_eq!(reply["result"]["ok"], true);
        // An id that never comes is an ended stream, not a hang.
        let only_other = b"data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\r\n\r\n";
        assert!(reply_from_stream(only_other as &[u8], Some(9), &|| false, |_| {}).is_err());
    }

    #[test]
    fn a_multiline_event_is_joined_before_it_is_parsed() {
        let stream = b"data: {\"jsonrpc\":\"2.0\",\ndata: \"id\":4,\"result\":{\"a\":1}}\r\n\r\n";
        let reply = reply_from_stream(stream as &[u8], Some(4), &|| false, |_| {}).unwrap();
        assert_eq!(reply["result"]["a"], 1);
    }

    #[test]
    fn an_error_answer_carries_the_server_s_words() {
        assert!(http_error(401, "").contains("no key"));
        assert!(http_error(404, "nope").contains("nope"));
        let reply = json!({"error": {"code": -32601, "message": "no"}});
        assert_eq!(jsonrpc_error(&reply), "no (code -32601)");
    }

    #[test]
    fn a_url_server_is_built_from_its_config() {
        let config = McpServerConfig {
            command: None,
            url: Some("https://mcp.example.com/mcp".into()),
            headers: [("Authorization".to_string(), "Bearer x".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let client = HttpClient::new("example", &config).unwrap();
        assert_eq!(client.inner.headers[0].0, "Authorization");
        // A stdio config is not this client's to build.
        let stdio = McpServerConfig {
            command: Some("npx".into()),
            ..Default::default()
        };
        assert!(HttpClient::new("stdio", &stdio).is_err());
    }

    #[test]
    fn dropping_a_client_does_not_wait_for_the_server() {
        use std::io::Read;
        use std::net::TcpListener;

        // A server that takes the request and never answers it.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 16];
            let n = stream.read(&mut buf).unwrap_or(0);
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            std::thread::sleep(Duration::from_secs(5));
        });
        let config = McpServerConfig {
            url: Some(url),
            ..Default::default()
        };
        let client = HttpClient::new("silent", &config).unwrap();
        *client.inner.session.lock().unwrap() = "s1".into();

        let started = Instant::now();
        drop(client);
        assert!(started.elapsed() < Duration::from_millis(500));
        // The session is still ended, off the dropping thread.
        let request = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(request.starts_with("DELETE "), "{request}");
    }

    #[test]
    fn an_endless_answer_is_cut_at_the_cap() {
        let error = read_capped(std::io::repeat(b'x')).unwrap_err();
        assert!(error.contains("is over"), "{error}");
        assert_eq!(read_capped(&b"{}"[..]).unwrap(), "{}");
    }

    /// One HTTP request off `stream`: its `Mcp-Session-Id` and its body.
    fn read_request(stream: &mut std::net::TcpStream) -> (Option<String>, Value) {
        let mut reader = BufReader::new(stream);
        let mut session = None;
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                match key.to_ascii_lowercase().as_str() {
                    "mcp-session-id" => session = Some(value.trim().to_string()),
                    "content-length" => length = value.trim().parse().unwrap(),
                    _ => {}
                }
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        (
            session,
            serde_json::from_slice(&body).unwrap_or(Value::Null),
        )
    }

    /// A server that drops the session (404 to a request carrying it) is
    /// sent a new `initialize` with no session, then the request again.
    #[test]
    fn a_dropped_session_is_opened_again_and_the_request_retried() {
        use std::io::Write;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut sessions = 0;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let (session, body) = read_request(&mut stream);
                let method = body["method"].as_str().unwrap_or("").to_string();
                let _ = tx.send(format!("{method} {}", session.as_deref().unwrap_or("-")));
                let id = body["id"].clone();
                let (status, headers, reply) = match (method.as_str(), session.as_deref()) {
                    ("initialize", None) => {
                        sessions += 1;
                        (
                            "200 OK",
                            format!("Mcp-Session-Id: s{sessions}\r\n"),
                            json!({ "jsonrpc": "2.0", "id": id, "result": {
                                "protocolVersion": "2025-06-18", "serverInfo": { "name": "x" } } })
                            .to_string(),
                        )
                    }
                    ("notifications/initialized", _) => {
                        ("202 Accepted", String::new(), String::new())
                    }
                    // The first session is dropped as soon as it is used.
                    (_, Some("s1")) => ("404 Not Found", String::new(), String::new()),
                    (_, Some("s2")) => (
                        "200 OK",
                        String::new(),
                        json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": [] } })
                            .to_string(),
                    ),
                    _ => ("400 Bad Request", String::new(), String::new()),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        let config = McpServerConfig {
            url: Some(url),
            timeout_secs: 5,
            ..Default::default()
        };
        let client = HttpClient::new("dropping", &config).unwrap();
        client.initialize().unwrap();
        let result = client.request("tools/list", json!({}), None).unwrap();
        assert_eq!(result["tools"], json!([]));
        let seen: Vec<String> = rx.try_iter().collect();
        assert_eq!(
            seen,
            [
                "initialize -",
                "notifications/initialized s1",
                "tools/list s1",
                "initialize -",
                "notifications/initialized s2",
                "tools/list s2",
            ]
        );
        drop(client);
    }

    #[test]
    fn a_stream_is_dropped_between_events_once_nobody_waits() {
        let stream = b"data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n\
                      data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{}}\n\n";
        let error = reply_from_stream(stream as &[u8], Some(7), &|| true, |_| {}).unwrap_err();
        assert!(error.contains("nobody waits"), "{error}");
    }

    /// A tool call whose answer streams on without end stops at a cancel,
    /// and the server is told the request is cancelled.
    #[test]
    fn a_cancel_ends_a_streaming_answer_and_reaches_the_server() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let mut seen = Vec::new();
                    let mut buf = [0u8; 4096];
                    // Headers and the small body arrive together, or nearly.
                    stream
                        .set_read_timeout(Some(Duration::from_millis(300)))
                        .unwrap();
                    while let Ok(n) = stream.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        seen.extend_from_slice(&buf[..n]);
                        if seen.ends_with(b"}") {
                            break;
                        }
                    }
                    let request = String::from_utf8_lossy(&seen).into_owned();
                    if request.contains("notifications/cancelled") {
                        let _ = stream.write_all(
                            b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                        let _ = tx.send(request);
                        return;
                    }
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                    );
                    for _ in 0..100 {
                        let progress = b"data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n";
                        if stream.write_all(progress).is_err() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                });
            }
        });
        let config = McpServerConfig {
            url: Some(url),
            timeout_secs: 30,
            ..Default::default()
        };
        let client = HttpClient::new("streaming", &config).unwrap();
        let cancel = CancelToken::new();
        let canceller = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            canceller.cancel();
        });
        let started = Instant::now();
        let error = client
            .request("tools/call", json!({ "name": "slow" }), Some(&cancel))
            .unwrap_err();
        assert!(error.contains("aborted"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
        let notice = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(notice.contains("\"requestId\":1"), "{notice}");
    }
}
