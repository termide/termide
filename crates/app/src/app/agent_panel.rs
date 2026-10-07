//! Opening the coding agent panel: everything the panel needs is resolved
//! from configuration here, so the panel crate stays free of config and
//! filesystem policy.

use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};

use anyhow::Result;
use termide_agent_acp::{AcpProvider, AcpRuntime};
use termide_agent_core::Mode;
use termide_agent_core::{
    apply_tool_texts, build_system_prompt, discover_context_files, ensure_global_layout,
    permission_channel, AcpConfig, AcpFlavor, Agent, AgentDirs, AgentEvent, AssistantMessage,
    AutoDenyPrompter, Backend, BackendSetup, CancelToken, ChainedHooks, CompactionPolicy, Decision,
    HostTools, IntentLog, Message, ModeHandle, ModelChoice, ModelSpec, PermissionAnswer,
    PermissionHooks, PermissionRules, PersistScope, PlanGuard, PromptOptions, Provider, Refusals,
    ReviewerSetup, Session, SessionSummary, ShellOutput, ShellRunner, StreamEvent, ThinkingLevel,
    Tool, ToolCall, ToolContext, ToolRegistry, ToolUpdate, Usage, UserMessage, DEFAULT_AGENT,
    GLOBAL_AGENT_DIR, SESSIONS_DIR,
};
use termide_agent_hooks::CommandHooks;
use termide_agent_mcp::{Connections, TokenStore};
use termide_agent_providers::{
    AnthropicProvider, Compat, OpenAiCompatProvider, ReasoningParam, Slots, SlottedProvider,
};
use termide_agent_recall::{PanelDir, RecallSetup, RecallTool, RepoFinder, Solver, TimeLimits};
use termide_agent_tools::{
    builtin_tools, BashTool, QuestionTool, SkillTool, SubagentOutcome, SubagentRun,
    SuggestCommandTool, TaskTool,
};
use termide_agent_web::{web_tools, Web, WebConfig};
use termide_config::{AiSettings, Connection, WebSettings};
use termide_panel_agent::{
    AgentCatalog, AgentEntry, AgentPanel, AgentPanelSetup, AgentProfile, BackendFactory,
    ConnectionCatalog, ConnectionChoice, ConnectionEntry, FoldMode, HooksFactory,
};

use super::App;

mod headless;
pub use headless::{run_agent_headless, run_recall, HeadlessOutput};

impl App {
    /// Open a new agent panel. Like a new terminal, each call opens another
    /// one, so several agents can work in a project at once. Reports an
    /// error instead of opening when there is no connection to run on.
    pub(in crate::app) fn handle_open_agent(&mut self) -> Result<()> {
        self.close_help_panels();

        let settings = self.state.config.ai.clone();
        if usable_connection(&settings).is_none() {
            let t = termide_i18n::t();
            self.show_error_modal(t.agent_not_configured().to_string());
            return Ok(());
        }

        // Like a new terminal, the agent works where the focused panel is
        // (a file manager's directory, an editor's file); the project root
        // when the panel has no directory of its own.
        let project_root = self.state.project_root.clone();
        let cwd = self
            .layout_manager
            .active_panel_mut()
            .and_then(|p| p.get_working_directory())
            .unwrap_or_else(|| project_root.clone());
        let panel = AgentPanel::new(agent_setup(
            &settings,
            cwd,
            &project_root,
            DEFAULT_AGENT,
            None,
        ));
        self.add_panel(Box::new(panel));
        self.auto_save_layout();
        Ok(())
    }

    /// Fork an agent session: copy the log at `source` and open the copy in a
    /// new panel working in `cwd`. The panel that asked keeps its own session
    /// and is not touched, so both go on working the conversation apart, each
    /// writing its own log. The copy carries the agent and model its log
    /// recorded, as any reopened session does, and a name of its own so the
    /// two are told apart in the picker.
    pub(in crate::app) fn fork_agent_session(&mut self, source: &Path, cwd: &Path) -> Result<()> {
        let t = termide_i18n::t();
        let source_session = match Session::open(source) {
            Ok(session) => session,
            Err(error) => {
                log::warn!("cannot open {}: {error}", source.display());
                self.show_error_modal(t.agent_notice_cannot_fork_fmt(&error.to_string()));
                return Ok(());
            }
        };
        let name = fork_name(&source_session);
        let copy = match source_session.fork(cwd, name.as_deref()) {
            Ok(path) => path,
            Err(error) => {
                log::warn!(
                    "cannot fork the agent session {}: {error}",
                    source.display()
                );
                self.show_error_modal(t.agent_notice_cannot_fork_fmt(&error.to_string()));
                return Ok(());
            }
        };
        let settings = self.state.config.ai.clone();
        if let Some(panel) =
            restore_agent_panel(&settings, cwd.to_path_buf(), Some(copy.clone()), None)
        {
            self.add_panel(Box::new(panel));
            self.auto_save_layout();
            return Ok(());
        }
        // No connection to run on: the copy is a session no one has open, so
        // it goes back the way it came.
        if let Err(error) = std::fs::remove_file(&copy) {
            log::warn!(
                "cannot remove the unused session copy {}: {error}",
                copy.display()
            );
        }
        self.show_error_modal(t.agent_not_configured().to_string());
        Ok(())
    }
}

impl App {
    /// The banner of a fresh agent panel asked to work elsewhere: let the
    /// user pick the directory, starting from the one it works in.
    pub(in crate::app) fn ask_agent_cwd(&mut self, session: Option<PathBuf>, cwd: PathBuf) {
        let t = termide_i18n::t();
        let modal = termide_modal::DirectoryPickerModal::new(
            cwd.clone(),
            t.agent_cwd_title().to_string(),
            t.directory_picker_select().to_string(),
        );
        self.state.set_pending_action(
            termide_state::PendingAction::ChangeAgentCwd { session, cwd },
            termide_modal::ActiveModal::DirectoryPicker(Box::new(modal)),
        );
    }

    /// The picked directory for the agent panel that asked: its session log
    /// moves to that directory's sessions, keeping the connection, model and
    /// agent it was set up with, and the panel is rebuilt there in place —
    /// its tools, agents, hooks and shell all come from where it works.
    pub(in crate::app) fn handle_change_agent_cwd(
        &mut self,
        value: Box<dyn std::any::Any>,
        session: Option<&Path>,
        cwd: &Path,
    ) -> Result<()> {
        let Some(new_cwd) = value.downcast_ref::<PathBuf>() else {
            return Ok(());
        };
        if new_cwd == cwd {
            return Ok(());
        }
        // A modal moves no focus, so the panel that asked is the focused one;
        // checked all the same, as nothing else may be replaced.
        let agent = match self
            .layout_manager
            .active_panel_mut()
            .and_then(|panel| panel.to_state(Path::new("")))
        {
            Some(termide_core::PanelState::Agent {
                cwd: active_cwd,
                session: active_session,
                agent,
            }) if active_cwd == cwd && active_session.as_deref() == session => agent,
            _ => return Ok(()),
        };
        let settings = self.state.config.ai.clone();
        if usable_connection(&settings).is_none() {
            let t = termide_i18n::t();
            self.show_error_modal(t.agent_not_configured().to_string());
            return Ok(());
        }
        // A log that cannot move is left where it is, and the panel starts
        // over on a fresh one in the new directory.
        let moved = session
            .zip(session_dir_of(new_cwd))
            .and_then(|(path, dir)| {
                Session::open(path)
                    .and_then(|log| log.move_into(&dir, new_cwd))
                    .map_err(|error| {
                        log::warn!("cannot move the agent session {}: {error}", path.display());
                    })
                    .ok()
            });
        let Some(panel) = restore_agent_panel(&settings, new_cwd.clone(), moved, agent) else {
            return Ok(());
        };
        // The old panel lets its log go; it finds the file moved, and an
        // empty one it would have discarded is already gone.
        if let Some(mut old) = self.layout_manager.replace_active_panel(Box::new(panel)) {
            old.kill_processes();
        }
        self.state.needs_redraw = true;
        self.auto_save_layout();
        Ok(())
    }
}

/// The name a fork of `source` takes, so the two are told apart in the
/// session picker and in the panel's title: `Refactor` becomes `Refactor (2)`,
/// then `Refactor (3)`. A source the user never named is labelled by its first
/// prompt, so the copy takes that with the counter on it — otherwise the
/// picker would show two identical rows, which is what the suffix is for.
///
/// `None` when there is nothing to tell apart: a session nothing was ever sent
/// to. Naming its copy would mark that copy worth keeping — a named session
/// survives even when empty — and so leave an empty log on the disk that
/// would otherwise be discarded once its panel closes.
///
/// The counter counts up from the names already in the directory, so forking
/// one session twice gives two different names without either panel having to
/// remember the other. Two forks cannot race for one number: this runs on the
/// app's main thread, which is the only place a fork is made.
fn fork_name(source: &Session) -> Option<String> {
    if source.is_empty() {
        return None;
    }
    // A name forked before keeps its counter, so the next one counts on from
    // it rather than starting again at two.
    let base = match source.name() {
        Some(name) => strip_fork_suffix(name).to_string(),
        None => SessionSummary::from(source).label(),
    };
    let taken: Vec<String> = source
        .path()
        .parent()
        .and_then(|dir| Session::list(dir).ok())
        .map(|list| {
            list.into_iter()
                .filter_map(|summary| summary.name)
                .collect()
        })
        .unwrap_or_default();
    (2..)
        .find(|n| !taken.iter().any(|name| name == &format!("{base} ({n})")))
        .map(|n| format!("{base} ({n})"))
}

/// A name's fork counter taken off: `Refactor (2)` → `Refactor`, so the next
/// fork of it is `Refactor (3)`. Only the suffix this code writes counts as a
/// counter — digits in brackets after a space, two or more — so a name the
/// user gave, `work (draft)` or `fix (1)`, keeps its brackets.
fn strip_fork_suffix(name: &str) -> &str {
    let trimmed = name.trim_end();
    let Some(head) = trimmed.strip_suffix(')') else {
        return name;
    };
    let Some((before, counter)) = head.rsplit_once('(') else {
        return name;
    };
    match counter.parse::<usize>() {
        Ok(n) if n >= 2 && before.ends_with(' ') => before.trim_end(),
        _ => name,
    }
}

/// Rebuild an agent panel saved in a project layout. `None` when there is
/// no connection to run on any more; a session log that has gone missing starts a
/// fresh session in the same project, an agent definition that has gone
/// missing falls back to the default one.
pub(crate) fn restore_agent_panel(
    settings: &AiSettings,
    cwd: PathBuf,
    session: Option<PathBuf>,
    agent: Option<String>,
) -> Option<AgentPanel> {
    if usable_connection(settings).is_none() {
        log::warn!("agent panel not restored: no [ai.connections] entry to run on");
        return None;
    }
    // Exclusive: if this session is already open in another restored panel,
    // fall back to a fresh one rather than back two panels with one log.
    let session = session.and_then(|path| match Session::open_exclusive(&path) {
        Ok(session) => Some(session),
        Err(error) => {
            log::warn!("cannot reopen agent session {}: {error}", path.display());
            None
        }
    });
    // termide's project root is its working directory, which follows the
    // current project; the layout restore runs off the App, so read it from
    // the same source.
    let project_root = std::env::current_dir().unwrap_or_else(|_| cwd.clone());
    Some(AgentPanel::new(agent_setup(
        settings,
        cwd,
        &project_root,
        agent.as_deref().unwrap_or(DEFAULT_AGENT),
        session,
    )))
}

/// The OAuth sign-ins of MCP servers, under the configuration's agent
/// directory: live tokens, so the file is created readable by the user only.
const MCP_AUTH_FILE: &str = "mcp-auth.json";

/// The agent definitions of one panel: `agents/<name>/` across the agent
/// directories of the panel's working directory, the project and the
/// configuration, turned into prompts and tool sets.
struct FsCatalog {
    cwd: PathBuf,
    project_root: PathBuf,
    dirs: AgentDirs,
    /// The panel's MCP servers; connected once, shared by every agent.
    mcp: Arc<Connections>,
    /// Builds and runs a named agent as a subagent for the `task` tool;
    /// set once the provider and settings are known.
    subagents: Option<Arc<Subagents>>,
    /// The web tools' service; set once the settings are known.
    web: Option<Arc<Web>>,
    /// The project's `recall`; set once the settings are known.
    recall: Option<Arc<RecallTool>>,
}

impl FsCatalog {
    fn new(cwd: &Path, project_root: &Path) -> Self {
        let global_agent_dir = termide_config::get_config_dir()
            .ok()
            .map(|dir| dir.join(GLOBAL_AGENT_DIR));
        if let Some(global) = &global_agent_dir {
            if let Err(error) = ensure_global_layout(global) {
                log::warn!("cannot lay out {}: {error}", global.display());
            }
        }
        Self::with_global(cwd, project_root, global_agent_dir)
    }

