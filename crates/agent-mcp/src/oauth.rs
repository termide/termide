//! Signing in to a `url` server that answers `401` and names an OAuth
//! authorization server, as the MCP authorization specification lays it out:
//!
//! 1. the server's protected-resource metadata (RFC 9728) — from the
//!    `resource_metadata` of its `WWW-Authenticate`, or the well-known path —
//!    names the authorization server; a server too old to publish it is its
//!    own;
//! 2. that server's metadata (RFC 8414, or OpenID discovery) gives the
//!    endpoints;
//! 3. without a configured `client_id`, termide registers itself (RFC 7591)
//!    as a public client;
//! 4. the browser opens on the authorization endpoint with PKCE (`S256`) and
//!    `resource` (RFC 8707), and returns to a one-shot listener on the
//!    loopback (RFC 8252);
//! 5. the code is exchanged for tokens, which are kept and refreshed.
//!
//! The grants are kept in one JSON file in termide's configuration directory,
//! readable by the user only, keyed by the server's URL: the same server
//! configured in two projects is signed in to once. Claude Code keeps them in
//! the system keychain and Codex in a file or the keyring; a file is what
//! works the same on every system termide runs on, with no service to depend
//! on, and it is next to the configuration that already holds static keys.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use termide_agent_core::{expand_env, McpOAuth};

/// How long the browser has to come back. Long enough to type a password
/// and pass a second factor, short enough that a forgotten tab does not hold
/// a port for the rest of the day.
const LOGIN_WAIT: Duration = Duration::from_secs(300);

/// A token this close to its expiry is refreshed before it is sent, so a
/// request does not leave with one that lapses on the way.
const EXPIRY_MARGIN_SECS: u64 = 30;

/// The path the browser returns to on the loopback.
const CALLBACK_PATH: &str = "/callback";

/// Read-modify-write of the grants file is serialised within the process;
/// two termide processes writing at once is the remaining race, and the cost
/// of losing it is one more sign-in.
static STORE_LOCK: Mutex<()> = Mutex::new(());

/// What one sign-in left: enough to send the token and to renew it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Seconds since the epoch; absent when the server did not say.
    #[serde(default)]
    pub expires_at: Option<u64>,
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    /// `client_secret_basic` or `client_secret_post` for a confidential
    /// client; `none` for the public one termide registers.
    #[serde(default)]
    pub auth_method: Option<String>,
}

impl Grant {
    /// Still good to send, by what the server said of its lifetime.
    #[must_use]
    pub fn is_fresh(&self) -> bool {
        self.expires_at
            .is_none_or(|at| now_secs() + EXPIRY_MARGIN_SECS < at)
    }
}

/// The grants file. Absent until the first sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenStore {
    path: PathBuf,
}

impl TokenStore {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The grant kept for the server at `url`.
    #[must_use]
    pub fn get(&self, url: &str) -> Option<Grant> {
        let _guard = STORE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        self.read().remove(&resource_key(url))
    }

    /// Keep `grant` for the server at `url`.
    ///
    /// # Errors
    ///
    /// When the file cannot be written.
    pub fn put(&self, url: &str, grant: &Grant) -> Result<(), String> {
        let _guard = STORE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let mut grants = self.read();
        grants.insert(resource_key(url), grant.clone());
        self.write(&grants)
    }

    /// Forget the server at `url`; `true` when there was something to forget.
    ///
    /// # Errors
    ///
    /// When the file cannot be written.
    pub fn remove(&self, url: &str) -> Result<bool, String> {
        let _guard = STORE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let mut grants = self.read();
        if grants.remove(&resource_key(url)).is_none() {
            return Ok(false);
        }
        self.write(&grants).map(|()| true)
    }

    fn read(&self) -> BTreeMap<String, Grant> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return BTreeMap::new();
        };
        serde_json::from_str(&text).unwrap_or_else(|error| {
            log::warn!("ignoring {}: {error}", self.path.display());
            BTreeMap::new()
        })
    }

    /// Through a temporary file renamed into place, created readable by the
    /// user only: the file holds live tokens.
    fn write(&self, grants: &BTreeMap<String, Grant>) -> Result<(), String> {
        let fail = |error: std::io::Error| format!("cannot write {}: {error}", self.path.display());
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        let text = serde_json::to_string_pretty(grants).map_err(|error| error.to_string())?;
        let temp = self.path.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(fail)?;
        file.write_all(text.as_bytes()).map_err(fail)?;
        file.sync_all().map_err(fail)?;
        std::fs::rename(&temp, &self.path).map_err(fail)
    }
}

