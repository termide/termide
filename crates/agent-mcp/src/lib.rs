//! MCP client of the termide coding agent: tools from servers over stdio or
//! over Streamable HTTP; and the server that serves termide's own tools to an
//! external agent.
//!
//! Servers are connected in the background when a panel opens, one thread
//! each, because an `npx` server can take seconds to come up and the UI
//! must not wait for it. Their tools reach the agent as [`LateTools`] through
//! a subscription: whoever subscribes gets what has connected so far at once
//! and the rest as it arrives. One connection per server is shared by every
//! panel-side consumer; requests on it are serialised.
//!
//! The set is not fixed once connected: a server that announces a change of
//! tools is listed again, a reload of the configuration connects what changed
//! and lets go of what left, and `/mcp login` signs in to a server that asks
//! for OAuth. Each of these reaches the subscribers as one more
//! [`LateTools`] for the server, replacing what it said before.

mod client;
mod http;
mod oauth;
mod server;
mod tool;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use termide_agent_core::{
    LateTools, McpReload, McpServerConfig, McpServerState, McpSignIn, McpStatus, McpTarget, Tool,
};

pub use client::{McpClient, McpToolInfo, McpTransport, OnMessage, PROTOCOL_VERSION};
pub use http::HttpClient;
pub use oauth::{Grant, TokenStore};
pub use server::{McpServer, SERVER_NAME};
pub use tool::{tool_name, McpTool};

/// Above this many tools from one server without a `tools` filter, the
/// panel is told: every schema goes into every request.
pub const MANY_TOOLS: usize = 20;

/// The notification a server sends when its tools change.
const TOOLS_CHANGED: &str = "notifications/tools/list_changed";

enum State {
    Pending,
    Ready(Vec<Arc<dyn Tool>>),
    Failed(String),
    NeedsLogin,
    SigningIn,
}

/// One configured server and where its connection stands.
struct Entry {
    config: McpServerConfig,
    state: State,
    /// Bumped at every connection attempt: what a thread brings back for an
    /// older one — a server reloaded meanwhile — is dropped, not shown.
    generation: u64,
    /// The live connection, kept to list the tools again when they change.
    client: Option<Arc<dyn McpTransport>>,
    /// Bumped at every new listing; only the newest one is applied, so two
    /// changes in a row cannot land in the wrong order.
    listing: u64,
    /// Set to give up a sign-in waiting in the browser.
    login_cancel: Option<Arc<AtomicBool>>,
}