    fn with_global(cwd: &Path, project_root: &Path, global_agent_dir: Option<PathBuf>) -> Self {
        let dirs = AgentDirs::new(cwd, Some(project_root), global_agent_dir.as_deref());
        // OAuth sign-ins are the user's, kept beside the user's own agent
        // files rather than in any project.
        let mcp = match &global_agent_dir {
            Some(global) => Connections::with_tokens(
                dirs.mcp_servers(),
                TokenStore::new(global.join(MCP_AUTH_FILE)),
            ),
            None => Connections::new(dirs.mcp_servers()),
        };
        Self {
            cwd: cwd.to_path_buf(),
            project_root: project_root.to_path_buf(),
            mcp,
            dirs,
            subagents: None,
            web: None,
            recall: None,
        }
    }
}

impl AgentCatalog for FsCatalog {
    fn set_mode(&self, mode: Mode) {
        if let Some(subagents) = &self.subagents {
            subagents.mode.set(mode);
        }
    }

    fn mcp_status(&self) -> Vec<termide_agent_core::McpServerState> {
        self.mcp.status()
    }

    fn mcp_reconnect(&self, server: &str) -> Result<termide_agent_core::McpReload, String> {
        self.mcp.reconnect(server, &self.dirs.mcp_servers())
    }

    /// The configuration is read from the files again, at every level, so
    /// an edit to any of them is what the reload applies.
    fn mcp_reload(&self) -> Option<termide_agent_core::McpReload> {
        Some(self.mcp.reload(self.dirs.mcp_servers()))
    }

    fn mcp_login(&self, server: &str) -> Result<(), String> {
        self.mcp.login(server)
    }

    fn mcp_logout(&self, server: &str) -> Result<bool, String> {
        self.mcp.logout(server)
    }

    fn prompts(&self) -> Vec<termide_agent_core::PromptTemplate> {
        self.dirs.prompts()
    }

    fn commands(&self) -> Vec<termide_agent_core::CommandScript> {
        self.dirs.commands()
    }

    fn skills(&self) -> Vec<termide_agent_core::SkillInfo> {
        self.dirs.skills()
    }

    fn definition_problems(&self) -> Vec<termide_agent_core::DefinitionProblem> {
        self.dirs.definition_problems()
    }

    fn list(&self) -> Vec<AgentEntry> {
        self.dirs
            .agents()
            .into_iter()
            .map(|name| {
                // The default agent needs no definition, so unless one gives
                // it a description it is described by the panel's own label.
                let mut description = self.dirs.spec(&name).description;
                if description.is_empty() && name == DEFAULT_AGENT {
                    description = termide_i18n::t().panel_agent().to_string();
                }
                AgentEntry { name, description }
            })
            .collect()
    }

    /// The default agent always resolves; another name only when a root
    /// defines it.
    fn resolve(&self, name: &str) -> Option<AgentProfile> {
        self.resolve_without(name, &std::collections::BTreeSet::new())
    }

    /// The profile with what the session switched off left out of the
    /// registry and of the prompt: a tool by name, a skill as `skill:<name>`.
    fn resolve_without(
        &self,
        name: &str,
        off: &std::collections::BTreeSet<String>,
    ) -> Option<AgentProfile> {
        if name != DEFAULT_AGENT && !self.dirs.agents().iter().any(|n| n == name) {
            return None;
        }
        let definition = self.dirs.agent(name);
        // An external agent brings its own tools; ours would only confuse it.
        let backend: Option<BackendFactory> = definition.spec.acp.clone().map(|config| {
            let agent = name.to_string();
            Arc::new(move |setup: termide_agent_core::BackendSetup| {
                AcpRuntime::start(&agent, &config, setup)
                    .map(|runtime| Box::new(runtime) as Box<dyn termide_agent_core::Backend>)
            }) as BackendFactory
        });
        let mut tools = if backend.is_some() {
            termide_agent_core::ToolRegistry::new()
        } else {
            base_tools(&self.dirs, self.web.as_ref(), self.recall.as_ref())
        };
        // Someone watches the panel's agent to answer its questions and to
        // confirm a command it is offered; a subagent and headless mode build
        // their tools without either.
        if backend.is_none() {
            tools.insert(Arc::new(QuestionTool));
            tools.insert(Arc::new(SuggestCommandTool));
        }
        // The `task` tool lets this agent hand work to the others, and its
        // `tools` list can leave it out like any other capability; only when
        // there are custom agents to delegate to, and never for an external
        // agent (it drives its own tools) or a subagent (no nesting: the
        // subagent build path adds no task tool).
        if let Some(subagents) = &self.subagents {
            if backend.is_none() {
                let delegates = self.delegatable(name);
                if !delegates.is_empty() {
                    let runner = Arc::clone(subagents);
                    let run: SubagentRun =
                        Arc::new(move |agent, prompt, ctx, cancel, on_update| {
                            runner.run(agent, prompt, ctx, cancel, on_update)
                        });
                    tools.insert(Arc::new(TaskTool::new(delegates, run)));
                }
            }
        }
        restrict_tools(&mut tools, &definition.spec.tools, name);
        // Skills are instructions, not a capability, so an agent's `tools`
        // list does not govern them: the tool comes with the skills.
        let all_skills = self.dirs.skills();
        let skill_names: Vec<String> = all_skills.iter().map(|skill| skill.name.clone()).collect();
        let skills: Vec<_> = all_skills
            .into_iter()
            .filter(|skill| !off.contains(&format!("skill:{}", skill.name)))
            .collect();
        if !skill_names.is_empty() && backend.is_none() {
            tools.insert(Arc::new(SkillTool::new(skills.clone())));
        }
        // What the session switched off leaves the registry here, before the
        // prompt lists the tools; a `skill` tool left with no skills goes too.
        let offered: Vec<String> = tools.iter().map(|tool| tool.name().to_string()).collect();
        for tool in off {
            tools.remove(tool);
        }
        if skills.is_empty() {
            tools.remove("skill");
        }
        // The user's `ai/tools/<name>.md` texts, before the prompt lists them.
        apply_tool_texts(&mut tools, &self.dirs.tool_texts());
        // The default agent's `AGENT.md` is the prompt template itself, not an
        // instruction file, so no global file joins the chain.
        let context_files = discover_context_files(&self.cwd, Some(&self.project_root), None);
        let mut options = PromptOptions::new(&self.cwd, &tools, &context_files);
        options.skills = &skills;
        options.soul = definition.soul.as_deref();
        Some(AgentProfile {
            system_prompt: build_system_prompt(&options),
            tools,
            model: definition.spec.model,
            mode: definition.spec.mode,
            // Subscribed with no servers too: a reload may bring some.
            late_tools: backend.is_none().then(|| self.mcp.subscribe()),
            backend,
            offered,
            skills: skill_names,
        })
    }
}

impl FsCatalog {
    /// The agents `caller` can delegate to with the `task` tool: every
    /// built-in-loop agent, the default one included, except `caller`
    /// itself (delegating to yourself is pointless) and external ones.
    fn delegatable(&self, caller: &str) -> Vec<(String, String)> {
        self.dirs
            .agents()
            .into_iter()
            .filter(|name| name != caller && self.dirs.spec(name).acp.is_none())
            .map(|name| {
                let description = self.dirs.spec(&name).description;
                (name, description)
            })
            .collect()
    }
}

/// The built-in tools followed by the web tools, before an agent's `tools`
/// list narrows them.
fn base_tools(
    dirs: &AgentDirs,
    web: Option<&Arc<Web>>,
    recall: Option<&Arc<RecallTool>>,
) -> ToolRegistry {
    let mut tools = builtin_tools(dirs.shims_dir());
    for tool in web.map(web_tools).unwrap_or_default() {
        tools.insert(tool);
    }
    if let Some(recall) = recall {
        tools.insert(Arc::clone(recall) as Arc<dyn termide_agent_core::Tool>);
    }
    tools
}

/// The web service of the process and the settings it was built from.
static SHARED: std::sync::Mutex<Option<(WebConfig, Arc<Web>)>> = std::sync::Mutex::new(None);

/// The web service the agents share, once an agent panel has created it.
fn current_web() -> Option<Arc<Web>> {
    SHARED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|(_, web)| Arc::clone(web))
}

/// For the AI menu: whether the agents' browser is shown in a window, `None`
/// when there is no browser to show (no agent yet, or no browser at all).
#[must_use]
pub fn web_browser_shown() -> Option<bool> {
    current_web()
        .filter(|web| web.can_show())
        .map(|web| web.is_watching())
}

/// Show or hide the agents' browser window; returns whether it is now shown.
pub(crate) fn toggle_web_browser() -> Option<bool> {
    let web = current_web().filter(|web| web.can_show())?;
    let shown = !web.is_watching();
    web.set_watching(shown);
    Some(shown)
}

/// The web service every agent of the process shares, so one browser serves
/// them all. It is rebuilt only when the settings that shape it change; a
/// panel still holding the old one keeps it until it closes.
fn shared_web(settings: &WebSettings, dirs: &AgentDirs) -> Arc<Web> {
    let config = web_config(settings, dirs);
    let mut shared = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
    match shared.as_ref() {
        Some((current, web)) if *current == config => Arc::clone(web),
        _ => {
            let web = Web::new(config.clone());
            *shared = Some((config, Arc::clone(&web)));
            web
        }
    }
}

fn web_config(settings: &WebSettings, dirs: &AgentDirs) -> WebConfig {
    let engine = dirs.web_engine(&settings.engine).and_then(|text| {
        termide_agent_web::Engine::from_toml(&text)
            .map_err(|error| log::warn!("search engine {}: {error}", settings.engine))
            .ok()
    });
    if engine.is_none() {
        log::warn!("no usable search engine named {}", settings.engine);
    }
    WebConfig {
        backend: termide_agent_web::Backend::parse(&settings.backend),
        engine,
        chrome_path: (!settings.chrome_path.trim().is_empty())
            .then(|| PathBuf::from(settings.chrome_path.trim())),
        display: termide_agent_web::Display::parse(&settings.display),
        profile: dirs
            .browser_profile()
            .unwrap_or_else(|| std::env::temp_dir().join("termide-browser")),
    }
}

/// Drop every tool not in `allowed`, warning about names that match none;
/// `None` keeps them all. Shared by the catalog and the subagent builder so
/// an agent's `tools` list means the same in both.
fn restrict_tools(tools: &mut ToolRegistry, allowed: &Option<Vec<String>>, agent: &str) {
    let Some(allowed) = allowed else { return };
    for unknown in allowed.iter().filter(|name| tools.get(name).is_none()) {
        log::warn!("agent {agent}: no tool named {unknown}");
    }
    for tool in tools
        .names()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>()
    {
        if !allowed.contains(&tool) {
            tools.remove(&tool);
        }
    }
}

/// Builds a named agent and runs it to completion as a subagent: the engine
/// behind the `task` tool. It shares the provider and the permission rules
/// with the panel, but has no one to prompt, so anything the rules and mode
/// do not already allow is refused.
/// The provider the panel runs on right now, with its model and context
/// window: what a delegated task inherits. A profile switch updates it.
#[derive(Clone)]
struct Active {
    /// The connection's name, whose `subagents` field picks where a
    /// delegated task runs.
    connection: String,
    /// The connection's provider kind: a CLI agent's `provider` is only a
    /// placeholder, which no subagent can run on.
    kind: String,
    provider: Arc<dyn Provider>,
    model: String,
    context_window: u64,
}

type ActiveSlot = Arc<std::sync::RwLock<Active>>;

/// The `[ai]` settings as last applied while termide runs. A panel's
/// connection picker reads them from here, so a connection added in the
/// settings modal is offered by the panels already open; `None` until the
/// settings change after startup. Global because panels restored from a
/// layout are built away from the `App`.
static APPLIED_AI: std::sync::RwLock<Option<AiSettings>> = std::sync::RwLock::new(None);

/// Record `settings` as the applied `[ai]` settings for open panels.
pub(crate) fn publish_ai_settings(settings: &AiSettings) {
    *APPLIED_AI
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(settings.clone());
    // A limit changed in the settings reaches the requests already queued.
    for (name, connection) in &settings.connections {
        if !connection.is_cli() {
            let _ = Slots::shared(name, connection.max_concurrent_requests as usize);
        }
    }
}

/// The settings applied last, else `built_with`, those a panel was built with.
fn applied_ai_settings(built_with: &AiSettings) -> AiSettings {
    APPLIED_AI
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .unwrap_or_else(|| built_with.clone())
}

/// The connections of `[ai]` for the panel's picker, built on demand.
struct AiConnections {
    /// The settings the panel was built with, until others are applied.
    settings: AiSettings,
    /// Where the reviewer's texts come from.
    dirs: AgentDirs,
    /// Shared with the subagent runner, so a switch reaches delegated tasks.
    active: ActiveSlot,
}

impl AiConnections {
    /// The settings applied last, else those the panel was built with.
    fn settings(&self) -> AiSettings {
        applied_ai_settings(&self.settings)
    }
}

impl ConnectionCatalog for AiConnections {
    fn list(&self) -> Vec<ConnectionEntry> {
        self.settings()
            .connections
            .iter()
            .map(|(name, connection)| ConnectionEntry {
                name: name.clone(),
                kind: connection.provider.clone(),
                model: connection.model.clone(),
            })
            .collect()
    }