/// The server's URL as the grant is filed under and as `resource` is sent:
/// without a fragment, scheme and host in lower case as the URL parser
/// leaves them.
#[must_use]
pub fn resource_key(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut parsed) => {
            parsed.set_fragment(None);
            parsed.to_string()
        }
        Err(_) => url.to_string(),
    }
}

/// Renew `grant` with its refresh token. A token endpoint that does not send
/// a new refresh token leaves the old one in force.
///
/// # Errors
///
/// When there is no refresh token, or the endpoint refuses it.
pub fn refresh(agent: &ureq::Agent, url: &str, grant: &Grant) -> Result<Grant, String> {
    let Some(refresh_token) = &grant.refresh_token else {
        return Err("the sign-in has lapsed and holds no refresh token".to_string());
    };
    let resource = resource_key(url);
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.as_str()),
        ("resource", resource.as_str()),
    ];
    let answer = token_request(agent, grant, &mut form)?;
    let mut renewed = grant_from(&answer, grant)?;
    if renewed.refresh_token.is_none() {
        renewed.refresh_token = grant.refresh_token.clone();
    }
    Ok(renewed)
}

/// What a sign-in needs from the caller.
pub struct Login<'a> {
    pub url: &'a str,
    pub oauth: &'a McpOAuth,
    pub agent: &'a ureq::Agent,
    /// Called once with the address the user is sent to: it opens the
    /// browser, and the panel shows it for a browser that did not open.
    pub on_url: &'a dyn Fn(&str),
    /// Set to give up waiting for the browser.
    pub cancel: &'a AtomicBool,
}

/// Run the whole flow for the server at `login.url` and return the grant.
///
/// # Errors
///
/// The step that failed, in words the panel shows.
pub fn login(login: &Login<'_>) -> Result<Grant, String> {
    let challenge = probe(login.agent, login.url)?;
    let (issuer, mut scopes) = protected_resource(login.agent, login.url, &challenge);
    let server = authorization_server(login.agent, &issuer);
    if let Some(methods) = &server.pkce_methods {
        if !methods.iter().any(|m| m == "S256") {
            return Err(format!(
                "{issuer} does not offer PKCE with S256, which every sign-in here uses"
            ));
        }
    }
    if let Some(wanted) = &login.oauth.scopes {
        scopes = Some(wanted.join(" "));
    }

    let port = login.oauth.callback_port.unwrap_or(0);
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|error| format!("cannot listen on 127.0.0.1:{port} for the browser: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let redirect = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");

    let client = match &login.oauth.client_id {
        Some(id) => Client {
            id: id.clone(),
            secret: login
                .oauth
                .client_secret
                .as_deref()
                .map(|secret| expand_env(secret, |name| std::env::var(name).ok())),
            auth_method: None,
        },
        None => {
            let Some(endpoint) = &server.registration_endpoint else {
                return Err(format!(
                    "{issuer} does not register clients itself; \
                     set oauth.client_id for this server"
                ));
            };
            register(login.agent, endpoint, &redirect, scopes.as_deref())?
        }
    };

    let verifier = random_token();
    let state = random_token();
    let resource = resource_key(login.url);
    let mut authorize = url::Url::parse(&server.authorization_endpoint)
        .map_err(|error| format!("bad authorization endpoint: {error}"))?;
    {
        let mut query = authorize.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &client.id)
            .append_pair("redirect_uri", &redirect)
            .append_pair("code_challenge", &pkce_challenge(&verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("resource", &resource);
        if let Some(scopes) = &scopes {
            query.append_pair("scope", scopes);
        }
    }
    (login.on_url)(authorize.as_str());

    let code = wait_for_code(&listener, &state, login.cancel)?;
    let pending = Grant {
        access_token: String::new(),
        refresh_token: None,
        expires_at: None,
        token_endpoint: server.token_endpoint.clone(),
        client_id: client.id,
        client_secret: client.secret,
        auth_method: client.auth_method,
    };
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect.as_str()),
        ("code_verifier", verifier.as_str()),
        ("resource", resource.as_str()),
    ];
    let answer = token_request(login.agent, &pending, &mut form)?;
    grant_from(&answer, &pending)
}