impl Entry {
    fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            state: State::Pending,
            generation: 0,
            client: None,
            listing: 0,
            login_cancel: None,
        }
    }

    fn cancel_login(&mut self) {
        if let Some(cancel) = self.login_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

type Opener = Box<dyn Fn(&str) + Send + Sync>;

/// The configured servers of one panel and their connections.
pub struct Connections {
    entries: Mutex<BTreeMap<String, Entry>>,
    subscribers: Mutex<Vec<Sender<LateTools>>>,
    started: AtomicBool,
    /// Where OAuth sign-ins are kept; without it a server that asks for one
    /// is reported as refusing us.
    tokens: Option<Arc<TokenStore>>,
    /// Opens the browser on a sign-in's address.
    opener: Opener,
    /// The handlers a connection calls hold this, not the connections: a
    /// connection is owned here, and must not own its owner back.
    me: Weak<Connections>,
}

impl Connections {
    /// The servers, with no OAuth sign-in.
    #[must_use]
    pub fn new(servers: BTreeMap<String, McpServerConfig>) -> Arc<Self> {
        Self::build(servers, None, Box::new(|_| {}))
    }

    /// The servers, with the sign-ins kept in `tokens`; `/mcp login` opens
    /// the system browser.
    #[must_use]
    pub fn with_tokens(
        servers: BTreeMap<String, McpServerConfig>,
        tokens: TokenStore,
    ) -> Arc<Self> {
        Self::with_opener(servers, tokens, |url| {
            if let Err(error) = open::that_detached(url) {
                log::warn!("cannot open the browser: {error}");
            }
        })
    }

    /// As [`Connections::with_tokens`], with `opener` in place of the browser.
    #[must_use]
    pub fn with_opener(
        servers: BTreeMap<String, McpServerConfig>,
        tokens: TokenStore,
        opener: impl Fn(&str) + Send + Sync + 'static,
    ) -> Arc<Self> {
        Self::build(servers, Some(Arc::new(tokens)), Box::new(opener))
    }

    fn build(
        servers: BTreeMap<String, McpServerConfig>,
        tokens: Option<Arc<TokenStore>>,
        opener: Opener,
    ) -> Arc<Self> {
        let entries = servers
            .into_iter()
            .map(|(name, config)| (name, Entry::new(config)))
            .collect();
        Arc::new_cyclic(|me| Self {
            entries: Mutex::new(entries),
            subscribers: Mutex::new(Vec::new()),
            started: AtomicBool::new(false),
            tokens,
            opener,
            me: me.clone(),
        })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Start connecting (the first time) and receive every server's tools:
    /// those already connected immediately, the others when they are, and
    /// every change after.
    pub fn subscribe(&self) -> Receiver<LateTools> {
        if !self.started.swap(true, Ordering::Relaxed) {
            let mut entries = self.lock();
            for (name, entry) in entries.iter_mut() {
                self.spawn_connect(name, entry);
            }
        }
        let (tx, rx) = mpsc::channel();
        {
            let entries = self.lock();
            for (name, entry) in entries.iter() {
                let source = name.clone();
                let event = match &entry.state {
                    State::Pending | State::SigningIn => continue,
                    State::Ready(tools) => LateTools::Ready {
                        source,
                        tools: tools.clone(),
                    },
                    State::Failed(error) => LateTools::Failed {
                        source,
                        error: error.clone(),
                    },
                    State::NeedsLogin => LateTools::NeedsLogin { source },
                };
                let _ = tx.send(event);
            }
        }
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tx);
        rx
    }

    fn broadcast(&self, event: &LateTools) {
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|tx| tx.send(event.clone()).is_ok());
    }

    /// A new attempt at `entry`'s connection, on a thread of its own; the
    /// one before it, if any, is let go.
    fn spawn_connect(&self, name: &str, entry: &mut Entry) {
        entry.generation += 1;
        entry.state = State::Pending;
        entry.client = None;
        let generation = entry.generation;
        let config = entry.config.clone();
        // Held to the end of the attempt, so its outcome reaches whoever
        // subscribed even when the owner lets go meanwhile.
        let Some(this) = self.me.upgrade() else {
            return;
        };
        let name = name.to_string();
        let spawned = std::thread::Builder::new()
            .name(format!("termide-mcp-{name}"))
            .spawn(move || this.connect(&name, &config, generation));
        if let Err(error) = spawned {
            entry.state = State::Failed(format!("cannot start a thread: {error}"));
        }
    }

    fn connect(&self, name: &str, config: &McpServerConfig, generation: u64) {
        let outcome = open(
            name,
            config,
            self.tokens.clone(),
            self.on_message(name, generation),
        );
        let event = {
            let mut entries = self.lock();
            let Some(entry) = entries.get_mut(name).filter(|e| e.generation == generation) else {
                return;
            };
            let source = name.to_string();
            match outcome {
                Ok((client, tools)) => {
                    entry.client = Some(client);
                    entry.state = State::Ready(tools.clone());
                    LateTools::Ready { source, tools }
                }
                Err(Refusal::NeedsLogin) => {
                    entry.state = State::NeedsLogin;
                    LateTools::NeedsLogin { source }
                }
                Err(Refusal::Failed(error)) => {
                    entry.state = State::Failed(error.clone());
                    LateTools::Failed { source, error }
                }
            }
        };
        self.broadcast(&event);
    }

    /// What a connection calls with a server's notifications: a change of
    /// tools lists them again, off the reader's thread.
    fn on_message(&self, name: &str, generation: u64) -> OnMessage {
        let me = self.me.clone();
        let name = name.to_string();
        Arc::new(move |method, _params| {
            if method != TOOLS_CHANGED {
                return;
            }
            let me = me.clone();
            let name = name.clone();
            let _ = std::thread::Builder::new()
                .name(format!("termide-mcp-{name}-list"))
                .spawn(move || {
                    if let Some(this) = me.upgrade() {
                        this.relist(&name, generation);
                    }
                });
        })
    }

    /// List `name`'s tools again and hand the new set to every subscriber.
    /// A listing that fails keeps the set there was.
    fn relist(&self, name: &str, generation: u64) {
        let (client, config, ticket) = {
            let mut entries = self.lock();
            let Some(entry) = entries.get_mut(name).filter(|e| e.generation == generation) else {
                return;
            };
            let Some(client) = entry.client.clone() else {
                return;
            };
            entry.listing += 1;
            (client, entry.config.clone(), entry.listing)
        };
        let tools = match wrap_tools(name, &config, &client) {
            Ok(tools) => tools,
            Err(error) => {
                log::warn!("mcp {name}: the tools changed, but cannot be listed: {error}");
                return;
            }
        };
        {
            let mut entries = self.lock();
            let Some(entry) = entries
                .get_mut(name)
                .filter(|e| e.generation == generation && e.listing == ticket)
            else {
                return;
            };
            entry.state = State::Ready(tools.clone());
        }
        log::info!("mcp {name}: the tools changed, {} now", tools.len());
        self.broadcast(&LateTools::Ready {
            source: name.to_string(),
            tools,
        });
    }

    /// Where every configured server stands, by name.
    #[must_use]
    pub fn status(&self) -> Vec<McpServerState> {
        let entries: Vec<(String, McpStatus, McpServerConfig)> = self
            .lock()
            .iter()
            .map(|(name, entry)| {
                let status = match &entry.state {
                    State::Pending => McpStatus::Connecting,
                    State::Ready(tools) => McpStatus::Ready { tools: tools.len() },
                    State::Failed(error) => McpStatus::Failed(error.clone()),
                    State::NeedsLogin => McpStatus::NeedsLogin,
                    State::SigningIn => McpStatus::SigningIn,
                };
                (name.clone(), status, entry.config.clone())
            })
            .collect();
        // The grants file is read outside the lock: it is a file, and the
        // connections must not wait on it.
        entries
            .into_iter()
            .map(|(name, status, config)| McpServerState {
                sign_in: self.sign_in_state(&config),
                name,
                status,
            })
            .collect()
    }

    fn sign_in_state(&self, config: &McpServerConfig) -> McpSignIn {
        let (Some(tokens), Ok(McpTarget::Http { url })) = (&self.tokens, config.target()) else {
            return McpSignIn::None;
        };
        let static_key = config
            .headers
            .keys()
            .any(|key| key.trim().eq_ignore_ascii_case("authorization"));
        if static_key {
            McpSignIn::None
        } else if tokens.get(url).is_some() {
            McpSignIn::SignedIn
        } else {
            McpSignIn::SignedOut
        }
    }

    /// Connect `name` again with its configuration from `servers`, whether
    /// or not it changed — or let it go, when it is no longer there. The
    /// other servers are left as they are.
    ///
    /// # Errors
    ///
    /// When no server of that name is configured or connected.
    pub fn reconnect(
        &self,
        name: &str,
        servers: &BTreeMap<String, McpServerConfig>,
    ) -> Result<McpReload, String> {
        let mut report = McpReload::default();
        {
            let mut entries = self.lock();
            match servers.get(name) {
                Some(config) => {
                    let entry = entries
                        .entry(name.to_string())
                        .or_insert_with(|| Entry::new(config.clone()));
                    entry.cancel_login();
                    entry.config = config.clone();
                    if self.started.load(Ordering::Relaxed) {
                        self.spawn_connect(name, entry);
                    } else {
                        entry.state = State::Pending;
                    }
                    report.started.push(name.to_string());
                }
                None => {
                    let mut entry = entries
                        .remove(name)
                        .ok_or_else(|| format!("no MCP server named {name}"))?;
                    entry.cancel_login();
                    report.removed.push(name.to_string());
                }
            }
        }
        if !report.removed.is_empty() {
            self.broadcast(&LateTools::Gone {
                source: name.to_string(),
            });
        }
        Ok(report)
    }

    /// Take `servers` as the configuration from now on: a server no longer in
    /// it goes with its tools, a new or changed one connects, one that was not
    /// connected tries again, and a connected one left as it was stays.
    pub fn reload(&self, servers: BTreeMap<String, McpServerConfig>) -> McpReload {
        let mut report = McpReload::default();
        let started = self.started.load(Ordering::Relaxed);
        {
            let mut entries = self.lock();
            entries.retain(|name, entry| {
                if servers.contains_key(name) {
                    return true;
                }
                entry.cancel_login();
                report.removed.push(name.clone());
                false
            });
            for (name, config) in servers {
                let entry = entries
                    .entry(name.clone())
                    .or_insert_with(|| Entry::new(config.clone()));
                let unchanged = entry.config == config;
                if unchanged && matches!(entry.state, State::Ready(_) | State::SigningIn) {
                    report.kept.push(name);
                    continue;
                }
                entry.cancel_login();
                entry.config = config;
                if started {
                    self.spawn_connect(&name, entry);
                } else {
                    entry.state = State::Pending;
                }
                report.started.push(name);
            }
        }
        for source in &report.removed {
            self.broadcast(&LateTools::Gone {
                source: source.clone(),
            });
        }
        report
    }

    /// Sign in to `name` in the browser, then connect it with the sign-in.
    /// Returns at once; the outcome arrives to the subscribers.
    ///
    /// # Errors
    ///
    /// When there is no such server, it is not a `url` one, sign-ins are not
    /// kept here, or one is already under way.
    pub fn login(&self, name: &str) -> Result<(), String> {
        let tokens = self
            .tokens
            .clone()
            .ok_or("sign-ins are not kept in this session")?;
        let (config, generation, cancel) = {
            let mut entries = self.lock();
            let entry = entries
                .get_mut(name)
                .ok_or_else(|| format!("no MCP server named {name}"))?;
            if !matches!(entry.config.target(), Ok(McpTarget::Http { .. })) {
                return Err(format!("{name} is not a url server"));
            }
            if matches!(entry.state, State::SigningIn) {
                return Err(format!("a sign-in to {name} is already waiting"));
            }
            let cancel = Arc::new(AtomicBool::new(false));
            entry.login_cancel = Some(Arc::clone(&cancel));
            entry.generation += 1;
            entry.state = State::SigningIn;
            entry.client = None;
            (entry.config.clone(), entry.generation, cancel)
        };
        let this = self.me.upgrade().ok_or("the connections are closing")?;
        let name = name.to_string();
        std::thread::Builder::new()
            .name(format!("termide-mcp-{name}-login"))
            .spawn(move || this.sign_in(&name, &config, generation, &tokens, &cancel))
            .map(|_| ())
            .map_err(|error| format!("cannot start a thread: {error}"))
    }

    fn sign_in(
        &self,
        name: &str,
        config: &McpServerConfig,
        generation: u64,
        tokens: &TokenStore,
        cancel: &AtomicBool,
    ) {
        let url = config.url.clone().unwrap_or_default();
        let timeout = Duration::from_secs(config.timeout_secs);
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(timeout)
            .timeout_read(timeout)
            .user_agent(concat!("termide-agent/", env!("CARGO_PKG_VERSION")))
            .build();
        let on_url = |address: &str| {
            self.broadcast(&LateTools::LoginStarted {
                source: name.to_string(),
                url: address.to_string(),
            });
            (self.opener)(address);
        };
        let outcome = oauth::login(&oauth::Login {
            url: &url,
            oauth: &config.oauth,
            agent: &agent,
            on_url: &on_url,
            cancel,
        })
        .and_then(|grant| tokens.put(&url, &grant));
        let mut entries = self.lock();
        let Some(entry) = entries.get_mut(name).filter(|e| e.generation == generation) else {
            return;
        };
        entry.login_cancel = None;
        match outcome {
            Ok(()) => self.spawn_connect(name, entry),
            Err(error) => {
                let error = format!("sign-in failed: {error}");
                entry.state = State::Failed(error.clone());
                drop(entries);
                self.broadcast(&LateTools::Failed {
                    source: name.to_string(),
                    error,
                });
            }
        }
    }

    /// Forget the sign-in kept for `name` and connect it again without one;
    /// `false` when none was kept.
    ///
    /// # Errors
    ///
    /// When there is no such server, it is not a `url` one, or the file of
    /// sign-ins cannot be written.
    pub fn logout(&self, name: &str) -> Result<bool, String> {
        let tokens = self
            .tokens
            .clone()
            .ok_or("sign-ins are not kept in this session")?;
        let mut entries = self.lock();
        let entry = entries
            .get_mut(name)
            .ok_or_else(|| format!("no MCP server named {name}"))?;
        let Ok(McpTarget::Http { url }) = entry.config.target() else {
            return Err(format!("{name} is not a url server"));
        };
        let url = url.to_string();
        entry.cancel_login();
        let removed = tokens.remove(&url)?;
        if self.started.load(Ordering::Relaxed) {
            self.spawn_connect(name, entry);
        }
        Ok(removed)
    }
}