    fn build(&self, name: &str, agent: &str) -> Option<ConnectionChoice> {
        let settings = self.settings();
        let connection = settings.connections.get(name)?;
        Some(ConnectionChoice {
            name: name.to_string(),
            kind: connection.provider.clone(),
            provider: connection_provider(name, connection),
            model: connection.model.clone(),
            context_window: connection.effective_context_window(),
            backend: cli_provider_backend(&connection.provider, agent),
            reviewer: reviewer_setup(&settings, &self.dirs, name),
        })
    }

    fn activate(&self, choice: &ConnectionChoice) {
        *self
            .active
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Active {
            connection: choice.name.clone(),
            kind: choice.kind.clone(),
            provider: Arc::clone(&choice.provider),
            model: choice.model.clone(),
            context_window: choice.context_window,
        };
    }
}

/// The connection new sessions start on, if there is any.
fn usable_connection(settings: &AiSettings) -> Option<(&str, &Connection)> {
    let name = settings.default_connection()?;
    Some((name, &settings.connections[name]))
}

/// `model`, or — left empty, to the provider — the first model the provider
/// lists; `None` when it lists none. Blocks on the request, so it runs on a
/// worker thread.
fn resolve_model(provider: &dyn Provider, model: &str) -> Option<String> {
    if !model.trim().is_empty() {
        return Some(model.to_string());
    }
    provider
        .list_models()
        .ok()?
        .into_iter()
        .next()
        .map(|model| model.id)
}

/// The connection `session` runs on: the one it recorded while that still
/// exists, else the one new sessions start on.
fn session_connection(
    settings: &AiSettings,
    session: Option<&Session>,
) -> Option<(String, Connection)> {
    let recorded = session
        .and_then(Session::current_connection)
        .filter(|name| settings.connections.contains_key(name));
    // A log that names no connection (one begun before connections were
    // recorded) still names the provider its model ran on: a connection of
    // that provider fits it, the one new sessions start on first.
    let by_provider = || {
        let provider = session.and_then(Session::current_model)?.provider;
        settings
            .default_connection()
            .filter(|name| settings.connections[*name].provider == provider)
            .or_else(|| {
                settings
                    .connections
                    .iter()
                    .find(|(_, c)| c.provider == provider)
                    .map(|(name, _)| name.as_str())
            })
            .map(str::to_string)
    };
    let name = recorded
        .or_else(by_provider)
        .or_else(|| settings.default_connection().map(str::to_string))?;
    let connection = settings.connections[&name].clone();
    Some((name, connection))
}

/// The API key the connection names, read from its environment variable.
fn api_key_of(connection: &Connection) -> Option<String> {
    (!connection.api_key_env.is_empty())
        .then(|| std::env::var(&connection.api_key_env).ok())
        .flatten()
}

/// A side call's provider when its connection names no model: the first
/// model the endpoint lists answers, as it does for a session. Asked once, on
/// the first call — side calls run on the agent's thread, never the UI's, so
/// the request may block there.
struct FirstListedModel {
    inner: Arc<dyn Provider>,
    model: std::sync::OnceLock<Option<String>>,
}

impl Provider for FirstListedModel {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn stream(
        &self,
        request: &termide_agent_core::Request<'_>,
        on_event: &mut dyn FnMut(termide_agent_core::StreamEvent),
        cancel: &CancelToken,
    ) -> termide_agent_core::AssistantMessage {
        let listed = request
            .model
            .id
            .trim()
            .is_empty()
            .then(|| {
                self.model
                    .get_or_init(|| resolve_model(self.inner.as_ref(), ""))
                    .clone()
            })
            .flatten();
        let Some(id) = listed else {
            return self.inner.stream(request, on_event, cancel);
        };
        let model = ModelSpec {
            id,
            ..request.model.clone()
        };
        let request = termide_agent_core::Request {
            model: &model,
            system_prompt: request.system_prompt,
            messages: request.messages,
            tools: request.tools,
            thinking: request.thinking,
        };
        self.inner.stream(&request, on_event, cancel)
    }

    fn endpoint(&self) -> Option<String> {
        self.inner.endpoint()
    }

    fn list_models(&self) -> Result<Vec<termide_agent_core::ModelInfo>, String> {
        self.inner.list_models()
    }

    fn thinking_levels(&self, model: &str) -> Vec<ThinkingLevel> {
        self.inner.thinking_levels(model)
    }
}

/// The model a side call (`purpose` names it in warnings) runs on: the
/// connection `connection` names with `model` in place of its own when one is
/// given — a CLI agent's through its subscription, see [`AcpProvider`]; or,
/// with no connection named, the session's under `model`. A name that matches
/// no connection falls back to the session's model.
fn side_model(
    settings: &AiSettings,
    choice: &termide_config::SideModel,
    purpose: &str,
) -> ModelChoice {
    let name = choice.connection.trim();
    let model = choice.model.trim();
    let session = || ModelChoice {
        own: None,
        session_model: (!model.is_empty()).then(|| model.to_string()),
    };
    match settings.connections.get(name) {
        _ if name.is_empty() => session(),
        Some(connection) if !termide_config::is_cli_provider(&connection.provider) => {
            let id = if model.is_empty() {
                connection.model.clone()
            } else {
                model.to_string()
            };
            let spec = ModelSpec {
                provider: "agent".to_string(),
                id: id.clone(),
                context_window: connection.effective_context_window(),
                max_tokens: None,
                thinking: ThinkingLevel::Off,
            };
            let mut provider = connection_provider(name, connection);
            if id.trim().is_empty() {
                provider = Arc::new(FirstListedModel {
                    inner: provider,
                    model: std::sync::OnceLock::new(),
                });
            }
            ModelChoice::own(provider, spec)
        }
        Some(connection) => match cli_acp_config(&connection.provider) {
            Some(config) => {
                let id = if model.is_empty() {
                    connection.model.trim().to_string()
                } else {
                    model.to_string()
                };
                let spec = ModelSpec {
                    provider: "agent".to_string(),
                    id,
                    context_window: 0,
                    max_tokens: None,
                    thinking: ThinkingLevel::Off,
                };
                let cwd = std::env::current_dir().unwrap_or_default();
                let provider = AcpProvider::new(name, config, cwd);
                ModelChoice::own(Arc::new(provider), spec)
            }
            None => {
                log::warn!("{purpose} names {name:?}, which no side call can use");
                ModelChoice::default()
            }
        },
        None => {
            log::warn!("{purpose} names no model connection {name:?}; the session's model serves");
            ModelChoice::default()
        }
    }
}

/// The `auto` mode reviewer: the texts of `system/classify.md`, and the
/// model `[ai.auto_reviewer]` names, see [`side_model`].
fn reviewer_setup(settings: &AiSettings, dirs: &AgentDirs, session: &str) -> ReviewerSetup {
    ReviewerSetup {
        prompt: dirs.classify_prompt(),
        model: side_model(
            settings,
            &settings.reviewer_of(session),
            &format!("the reviewer of connection {session}"),
        ),
    }
}

/// The `recall` tool of a project: its session logs, the repositories it
/// holds, the panel's directory at `cwd` when that lies outside it, and the
/// solver when `[ai.recall]` turns it on.
fn recall_tool(
    settings: &AiSettings,
    dirs: &AgentDirs,
    project_root: &Path,
    cwd: &Path,
) -> Arc<RecallTool> {
    let project_root =
        dunce::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let panel_dir = dunce::canonicalize(cwd)
        .ok()
        .filter(|dir| !dir.starts_with(&project_root))
        .map(|dir| PanelDir {
            dir,
            sessions_dir: session_dir_of(cwd),
        });
    let solver = settings.recall.solver.then(|| Solver {
        prompt: dirs.recall_prompt(),
        model: side_model(
            settings,
            &termide_config::SideModel {
                connection: settings.recall.connection.clone(),
                model: settings.recall.model.clone(),
            },
            "[ai.recall]",
        ),
    });
    // 0 is no limit.
    let seconds = |secs: u64| (secs > 0).then(|| std::time::Duration::from_secs(secs));
    Arc::new(RecallTool::new(RecallSetup {
        sessions_dir: session_dir_of(&project_root),
        project_root,
        // Found at each search, on the agent's thread: a repository made
        // after the panel opened is searched too.
        find_repos: RepoFinder::new(termide_git::project_repos),
        panel_dir,
        solver,
        time_limits: TimeLimits {
            sessions: seconds(settings.recall.sessions_timeout_secs),
            git: seconds(settings.recall.git_timeout_secs),
            files: seconds(settings.recall.files_timeout_secs),
        },
    }))
}

struct Subagents {
    active: ActiveSlot,
    /// The `[ai]` settings the panel was built with, until others are
    /// applied: where the session's connection sends its subagents.
    settings: AiSettings,
    dirs: AgentDirs,
    web: Arc<Web>,
    recall: Arc<RecallTool>,
    /// The panel's MCP servers: a subagent gets the tools of those connected
    /// when its task starts.
    mcp: Arc<Connections>,
    cwd: PathBuf,
    project_root: PathBuf,
    rules: PermissionRules,
    /// The session's live permission mode, which a delegated task runs in
    /// unless its definition names its own.
    mode: termide_agent_core::ModeHandle,
    max_tokens: Option<u64>,
    reasoning: ThinkingLevel,
    compaction: CompactionPolicy,
    /// What the subagent's model reads when a call is refused.
    refusals: Refusals,
}

/// The mode a subagent runs in: its own, else the delegating agent's. Plan
/// mode lets a delegation through only because the subagent cannot change
/// anything either, so it overrides the agent's own mode.
fn subagent_mode(parent: Mode, own: Option<Mode>) -> Mode {
    match parent {
        Mode::Plan => Mode::Plan,
        parent => own.unwrap_or(parent),
    }
}

/// A runaway subagent is cut off after this many model calls.
const SUBAGENT_MAX_TURNS: usize = 50;

/// Where a task delegated from a session runs.
#[derive(Debug, PartialEq)]
enum SubagentTarget {
    /// termide's loop on the session's own model connection.
    Session,
    /// termide's loop on another model connection, by name.
    Model(String, Connection),
    /// A session of its own of Claude Code on this connection, by name,
    /// calling termide's tools.
    Cli(String, Connection),
}

/// Where a task delegated from a session on `active` runs: on the connection
/// its connection's `subagents` field names, else on the session's own — for
/// a Claude Code session a copy of Claude Code. Codex and Gemini CLI keep
/// tools of their own and run none: one named there is passed over, and a
/// session on one with nothing usable named has nowhere to run them, which
/// the error says.
fn subagent_target(settings: &AiSettings, active: &Active) -> Result<SubagentTarget, String> {
    let session = active.connection.as_str();
    let named = settings
        .connections
        .get(session)
        .map(|connection| connection.subagents.trim())
        .unwrap_or_default();
    if !named.is_empty() && named != session {
        match settings.connections.get(named) {
            Some(connection) if !connection.is_cli() => {
                return Ok(SubagentTarget::Model(named.to_string(), connection.clone()));
            }
            Some(connection) if connection.runs_subagents() => {
                return Ok(SubagentTarget::Cli(named.to_string(), connection.clone()));
            }
            Some(_) => log::warn!(
                "connection {session}: subagents names {named}, a CLI agent, which cannot run them"
            ),
            None => log::warn!("connection {session}: subagents names no connection {named:?}"),
        }
    }
    if !termide_config::is_cli_provider(&active.kind) {
        return Ok(SubagentTarget::Session);
    }
    let own = settings
        .connections
        .get(session)
        .cloned()
        .unwrap_or_else(|| Connection {
            provider: active.kind.clone(),
            ..Connection::default()
        });
    if own.runs_subagents() {
        return Ok(SubagentTarget::Cli(session.to_string(), own));
    }
    Err(format!(
        "the connection {session} is a CLI agent on tools of its own, which cannot run \
         subagents; a model connection or a Claude Code one chosen under Subagents on its \
         settings page runs them"
    ))
}

/// Why a run ended without an answer, when its last reply says.
fn run_error(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::Assistant(assistant) => Some(assistant.error_message.clone()),
            _ => None,
        })
        .flatten()
}

/// A subagent's progress as the delegating call shows it: what it has said
/// so far, and its answer — the last text it said.
#[derive(Default)]
struct SubagentReport {
    progress: String,
    answer: Option<String>,
    error: Option<String>,
    turns: usize,
    /// What its model calls spent so far.
    spent: Usage,
}

impl SubagentReport {
    /// Note a finished message of the subagent's.
    fn said(&mut self, message: &AssistantMessage, on_update: &mut dyn FnMut(ToolUpdate)) {
        self.spent.add(&message.usage);
        let text = message.plain_text();
        let text = text.trim();
        if text.is_empty() {
            self.error = message.error_message.clone();
            return;
        }
        self.error = None;
        if !self.progress.is_empty() {
            self.progress.push_str("\n\n");
        }
        self.progress.push_str(text);
        self.answer = Some(text.to_string());
        on_update(ToolUpdate::Output(self.progress.clone()));
    }

    /// Show, under what the subagent said, that its request waits for a
    /// free slot of its connection (`ahead` others before it), or no longer.
    fn waiting(&self, ahead: Option<usize>, on_update: &mut dyn FnMut(ToolUpdate)) {
        let t = termide_i18n::t();
        let line = match ahead {
            None => None,
            Some(0) => Some(t.agent_queued().to_string()),
            Some(ahead) => Some(t.agent_queued_ahead_fmt(ahead)),
        };
        let shown = match (line, self.progress.is_empty()) {
            (None, _) => self.progress.clone(),
            (Some(line), true) => format!("⏳ {line}"),
            (Some(line), false) => format!("{}\n\n⏳ {line}", self.progress),
        };
        on_update(ToolUpdate::Output(shown));
    }