/// The challenge of a server that wants a sign-in: its `WWW-Authenticate`,
/// asked with an `initialize` it must refuse.
fn probe(agent: &ureq::Agent, url: &str) -> Result<String, String> {
    let body = json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {
            "protocolVersion": crate::client::PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "termide", "version": env!("CARGO_PKG_VERSION") }
        }
    });
    match agent
        .post(url)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json, text/event-stream")
        .send_string(&body.to_string())
    {
        Ok(_) => Err("the server let us in without a sign-in".to_string()),
        Err(ureq::Error::Status(401 | 403, response)) => Ok(response
            .header("WWW-Authenticate")
            .unwrap_or_default()
            .to_string()),
        Err(ureq::Error::Status(code, _)) => Err(format!(
            "the server answered HTTP {code}, not a request to sign in"
        )),
        Err(error) => Err(format!("cannot reach {url}: {error}")),
    }
}

/// One `key="value"` (or bare `key=value`) parameter of a `WWW-Authenticate`.
fn challenge_param(challenge: &str, key: &str) -> Option<String> {
    let lower = challenge.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find(key) {
        let start = from + at;
        from = start + key.len();
        let before_ok = start == 0 || matches!(lower.as_bytes()[start - 1], b' ' | b',' | b'\t');
        let rest = challenge[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let rest = rest[1..].trim_start();
        return Some(if let Some(quoted) = rest.strip_prefix('"') {
            quoted.split('"').next().unwrap_or("").to_string()
        } else {
            rest.split([',', ' ']).next().unwrap_or("").to_string()
        });
    }
    None
}

/// The authorization server and the scopes for the resource at `url`. A
/// server that publishes no metadata is taken to be its own authorization
/// server, as the 2025-03-26 revision had it.
fn protected_resource(agent: &ureq::Agent, url: &str, challenge: &str) -> (String, Option<String>) {
    let scope_hint = challenge_param(challenge, "scope");
    let mut candidates = Vec::new();
    if let Some(metadata) = challenge_param(challenge, "resource_metadata") {
        candidates.push(metadata);
    }
    if let Ok(parsed) = url::Url::parse(url) {
        let origin = parsed.origin().ascii_serialization();
        let path = parsed.path().trim_end_matches('/');
        if !path.is_empty() {
            candidates.push(format!(
                "{origin}/.well-known/oauth-protected-resource{path}"
            ));
        }
        candidates.push(format!("{origin}/.well-known/oauth-protected-resource"));
    }
    for candidate in candidates {
        let Some(metadata) = get_json(agent, &candidate) else {
            continue;
        };
        let Some(issuer) = metadata["authorization_servers"]
            .as_array()
            .and_then(|servers| servers.first())
            .and_then(Value::as_str)
        else {
            continue;
        };
        let scopes = scope_hint.clone().or_else(|| {
            metadata["scopes_supported"].as_array().map(|scopes| {
                scopes
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
        });
        return (issuer.to_string(), scopes.filter(|s| !s.is_empty()));
    }
    let origin = url::Url::parse(url)
        .map(|parsed| parsed.origin().ascii_serialization())
        .unwrap_or_else(|_| url.to_string());
    (origin, scope_hint)
}

/// An authorization server's endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthServer {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    /// `None` when the metadata does not say.
    pkce_methods: Option<Vec<String>>,
}

/// The endpoints of `issuer`, from its RFC 8414 or OpenID metadata, in the
/// order the specification lists them; the conventional paths under the
/// issuer when it publishes neither.
fn authorization_server(agent: &ureq::Agent, issuer: &str) -> AuthServer {
    let trimmed = issuer.trim_end_matches('/');
    let mut candidates = Vec::new();
    if let Ok(parsed) = url::Url::parse(trimmed) {
        let origin = parsed.origin().ascii_serialization();
        let path = parsed.path().trim_end_matches('/');
        if path.is_empty() {
            candidates.push(format!("{origin}/.well-known/oauth-authorization-server"));
            candidates.push(format!("{origin}/.well-known/openid-configuration"));
        } else {
            candidates.push(format!(
                "{origin}/.well-known/oauth-authorization-server{path}"
            ));
            candidates.push(format!("{origin}/.well-known/openid-configuration{path}"));
            candidates.push(format!("{trimmed}/.well-known/openid-configuration"));
        }
    }
    for candidate in candidates {
        let Some(metadata) = get_json(agent, &candidate) else {
            continue;
        };
        let (Some(authorize), Some(token)) = (
            metadata["authorization_endpoint"].as_str(),
            metadata["token_endpoint"].as_str(),
        ) else {
            continue;
        };
        return AuthServer {
            authorization_endpoint: authorize.to_string(),
            token_endpoint: token.to_string(),
            registration_endpoint: metadata["registration_endpoint"]
                .as_str()
                .map(str::to_string),
            pkce_methods: metadata["code_challenge_methods_supported"]
                .as_array()
                .map(|methods| {
                    methods
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                }),
        };
    }
    AuthServer {
        authorization_endpoint: format!("{trimmed}/authorize"),
        token_endpoint: format!("{trimmed}/token"),
        registration_endpoint: Some(format!("{trimmed}/register")),
        pkce_methods: None,
    }
}

fn get_json(agent: &ureq::Agent, url: &str) -> Option<Value> {
    let response = agent
        .get(url)
        .set("Accept", "application/json")
        .call()
        .ok()?;
    serde_json::from_reader(response.into_reader().take(1 << 20)).ok()
}

/// The client the sign-in runs as.
struct Client {
    id: String,
    secret: Option<String>,
    auth_method: Option<String>,
}

/// Register termide as a public client that returns to `redirect`. Done anew
/// at each sign-in: the port changes, and a client is cheap to the server.
fn register(
    agent: &ureq::Agent,
    endpoint: &str,
    redirect: &str,
    scopes: Option<&str>,
) -> Result<Client, String> {
    let mut body = json!({
        "client_name": "termide",
        "redirect_uris": [redirect],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    if let Some(scopes) = scopes {
        body["scope"] = json!(scopes);
    }
    let response = agent
        .post(endpoint)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .send_string(&body.to_string())
        .map_err(|error| format!("client registration failed: {}", describe(error)))?;
    let answer: Value = serde_json::from_reader(response.into_reader().take(1 << 20))
        .map_err(|error| format!("client registration answered with no JSON: {error}"))?;
    let id = answer["client_id"]
        .as_str()
        .ok_or("client registration answered with no client_id")?;
    Ok(Client {
        id: id.to_string(),
        secret: answer["client_secret"].as_str().map(str::to_string),
        auth_method: answer["token_endpoint_auth_method"]
            .as_str()
            .map(str::to_string),
    })
}

/// POST `form` to the grant's token endpoint, authenticating as its client.
fn token_request<'a>(
    agent: &ureq::Agent,
    grant: &'a Grant,
    form: &mut Vec<(&'a str, &'a str)>,
) -> Result<Value, String> {
    let mut call = agent
        .post(&grant.token_endpoint)
        .set("Accept", "application/json");
    match (&grant.client_secret, grant.auth_method.as_deref()) {
        (Some(secret), Some("client_secret_post")) => {
            form.push(("client_id", &grant.client_id));
            form.push(("client_secret", secret));
        }
        // RFC 7591's default for a client that holds a secret.
        (Some(secret), _) => {
            let pair = format!("{}:{}", form_encode(&grant.client_id), form_encode(secret));
            call = call.set(
                "Authorization",
                &format!(
                    "Basic {}",
                    base64::engine::general_purpose::STANDARD.encode(pair)
                ),
            );
        }
        (None, _) => form.push(("client_id", &grant.client_id)),
    }
    let response = call
        .send_form(form)
        .map_err(|error| format!("the token endpoint refused: {}", describe(error)))?;
    serde_json::from_reader(response.into_reader().take(1 << 20))
        .map_err(|error| format!("the token endpoint answered with no JSON: {error}"))
}

/// The grant a token answer makes, keeping the client of `base`.
fn grant_from(answer: &Value, base: &Grant) -> Result<Grant, String> {
    let token = answer["access_token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .ok_or("the token endpoint answered with no access_token")?;
    if let Some(kind) = answer["token_type"].as_str() {
        if !kind.eq_ignore_ascii_case("bearer") {
            return Err(format!("a {kind} token, where a bearer one is sent"));
        }
    }
    Ok(Grant {
        access_token: token.to_string(),
        refresh_token: answer["refresh_token"].as_str().map(str::to_string),
        expires_at: answer["expires_in"]
            .as_u64()
            .map(|seconds| now_secs() + seconds),
        ..base.clone()
    })
}

/// The OAuth error a refusal carries, or the status when it carries none.
fn describe(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, response) => {
            let text = response.into_string().unwrap_or_default();
            let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            match (
                parsed["error"].as_str(),
                parsed["error_description"].as_str(),
            ) {
                (Some(error), Some(detail)) => format!("HTTP {code}: {error}: {detail}"),
                (Some(error), None) => format!("HTTP {code}: {error}"),
                _ => format!("HTTP {code}"),
            }
        }
        other => other.to_string(),
    }
}

/// Wait for the browser to come back to the loopback with the code, until
/// [`LOGIN_WAIT`] or `cancel`. Anything but the callback (a favicon) is
/// answered and waited past.
fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    cancel: &AtomicBool,
) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + LOGIN_WAIT;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("the sign-in was abandoned".to_string());
        }
        if Instant::now() > deadline {
            return Err(format!(
                "the browser did not return within {} minutes",
                LOGIN_WAIT.as_secs() / 60
            ));
        }
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(error) => return Err(format!("the callback listener failed: {error}")),
        };
        match read_callback(stream, state) {
            Some(outcome) => return outcome,
            None => continue,
        }
    }
}

