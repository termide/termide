//! MCP client of the termide coding agent: tools from servers over stdio;
//! and the server that serves termide's own tools to an external agent.
//!
//! Servers are started in the background when a panel opens, one thread
//! each, because an `npx` server can take seconds to come up and the UI
//! must not wait for it. Their tools reach the agent as [`LateTools`] through
//! a subscription: whoever subscribes gets what has connected so far at once
//! and the rest as it arrives. One connection per server is shared by every
//! panel-side consumer; requests on it are serialised.

mod client;
mod server;
mod tool;

use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, Once};

use termide_agent_core::{LateTools, McpServerConfig, Tool};

pub use client::{McpClient, McpToolInfo, PROTOCOL_VERSION};
pub use server::{McpServer, SERVER_NAME};
pub use tool::{tool_name, McpTool};

/// Above this many tools from one server without a `tools` filter, the
/// panel is told: every schema goes into every request.
pub const MANY_TOOLS: usize = 20;

enum State {
    Pending,
    Ready(Vec<Arc<dyn Tool>>),
    Failed(String),
}

/// The configured servers of one panel and their connections.
pub struct Connections {
    servers: BTreeMap<String, McpServerConfig>,
    state: Mutex<HashMap<String, State>>,
    subscribers: Mutex<Vec<Sender<LateTools>>>,
    started: Once,
}

impl Connections {
    #[must_use]
    pub fn new(servers: BTreeMap<String, McpServerConfig>) -> Arc<Self> {
        let state = servers
            .keys()
            .map(|name| (name.clone(), State::Pending))
            .collect();
        Arc::new(Self {
            servers,
            state: Mutex::new(state),
            subscribers: Mutex::new(Vec::new()),
            started: Once::new(),
        })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// Start connecting (the first time) and receive every server's tools:
    /// those already connected immediately, the others when they are.
    pub fn subscribe(self: &Arc<Self>) -> Receiver<LateTools> {
        self.started.call_once(|| {
            for (name, config) in &self.servers {
                let this = Arc::clone(self);
                let name = name.clone();
                let config = config.clone();
                std::thread::Builder::new()
                    .name(format!("termide-mcp-{name}"))
                    .spawn(move || this.connect(&name, &config))
                    .expect("spawn MCP connect thread");
            }
        });
        let (tx, rx) = mpsc::channel();
        {
            let state = self.state.lock().unwrap();
            for (name, state) in state.iter() {
                let event = match state {
                    State::Pending => continue,
                    State::Ready(tools) => LateTools::Ready {
                        source: name.clone(),
                        tools: tools.clone(),
                    },
                    State::Failed(error) => LateTools::Failed {
                        source: name.clone(),
                        error: error.clone(),
                    },
                };
                let _ = tx.send(event);
            }
        }
        self.subscribers.lock().unwrap().push(tx);
        rx
    }

    fn connect(&self, name: &str, config: &McpServerConfig) {
        let outcome = connect(name, config);
        let event = match &outcome {
            Ok(tools) => LateTools::Ready {
                source: name.to_string(),
                tools: tools.clone(),
            },
            Err(error) => LateTools::Failed {
                source: name.to_string(),
                error: error.clone(),
            },
        };
        self.state.lock().unwrap().insert(
            name.to_string(),
            match outcome {
                Ok(tools) => State::Ready(tools),
                Err(error) => State::Failed(error),
            },
        );
        self.subscribers
            .lock()
            .unwrap()
            .retain(|tx| tx.send(event.clone()).is_ok());
    }
}

/// Start `config`, shake hands and wrap its tools, honouring the `tools`
/// filter (unknown names are reported).
fn connect(name: &str, config: &McpServerConfig) -> Result<Vec<Arc<dyn Tool>>, String> {
    let client = Arc::new(McpClient::spawn(name, config)?);
    client.initialize()?;
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
        .map(|info| Arc::new(McpTool::new(name, info, Arc::clone(&client))) as Arc<dyn Tool>)
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
                command: "/nonexistent/termide-mcp-test-server".into(),
                args: vec![],
                env: BTreeMap::new(),
                cwd: None,
                tools: None,
                timeout_secs: 2,
                enabled: true,
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