    /// The delegating call's result.
    fn into_outcome(self, stopped: bool) -> SubagentOutcome {
        let spent = self.spent;
        let answer = match self.answer {
            Some(text) if self.turns > SUBAGENT_MAX_TURNS => Ok(format!(
                "{text}\n\n(subagent stopped after {SUBAGENT_MAX_TURNS} steps)"
            )),
            Some(text) => Ok(text),
            None if stopped => Err("the subagent was stopped".into()),
            None => Err(self.error.map_or_else(
                || "the subagent produced no answer".to_string(),
                |error| format!("the subagent failed: {error}"),
            )),
        };
        SubagentOutcome { answer, spent }
    }
}

impl Subagents {
    /// The tools, the skills and the system prompt the agent `name` runs
    /// with as a subagent: no `task` (delegation does not nest), and no
    /// `question` or `suggest_command` (nobody watches it).
    fn toolset(
        &self,
        name: &str,
        definition: &termide_agent_core::AgentDefinition,
    ) -> (ToolRegistry, Vec<termide_agent_core::SkillInfo>, String) {
        let mut tools = base_tools(&self.dirs, Some(&self.web), Some(&self.recall));
        restrict_tools(&mut tools, &definition.spec.tools, name);
        let skills = self.dirs.skills();
        if !skills.is_empty() {
            tools.insert(Arc::new(SkillTool::new(skills.clone())));
        }
        apply_tool_texts(&mut tools, &self.dirs.tool_texts());
        let context_files = discover_context_files(&self.cwd, Some(&self.project_root), None);
        let mut options = PromptOptions::new(&self.cwd, &tools, &context_files);
        options.skills = &skills;
        options.soul = definition.soul.as_deref();
        let system_prompt = build_system_prompt(&options);
        // As for the panel's agent, the MCP servers' tools are neither listed
        // in the prompt nor narrowed by the agent's `tools` list.
        for tool in self.mcp.ready_tools() {
            tools.insert(tool);
        }
        (tools, skills, system_prompt)
    }

    fn run(
        &self,
        name: &str,
        prompt: &str,
        ctx: &ToolContext,
        cancel: &CancelToken,
        on_update: &mut dyn FnMut(ToolUpdate),
    ) -> SubagentOutcome {
        self.delegate(name, prompt, ctx, cancel, on_update)
            .unwrap_or_else(|error| Err(error).into())
    }

    /// Run the task; an error is one that came before anything was spent.
    fn delegate(
        &self,
        name: &str,
        prompt: &str,
        ctx: &ToolContext,
        cancel: &CancelToken,
        on_update: &mut dyn FnMut(ToolUpdate),
    ) -> Result<SubagentOutcome, String> {
        let definition = self.dirs.agent(name);
        if definition.spec.acp.is_some() {
            return Err(format!(
                "{name} is an external agent and cannot be run as a subagent"
            ));
        }
        let (tools, skills, system_prompt) = self.toolset(name, &definition);

        let active = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let settings = applied_ai_settings(&self.settings);
        let target = subagent_target(&settings, &active)?;
        // The session's connection says who reviews, wherever the task runs.
        let reviewer = reviewer_setup(&settings, &self.dirs, &active.connection);

        let mut rules = self.rules.clone();
        rules.mode = subagent_mode(self.mode.get(), definition.spec.mode);
        let plan_guard = PlanGuard::new(ModeHandle::new(rules.mode)).with_refusals(&self.refusals);
        // Stops a run that will not stop itself; the parent's cancel stops
        // it too.
        let budget = CancelToken::new();
        let permissions = PermissionHooks::new(
            rules.clone(),
            Box::new(AutoDenyPrompter::new(
                self.refusals.unattended_subagent.clone(),
            )),
        )
        .with_classifier(Box::new(reviewer.classifier(budget.clone())))
        .with_refusals(self.refusals.clone());
        // The guard goes first, as in the panel's own chain: in plan mode a
        // session answer must not let a change through.
        let hooks = ChainedHooks::new(vec![Box::new(plan_guard), Box::new(permissions)]);

        let (provider, requested, context_window) = match target {
            SubagentTarget::Cli(connection_name, connection) => {
                let run = CliSubagent {
                    name: format!("{connection_name}:{name}"),
                    reviewer,
                    connection,
                    system_prompt,
                    tools,
                    skills,
                    hooks,
                    rules,
                };
                return self.run_cli(run, prompt, &budget, cancel, on_update);
            }
            // On a connection of its own the subagent runs that connection's
            // model: the id its `AGENT.md` names is one of the session's
            // endpoint.
            SubagentTarget::Model(name, connection) => (
                connection_provider(&name, &connection),
                connection.model.clone(),
                connection.effective_context_window(),
            ),
            SubagentTarget::Session => (
                Arc::clone(&active.provider),
                definition
                    .spec
                    .model
                    .clone()
                    .unwrap_or_else(|| active.model.clone()),
                active.context_window,
            ),
        };
        let Some(id) = resolve_model(provider.as_ref(), &requested) else {
            return Err("the provider lists no model to run the subagent on".into());
        };
        let model = ModelSpec {
            provider: "agent".to_string(),
            id,
            context_window,
            max_tokens: self.max_tokens,
            thinking: self.reasoning,
        };
        // The reviewer judges the subagent's calls against what the user asked
        // the delegating agent; the task itself counts as that agent's words.
        let parent = ctx
            .session
            .as_ref()
            .map(|session| IntentLog::delegated(&session.intent))
            .unwrap_or_default();
        let mut agent = Agent::new(provider, tools, model, self.cwd.clone())
            .with_system_prompt(system_prompt)
            .with_compaction(self.compaction)
            .with_delegated_intent(parent);

        let mut hooks = hooks;
        let mut report = SubagentReport::default();
        let finished = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            // The parent's stop reaches the run at once, not at its next
            // event: a long prefill or tool sends none.
            scope.spawn(|| {
                while !finished.load(std::sync::atomic::Ordering::Acquire) {
                    if cancel.is_cancelled() {
                        budget.cancel();
                        return;
                    }
                    std::thread::sleep(CLI_SUBAGENT_POLL);
                }
            });
            let mut emit = |event: AgentEvent| match &event {
                AgentEvent::MessageStart { .. } => {
                    report.turns += 1;
                    if report.turns > SUBAGENT_MAX_TURNS {
                        budget.cancel();
                    }
                }
                AgentEvent::MessageUpdate(StreamEvent::Queued { ahead }) => {
                    report.waiting(Some(*ahead), on_update);
                }
                AgentEvent::MessageUpdate(StreamEvent::Admitted) => {
                    report.waiting(None, on_update);
                }
                AgentEvent::MessageEnd(Message::Assistant(message)) => {
                    report.said(message, on_update);
                }
                _ => {}
            };
            agent.run(UserMessage::text(prompt), &mut hooks, &budget, &mut emit);
            finished.store(true, std::sync::atomic::Ordering::Release);
        });
        if report.answer.is_none() && report.error.is_none() {
            report.error = run_error(agent.messages());
        }
        Ok(report.into_outcome(cancel.is_cancelled()))
    }

    /// Run `run` as a session of its own of its CLI agent, which calls the
    /// subagent's tools through its hooks: the agent's model when the
    /// connection names one, nobody to ask — the agent's own permission
    /// requests are refused with the reason a subagent's calls get — and the
    /// process gone once the task is.
    fn run_cli(
        &self,
        run: CliSubagent,
        prompt: &str,
        budget: &CancelToken,
        cancel: &CancelToken,
        on_update: &mut dyn FnMut(ToolUpdate),
    ) -> Result<SubagentOutcome, String> {
        let config = cli_acp_config(&run.connection.provider)
            .ok_or_else(|| format!("{} is not a CLI agent", run.connection.provider))?;
        let (prompter, permission_rx) = permission_channel(budget.clone());
        let setup = BackendSetup {
            cwd: self.cwd.clone(),
            prompter,
            cancel: budget.clone(),
            mode: ModeHandle::new(run.rules.mode),
            rules: run.rules,
            persist: None,
            system_prompt: run.system_prompt,
            plan: self.dirs.plan_prompt(),
            goal: self.dirs.goal_prompt(),
            handoff: self.dirs.handoff_prompt(),
            reviewer: run.reviewer,
            host_tools: Some(HostTools {
                tools: run.tools,
                hooks: Box::new(run.hooks),
                context: ToolContext::new(self.cwd.clone()),
                skills: run.skills,
            }),
            resume: None,
            history: Vec::new(),
        };
        let runtime = AcpRuntime::start(&run.name, &config, setup)?;
        let drive = CliDrive {
            name: &run.name,
            model: run.connection.model.trim(),
            permission_rx: &permission_rx,
            refusal: &self.refusals.unattended_subagent,
            budget,
            cancel,
        };
        // The process goes with the runtime, once the task is over.
        drive.run(&runtime, &|| runtime.is_starting(), prompt, on_update)
    }
}

/// The run of a subagent on a CLI agent once its process is up.
struct CliDrive<'a> {
    name: &'a str,
    /// The model to pick for the task, empty for the agent's own.
    model: &'a str,
    /// The agent's own permission requests, refused: nobody watches.
    permission_rx: &'a std::sync::mpsc::Receiver<termide_agent_core::PermissionEnvelope>,
    refusal: &'a str,
    budget: &'a CancelToken,
    cancel: &'a CancelToken,
}

impl CliDrive<'_> {
    /// Pick the model, hand `backend` the task and follow it to its end,
    /// reporting what it says; `starting` says whether its handshake is
    /// still under way.
    fn run(
        &self,
        backend: &dyn Backend,
        starting: &dyn Fn() -> bool,
        prompt: &str,
        on_update: &mut dyn FnMut(ToolUpdate),
    ) -> Result<SubagentOutcome, String> {
        let stopped = || self.cancel.is_cancelled() || self.budget.is_cancelled();
        // The model goes before the task does, so the whole task runs on it.
        if !self.model.is_empty() {
            while starting() && !stopped() {
                std::thread::sleep(CLI_SUBAGENT_POLL);
            }
            let models = backend.available_models();
            match termide_agent_acp::match_model(self.model, &models) {
                Some(id) => {
                    if let Err(error) = backend.select_model(id) {
                        log::warn!(
                            "subagent {}: cannot pick {}: {error}",
                            self.name,
                            self.model
                        );
                    }
                }
                None if !models.is_empty() => log::warn!(
                    "subagent {}: no model {}; its default runs",
                    self.name,
                    self.model
                ),
                None => {}
            }
        }
        if stopped() {
            return Err("the subagent was stopped".into());
        }
        backend
            .prompt(UserMessage::text(prompt))
            .map_err(|error| format!("the subagent did not start: {error:?}"))?;
        let mut report = SubagentReport::default();
        let mut aborted: Option<std::time::Instant> = None;
        let mut idle = 0;
        'run: loop {
            for envelope in self.permission_rx.try_iter() {
                let _ = envelope
                    .reply
                    .send(PermissionAnswer::DenyWithReason(self.refusal.to_string()));
            }
            for event in backend.drain() {
                match event {
                    AgentEvent::MessageEnd(Message::Assistant(message)) => {
                        report.turns += 1;
                        report.said(&message, on_update);
                        if report.turns > SUBAGENT_MAX_TURNS {
                            self.budget.cancel();
                        }
                    }
                    AgentEvent::AgentEnd => break 'run,
                    _ => {}
                }
            }
            // An agent that went away without saying it ended is over too,
            // as is one that does not end within a while of being stopped.
            idle = if backend.is_busy() { 0 } else { idle + 1 };
            if idle > 1 || aborted.is_some_and(|at| at.elapsed() > CLI_SUBAGENT_ABORT_WAIT) {
                break;
            }
            if aborted.is_none() && stopped() {
                aborted = Some(std::time::Instant::now());
                self.budget.cancel();
                backend.abort();
            }
            std::thread::sleep(CLI_SUBAGENT_POLL);
        }
        Ok(report.into_outcome(self.cancel.is_cancelled()))
    }
}

/// What a subagent on a CLI agent runs with, built as for termide's loop.
struct CliSubagent {
    /// How its process is named in the logs.
    name: String,
    /// Reviews its requests in `auto` mode.
    reviewer: ReviewerSetup,
    connection: Connection,
    system_prompt: String,
    tools: ToolRegistry,
    skills: Vec<termide_agent_core::SkillInfo>,
    hooks: ChainedHooks,
    rules: PermissionRules,
}