/// One request to the listener: `None` when it is not the callback, the
/// code or the reason otherwise. The browser is told either way.
fn read_callback(mut stream: TcpStream, state: &str) -> Option<Result<String, String>> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut line = String::new();
    BufReader::new(stream.try_clone().ok()?)
        .read_line(&mut line)
        .ok()?;
    let target = line.split_whitespace().nth(1).unwrap_or("");
    let parsed = url::Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    if parsed.path() != CALLBACK_PATH {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return None;
    }
    let query: BTreeMap<String, String> = parsed.query_pairs().into_owned().collect();
    let outcome = if query.get("state").map(String::as_str) != Some(state) {
        Err("the browser returned with a state this sign-in did not send".to_string())
    } else if let Some(error) = query.get("error") {
        Err(match query.get("error_description") {
            Some(detail) => format!("the sign-in was refused: {error}: {detail}"),
            None => format!("the sign-in was refused: {error}"),
        })
    } else if let Some(code) = query.get("code") {
        Ok(code.clone())
    } else {
        Err("the browser returned with no code".to_string())
    };
    let message = match &outcome {
        Ok(_) => "Signed in. You can close this tab and return to termide.".to_string(),
        Err(reason) => format!("Sign-in failed: {reason}"),
    };
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>termide</title><p>{}</p>",
        message
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    );
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    Some(outcome)
}

