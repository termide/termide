//! Configuration of command hooks, `ai/hooks.toml`: one table per hook.
//!
//! The shape is data the agent directories carry; the runner that starts
//! the programs is the `termide-agent-hooks` crate.
//!
//! ```toml
//! [no-force-push]
//! event = "before_tool_call"
//! tools = ["bash"]
//! command = "scripts/guard.sh"
//! ```

use serde::{Deserialize, Serialize};

/// The file, at any level of the `ai` directory.
pub const HOOKS_FILE: &str = "hooks.toml";

/// Where in the loop a hook runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    /// Before a tool runs; may block it, rewrite its arguments or approve
    /// it in place of the permission prompt.
    BeforeToolCall,
    /// After a tool ran; may rewrite its result.
    AfterToolCall,
}

/// One external program run on an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookConfig {
    pub event: HookEvent,
    /// Program to run, with the working directory of the panel.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Tool names the hook applies to (`*` wildcards); every tool when empty.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Seconds before the program is killed and ignored.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// `false` at a higher level switches off a hook a lower level defines.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// The hook comes from a project rather than the configuration level, so
    /// it runs only once approved (see [`crate::approvals::Approvals`]). Set
    /// by [`crate::layers::AgentDirs::hooks`]; never read from a file.
    #[serde(skip)]
    pub from_project: bool,
}

impl HookConfig {
    /// What an approval of the hook is given for: what it runs, and when —
    /// the event and the tools, so an approved hook cannot later become a
    /// guard on every call — and how long it may take.
    #[must_use]
    pub fn launch_spec(&self) -> String {
        serde_json::json!({
            "event": self.event,
            "command": self.command,
            "args": self.args,
            "tools": self.tools,
            "timeout_secs": self.timeout_secs,
        })
        .to_string()
    }

    /// What the hook runs, for a person to read.
    #[must_use]
    pub fn launch_line(&self) -> String {
        std::iter::once(self.command.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn default_timeout() -> u64 {
    30
}

fn default_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn a_hook_table_parses_with_defaults() {
        let hooks: BTreeMap<String, HookConfig> = toml::from_str(
            "[guard]\nevent = \"before_tool_call\"\ntools = [\"bash\"]\ncommand = \"guard.sh\"\n\n[log]\nevent = \"after_tool_call\"\ncommand = \"log.sh\"\nenabled = false\n",
        )
        .unwrap();
        assert_eq!(hooks["guard"].event, HookEvent::BeforeToolCall);
        assert_eq!(hooks["guard"].timeout_secs, 30);
        assert!(hooks["guard"].args.is_empty());
        assert!(!hooks["log"].enabled);
    }
}