/// How often a subagent on a CLI agent is looked at while it runs.
const CLI_SUBAGENT_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// How long a stopped subagent on a CLI agent may take to end its turn.
const CLI_SUBAGENT_ABORT_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Everything the panel needs, resolved from `settings` for a panel working
/// in `cwd` inside the termide project at `project_root` as the agent named
/// `agent`: the provider, the model, the tools, the system prompt and where
/// the session logs live.
fn agent_setup(
    settings: &AiSettings,
    cwd: PathBuf,
    project_root: &Path,
    agent: &str,
    session: Option<Session>,
) -> AgentPanelSetup {
    // Callers open a panel only with a connection to run on.
    let (connection_name, connection) =
        session_connection(settings, session.as_ref()).unwrap_or_default();
    let provider_kind = connection.provider.clone();
    let provider = connection_provider(&connection_name, &connection);

    let mut catalog = FsCatalog::new(&cwd, project_root);
    let web = shared_web(&settings.web, &catalog.dirs);
    catalog.web = Some(Arc::clone(&web));
    let recall = recall_tool(settings, &catalog.dirs, project_root, &cwd);
    catalog.recall = Some(Arc::clone(&recall));
    // The subagent runner shares the provider, the rules and the model
    // defaults, so a delegated agent runs like the panel would run it.
    let active: ActiveSlot = Arc::new(std::sync::RwLock::new(Active {
        connection: connection_name.clone(),
        kind: provider_kind.clone(),
        provider: Arc::clone(&provider),
        model: connection.model.clone(),
        context_window: connection.effective_context_window(),
    }));
    let reviewer = reviewer_setup(settings, &catalog.dirs, &connection_name);
    catalog.subagents = Some(Arc::new(Subagents {
        active: Arc::clone(&active),
        settings: settings.clone(),
        dirs: catalog.dirs.clone(),
        web,
        recall,
        mcp: Arc::clone(&catalog.mcp),
        cwd: cwd.clone(),
        project_root: project_root.to_path_buf(),
        rules: settings.permissions.clone(),
        mode: termide_agent_core::ModeHandle::new(settings.permissions.mode),
        max_tokens: settings.output_limit(),
        reasoning: settings.reasoning,
        compaction: settings.compaction,
        refusals: catalog.dirs.refusals(),
    }));
    let compaction_prompts = catalog.dirs.compaction_prompts();
    let plan_prompt = catalog.dirs.plan_prompt();
    let goal_prompt = catalog.dirs.goal_prompt();
    let handoff_prompt = catalog.dirs.handoff_prompt();
    let refusals = catalog.dirs.refusals();
    let hooks: Option<HooksFactory> = {
        let configs = catalog.dirs.hooks();
        let hook_cwd = cwd.clone();
        (!configs.is_empty()).then(|| {
            Arc::new(move || {
                Box::new(CommandHooks::new(configs.clone(), hook_cwd.clone()))
                    as Box<dyn termide_agent_core::Hooks>
            }) as HooksFactory
        })
    };
    let (agent, profile) = match catalog.resolve(agent) {
        Some(profile) => (agent.to_string(), profile),
        None => {
            log::warn!("no agent named {agent}; using {DEFAULT_AGENT}");
            let profile = catalog
                .resolve(DEFAULT_AGENT)
                .expect("the default agent always resolves");
            (DEFAULT_AGENT.to_string(), profile)
        }
    };
    let model = ModelSpec {
        provider: "agent".to_string(),
        id: profile.model.unwrap_or_else(|| connection.model.clone()),
        context_window: connection.effective_context_window(),
        max_tokens: settings.output_limit(),
        thinking: settings.reasoning,
    };
    let mut rules = settings.permissions.clone();
    if let Some(mode) = profile.mode {
        rules.mode = mode;
    }

    // A CLI-adapter provider (Claude Code, Codex, Gemini CLI) is an explicit choice of
    // backend, so it drives its own ACP adapter — over any `command` the agent
    // definition might carry. Otherwise the agent's own backend (if any) wins.
    let provider_backend = cli_provider_backend(&provider_kind, &agent);
    let backend = provider_backend.clone().or(profile.backend);

    let session_dir = session_dir_of(&cwd);

    // Built before `cwd` moves into the setup: a hand-run command runs where
    // the panel works.
    let shell_run = user_shell_runner(&catalog.dirs, &cwd);
    // The reviewer of a connection switched to reads its texts from here.
    let connection_dirs = catalog.dirs.clone();

    AgentPanelSetup {
        cwd,
        agent,
        catalog: Arc::new(catalog),
        late_tools: profile.late_tools,
        hooks,
        backend,
        provider_backend,
        connections: Some(Arc::new(AiConnections {
            settings: settings.clone(),
            dirs: connection_dirs,
            active,
        })),
        connection: connection_name,
        provider,
        provider_kind,
        model,
        tools: profile.tools,
        rules,
        system_prompt: profile.system_prompt,
        compaction: settings.compaction,
        compaction_prompts,
        plan_prompt,
        goal_prompt,
        handoff_prompt,
        reviewer,
        refusals,
        fold: match settings.fold_blocks {
            termide_config::FoldBlocks::Immediately => FoldMode::Immediately,
            termide_config::FoldBlocks::OnFinish => FoldMode::OnFinish,
            termide_config::FoldBlocks::Never => FoldMode::Never,
        },
        persist_rule: Some(persist_rule),
        session_dir,
        session,
        shell_run: Some(shell_run),
    }
}

/// The runner behind `!` in the agent panel's input and `[Run]` on a
/// `suggest_command` card: the same `bash` tool the agent calls, so timeouts,
/// output cleaning and the full-log file behave identically either way. It is
/// built here rather than taken from the agent's registry because a command
/// the user ran is not governed by the tools an agent profile was given —
/// `restrict_tools` may well have left `bash` out of those.
/// Where the session logs of a panel working in `cwd` live: filed by that
/// directory, under the same `ai/` directory as the agents,
/// `<config>/ai/sessions/<path>/`.
fn session_dir_of(cwd: &Path) -> Option<PathBuf> {
    termide_config::get_config_dir().ok().map(|dir| {
        dir.join(GLOBAL_AGENT_DIR)
            .join(SESSIONS_DIR)
            .join(termide_project::project_key(cwd))
    })
}

fn user_shell_runner(dirs: &AgentDirs, cwd: &Path) -> ShellRunner {
    let tool = BashTool {
        shim_path: dirs.shims_dir(),
        ..BashTool::default()
    };
    let cwd = cwd.to_path_buf();
    ShellRunner::new(move |command, cancel| {
        let call = ToolCall {
            id: "user-command".into(),
            name: "bash".into(),
            arguments: serde_json::json!({ "command": command }),
            extra_content: None,
        };
        // The user wrote the command, so there is no one to ask about it and
        // nothing for the reviewer to weigh; it runs in the panel's directory.
        let ctx = ToolContext::new(cwd.clone());
        let result = tool.execute(&call, &ctx, &mut |_| {}, cancel);
        Ok(ShellOutput {
            text: result.plain_text(),
            failed: result.is_error,
        })
    })
}

/// The ACP backend for a CLI-adapter provider (`claude_code`, `codex`,
/// `gemini_cli`): the panel drives the tool's own ACP adapter (Gemini CLI
/// speaks ACP itself), which signs in with the user's CLI
/// login (a subscription or an API key — the adapter's concern, not ours).
/// `None` for any other provider, so the built-in loop is used.
fn cli_provider_backend(provider: &str, agent: &str) -> Option<BackendFactory> {
    let config = cli_acp_config(provider)?;
    let agent = agent.to_string();
    Some(Arc::new(move |setup: termide_agent_core::BackendSetup| {
        AcpRuntime::start(&agent, &config, setup)
            .map(|runtime| Box::new(runtime) as Box<dyn termide_agent_core::Backend>)
    }) as BackendFactory)
}

/// How a CLI-adapter provider's ACP adapter runs; `None` for any other
/// provider.
fn cli_acp_config(provider: &str) -> Option<AcpConfig> {
    // Claude Code takes termide's prompt and tools in place of its own; Codex
    // and Gemini CLI keep their own and have termide's permission mode mapped
    // onto their modes.
    let (package, flag, flavor) = match provider {
        "claude_code" => (
            "@agentclientprotocol/claude-agent-acp@latest",
            None,
            AcpFlavor::ClaudeCode,
        ),
        "codex" => (
            "@agentclientprotocol/codex-acp@latest",
            None,
            AcpFlavor::Codex,
        ),
        "gemini_cli" => (
            "@google/gemini-cli@latest",
            Some("--acp"),
            AcpFlavor::GeminiCli,
        ),
        _ => return None,
    };
    // The adapters ship on npm; `npx -y` fetches on first use. `@latest`,
    // because npx otherwise keeps running the copy it fetched first, and an
    // old adapter lists old models (and none at all, for Codex). A power
    // user who wants a pinned binary can point an agent's own `command` at it
    // and pick a compatible provider instead.
    Some(AcpConfig {
        command: "npx".to_string(),
        args: ["-y", package]
            .into_iter()
            .chain(flag)
            .map(str::to_string)
            .collect(),
        env: std::collections::BTreeMap::new(),
        timeout_secs: 120,
        flavor,
    })
}

/// Start an off-thread `list_models` for the settings modal's model dropdown
/// of `connection`. `None` for a CLI agent (its models come over ACP at
/// runtime). The receiver is polled by the app loop.
pub(crate) fn spawn_settings_model_fetch(
    connection: &Connection,
) -> Option<std::sync::mpsc::Receiver<Result<Vec<termide_agent_core::ModelInfo>, String>>> {
    if connection.is_cli() {
        return None;
    }
    let provider = build_provider(connection, api_key_of(connection));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(provider.list_models());
    });
    Some(rx)
}

/// The provider of connection `name` for a session or a side call: its
/// requests take the connection's slots, which every panel of the process
/// shares (see `max_concurrent_requests`). A CLI agent's is a placeholder
/// that serves no request, so it takes none.
fn connection_provider(name: &str, connection: &Connection) -> Arc<dyn Provider> {
    let provider = build_provider(connection, api_key_of(connection));
    if connection.is_cli() {
        return provider;
    }
    let slots = Slots::shared(name, connection.max_concurrent_requests as usize);
    Arc::new(SlottedProvider::new(provider, slots))
}

/// The provider `connection` names: the Anthropic Messages API, or the
/// OpenAI-compatible endpoint for everything else. An unknown name falls
/// back to OpenAI-compatible with a warning.
fn build_provider(connection: &Connection, api_key: Option<String>) -> Arc<dyn Provider> {
    let settings = connection;
    match settings.provider.trim().to_ascii_lowercase().as_str() {
        // A CLI-adapter provider runs over ACP; the built-in model provider is
        // unused, but something must be returned. A quiet OpenAI-compatible
        // placeholder avoids the "unknown provider" warning below.
        "claude_code" | "codex" | "gemini_cli" => Arc::new(
            OpenAiCompatProvider::new("agent", settings.base_url.clone()).with_api_key(api_key),
        ),
        "anthropic_compatible" | "anthropic" => {
            // The default base URL points at a local OpenAI server, which is
            // not Anthropic's; use the API root unless the user set another.
            let mut anthropic = if settings.base_url == default_openai_base_url() {
                AnthropicProvider::new("agent")
            } else {
                AnthropicProvider::with_base_url("agent", settings.base_url.clone())
            };
            anthropic = anthropic.with_api_key(api_key);
            Arc::new(anthropic)
        }
        other => {
            if !other.is_empty()
                && other != "openai_compatible"
                && other != "openai"
                && other != "openai-compatible"
            {
                log::warn!("unknown agent provider {other:?}; using the OpenAI-compatible one");
            }
            Arc::new(
                OpenAiCompatProvider::new("agent", settings.base_url.clone())
                    .with_api_key(api_key)
                    .with_compat(Compat {
                        reasoning: reasoning_param(settings),
                        prefill_progress: settings.prefill_progress,
                        ..Compat::default()
                    }),
            )
        }
    }
}

/// The field an OpenAI-compatible connection takes the reasoning level in.
/// Left to `auto`, the hosted APIs known to take `reasoning_effort` get it, a
/// server on this machine or the local network the chat template's
/// `enable_thinking` switch (llama.cpp, vLLM, omlx and LM Studio take it or
/// leave it be), and any other server nothing: an unknown field may fail its
/// requests.
fn reasoning_param(connection: &Connection) -> ReasoningParam {
    match connection.reasoning_param {
        termide_config::ReasoningParam::ReasoningEffort => ReasoningParam::Effort,
        termide_config::ReasoningParam::EnableThinking => ReasoningParam::EnableThinking,
        termide_config::ReasoningParam::None => ReasoningParam::None,
        termide_config::ReasoningParam::Auto => {
            let host = url_host(&connection.base_url);
            let hosted = [
                "api.openai.com",
                "openrouter.ai",
                "generativelanguage.googleapis.com",
            ]
            .iter()
            .any(|known| host == *known || host.ends_with(&format!(".{known}")));
            if hosted {
                ReasoningParam::Effort
            } else if is_local_host(&host) {
                ReasoningParam::EnableThinking
            } else {
                ReasoningParam::None
            }
        }
    }
}

/// The lowercased host of `url`, without its port or an IPv6 address's
/// brackets.
fn url_host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split('/').next().unwrap_or("");
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    host.to_ascii_lowercase()
}

/// Whether `host` is this machine or an address of a private network.
fn is_local_host(host: &str) -> bool {
    use std::net::IpAddr;
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        // Loopback, unique local (fc00::/7) and link-local (fe80::/10).
        Ok(IpAddr::V6(ip)) => {
            ip.is_loopback()
                || ip.segments()[0] & 0xfe00 == 0xfc00
                || ip.segments()[0] & 0xffc0 == 0xfe80
        }
        Err(_) => false,
    }
}