/// Why a server is not connected.
enum Refusal {
    NeedsLogin,
    Failed(String),
}

type Opened = (Arc<dyn McpTransport>, Vec<Arc<dyn Tool>>);

/// Start `config` over the transport it names, shake hands, start listening
/// and wrap its tools.
fn open(
    name: &str,
    config: &McpServerConfig,
    tokens: Option<Arc<TokenStore>>,
    on_message: OnMessage,
) -> Result<Opened, Refusal> {
    let client: Arc<dyn McpTransport> = match config.target().map_err(Refusal::Failed)? {
        McpTarget::Stdio { .. } => {
            let client = Arc::new(McpClient::spawn(name, config).map_err(Refusal::Failed)?);
            client.initialize().map_err(Refusal::Failed)?;
            client
        }
        McpTarget::Http { .. } => {
            let client =
                Arc::new(HttpClient::with_tokens(name, config, tokens).map_err(Refusal::Failed)?);
            if let Err(error) = client.initialize() {
                return Err(if client.needs_login() {
                    Refusal::NeedsLogin
                } else {
                    Refusal::Failed(error)
                });
            }
            client
        }
    };
    client.listen(on_message);
    let tools = wrap_tools(name, config, &client).map_err(Refusal::Failed)?;
    Ok((client, tools))
}

