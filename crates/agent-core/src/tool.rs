//! The tool contract and the registry the loop resolves calls against.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::ask::UserAsker;
use crate::cancel::CancelToken;
use crate::classifier::SessionView;
use crate::message::{ToolCall, ToolResultMessage};
use crate::provider::ToolSpec;
use crate::shell::ShellRunner;
use crate::suggest::CommandSuggester;

/// Environment a tool runs in.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Directory relative paths resolve against; the project root by default.
    pub cwd: PathBuf,
    /// Someone to put a question to; `None` when no one is watching the run
    /// (a subagent, headless mode).
    pub asker: Option<UserAsker>,
    /// Someone to offer a command to; `None` when no one is watching to
    /// confirm one (a subagent, headless mode).
    pub suggester: Option<CommandSuggester>,
    /// How to run a command the user confirmed; `None` when there is no way
    /// to. A tool must not run something through this unless the user asked.
    pub shell_run: Option<ShellRunner>,
    /// The session the call comes from, for the `auto` mode reviewer; `None`
    /// for a call from outside termide's own loop.
    pub session: Option<SessionView>,
    /// The call's own cancel token, when the call can be given up apart from
    /// the run — an external agent that stopped waiting for it: a question
    /// asked on its behalf is withdrawn with it. `None` for a call of
    /// termide's own loop, whose questions end with the run.
    pub withdrawn: Option<CancelToken>,
}

impl ToolContext {
    /// A context in `cwd` with no one to ask.
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            asker: None,
            suggester: None,
            shell_run: None,
            session: None,
            withdrawn: None,
        }
    }
}

/// Partial progress reported while a tool is still running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolUpdate {
    /// Output produced so far, for streaming display (a shell command's
    /// stdout, for example). Carries the accumulated text, not a delta.
    Output(String),
}

/// Something the model can call.
///
/// Tools are shared across threads and may run concurrently in the future,
/// so `execute` takes `&self`; keep per-call state local.
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    /// Full description sent to the model as part of the tool schema.
    fn description(&self) -> &str;

    /// JSON Schema of the arguments object.
    fn parameters(&self) -> Value;

    /// One-line summary listed in the system prompt; `None` hides the tool
    /// from that list (it stays callable).
    fn prompt_snippet(&self) -> Option<&str> {
        None
    }

    /// Usage rules merged into the system prompt's guidelines section.
    fn prompt_guidelines(&self) -> &[String] {
        &[]
    }

    /// The description from `template` (a tool's text, see
    /// [`crate::tool_text`]) with the tool's own placeholders filled, such as
    /// the agents `task` can delegate to. Nothing to fill by default.
    fn render_description(&self, template: &str) -> String {
        template.to_string()
    }

    /// Run the call. Failures are returned as a result with `is_error`
    /// set, never panicked or propagated, so the model can recover.
    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> ToolResultMessage;

    /// The schema entry handed to the provider.
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters(),
        }
    }
}

/// Tools that arrive after the agent started — an MCP server that finished
/// connecting — or word that they will not.
///
/// Every event names its `source` and speaks for all of that source's tools:
/// `Ready` is the whole set, the first one or one that replaces the set
/// before it (the server changed its list, or it was reconnected), and every
/// other event but `LoginStarted` leaves the source with none.
#[derive(Clone)]
pub enum LateTools {
    Ready {
        source: String,
        tools: Vec<Arc<dyn Tool>>,
    },
    Failed {
        source: String,
        error: String,
    },
    /// The source is no longer configured.
    Gone {
        source: String,
    },
    /// The source answers only a signed-in client, and no sign-in is kept
    /// for it (or the one kept has lapsed).
    NeedsLogin {
        source: String,
    },
    /// A sign-in for the source waits in the browser at `url`.
    LoginStarted {
        source: String,
        url: String,
    },
}

impl LateTools {
    #[must_use]
    pub fn source(&self) -> &str {
        match self {
            Self::Ready { source, .. }
            | Self::Failed { source, .. }
            | Self::Gone { source }
            | Self::NeedsLogin { source }
            | Self::LoginStarted { source, .. } => source,
        }
    }
}

/// Ordered set of tools; insertion order is the order the model sees.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a tool, replacing an existing one with the same name in place so
    /// a user override keeps the built-in's position.
    pub fn insert(&mut self, tool: Arc<dyn Tool>) {
        match self.tools.iter().position(|t| t.name() == tool.name()) {
            Some(index) => self.tools[index] = tool,
            None => self.tools.push(tool),
        }
    }

    pub fn remove(&mut self, name: &str) -> Option<Arc<dyn Tool>> {
        let index = self.tools.iter().position(|t| t.name() == name)?;
        Some(self.tools.remove(index))
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Tool>> {
        self.tools.iter()
    }

    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.tools.iter().map(|t| t.name()).collect()
    }

    #[must_use]
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.iter().map(|t| t.spec()).collect()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.names())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Named(&'static str);

    impl Tool for Named {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "test"
        }
        fn parameters(&self) -> Value {
            json!({ "type": "object" })
        }
        fn execute(
            &self,
            call: &ToolCall,
            _ctx: &ToolContext,
            _on_update: &mut dyn FnMut(ToolUpdate),
            _cancel: &CancelToken,
        ) -> ToolResultMessage {
            ToolResultMessage::text(call, self.0)
        }
    }

    #[test]
    fn insert_replaces_same_name_in_place() {
        let mut registry = ToolRegistry::new();
        registry.insert(Arc::new(Named("read")));
        registry.insert(Arc::new(Named("bash")));
        registry.insert(Arc::new(Named("read")));
        assert_eq!(registry.names(), vec!["read", "bash"]);
        assert_eq!(registry.len(), 2);
        assert!(registry.get("bash").is_some());
        assert!(registry.remove("read").is_some());
        assert_eq!(registry.names(), vec!["bash"]);
    }
}