/// The shipped default base URL, used to tell "left at default" from "set on
/// purpose" when picking a provider.
fn default_openai_base_url() -> String {
    Connection::default().base_url
}

/// Persist an "allow always" rule as `[ai.permissions.<tool>]` in the
/// project's `.termide/config.toml` or the global configuration.
///
/// The rule is merged into the document rather than appended, so a repeated
/// grant updates it in place instead of writing a second `[ai.permissions.…]`
/// table (two tables of the same name are invalid TOML and would break the
/// whole file). `toml_edit` keeps the file's other settings, comments and
/// layout intact.
fn persist_rule(tool: &str, pattern: &str, decision: Decision, scope: PersistScope) {
    let path = match scope {
        PersistScope::Project => match std::env::current_dir() {
            Ok(cwd) => termide_config::project_config_path(&cwd),
            Err(_) => return,
        },
        PersistScope::Global => match termide_config::Config::config_file_path() {
            Ok(path) => path,
            Err(error) => {
                log::warn!("cannot find the global configuration: {error}");
                return;
            }
        },
    };
    if let Some(dir) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(dir) {
            log::warn!("cannot create {}: {error}", dir.display());
            return;
        }
    }
    if let Err(error) = merge_permission_rule(&path, tool, pattern, decision) {
        log::warn!(
            "cannot record the permission rule in {}: {error}",
            path.display()
        );
    }
}