/// 32 random bytes, URL-safe: a PKCE verifier or a state.
fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("the system has a random source");
    URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// `application/x-www-form-urlencoded`, which client credentials in a Basic
/// header are encoded with before they are joined (RFC 6749 §2.3.1).
fn form_encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pkce_challenge_is_the_rfc_example() {
        // As `openssl dgst -sha256 -binary | base64` computes it, made URL-safe
        // and unpadded.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r2wW1gFWFOEjXk"),
            "dZrJz5b___3oA5uVny1DZqEO9NM98brdg_f4AEY03Bc"
        );
        assert_ne!(random_token(), random_token());
    }

    #[test]
    fn challenge_parameters_are_read_quoted_or_bare() {
        let header = r#"Bearer realm="x", resource_metadata="https://a.example/.well-known/oauth-protected-resource", scope="read write""#;
        assert_eq!(
            challenge_param(header, "resource_metadata").as_deref(),
            Some("https://a.example/.well-known/oauth-protected-resource")
        );
        assert_eq!(
            challenge_param(header, "scope").as_deref(),
            Some("read write")
        );
        assert_eq!(
            challenge_param("Bearer error=invalid_token", "error").as_deref(),
            Some("invalid_token")
        );
        assert_eq!(challenge_param("Bearer realm=\"x\"", "scope"), None);
    }

    #[test]
    fn grants_are_kept_per_server_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::new(dir.path().join("mcp-auth.json"));
        let grant = Grant {
            access_token: "a".into(),
            refresh_token: Some("r".into()),
            expires_at: None,
            token_endpoint: "https://as.example/token".into(),
            client_id: "c".into(),
            client_secret: None,
            auth_method: None,
        };
        assert!(store.get("https://mcp.example/mcp").is_none());
        store.put("https://MCP.example/mcp#x", &grant).unwrap();
        assert_eq!(store.get("https://mcp.example/mcp"), Some(grant.clone()));
        assert!(store.get("https://other.example/mcp").is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(store.remove("https://mcp.example/mcp").unwrap());
        assert!(!store.remove("https://mcp.example/mcp").unwrap());
    }

    #[test]
    fn freshness_follows_the_expiry_with_a_margin() {
        let mut grant = Grant {
            access_token: "a".into(),
            refresh_token: None,
            expires_at: None,
            token_endpoint: String::new(),
            client_id: String::new(),
            client_secret: None,
            auth_method: None,
        };
        assert!(grant.is_fresh());
        grant.expires_at = Some(now_secs() + 10);
        assert!(!grant.is_fresh());
        grant.expires_at = Some(now_secs() + 3600);
        assert!(grant.is_fresh());
    }
}
