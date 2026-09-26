//! Configuration of an external agent reached over ACP (the Agent Client
//! Protocol), the `[acp]` table of an `agent.toml`:
//!
//! ```toml
//! description = "Claude Code through its ACP adapter"
//!
//! [acp]
//! command = "npx"
//! args = ["-y", "@agentclientprotocol/claude-agent-acp"]
//! ```
//!
//! The client that speaks to the process is the `termide-agent-acp` crate.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpConfig {
    /// Program to run; it speaks ACP on its stdin/stdout.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment for the process; `$NAME` and `${NAME}` come from
    /// termide's own environment.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Seconds to wait for the agent to start and open a session.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Which adapter this is, when termide knows it; set by the provider,
    /// never by a file.
    #[serde(skip)]
    pub flavor: AcpFlavor,
}

/// An ACP adapter termide knows how to take further than the protocol: the
/// CLI agents a connection names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AcpFlavor {
    /// Any agent: driven over plain ACP, answering to its own configuration.
    #[default]
    Generic,
    /// Claude Code's adapter: it takes termide's system prompt and tools in
    /// place of its own, and leaves every permission decision to termide.
    ClaudeCode,
    /// Codex's adapter: it keeps its prompt and tools, and termide maps the
    /// permission mode onto Codex's modes.
    Codex,
}

fn default_timeout() -> u64 {
    120
}