/// The server's tools as the agent's, honouring the `tools` filter (unknown
/// names are reported).
fn wrap_tools(
    name: &str,
    config: &McpServerConfig,
    client: &Arc<dyn McpTransport>,
) -> Result<Vec<Arc<dyn Tool>>, String> {
    let mut infos = client.list_tools()?;
    if let Some(wanted) = &config.tools {
        for missing in wanted
            .iter()
            .filter(|w| !infos.iter().any(|t| &t.name == *w))
        {
            log::warn!("mcp {name}: no tool named {missing}");
        }
        infos.retain(|info| wanted.contains(&info.name));
    } else if infos.len() > MANY_TOOLS {
        log::warn!(
            "mcp {name}: {} tools, each sent with every request; consider tools = [...] in mcp.toml",
            infos.len()
        );
    }
    Ok(infos
        .into_iter()
        .map(|info| Arc::new(McpTool::new(name, info, Arc::clone(client))) as Arc<dyn Tool>)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn subscribers_learn_of_failures_and_late_ones_get_the_replay() {
        let mut servers = BTreeMap::new();
        servers.insert(
            "ghost".to_string(),
            McpServerConfig {
                command: Some("/nonexistent/termide-mcp-test-server".into()),
                timeout_secs: 2,
                ..Default::default()
            },
        );
        let connections = Connections::new(servers);
        assert!(!connections.is_empty());
        let first = connections.subscribe();
        let event = first.recv_timeout(Duration::from_secs(5)).unwrap();
        let LateTools::Failed { source, error } = event else {
            panic!("expected a failure");
        };
        assert_eq!(source, "ghost");
        assert!(error.contains("cannot start"), "{error}");

        // A subscriber arriving after the fact gets the same word at once.
        let late = connections.subscribe();
        assert!(matches!(
            late.recv_timeout(Duration::from_secs(1)).unwrap(),
            LateTools::Failed { .. }
        ));
        assert!(Connections::new(BTreeMap::new()).is_empty());
    }
}