/// Set `[ai.permissions.<tool>] <pattern> = <decision>` in the TOML document at
/// `path`, creating the file and the tables as needed. A file that does not
/// parse is left untouched (a hand-edit to fix) rather than appended to.
fn merge_permission_rule(path: &Path, tool: &str, pattern: &str, decision: Decision) -> Result<()> {
    use toml_edit::{value, DocumentMut, Item, Table};

    let value_str = match decision {
        Decision::Allow => "allow",
        Decision::Ask => "ask",
        Decision::Deny => "deny",
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let mut doc: DocumentMut = text.parse()?;

    // Walk to `ai.permissions.<tool>`, creating implicit tables on the way so
    // the sections merge with whatever the file already holds.
    let permissions = doc
        .entry("ai")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .and_then(|ai| {
            ai.entry("permissions")
                .or_insert_with(|| Item::Table(Table::new()))
                .as_table_mut()
        })
        .ok_or_else(|| anyhow::anyhow!("[ai] or [ai.permissions] is not a table"))?;
    permissions.set_implicit(true);
    let table = permissions
        .entry(tool)
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("[ai.permissions.{tool}] is not a table"))?;
    table.insert(pattern, value(value_str));

    std::fs::write(path, doc.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use termide_agent_core::{AGENT_FILE, DEFAULT_AGENT_FILE, SEED_TOOLS};

    #[test]
    fn a_side_call_runs_on_a_connection_a_subscription_or_the_sessions_model() {
        let mut settings = AiSettings::default();
        for (name, provider, model) in [
            ("local", "openai_compatible", "qwen"),
            ("claude", "claude_code", ""),
        ] {
            settings.connections.insert(
                name.to_string(),
                termide_config::Connection {
                    provider: provider.to_string(),
                    model: model.to_string(),
                    ..termide_config::Connection::default()
                },
            );
        }
        let pick = |connection: &str, model: &str| {
            side_model(
                &settings,
                &termide_config::SideModel {
                    connection: connection.into(),
                    model: model.into(),
                },
                "test",
            )
        };
        // An endpoint's model, or another of it.
        let own = |choice: ModelChoice| {
            let (provider, model) = choice.own.expect("a model of its own");
            (provider.name().to_string(), model.id)
        };
        assert_eq!(own(pick("local", "")).1, "qwen");
        assert_eq!(own(pick("local", "big")).1, "big");
        // A CLI agent through its subscription.
        assert_eq!(own(pick("claude", "haiku")), ("acp".into(), "haiku".into()));
        // None named: the session's, under another model when one is.
        let session = pick("", "haiku");
        assert!(session.own.is_none());
        assert_eq!(session.session_model.as_deref(), Some("haiku"));
        assert!(pick("", "").session_model.is_none());
        // A name that matches nothing: the session's.
        assert!(pick("gone", "").own.is_none());
    }

    /// Every built-in tool a panel can offer has a shipped text, and every
    /// shipped text a tool: a tool whose `ToolText::seed` name is wrong, or
    /// one added without its `assets/tools/<name>.md`, would otherwise reach
    /// the model with no description and stay off the prompt's tool list.
    #[test]
    fn every_built_in_tool_has_its_shipped_text() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = AgentDirs::new(tmp.path(), Some(tmp.path()), None);
        let web = termide_agent_web::Web::new(WebConfig {
            backend: termide_agent_web::Backend::Http,
            engine: None,
            chrome_path: None,
            display: termide_agent_web::Display::Headless,
            profile: tmp.path().join("browser"),
        });
        let recall = recall_tool(&AiSettings::default(), &dirs, tmp.path(), tmp.path());
        let mut tools = base_tools(&dirs, Some(&web), Some(&recall));
        // Offered only with a browser to search in, which a test has not.
        tools.insert(Arc::new(termide_agent_web::WebSearchTool::new(web)));
        tools.insert(Arc::new(QuestionTool));
        tools.insert(Arc::new(SuggestCommandTool));
        let run: SubagentRun = Arc::new(|_, _, _, _, _| unreachable!());
        tools.insert(Arc::new(TaskTool::new(
            vec![("review".into(), "Reviews".into())],
            run,
        )));
        tools.insert(Arc::new(SkillTool::new(Vec::new())));

        let mut names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
        names.sort_unstable();
        let mut shipped: Vec<&str> = SEED_TOOLS.iter().map(|(name, _)| *name).collect();
        shipped.sort_unstable();
        assert_eq!(names, shipped);
        for tool in tools.iter() {
            assert!(!tool.description().trim().is_empty(), "{}", tool.name());
            assert!(
                tool.prompt_snippet().is_some_and(|s| !s.trim().is_empty()),
                "{}",
                tool.name()
            );
        }
    }

    /// A local endpoint (the one new sessions start on) and a hosted one.
    fn with_cloud() -> AiSettings {
        let mut settings = AiSettings {
            connection: "local".into(),
            ..AiSettings::default()
        };
        settings.connections.insert(
            "local".into(),
            Connection {
                model: "local-model".into(),
                ..Connection::default()
            },
        );
        settings.connections.insert(
            "cloud".into(),
            Connection {
                provider: "anthropic_compatible".into(),
                model: "claude-x".into(),
                context_window_fallback: Some(200_000),
                ..Connection::default()
            },
        );
        settings
    }

    /// What a panel on connection `name` of `settings` runs on.
    fn active_on(settings: &AiSettings, name: &str) -> Active {
        let connection = &settings.connections[name];
        Active {
            connection: name.to_string(),
            kind: connection.provider.clone(),
            provider: build_provider(connection, None),
            model: connection.model.clone(),
            context_window: connection.effective_context_window(),
        }
    }

    #[test]
    fn hosted_apis_take_reasoning_effort_local_servers_the_switch_and_others_nothing() {
        let at = |base_url: &str, param| {
            reasoning_param(&Connection {
                base_url: base_url.into(),
                reasoning_param: param,
                ..Connection::default()
            })
        };
        let auto = termide_config::ReasoningParam::Auto;
        assert_eq!(
            at("https://api.openai.com/v1", auto),
            ReasoningParam::Effort
        );
        assert_eq!(
            at("https://openrouter.ai/api/v1", auto),
            ReasoningParam::Effort
        );
        assert_eq!(
            at(
                "https://generativelanguage.googleapis.com/v1beta/openai",
                auto
            ),
            ReasoningParam::Effort
        );
        for local in [
            "http://127.0.0.1:10000/v1",
            "http://localhost:8080/v1",
            "http://studio.local:1234/v1",
            "http://192.168.1.20:8000/v1",
            "http://10.0.0.5/v1",
            "http://[::1]:8080/v1",
            "http://[fd00::5]:8080/v1",
        ] {
            assert_eq!(at(local, auto), ReasoningParam::EnableThinking, "{local}");
        }
        assert_eq!(at("https://api.mistral.ai/v1", auto), ReasoningParam::None);
        assert_eq!(at("http://8.8.8.8:8080/v1", auto), ReasoningParam::None);
        assert_eq!(
            at("https://api.openai.com.evil.example/v1", auto),
            ReasoningParam::None
        );
        assert_eq!(
            at(
                "http://127.0.0.1:8080/v1",
                termide_config::ReasoningParam::EnableThinking
            ),
            ReasoningParam::EnableThinking
        );
        assert_eq!(
            at(
                "https://api.openai.com/v1",
                termide_config::ReasoningParam::None
            ),
            ReasoningParam::None
        );
    }

    #[test]
    fn connections_build_and_hand_their_provider_to_delegated_tasks() {
        let mut settings = with_cloud();
        // `cloud`'s calls are reviewed by `local`'s model; `local` leaves it
        // to `[ai.auto_reviewer]`, which says the session's model.
        settings.connections.get_mut("cloud").unwrap().reviewer = "local".into();
        let active: ActiveSlot = Arc::new(std::sync::RwLock::new(active_on(&settings, "local")));
        let connections = AiConnections {
            settings,
            dirs: AgentDirs::new(Path::new("/tmp"), None, None),
            active: Arc::clone(&active),
        };
        let names: Vec<String> = connections.list().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["cloud", "local"]);
        let cloud = connections.build("cloud", DEFAULT_AGENT).unwrap();
        assert_eq!(cloud.kind, "anthropic_compatible");
        assert_eq!(
            (cloud.model.as_str(), cloud.context_window),
            ("claude-x", 200_000)
        );
        assert!(cloud.backend.is_none(), "an endpoint, not a CLI agent");
        let reviewer = |choice: &ConnectionChoice| {
            choice
                .reviewer
                .model
                .own
                .as_ref()
                .map(|(_, model)| model.id.clone())
        };
        assert_eq!(reviewer(&cloud).as_deref(), Some("local-model"));
        let local = connections.build("local", DEFAULT_AGENT).unwrap();
        assert_eq!(reviewer(&local), None);
        assert!(connections.build("missing", DEFAULT_AGENT).is_none());
        // Switching to it moves what a delegated task runs on.
        connections.activate(&cloud);
        let now = active.read().unwrap().clone();
        assert_eq!(
            (now.model.as_str(), now.context_window),
            ("claude-x", 200_000)
        );
        assert_eq!(
            (now.connection.as_str(), now.kind.as_str()),
            ("cloud", "anthropic_compatible")
        );
    }

    /// A delegated task runs on the session's connection, or on the one its
    /// `subagents` field names: termide's loop on a model connection, a copy
    /// of Claude Code on its; Codex and Gemini CLI run none.
    #[test]
    fn subagents_run_where_the_session_connection_says() {
        let mut settings = with_cloud();
        for (name, provider) in [("claude", "claude_code"), ("codex", "codex")] {
            settings.connections.insert(
                name.into(),
                Connection {
                    provider: provider.into(),
                    ..Connection::default()
                },
            );
        }
        let target = |settings: &AiSettings, session: &str| {
            subagent_target(settings, &active_on(settings, session)).map(|target| match target {
                SubagentTarget::Session => "session".to_string(),
                SubagentTarget::Model(name, _) => format!("model {name}"),
                SubagentTarget::Cli(name, _) => format!("cli {name}"),
            })
        };
        // Nothing named: the session's own — termide's loop, or a copy of
        // Claude Code.
        assert_eq!(target(&settings, "local").as_deref(), Ok("session"));
        assert_eq!(target(&settings, "claude").as_deref(), Ok("cli claude"));
        // Codex has nowhere to run them, and says where to choose one.
        let error = target(&settings, "codex").unwrap_err();
        assert!(error.contains("Subagents"), "{error}");

        let set = |settings: &mut AiSettings, on: &str, to: &str| {
            settings.connections.get_mut(on).unwrap().subagents = to.into();
        };
        set(&mut settings, "codex", "local");
        assert_eq!(target(&settings, "codex").as_deref(), Ok("model local"));
        set(&mut settings, "local", "claude");
        assert_eq!(target(&settings, "local").as_deref(), Ok("cli claude"));
        set(&mut settings, "claude", "cloud");
        assert_eq!(target(&settings, "claude").as_deref(), Ok("model cloud"));
        // Itself, Codex or a name of no connection: the session's own.
        for named in ["local", "codex", "gone"] {
            set(&mut settings, "local", named);
            assert_eq!(
                target(&settings, "local").as_deref(),
                Ok("session"),
                "{named}"
            );
        }
        set(&mut settings, "codex", "gone");
        assert!(target(&settings, "codex").is_err());
    }

    /// A CLI agent standing in for Claude Code: it offers models, answers a
    /// task with two messages and ends, or ends when stopped.
    #[derive(Default)]
    struct FakeCli {
        picked: std::sync::Mutex<Option<String>>,
        prompted: std::sync::Mutex<Vec<String>>,
        events: std::sync::Mutex<std::collections::VecDeque<AgentEvent>>,
        busy: std::sync::atomic::AtomicBool,
    }

    impl Backend for FakeCli {
        fn prompt(&self, message: UserMessage) -> Result<(), termide_agent_core::PromptError> {
            use termide_agent_core::StopReason;
            self.prompted.lock().unwrap().push(message.model_text());
            self.busy.store(true, std::sync::atomic::Ordering::SeqCst);
            let said = |text: &str| {
                AgentEvent::MessageEnd(Message::Assistant(AssistantMessage {
                    content: vec![termide_agent_core::AssistantContent::Text { text: text.into() }],
                    stop_reason: StopReason::Stop,
                    usage: Usage {
                        input: 100,
                        output: 10,
                        ..Usage::default()
                    },
                    provider: "acp".into(),
                    model: "m".into(),
                    error_message: None,
                    timestamp: 0,
                }))
            };
            self.events.lock().unwrap().extend([
                AgentEvent::AgentStart,
                said("working"),
                said("the answer"),
                AgentEvent::AgentEnd,
            ]);
            Ok(())
        }
        fn steer(&self, _message: UserMessage) {}
        fn queue_lens(&self) -> (usize, usize) {
            (0, 0)
        }
        fn abort(&self) {
            self.events.lock().unwrap().push_back(AgentEvent::AgentEnd);
        }
        fn is_busy(&self) -> bool {
            self.busy.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn drain(&self) -> Vec<AgentEvent> {
            let events: Vec<_> = self.events.lock().unwrap().drain(..).collect();
            if events.iter().any(|e| matches!(e, AgentEvent::AgentEnd)) {
                self.busy.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            events
        }
        fn update(
            &self,
            _update: Box<dyn FnOnce(&mut Agent) + Send>,
        ) -> Result<(), termide_agent_core::PromptError> {
            Err(termide_agent_core::PromptError::Unsupported)
        }
        fn compact(&self, _focus: Option<String>) -> Result<(), termide_agent_core::PromptError> {
            Err(termide_agent_core::PromptError::Unsupported)
        }
        fn available_models(&self) -> Vec<termide_agent_core::BackendModel> {
            ["claude-opus-5", "claude-haiku-5"]
                .map(|id| termide_agent_core::BackendModel {
                    id: id.into(),
                    name: id.into(),
                })
                .to_vec()
        }
        fn select_model(&self, model_id: String) -> Result<(), String> {
            *self.picked.lock().unwrap() = Some(model_id);
            Ok(())
        }
        fn into_agent(self: Box<Self>) -> Option<Agent> {
            None
        }
    }

    /// A subagent on Claude Code runs on the connection's model, reports
    /// what it says as it goes and answers with its last words; stopped
    /// before it starts, it is not handed the task.
    #[test]
    fn a_subagent_on_a_cli_agent_runs_its_task_to_the_end() {
        let (_prompter, permission_rx) = permission_channel(CancelToken::new());
        let (budget, cancel) = (CancelToken::new(), CancelToken::new());
        let drive = CliDrive {
            name: "claude:search",
            model: "haiku",
            permission_rx: &permission_rx,
            refusal: "nobody to ask",
            budget: &budget,
            cancel: &cancel,
        };
        let cli = FakeCli::default();
        let mut progress = Vec::new();
        let answer = drive.run(&cli, &|| false, "find it", &mut |update| {
            let ToolUpdate::Output(text) = update;
            progress.push(text);
        });
        let outcome = answer.unwrap();
        assert_eq!(outcome.answer.as_deref(), Ok("the answer"));
        // Every reply's tokens count, not the last one's alone.
        assert_eq!((outcome.spent.input, outcome.spent.output), (200, 20));
        assert_eq!(
            cli.picked.lock().unwrap().as_deref(),
            Some("claude-haiku-5")
        );
        assert_eq!(*cli.prompted.lock().unwrap(), ["find it"]);
        assert_eq!(progress.last().unwrap(), "working\n\nthe answer");

        cancel.cancel();
        let stopped = FakeCli::default();
        let answer = drive.run(&stopped, &|| false, "find it", &mut |_| {});
        assert!(answer.is_err());
        assert!(stopped.prompted.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failed_subagent_run_says_why() {
        use termide_agent_core::{AssistantMessage, StopReason};
        let failed = |error: Option<&str>| {
            let mut reply = AssistantMessage::failed("agent", "m", StopReason::Error, "");
            reply.error_message = error.map(str::to_string);
            Message::Assistant(reply)
        };
        let refused = failed(Some("connection refused"));
        assert_eq!(
            run_error(std::slice::from_ref(&refused)).as_deref(),
            Some("connection refused")
        );
        // Only the last reply counts: a later one without an error has none.
        assert_eq!(run_error(&[refused, failed(None)]), None);
        assert_eq!(run_error(&[]), None);
    }

    #[test]
    fn an_open_panel_offers_the_connections_applied_since() {
        let mut built_with = with_cloud();
        built_with.connections.remove("cloud");
        let connections = AiConnections {
            active: Arc::new(std::sync::RwLock::new(active_on(&built_with, "local"))),
            settings: built_with,
            dirs: AgentDirs::new(Path::new("/tmp"), None, None),
        };
        // Applying the settings modal adds `cloud`; the panel offers it at
        // once. (The same settings the other tests build with, since the
        // applied ones are shared across the process.)
        publish_ai_settings(&with_cloud());
        let names: Vec<String> = connections.list().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["cloud", "local"]);
        assert!(connections.build("cloud", DEFAULT_AGENT).is_some());
    }

    #[test]
    fn a_session_reopens_on_the_connection_it_recorded_while_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), dir.path()).unwrap();
        let mut settings = with_cloud();
        let name = |settings: &AiSettings, session: &Session| {
            session_connection(settings, Some(session)).map(|(name, _)| name)
        };
        // Nothing recorded: the one new sessions start on.
        assert_eq!(name(&settings, &session).as_deref(), Some("local"));
        session.append_connection_change("cloud").unwrap();
        assert_eq!(name(&settings, &session).as_deref(), Some("cloud"));
        // A connection since removed falls back to the one new sessions use.
        settings.connections.remove("cloud");
        assert_eq!(name(&settings, &session).as_deref(), Some("local"));
        assert_eq!(name(&AiSettings::default(), &session), None);
    }

    /// A log from before connections were recorded: its model ran on Claude
    /// Code, so it reopens on the Claude Code connection, not on the default
    /// endpoint with Claude's model id.
    #[test]
    fn a_log_without_a_connection_reopens_on_one_of_its_provider() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), dir.path()).unwrap();
        session
            .append_model_change("claude_code", "opus[1m]", None)
            .unwrap();
        let mut settings = with_cloud();
        settings.connections.insert(
            "claude".into(),
            Connection {
                provider: "claude_code".into(),
                ..Connection::default()
            },
        );
        let reopened = session_connection(&settings, Some(&session)).map(|(name, _)| name);
        assert_eq!(reopened.as_deref(), Some("claude"));
        // No connection of that provider left: the one new sessions start on.
        settings.connections.remove("claude");
        let reopened = session_connection(&settings, Some(&session)).map(|(name, _)| name);
        assert_eq!(reopened.as_deref(), Some("local"));
    }

    #[test]
    fn a_fork_is_named_after_its_source_with_a_counter_that_stays_unique() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), dir.path()).unwrap();
        // Nothing sent yet: naming the copy would keep an empty log alive, so
        // the copy stays unnamed and is discarded with its panel.
        assert_eq!(fork_name(&session), None);

        session.set_name("Refactor").unwrap();
        assert_eq!(fork_name(&session).as_deref(), Some("Refactor (2)"));

        // Forking counts on from the names already in the directory: with the
        // first copy listed under `(2)`, the next fork takes `(3)`.
        let first_copy = session
            .fork(dir.path(), fork_name(&session).as_deref())
            .unwrap();
        let forked = Session::open_exclusive(&first_copy).unwrap();
        assert_eq!(forked.name(), Some("Refactor (2)"));
        assert_eq!(fork_name(&session).as_deref(), Some("Refactor (3)"));
        // Forking the copy counts on too, from the name it carries.
        assert_eq!(fork_name(&forked).as_deref(), Some("Refactor (3)"));

        // A source the user never named is labelled by its first prompt, so
        // the copy's suffix is what tells the two rows apart.
        let mut plain = Session::create(dir.path(), dir.path()).unwrap();
        plain
            .append_message(&Message::User(UserMessage::text("fix the bug")))
            .unwrap();
        assert_eq!(fork_name(&plain).as_deref(), Some("fix the bug (2)"));
    }

    #[test]
    fn a_fork_name_is_the_lowest_free_counter_in_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), dir.path()).unwrap();
        session.set_name("Refactor").unwrap();
        let copy = session.fork(dir.path(), Some("Refactor (2)")).unwrap();
        assert_eq!(fork_name(&session).as_deref(), Some("Refactor (3)"));

        // The counter counts the names in use, not the forks made: rename the
        // copy away and `(2)` is free again, so no number runs up for nothing.
        let mut forked = Session::open(&copy).unwrap();
        forked.set_name("Refactor (7)").unwrap();
        assert_eq!(fork_name(&session).as_deref(), Some("Refactor (2)"));
        // A fork of the renamed copy takes the same free number: whichever
        // forks first writes it, and the other then counts on from there.
        assert_eq!(fork_name(&forked).as_deref(), Some("Refactor (2)"));
        drop(forked);
        std::fs::remove_file(&copy).unwrap();
        assert_eq!(fork_name(&session).as_deref(), Some("Refactor (2)"));
    }

    #[test]
    fn a_fork_suffix_counts_on_from_the_name_it_carries() {
        assert_eq!(strip_fork_suffix("Refactor (2)"), "Refactor");
        assert_eq!(strip_fork_suffix("Refactor (12)"), "Refactor");
        // The counter starts at two: one fork makes the second of a kind.
        assert_eq!(strip_fork_suffix("Refactor (1)"), "Refactor (1)");
        // Not a counter: other brackets, other contents, no brackets.
        assert_eq!(strip_fork_suffix("work (draft)"), "work (draft)");
        assert_eq!(strip_fork_suffix("2 of 3"), "2 of 3");
        assert_eq!(strip_fork_suffix("()"), "()");
        assert_eq!(strip_fork_suffix("Refactor"), "Refactor");
    }

    #[test]
    fn cli_providers_get_an_acp_backend() {
        // A CLI-adapter provider drives an external agent over ACP; a
        // wire-protocol provider uses the built-in loop (no backend override).
        assert!(cli_provider_backend("claude_code", "default").is_some());
        assert!(cli_provider_backend("codex", "default").is_some());
        assert!(cli_provider_backend("gemini_cli", "default").is_some());
        assert!(cli_provider_backend("openai_compatible", "default").is_none());
        assert!(cli_provider_backend("anthropic_compatible", "default").is_none());
    }

    #[test]
    fn a_rule_is_merged_under_ai_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ai]\nmodel = \"m\"  # keep me\n").unwrap();

        merge_permission_rule(&path, "bash", "cat *", Decision::Allow).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        // Written under [ai.permissions.bash] (not the old [agent.…]), and the
        // rest of the file is preserved.
        assert!(text.contains("[ai.permissions.bash]"), "{text}");
        assert!(text.contains("\"cat *\" = \"allow\""), "{text}");
        assert!(text.contains("# keep me"), "{text}");
        assert!(!text.contains("[agent"), "{text}");

        // The document is valid and the config loader reads the rule back.
        let config = termide_config::Config::load_from(&path).unwrap();
        assert_eq!(
            config.ai.permissions.evaluate("bash", "cat notes.txt"),
            Some(Decision::Allow)
        );

        // A second grant updates in place — no duplicate table, still valid.
        merge_permission_rule(&path, "bash", "python3 *", Decision::Allow).unwrap();
        merge_permission_rule(&path, "bash", "cat *", Decision::Allow).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("[ai.permissions.bash]").count(), 1, "{text}");
        let config = termide_config::Config::load_from(&path).unwrap();
        assert_eq!(
            config.ai.permissions.evaluate("bash", "python3 x.py"),
            Some(Decision::Allow)
        );
    }

    #[test]
    fn a_broken_config_is_left_for_a_hand_fix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // Two tables of the same name: invalid TOML.
        let broken = "[ai.permissions.bash]\n\"cat *\" = \"allow\"\n[ai.permissions.bash]\n\"ls *\" = \"allow\"\n";
        std::fs::write(&path, broken).unwrap();
        assert!(merge_permission_rule(&path, "bash", "python3 *", Decision::Allow).is_err());
        // The file is untouched, not appended to.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }
    #[test]
    fn restore_is_skipped_without_a_connection_to_run_on() {
        let mut settings = AiSettings::default();
        assert!(restore_agent_panel(&settings, PathBuf::from("/tmp"), None, None).is_none());
        // A connection with no model named leaves the model to the provider.
        settings
            .connections
            .insert("local".into(), Connection::default());
        assert!(usable_connection(&settings).is_some());
    }

    /// A provider listing `ids`.
    struct Lists(Vec<&'static str>);

    impl Provider for Lists {
        fn name(&self) -> &str {
            "lists"
        }

        fn stream(
            &self,
            _: &termide_agent_core::Request<'_>,
            _: &mut dyn FnMut(termide_agent_core::StreamEvent),
            _: &CancelToken,
        ) -> termide_agent_core::AssistantMessage {
            unreachable!("only asked for its models")
        }

        fn list_models(&self) -> Result<Vec<termide_agent_core::ModelInfo>, String> {
            Ok(self
                .0
                .iter()
                .map(|id| termide_agent_core::ModelInfo {
                    id: (*id).to_string(),
                    context_window: None,
                })
                .collect())
        }
    }

    /// A provider that answers with the model it was asked for, counting how
    /// often it listed its models.
    struct Echoes(std::sync::atomic::AtomicUsize);

    impl Provider for Echoes {
        fn name(&self) -> &str {
            "echoes"
        }

        fn stream(
            &self,
            request: &termide_agent_core::Request<'_>,
            _: &mut dyn FnMut(termide_agent_core::StreamEvent),
            _: &CancelToken,
        ) -> termide_agent_core::AssistantMessage {
            termide_agent_core::AssistantMessage::failed(
                "echoes",
                request.model.id.clone(),
                termide_agent_core::StopReason::Error,
                "echo",
            )
        }

        fn list_models(&self) -> Result<Vec<termide_agent_core::ModelInfo>, String> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(vec![termide_agent_core::ModelInfo {
                id: "first".into(),
                context_window: None,
            }])
        }
    }

    #[test]
    fn a_side_call_with_no_model_named_asks_the_first_listed_once() {
        let inner = Arc::new(Echoes(std::sync::atomic::AtomicUsize::new(0)));
        let provider = FirstListedModel {
            inner: Arc::clone(&inner) as Arc<dyn Provider>,
            model: std::sync::OnceLock::new(),
        };
        let ask = |id: &str| {
            let model = ModelSpec {
                provider: "agent".into(),
                id: id.into(),
                context_window: 0,
                max_tokens: None,
                thinking: ThinkingLevel::Off,
            };
            let request = termide_agent_core::Request {
                model: &model,
                system_prompt: "",
                messages: &[],
                tools: &[],
                thinking: ThinkingLevel::Off,
            };
            provider
                .stream(&request, &mut |_| {}, &CancelToken::new())
                .model
        };
        assert_eq!(ask(""), "first");
        assert_eq!(ask(""), "first");
        assert_eq!(inner.0.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(ask("named"), "named");
    }

    #[test]
    fn a_model_left_to_the_provider_is_its_first() {
        let provider = Lists(vec!["first", "second"]);
        assert_eq!(resolve_model(&provider, "").as_deref(), Some("first"));
        assert_eq!(resolve_model(&provider, "named").as_deref(), Some("named"));
        assert_eq!(resolve_model(&Lists(vec![]), ""), None);
    }

    #[test]
    fn plan_mode_holds_a_subagent_whatever_its_own_mode() {
        assert_eq!(subagent_mode(Mode::Plan, Some(Mode::All)), Mode::Plan);
        assert_eq!(subagent_mode(Mode::Plan, None), Mode::Plan);
        assert_eq!(subagent_mode(Mode::Edit, Some(Mode::Ask)), Mode::Ask);
        assert_eq!(subagent_mode(Mode::Auto, None), Mode::Auto);
    }

    /// With subagents wired, every agent gets `task`, unless its `tools`
    /// list leaves it out.
    /// A provider that says nothing until its request is stopped, as a long
    /// prefill does.
    struct Silent;

    impl Provider for Silent {
        fn name(&self) -> &str {
            "silent"
        }
        fn stream(
            &self,
            request: &termide_agent_core::Request<'_>,
            _on_event: &mut dyn FnMut(StreamEvent),
            cancel: &CancelToken,
        ) -> AssistantMessage {
            while !cancel.is_cancelled() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            AssistantMessage::failed(
                "silent",
                &request.model.id,
                termide_agent_core::StopReason::Aborted,
                "aborted",
            )
        }
    }

    /// The parent's stop ends a subagent at once, even while its model says
    /// nothing.
    #[test]
    fn a_stopped_parent_stops_its_subagent_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = AgentDirs::new(tmp.path(), Some(tmp.path()), None);
        let settings = with_cloud();
        let mut active = active_on(&settings, "local");
        active.provider = Arc::new(Silent);
        let subagents = Subagents {
            active: Arc::new(std::sync::RwLock::new(active)),
            settings: settings.clone(),
            recall: recall_tool(&settings, &dirs, tmp.path(), tmp.path()),
            web: shared_web(&settings.web, &dirs),
            dirs,
            mcp: Connections::new(std::collections::BTreeMap::new()),
            cwd: tmp.path().to_path_buf(),
            project_root: tmp.path().to_path_buf(),
            rules: settings.permissions.clone(),
            mode: termide_agent_core::ModeHandle::new(settings.permissions.mode),
            max_tokens: settings.output_limit(),
            reasoning: ThinkingLevel::Off,
            compaction: settings.compaction,
            refusals: Refusals::default(),
        };
        let cancel = CancelToken::new();
        let stopper = {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                cancel.cancel();
            })
        };
        let started = std::time::Instant::now();
        let result = subagents.run(
            DEFAULT_AGENT,
            "look around",
            &ToolContext::new(tmp.path().to_path_buf()),
            &cancel,
            &mut |_| {},
        );
        stopper.join().unwrap();
        assert_eq!(result.answer, Err("the subagent was stopped".to_string()));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn a_waiting_subagent_says_so_under_what_it_said() {
        let t = termide_i18n::t();
        let mut report = SubagentReport::default();
        let mut shown = Vec::new();
        report.waiting(Some(2), &mut |ToolUpdate::Output(text)| shown.push(text));
        assert_eq!(
            shown.last().unwrap(),
            &format!("⏳ {}", t.agent_queued_ahead_fmt(2))
        );
        let said = AssistantMessage::failed("m", "m", termide_agent_core::StopReason::Stop, "");
        let mut said = said;
        said.error_message = None;
        said.content = vec![termide_agent_core::AssistantContent::Text {
            text: "looked".into(),
        }];
        report.said(&said, &mut |ToolUpdate::Output(text)| shown.push(text));
        report.waiting(Some(0), &mut |ToolUpdate::Output(text)| shown.push(text));
        assert_eq!(
            shown.last().unwrap(),
            &format!("looked\n\n⏳ {}", t.agent_queued())
        );
        report.waiting(None, &mut |ToolUpdate::Output(text)| shown.push(text));
        assert_eq!(shown.last().unwrap(), "looked");
    }

    /// A subagent gets the tools of the MCP servers connected when its task
    /// starts, whatever its `tools` list says, and they stay out of its
    /// prompt as they do of the panel's.
    #[cfg(unix)]
    #[test]
    fn a_subagent_gets_the_connected_mcp_servers_tools() {
        let script = r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","serverInfo":{"name":"sh","version":"0"}}}\n' "$id";;
    *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"lookup","description":"Look up","inputSchema":{"type":"object"}}]}}\n' "$id";;
  esac
done"#;
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("ai");
        let dir = global.join("agents").join("search");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(AGENT_FILE), "---\ntools: read\n---\n").unwrap();
        let catalog = FsCatalog::with_global(tmp.path(), tmp.path(), Some(global));
        let servers = [(
            "docs".to_string(),
            termide_agent_core::McpServerConfig {
                command: Some("sh".into()),
                args: vec!["-c".into(), script.into()],
                timeout_secs: 5,
                ..Default::default()
            },
        )]
        .into();
        let mcp = Connections::new(servers);
        // The panel's subscription is what connects the servers.
        let late = mcp.subscribe();
        assert!(matches!(
            late.recv_timeout(std::time::Duration::from_secs(10))
                .unwrap(),
            termide_agent_core::LateTools::Ready { .. }
        ));
        let settings = with_cloud();
        let subagents = Subagents {
            active: Arc::new(std::sync::RwLock::new(active_on(&settings, "local"))),
            settings: settings.clone(),
            dirs: catalog.dirs.clone(),
            recall: recall_tool(&settings, &catalog.dirs, tmp.path(), tmp.path()),
            web: shared_web(&settings.web, &catalog.dirs),
            mcp,
            cwd: tmp.path().to_path_buf(),
            project_root: tmp.path().to_path_buf(),
            rules: settings.permissions.clone(),
            mode: termide_agent_core::ModeHandle::new(settings.permissions.mode),
            max_tokens: settings.output_limit(),
            reasoning: ThinkingLevel::Off,
            compaction: settings.compaction,
            refusals: Refusals::default(),
        };
        let definition = catalog.dirs.agent("search");
        let (tools, _, prompt) = subagents.toolset("search", &definition);
        assert_eq!(tools.names(), ["read", "docs__lookup"]);
        assert!(!prompt.contains("docs__lookup"));
    }

    #[test]
    fn an_agent_tools_list_governs_the_task_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("ai");
        for (agent, tools) in [("search", "read"), ("lead", "read, task")] {
            let dir = global.join("agents").join(agent);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(AGENT_FILE), format!("---\ntools: {tools}\n---\n")).unwrap();
        }
        let mut catalog = FsCatalog::with_global(tmp.path(), tmp.path(), Some(global));
        let settings = with_cloud();
        catalog.subagents = Some(Arc::new(Subagents {
            active: Arc::new(std::sync::RwLock::new(active_on(&settings, "local"))),
            settings: settings.clone(),
            dirs: catalog.dirs.clone(),
            recall: recall_tool(&settings, &catalog.dirs, tmp.path(), tmp.path()),
            web: shared_web(&settings.web, &catalog.dirs),
            mcp: Arc::clone(&catalog.mcp),
            cwd: tmp.path().to_path_buf(),
            project_root: tmp.path().to_path_buf(),
            rules: settings.permissions.clone(),
            mode: termide_agent_core::ModeHandle::new(settings.permissions.mode),
            max_tokens: settings.output_limit(),
            reasoning: ThinkingLevel::Off,
            compaction: settings.compaction,
            refusals: Refusals::default(),
        }));

        assert_eq!(catalog.resolve("search").unwrap().tools.names(), ["read"]);
        assert_eq!(
            catalog.resolve("lead").unwrap().tools.names(),
            ["read", "task"]
        );
        assert!(catalog
            .resolve(DEFAULT_AGENT)
            .unwrap()
            .tools
            .get("task")
            .is_some());
    }

    /// `AGENT.md`'s front matter narrows the tools and names a model and a mode; a name no
    /// root defines does not resolve, the default always does.
    #[test]
    fn the_catalog_turns_definitions_into_profiles() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("ai");
        let review = global.join("agents/review");
        std::fs::create_dir_all(&review).unwrap();
        std::fs::create_dir_all(global.join("agents/default")).unwrap();
        std::fs::write(
            global.join(DEFAULT_AGENT_FILE),
            "Root template.\n\n{{tools}}\n",
        )
        .unwrap();
        std::fs::write(
            review.join(AGENT_FILE),
            "---\ndescription: Reviews diffs\nmodel: big\nmode: auto\ntools: read, bash, nope\n---\n\
             You review.\n\n{{tools}}\n",
        )
        .unwrap();
        let catalog = FsCatalog::with_global(tmp.path(), tmp.path(), Some(global.clone()));

        let entries = catalog.list();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["default", "review"]);
        // The default agent, defined nowhere, is described by the panel's label.
        assert_eq!(entries[0].description, termide_i18n::t().panel_agent());
        assert_eq!(entries[1].description, "Reviews diffs");
        let review = catalog.resolve("review").unwrap();
        assert!(review.system_prompt.starts_with("You review.\n\n- read:"));
        assert_eq!(review.tools.names(), ["read", "bash"]);
        assert_eq!(review.model.as_deref(), Some("big"));
        assert_eq!(review.mode, Some(termide_agent_core::Mode::Auto));

        let default = catalog.resolve(DEFAULT_AGENT).unwrap();
        // The built-in four, plus the two that speak to the user watching the
        // panel: `question` and `suggest_command`.
        assert_eq!(
            default.tools.names(),
            [
                "read",
                "edit",
                "write",
                "bash",
                "question",
                "suggest_command"
            ]
        );
        assert!(default
            .system_prompt
            .starts_with("Root template.\n\n- read:"));
        assert!(default.model.is_none());
        assert!(catalog.resolve("missing").is_none());

        // A skill adds the `skill` tool, to every agent, and a prompt line.
        let skill = global.join("skills/deploy");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: deploy\ndescription: Ship it\n---\nSteps.\n",
        )
        .unwrap();
        std::fs::write(global.join(DEFAULT_AGENT_FILE), "{{skills}}\n").unwrap();
        let review = catalog.resolve("review").unwrap();
        assert_eq!(review.tools.names(), ["read", "bash", "skill"]);

        // A `command` makes an external agent: no tools of ours, a backend.
        let outside = global.join("agents/outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(
            outside.join(AGENT_FILE),
            "---\ndescription: Claude Code\ncommand: npx -y @agentclientprotocol/claude-agent-acp\n---\n",
        )
        .unwrap();
        let outside = catalog.resolve("outside").unwrap();
        assert!(outside.backend.is_some());
        assert!(outside.tools.is_empty());
        assert!(outside.late_tools.is_none());
        let default = catalog.resolve(DEFAULT_AGENT).unwrap();
        assert_eq!(default.system_prompt, "- deploy: Ship it\n");
    }
}
