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
    Failure, FailureKind, HostTools, IntentLog, LimitPolicy, Message, MessageOrigin, ModeHandle,
    ModelChoice, ModelSpec, OriginKind, PermissionAnswer, PermissionHooks, PermissionRules,
    PersistScope, PlanGuard, PromptOptions, Provider, Refusals, ReviewerSetup, Session,
    SessionParent, SessionSummary, ShellOutput, ShellRunner, SideModels, Spender, StreamEvent,
    ThinkingLevel, Tool, ToolCall, ToolContext, ToolRegistry, ToolUpdate, Usage, UserMessage,
    DEFAULT_AGENT, GLOBAL_AGENT_DIR, SESSIONS_DIR,
};
use termide_agent_hooks::CommandHooks;
use termide_agent_mcp::{Connections, TokenStore};
use termide_agent_providers::{
    AnthropicProvider, Compat, OpenAiCompatProvider, ReasoningParam, Slots, SlottedProvider,
};
use termide_agent_recall::{PanelDir, RecallSetup, RecallTool, RepoFinder, Solver, TimeLimits};
use termide_agent_tools::{
    builtin_tools, JobsTool, OwnTasks, QuestionTool, ShellTool, SkillTool, SubagentOutcome,
    SubagentRun, SubagentTask, SuggestCommandTool, SuggestGoalTool, SuggestLoopTool, TaskTool,
    TerminalsTool,
};
use termide_agent_web::{web_tools, Web, WebConfig};
use termide_config::{AiSettings, Connection, WebSettings};
use termide_core::AgentSetupState;
use termide_panel_agent::{
    AgentCatalog, AgentEntry, AgentPanel, AgentPanelSetup, AgentProfile, BackendFactory,
    ConnectionCatalog, ConnectionChoice, ConnectionEntry, FoldMode, HeldHook, HooksFactory,
    Trusted,
};

use super::api_keys::{Broker, KeyedProvider};
use super::App;

mod debug_tool;
pub(in crate::app) use debug_tool::Waiter as DebugWaiter;
mod headless;
pub(crate) mod live;
mod lsp_check;
pub(in crate::app) use lsp_check::LspChecks;
mod peers;
mod snippets;
pub(in crate::app) use snippets::SnippetSources;
pub(crate) mod terminals;
pub use headless::{run_agent_headless, run_recall, HeadlessOutput};
pub(super) use peers::same_dir;

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
    /// two are told apart in the picker. With `focus`, the copy is compacted
    /// on it once it opens.
    pub(in crate::app) fn fork_agent_session(
        &mut self,
        source: &Path,
        cwd: &Path,
        focus: Option<String>,
    ) -> Result<()> {
        if let Err(error) = self.open_fork(source, cwd, focus, None) {
            self.show_error_modal(error);
        }
        Ok(())
    }

    /// Copy the log at `source` and open the copy in a new panel working in
    /// `cwd`, compacted on `focus` when given: the new session's id and its
    /// panel's number, or why it was not opened. `handover` is a fork the
    /// agent asked for with its `fork` call: the copy, made in the middle of
    /// the run that call is part of, is brought to rest first — the call
    /// answered as the one that made the fork — and then takes the part of
    /// the work handed over as its first request.
    pub(in crate::app) fn open_fork(
        &mut self,
        source: &Path,
        cwd: &Path,
        focus: Option<String>,
        handover: Option<Handover>,
    ) -> Result<(String, u64), String> {
        let t = termide_i18n::t();
        let source_session = Session::open(source).map_err(|error| {
            log::warn!("cannot open {}: {error}", source.display());
            t.agent_notice_cannot_fork_fmt(&error.to_string())
        })?;
        let name = fork_name(
            &source_session,
            handover.as_ref().map(|handover| handover.title.as_str()),
        );
        let copy = source_session.fork(cwd, name.as_deref()).map_err(|error| {
            log::warn!(
                "cannot fork the agent session {}: {error}",
                source.display()
            );
            t.agent_notice_cannot_fork_fmt(&error.to_string())
        })?;
        // What goes wrong from here leaves a copy no one has open, so it goes
        // back the way it came.
        let discard_copy = |error: String| {
            if let Err(error) = std::fs::remove_file(&copy) {
                log::warn!(
                    "cannot remove the unused session copy {}: {error}",
                    copy.display()
                );
            }
            error
        };
        let first = match handover {
            Some(Handover { call, first, .. }) => {
                settle_fork(&copy, &call)
                    .map_err(|error| discard_copy(t.agent_notice_cannot_fork_fmt(&error)))?;
                Some(first)
            }
            None => None,
        };
        let settings = self.state.config.ai.clone();
        let Some(mut panel) = restore_agent_panel(
            &settings,
            cwd.to_path_buf(),
            Some(copy.clone()),
            None,
            AgentSetupState::default(),
        ) else {
            return Err(discard_copy(t.agent_not_configured().to_string()));
        };
        panel.start_fork(focus, first);
        let opened = (panel.session_id().unwrap_or_default(), panel.panel_id());
        self.add_panel(Box::new(panel));
        self.auto_save_layout();
        Ok(opened)
    }
}

/// A fork the agent asked for: the id of its `fork` call, the part of the
/// work the fork takes over, as the agent's message, and the name the copy
/// takes for it.
pub(in crate::app) struct Handover {
    pub call: String,
    pub first: UserMessage,
    pub title: String,
}

/// What the copy's log answers the `fork` call with, in place of the result
/// the session it was copied from gets.
const FORKED_RESULT: &str = "This session is the fork this call made: a copy of the \
     conversation in a panel of its own. The part of the work it takes over comes next. \
     Nothing goes back to the session it was copied from, which carries on with the rest; \
     you work on with the user here.";

/// Bring the copy at `copy` to rest where the run that forked it stood: the
/// `fork` call `call` answered as the one that made the fork, any other call
/// of that reply as one the fork does not run, and the run ended.
fn settle_fork(copy: &Path, call: &str) -> Result<(), String> {
    let mut session = Session::open_exclusive(copy).map_err(|error| error.to_string())?;
    session
        .settle(|open| {
            if open.id == call {
                termide_agent_core::ToolResultMessage::text(open, FORKED_RESULT)
            } else {
                termide_agent_core::ToolResultMessage::error(
                    open,
                    "not run: the session was forked before this call ran",
                )
            }
        })
        .map_err(|error| error.to_string())
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
        )
        .with_keybindings(&self.state.config.file_manager.keybindings);
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
        value: termide_modal::ModalOutput,
        session: Option<&Path>,
        cwd: &Path,
    ) -> Result<()> {
        let Some(new_cwd) = value.get::<PathBuf>() else {
            return Ok(());
        };
        if new_cwd == cwd {
            return Ok(());
        }
        // A modal moves no focus, so the panel that asked is the focused one;
        // checked all the same, as nothing else may be replaced.
        let (agent, setup) = match self
            .layout_manager
            .active_panel_mut()
            .and_then(|panel| panel.to_state(Path::new("")))
        {
            Some(termide_core::PanelState::Agent {
                cwd: active_cwd,
                session: active_session,
                agent,
                setup,
            }) if active_cwd == cwd && active_session.as_deref() == session => (agent, setup),
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
        // Rebuilt in place, the panel keeps its number: other agents address
        // it by that.
        let number = self
            .layout_manager
            .active_panel()
            .and_then(|panel| panel.downcast_ref::<AgentPanel>())
            .map(AgentPanel::panel_id);
        let Some(panel) =
            restore_numbered_agent_panel(&settings, new_cwd.clone(), moved, agent, setup, number)
        else {
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
///
/// `title` is the name the agent gave a fork it asked for: the copy takes it
/// as it is, and with a counter only when another session there has it.
fn fork_name(source: &Session, title: Option<&str>) -> Option<String> {
    if source.is_empty() {
        return None;
    }
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
    let title = title.map(str::trim).filter(|title| !title.is_empty());
    if let Some(title) = title.filter(|title| !taken.iter().any(|name| name == title)) {
        return Some(title.to_string());
    }
    // A name forked before keeps its counter, so the next one counts on from
    // it rather than starting again at two.
    let base = match title {
        Some(title) => Session::split_fork_counter(title).0.to_string(),
        None => match source.name() {
            Some(name) => Session::split_fork_counter(name).0.to_string(),
            None => SessionSummary::from(source).label(),
        },
    };
    (2..)
        .find(|n| !taken.iter().any(|name| name == &format!("{base} ({n})")))
        .map(|n| format!("{base} ({n})"))
}

/// How long a fork's name runs, in characters, when it comes from the first
/// line of the request the agent handed over.
const FORK_TITLE_CHARS: usize = 48;

/// The name of a fork the agent asked for: the `title` it gave, or else the
/// first line of the part of the work it handed over, cut short — so the
/// panel says what that copy is doing rather than repeating the name of the
/// session it came from.
fn fork_title(title: Option<&str>, prompt: &str) -> String {
    if let Some(title) = title.map(str::trim).filter(|title| !title.is_empty()) {
        return title.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= FORK_TITLE_CHARS {
        return line;
    }
    let cut: String = line.chars().take(FORK_TITLE_CHARS - 1).collect();
    // Cut at the last word that fits, when there is one to cut at.
    let cut = match cut.rsplit_once(' ') {
        Some((words, _)) if !words.is_empty() => words,
        _ => cut.as_str(),
    };
    format!("{}…", cut.trim_end())
}

/// Rebuild an agent panel saved in a project layout. `None` when there is
/// no connection to run on any more; a session log that has gone missing starts a
/// fresh session in the same project — on the `setup` saved for it when
/// nothing was sent in it —, an agent definition that has gone missing falls
/// back to the default one.
pub(crate) fn restore_agent_panel(
    settings: &AiSettings,
    cwd: PathBuf,
    session: Option<PathBuf>,
    agent: Option<String>,
    setup: AgentSetupState,
) -> Option<AgentPanel> {
    restore_numbered_agent_panel(settings, cwd, session, agent, setup, None)
}

/// [`restore_agent_panel`] for a panel taking the place of the one numbered
/// `panel_id`, whose number it keeps; `None` gives it a number of its own.
fn restore_numbered_agent_panel(
    settings: &AiSettings,
    cwd: PathBuf,
    session: Option<PathBuf>,
    agent: Option<String>,
    setup: AgentSetupState,
    panel_id: Option<u64>,
) -> Option<AgentPanel> {
    if usable_connection(settings).is_none() {
        log::warn!("agent panel not restored: no [ai.connections] entry to run on");
        return None;
    }
    // Exclusive: if this session is already open in another restored panel,
    // fall back to a fresh one rather than back two panels with one log. One
    // another termide holds — maybe running it right now, in a window that
    // is out of sight — is shown read-only, for the panel to take it up once
    // that termide lets it go.
    let session = session.and_then(|path| match Session::open_exclusive(&path) {
        Ok(session) => Some(session),
        Err(error) if Session::held_elsewhere(&path).is_some() => {
            log::info!(
                "agent session {} is held elsewhere: {error}",
                path.display()
            );
            Session::open_read_only(&path)
                .map_err(|error| log::warn!("cannot read {}: {error}", path.display()))
                .ok()
        }
        Err(error) => {
            log::warn!("cannot reopen agent session {}: {error}", path.display());
            None
        }
    });
    let agent = agent.as_deref().unwrap_or(DEFAULT_AGENT);
    let session = session.or_else(|| {
        seeded_session(
            settings,
            session_dir_of(&cwd).as_deref(),
            &cwd,
            agent,
            &setup,
        )
    });
    // termide's project root is its working directory, which follows the
    // current project; the layout restore runs off the App, so read it from
    // the same source.
    let project_root = std::env::current_dir().unwrap_or_else(|_| cwd.clone());
    let mut panel_setup = agent_setup(settings, cwd, &project_root, agent, session);
    panel_setup.panel_id = panel_id;
    Some(AgentPanel::new(panel_setup))
}

/// A fresh session log for `cwd` in `dir` that starts on `setup`: the saved
/// connection while it is still configured, with the model, reasoning level
/// and agent options picked on it. `None` when nothing was saved, so the
/// panel starts its session as a new one does.
fn seeded_session(
    settings: &AiSettings,
    dir: Option<&Path>,
    cwd: &Path,
    agent: &str,
    setup: &AgentSetupState,
) -> Option<Session> {
    if setup.is_empty() {
        return None;
    }
    // The model was picked on the saved connection: one that is gone takes
    // the default connection with its own model.
    let saved = setup
        .connection
        .as_ref()
        .and_then(|name| Some((name.clone(), settings.connections.get(name)?.clone())));
    let kept_model = saved.is_some();
    let (name, connection) = saved.or_else(|| session_connection(settings, None))?;
    let mut session = match Session::create_exclusive(dir?, cwd) {
        Ok(session) => session,
        Err(error) => {
            log::warn!("cannot start an agent session log: {error}");
            return None;
        }
    };
    // The order a new session records its setup in: the connection first,
    // the model after it is that connection's.
    let model = setup
        .model
        .as_deref()
        .filter(|_| kept_model)
        .unwrap_or(&connection.model);
    let written = session
        .append_connection_change(&name)
        .and_then(|_| session.append_model_change(&connection.provider, model, None))
        .and_then(|_| session.append_agent_change(agent))
        .and_then(
            |_| match setup.thinking.as_deref().and_then(ThinkingLevel::parse) {
                Some(level) => session.append_thinking_change(level).map(drop),
                None => Ok(()),
            },
        )
        .and_then(|_| {
            setup.options.iter().try_for_each(|(option, value)| {
                session.append_agent_option(option, value).map(drop)
            })
        });
    if let Err(error) = written {
        log::warn!("agent session write failed: {error}");
    }
    Some(session)
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
    /// The project's approved launches: its servers and hooks start only
    /// once approved.
    approvals: termide_agent_core::approvals::Approvals,
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
        // Kept with termide's own data, where a repository cannot write.
        let approvals = termide_agent_core::approvals::Approvals::new(
            termide_project::approvals_path(project_root).ok(),
        );
        Self::with_global(
            cwd,
            project_root,
            global_agent_dir,
            dirs::home_dir().as_deref(),
            approvals,
        )
    }

    /// `home` is the user's home, whose `.agents/skills` join as the lowest
    /// skill level. A test passes `None` to be about the directories it made
    /// and nothing else: the skills a machine happens to keep at home would
    /// otherwise add a `skill` tool to every tool set it asserts on.
    fn with_global(
        cwd: &Path,
        project_root: &Path,
        global_agent_dir: Option<PathBuf>,
        home: Option<&Path>,
        approvals: termide_agent_core::approvals::Approvals,
    ) -> Self {
        let dirs =
            AgentDirs::new(cwd, Some(project_root), global_agent_dir.as_deref()).with_home(home);
        let servers = mcp_servers(&dirs, &approvals);
        // OAuth sign-ins are the user's, kept beside the user's own agent
        // files rather than in any project.
        let mcp = match &global_agent_dir {
            Some(global) => {
                Connections::with_tokens(servers, TokenStore::new(global.join(MCP_AUTH_FILE)))
            }
            None => Connections::new(servers),
        };
        Self {
            cwd: cwd.to_path_buf(),
            project_root: project_root.to_path_buf(),
            mcp,
            approvals,
            dirs,
            subagents: None,
            web: None,
            recall: None,
        }
    }
}

impl AgentCatalog for FsCatalog {
    fn restart_delegate(
        &self,
        parent: u64,
        caller: &str,
        delegate: &termide_core::DelegateRestart,
    ) -> bool {
        self.subagents
            .as_ref()
            .is_some_and(|runner| runner.restart_background(parent, caller, delegate))
    }

    fn set_mode(&self, mode: Mode) {
        if let Some(subagents) = &self.subagents {
            subagents.mode.set(mode);
        }
    }

    fn mcp_status(&self) -> Vec<termide_agent_core::McpServerState> {
        self.mcp.status()
    }

    fn mcp_reconnect(&self, server: &str) -> Result<termide_agent_core::McpReload, String> {
        self.mcp
            .reconnect(server, &mcp_servers(&self.dirs, &self.approvals))
    }

    /// The configuration is read from the files again, at every level, so
    /// an edit to any of them is what the reload applies.
    fn mcp_reload(&self) -> Option<termide_agent_core::McpReload> {
        Some(self.mcp.reload(mcp_servers(&self.dirs, &self.approvals)))
    }

    fn held_hooks(&self) -> Vec<HeldHook> {
        let (_, held) = self.approvals.split_hooks(self.dirs.hooks());
        held.into_iter()
            .map(|(name, hook)| HeldHook {
                launch: hook.launch_line(),
                fingerprint: hook.fingerprint(),
                name,
            })
            .collect()
    }

    /// A server is approved as it is configured now and connected; a hook
    /// as it is configured now, running from the next run on — each only
    /// while the file read now still holds the launch that was shown.
    fn trust(&self, name: &str, shown: &str) -> Result<Trusted, String> {
        if let Some(server) = self
            .dirs
            .mcp_servers()
            .get(name)
            .filter(|server| server.from_project)
        {
            if !self.approvals.approve_mcp(name, server, shown)? {
                return Ok(Trusted::Changed {
                    hook: false,
                    launch: server.launch_line(),
                    fingerprint: server.fingerprint(),
                });
            }
            let servers = mcp_servers(&self.dirs, &self.approvals);
            return self
                .mcp
                .reconnect(name, &servers)
                .map(|report| Trusted::Approved(Some(report)));
        }
        if let Some(hook) = self.dirs.hooks().get(name).filter(|hook| hook.from_project) {
            if !self.approvals.approve_hook(name, hook, shown)? {
                return Ok(Trusted::Changed {
                    hook: true,
                    launch: hook.launch_line(),
                    fingerprint: hook.fingerprint(),
                });
            }
            return Ok(Trusted::Approved(None));
        }
        Err(termide_i18n::t().agent_notice_trust_unknown_fmt(name))
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
                let spec = self.dirs.spec(&name);
                let mut description = spec.description;
                if description.is_empty() && name == DEFAULT_AGENT {
                    description = termide_i18n::t().panel_agent().to_string();
                }
                AgentEntry {
                    name,
                    description,
                    icon: spec.icon,
                }
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
        // Someone watches the panel's agent to answer its questions, to
        // confirm a command it is offered and to start a goal or a loop it
        // offers; a subagent and headless mode build their tools without them.
        if backend.is_none() {
            tools.insert(Arc::new(QuestionTool));
            tools.insert(Arc::new(SuggestCommandTool));
            tools.insert(Arc::new(SuggestGoalTool));
            tools.insert(Arc::new(SuggestLoopTool));
            // The agents of the other panels are there to write to.
            for tool in peers::peer_tools(name, &self.project_root) {
                tools.insert(tool);
            }
            // Part of the work handed over to a copy of the session.
            tools.insert(peers::fork_tool(name, &self.project_root));
            // The debugger, driven as the user drives it in the debug panel.
            tools.insert(debug_tool::debug_tool(&self.project_root));
        }
        // The `task` tool lets this agent hand work to the others, and its
        // `tools` list can leave it out like any other capability; a lone
        // agent delegates to a copy of itself. Never for an external
        // agent (it drives its own tools) or a subagent (no nesting: the
        // subagent build path adds no task tool).
        if let Some(subagents) = &self.subagents {
            if backend.is_none() {
                let delegates = self.delegatable();
                if !delegates.is_empty() {
                    let runner = Arc::clone(subagents);
                    let caller = name.to_string();
                    let run: SubagentRun = Arc::new(move |task, ctx, cancel, on_update| {
                        if task.background {
                            return runner.start_background(&caller, task, ctx).into();
                        }
                        runner.run_detachable(&caller, task, ctx, cancel, on_update)
                    });
                    tools.insert(Arc::new(TaskTool::new(delegates, run)));
                }
            }
        }
        // What the agent set going is its own to see and stop.
        if backend.is_none() {
            tools.insert(Arc::new(JobsTool::new(Arc::new(PanelTasks))));
        }
        restrict_tools(&mut tools, &definition.spec.tools, name);
        // Skills are instructions, not a capability, so an agent's `tools`
        // list does not govern them: the tool comes with the skills.
        let all_skills = self.dirs.model_skills();
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
            late_tools: backend.is_none().then(|| {
                // MCP servers and the project's plugins share the channel.
                let (tx, rx) = std::sync::mpsc::channel();
                self.mcp.subscribe_into(tx.clone());
                super::plugins::tool_hub().subscribe(&self.project_root, tx);
                rx
            }),
            backend,
            offered,
            skills: skill_names,
        })
    }
}

impl FsCatalog {
    /// The agents an agent can delegate to with the `task` tool: every
    /// agent, the default one, the caller itself and the external ones
    /// included.
    fn delegatable(&self) -> Vec<(String, String)> {
        self.dirs
            .agents()
            .into_iter()
            // The caller itself too: a fresh copy of it, with a clean context,
            // is as good a delegate as any, and a lone agent can delegate.
            // External agents too: they run their own program for the task.
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
    // The shell runs commands in terminal sessions too, which `terminals`
    // looks after.
    let sessions: Arc<dyn termide_agent_core::TerminalSessions> =
        Arc::clone(terminals::agent_terminals()) as _;
    tools.insert(Arc::new(ShellTool {
        shell: terminals::agent_terminals().shell(),
        shim_path: dirs.shims_dir(),
        terminals: Some(Arc::clone(&sessions)),
        ..ShellTool::default()
    }));
    tools.insert(Arc::new(TerminalsTool::new(sessions)));
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
        // `jobs` stops what `task` hands over, so it comes with it.
        let with_task = tool == "jobs" && allowed.iter().any(|name| name == "task");
        if !allowed.contains(&tool) && !with_task {
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

/// Run the agent's commands in the shell picked for terminals, `configured`
/// (`terminal.default_shell`), or the one found when none is.
pub(crate) fn publish_shell(configured: Option<&str>) {
    terminals::agent_terminals().set_shell(configured);
}

/// Record `settings` as the applied `[ai]` settings for open panels.
pub(crate) fn publish_ai_settings(settings: &AiSettings) {
    *APPLIED_AI.write().unwrap_or_else(PoisonError::into_inner) = Some(settings.clone());
    terminals::agent_terminals().set_limit(settings.max_terminal_sessions);
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
        .unwrap_or_else(PoisonError::into_inner)
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
            backend: cli_provider_backend(connection, agent),
            reviewer: reviewer_setup(&settings, &self.dirs, name),
            side_models: side_models(&settings),
            settings: connection.clone(),
        })
    }

    fn activate(&self, choice: &ConnectionChoice) {
        *self.active.write().unwrap_or_else(PoisonError::into_inner) = Active {
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
        Some(connection) => match cli_acp_config(connection) {
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
/// model `[ai.auto_reviewer]` names, see [`side_model`]; with none named, on
/// a Claude Code or Codex connection the cheapest model its subscription
/// offers, see [`cheap_reviewer`].
fn reviewer_setup(settings: &AiSettings, dirs: &AgentDirs, session: &str) -> ReviewerSetup {
    let choice = settings.reviewer_of(session);
    let model = cheap_reviewer(settings, &choice, session).unwrap_or_else(|| {
        side_model(
            settings,
            &choice,
            &format!("the reviewer of connection {session}"),
        )
    });
    ReviewerSetup {
        prompt: dirs.classify_prompt(),
        model,
        spent: termide_agent_core::SpentMeter::default(),
    }
}

/// The models the `/goal` judge and the `/handoff` brief of termide's own
/// loop run on: what `[ai.goal_judge]` and `[ai.handoff]` name, see
/// [`side_model`]; the session's own where they name none.
fn side_models(settings: &AiSettings) -> SideModels {
    SideModels {
        goal_judge: side_model(settings, &settings.goal_judge, "[ai.goal_judge]"),
        handoff: side_model(settings, &settings.handoff, "[ai.handoff]"),
    }
}

/// The reviewer of a session on connection `session` when `choice` names no
/// model and that connection is Claude Code or Codex: the cheapest model of
/// the subscription, picked from those the agent offers — the session's own
/// model, often the frontier one, would spend the plan's limits on every
/// call reviewed. `None` elsewhere: the session's model reviews.
fn cheap_reviewer(
    settings: &AiSettings,
    choice: &termide_config::SideModel,
    session: &str,
) -> Option<ModelChoice> {
    if !choice.connection.trim().is_empty() || !choice.model.trim().is_empty() {
        return None;
    }
    let connection = settings.connections.get(session)?;
    if !matches!(connection.provider.as_str(), "claude_code" | "codex") {
        return None;
    }
    let config = cli_acp_config(connection)?;
    let cwd = std::env::current_dir().unwrap_or_default();
    let spec = ModelSpec {
        provider: "agent".to_string(),
        id: String::new(),
        context_window: 0,
        max_tokens: None,
        thinking: ThinkingLevel::Off,
    };
    Some(ModelChoice::own(
        Arc::new(AcpProvider::new(session, config, cwd).cheapest()),
        spec,
    ))
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
    /// Where a delegated task's session log goes: the panel's sessions;
    /// `None` keeps no log.
    session_dir: Option<PathBuf>,
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

/// A runaway subagent is cut off after this many model calls, unless its
/// `AGENT.md` names another number (`max_turns`).
const SUBAGENT_MAX_TURNS: usize = 50;

/// How many times one delegated task is parked at a limit before it is given
/// up. The waits double between tries, so six of them already cover most of a
/// day; past that the limit is the task's answer, and the user hears it with
/// the reset named.
const MAX_PARK_ATTEMPTS: u32 = 6;

/// Now, Unix seconds.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Where a task delegated from a session runs.
#[derive(Debug, PartialEq)]
enum SubagentTarget {
    /// termide's loop on the session's own model connection, under the model
    /// named in place of the session's, when one is.
    Session(Option<String>),
    /// termide's loop on another model connection, by name.
    Model(String, Connection),
    /// A session of its own of Claude Code on this connection, by name,
    /// calling termide's tools.
    Cli(String, Connection),
}

/// Where a task delegated from a session on `active` runs: where its
/// connection's `subagents` or `[ai.subagents]` says — a connection, under
/// the model named in place of its own — else on the session's own, for a
/// Claude Code session a copy of Claude Code. Codex and Gemini CLI keep
/// tools of their own and run none: one named there is passed over, and a
/// session on one with nothing usable named has nowhere to run them, which
/// the error says.
fn subagent_target(settings: &AiSettings, active: &Active) -> Result<SubagentTarget, String> {
    let session = active.connection.as_str();
    let side = settings.subagents_of(session);
    let named = side.connection.as_str();
    let model = (!side.model.is_empty()).then(|| side.model.clone());
    // The named model in place of the connection's own.
    let with_model = |connection: &Connection| Connection {
        model: model.clone().unwrap_or_else(|| connection.model.clone()),
        ..connection.clone()
    };
    if !named.is_empty() && named != session {
        match settings.connections.get(named) {
            Some(connection) if !connection.is_cli() => {
                return Ok(SubagentTarget::Model(
                    named.to_string(),
                    with_model(connection),
                ));
            }
            Some(connection) if connection.runs_subagents() => {
                return Ok(SubagentTarget::Cli(
                    named.to_string(),
                    with_model(connection),
                ));
            }
            Some(_) => log::warn!(
                "connection {session}: subagents names {named}, a CLI agent, which cannot run them"
            ),
            None => log::warn!("connection {session}: subagents names no connection {named:?}"),
        }
    }
    if !termide_config::is_cli_provider(&active.kind) {
        return Ok(SubagentTarget::Session(model));
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
        return Ok(SubagentTarget::Cli(session.to_string(), with_model(&own)));
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

/// A piece of a subagent's progress as the delegating call shows it.
enum Step {
    /// Something it said.
    Said(String),
    /// A tool it called, by the call's id: `· read src/main.rs`, marked `✗`
    /// once it fails.
    Call {
        id: String,
        line: String,
        failed: bool,
    },
}

/// The widest a subagent's call line gets before it is cut.
const SUBAGENT_CALL_CHARS: usize = 100;

/// A subagent's progress as the delegating call shows it: what it has said
/// and the tools it called so far, and its answer — the last text it said.
struct SubagentReport {
    steps: Vec<Step>,
    answer: Option<String>,
    error: Option<String>,
    /// What stopped it, when its last reply says: a limit and the moment it
    /// resets, rather than the limit's text alone.
    failure: Option<Failure>,
    turns: usize,
    /// The model calls it may take before it is cut off.
    max_turns: usize,
    /// What its model calls spent so far.
    spent: Usage,
    /// The connection its model calls went to, named in what the delegating
    /// agent is told when the task stops: another is what gets past a limit.
    connection: String,
}

impl SubagentReport {
    fn new(max_turns: usize, connection: &str) -> Self {
        Self {
            steps: Vec::new(),
            answer: None,
            error: None,
            failure: None,
            turns: 0,
            max_turns,
            spent: Usage::default(),
            connection: connection.to_string(),
        }
    }

    /// Count one model call of the subagent's; `true` once it has taken
    /// more than it may.
    fn turn(&mut self) -> bool {
        self.turns += 1;
        self.turns > self.max_turns
    }

    /// Note a finished message of the subagent's.
    fn said(&mut self, message: &AssistantMessage, on_update: &mut dyn FnMut(ToolUpdate)) {
        self.spent.add(&message.usage);
        let text = message.plain_text();
        let text = text.trim();
        if text.is_empty() {
            self.error = message.error_message.clone();
            self.failure = message.classify_failure();
            return;
        }
        self.error = None;
        self.failure = None;
        self.steps.push(Step::Said(text.to_string()));
        self.answer = Some(text.to_string());
        on_update(ToolUpdate::Output(self.progress()));
    }

    /// Note a tool the subagent calls, in the directory `cwd`.
    fn called(&mut self, call: &ToolCall, cwd: &Path, on_update: &mut dyn FnMut(ToolUpdate)) {
        let subject = termide_agent_core::subject_of(call, &ToolContext::new(cwd.to_path_buf()));
        let subject = subject.lines().next().unwrap_or("").trim();
        let mut line = if subject.is_empty() {
            format!("· {}", call.name)
        } else {
            format!("· {} {subject}", call.name)
        };
        if line.chars().count() > SUBAGENT_CALL_CHARS {
            line = line
                .chars()
                .take(SUBAGENT_CALL_CHARS - 1)
                .collect::<String>()
                + "…";
        }
        self.steps.push(Step::Call {
            id: call.id.clone(),
            line,
            failed: false,
        });
        on_update(ToolUpdate::Output(self.progress()));
    }

    /// Note how a call of the subagent's ended: a failed one is marked.
    fn call_ended(
        &mut self,
        result: &termide_agent_core::ToolResultMessage,
        on_update: &mut dyn FnMut(ToolUpdate),
    ) {
        if !result.is_error {
            return;
        }
        let mut marked = false;
        for step in &mut self.steps {
            if let Step::Call { id, failed, .. } = step {
                if *id == result.tool_call_id {
                    *failed = true;
                    marked = true;
                }
            }
        }
        if marked {
            on_update(ToolUpdate::Output(self.progress()));
        }
    }

    /// Its progress as text: what it said apart by blank lines, its calls
    /// one line each.
    fn progress(&self) -> String {
        let mut text = String::new();
        let mut after_call = false;
        for step in &self.steps {
            let (piece, call) = match step {
                Step::Said(said) => (said.clone(), false),
                Step::Call { line, failed, .. } => (
                    if *failed {
                        format!("{line} ✗")
                    } else {
                        line.clone()
                    },
                    true,
                ),
            };
            if !text.is_empty() {
                text.push_str(if call && after_call { "\n" } else { "\n\n" });
            }
            text.push_str(&piece);
            after_call = call;
        }
        text
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
        let progress = self.progress();
        let shown = match (line, progress.is_empty()) {
            (None, _) => progress,
            (Some(line), true) => format!("⏳ {line}"),
            (Some(line), false) => format!("{progress}\n\n⏳ {line}"),
        };
        on_update(ToolUpdate::Output(shown));
    }

    /// The delegating call's result.
    fn into_outcome(self, stopped: bool) -> SubagentOutcome {
        let spent = self.spent;
        let failure = self.failure;
        let answer = match self.answer {
            Some(text) if self.turns > self.max_turns => Ok(format!(
                "{text}\n\n(subagent stopped after {} steps)",
                self.max_turns
            )),
            Some(text) => Ok(text),
            None if stopped => Err("the subagent was stopped".into()),
            None => Err(self.give_up(
                self.error
                    .as_deref()
                    .map_or_else(
                        || "the subagent produced no answer".to_string(),
                        |error| format!("the subagent failed: {error}"),
                    )
                    .as_str(),
            )),
        };
        SubagentOutcome {
            answer,
            spent,
            session: None,
            parked: false,
            failure: (!stopped).then_some(failure).flatten(),
        }
    }

    /// The delegating call's result when the user ended the task in its
    /// panel: what the delegate said so far with `why`, else `why` as the
    /// error.
    fn ended_by_user(self, why: &str) -> SubagentOutcome {
        let answer = match self.answer {
            Some(text) => Ok(format!("{text}\n\n({why})")),
            None => Err(why.to_string()),
        };
        SubagentOutcome {
            answer,
            spent: self.spent,
            session: None,
            parked: false,
            failure: None,
        }
    }

    /// What the delegating agent reads when the task ends without an answer:
    /// the failure's own words, then where it stopped and what can still be
    /// done. A limit's text alone is worth nothing there — the agent repeats
    /// it to the user and stops — so the connection it stopped on and the
    /// moment its limit resets go with it: another connection, or the same one
    /// after that moment, is what gets the task done.
    fn give_up(&self, why: &str) -> String {
        let Some(failure) = self.failure else {
            return why.to_string();
        };
        let mut text = format!("{why} (connection {conn}", conn = self.connection);
        if let Some(at) = failure.resets_at {
            text.push_str(&format!(", resets {}", termide_panel_agent::clock(at)));
        }
        text.push_str(if failure.kind.passes() {
            "; the task was not carried on — re-issue it, or switch the connection"
        } else {
            "; repeating it as it is changes nothing"
        });
        text.push(')');
        text
    }
}

/// The moment a parked task goes again: the reset its limit named, or a
/// backoff that grows with the tries it has made.
fn wake_at(failure: &Failure, attempts: u32, now: u64) -> u64 {
    failure
        .retry_delay(attempts, now)
        .map_or(now, |delay| now + delay.as_secs())
}

/// What the delegating agent is told when its delegate's task is parked at a
/// limit: not a failure, and not an answer either — the task waits, and its
/// answer arrives later as a message of its own.
fn parked_words(agent: &str, failure: &Failure, connection: &str, wake: u64) -> String {
    let limit = match failure.kind {
        FailureKind::QuotaExhausted => "usage limit",
        FailureKind::RateLimited => "rate limit",
        _ => "temporary failure",
    };
    let waits = match failure.resets_at {
        Some(at) => format!("until {limit} resets {}", termide_panel_agent::clock(at)),
        None => format!(
            "and goes again after a wait that grows ({})",
            termide_panel_agent::clock(wake)
        ),
    };
    format!(
        "{agent} stopped on a {limit} of connection \"{connection}\" and is parked {waits}; \
         it goes again by itself then, at once if you switch the connection or ask its row to. \
         Its answer will reach you as a message of its own. Do not re-issue this task."
    )
}

/// The log of a delegated task's session, written as the task runs, so what
/// the delegate did can be read, searched and carried on afterwards. Nothing
/// is written when the log cannot be started; the task runs all the same.
struct DelegateLog(Option<Session>);

impl DelegateLog {
    /// A log in `dir`, of a session in `cwd` naming `parent`, that starts
    /// on its connection and model (empty for an agent's own) with its
    /// agent, as a panel's does; none without a `dir`.
    fn start(dir: Option<&Path>, cwd: &Path, parent: SessionParent, run: &DelegateRun) -> Self {
        let Some(dir) = dir else {
            return Self(None);
        };
        let mut session = match Session::create_delegate(dir, cwd, parent) {
            Ok(session) => session,
            Err(error) => {
                log::warn!("cannot start a delegate's session log: {error}");
                return Self(None);
            }
        };
        let mut written = session.append_connection_change(run.connection).map(drop);
        if !run.model.is_empty() {
            written = written.and_then(|()| {
                session
                    .append_model_change(run.provider, run.model, run.context_window)
                    .map(drop)
            });
        }
        if let Err(error) = written.and_then(|()| session.append_agent_change(run.agent).map(drop))
        {
            log::warn!("delegate session write failed: {error}");
        }
        Self(Some(session))
    }

    /// The log a delegated task already made, taken up to be run on: its
    /// header and its steps stay as they are, and the run goes on from there.
    fn resume(session: Session) -> Self {
        Self(Some(session))
    }

    /// Log what the task's reviewer spent since the last look, by model, as
    /// a panel logs its own reviewer's; the sum comes back for the outcome.
    fn note_reviews(&mut self, reviews: &termide_agent_core::SpentMeter) -> Usage {
        let mut total = Usage::default();
        for (model, usage) in reviews.take_by_model() {
            total.add(&usage);
            if let Some(session) = &mut self.0 {
                if let Err(error) = session.append_spent(Spender::Reviewer, &model, usage) {
                    log::warn!("delegate session write failed: {error}");
                }
            }
        }
        total
    }

    /// Write a finished message of the task's.
    fn note(&mut self, message: &Message) {
        if let Some(session) = &mut self.0 {
            if let Err(error) = session.append_message(message) {
                log::warn!("delegate session write failed: {error}");
            }
        }
    }

    /// Record that the task's run started, after its first message, as a
    /// panel's does: from here the log reads as a run under way, so a task
    /// cut off by termide quitting is known as unfinished and its session
    /// offers to carry on.
    fn note_run_start(&mut self) {
        if let Some(session) = &mut self.0 {
            if let Err(error) = session.append_run_start() {
                log::warn!("delegate session write failed: {error}");
            }
        }
    }

    /// Record that the task's run ended, `paused` when it stands at a pause
    /// rather than an answer: the reopened session then asks to carry it on.
    fn note_run_end(&mut self, paused: bool) {
        if let Some(session) = &mut self.0 {
            if let Err(error) = session.append_run_end(paused) {
                log::warn!("delegate session write failed: {error}");
            }
        }
    }

    /// Record the session an external agent opened for the task, so the
    /// task carried on from this log goes on in it.
    fn note_external_session(&mut self, agent: &str, session_id: &str) {
        if let Some(session) = &mut self.0 {
            if let Err(error) = session.append_external_session(agent, session_id) {
                log::warn!("delegate session write failed: {error}");
            }
        }
    }

    /// The external agent's session the task ran in, when the agent still
    /// holds it, and what the log holds of the task, for an agent that does
    /// not: told it as a recap.
    fn external_resume(&self) -> (Option<termide_agent_core::ExternalSessionRef>, Vec<Message>) {
        self.0.as_ref().map_or((None, Vec::new()), |session| {
            (session.external_session(), session.context_messages())
        })
    }

    /// Where the task works: the directory its log names.
    fn cwd(&self) -> Option<PathBuf> {
        self.0.as_ref().map(|session| session.header().cwd.clone())
    }

    /// The log's path, when there is one.
    fn path(&self) -> Option<PathBuf> {
        self.0.as_ref().map(|session| session.path().to_path_buf())
    }

    /// `outcome` with the log noted beside it.
    fn into_outcome(self, mut outcome: SubagentOutcome) -> SubagentOutcome {
        outcome.session = self.0.map(|session| session.path().to_path_buf());
        outcome
    }
}

/// What a delegate's session starts on, for its log.
struct DelegateRun<'a> {
    connection: &'a str,
    /// The connection's provider kind.
    provider: &'a str,
    model: &'a str,
    context_window: Option<u64>,
    agent: &'a str,
}

impl Subagents {
    /// The tools, the skills and the system prompt the agent `name` runs
    /// with as a subagent: no `task` (delegation does not nest), no
    /// `suggest_command` (nobody watches it run), and `question` only when
    /// its parent has a user to put the questions to (`asks`).
    fn toolset(
        &self,
        name: &str,
        definition: &termide_agent_core::AgentDefinition,
        asks: bool,
    ) -> (ToolRegistry, Vec<termide_agent_core::SkillInfo>, String) {
        // An external agent drives its own tools, as in a panel: none of
        // ours, so its prompt lists none.
        let external = definition.spec.acp.is_some();
        let mut tools = if external {
            ToolRegistry::new()
        } else {
            base_tools(&self.dirs, Some(&self.web), Some(&self.recall))
        };
        if asks && !external {
            tools.insert(Arc::new(QuestionTool));
        }
        restrict_tools(&mut tools, &definition.spec.tools, name);
        let skills = self.dirs.model_skills();
        if !skills.is_empty() && !external {
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
        if !external {
            for tool in self.mcp.ready_tools() {
                tools.insert(tool);
            }
        }
        (tools, skills, system_prompt)
    }

    /// Hand `task`, the agent `caller`'s, over in the background: it runs
    /// out of sight on a thread of its own, as a task a call waits for
    /// does, and its answer reaches the delegating panel as a message of the
    /// delegate's. What to tell the delegating agent, or why not.
    fn start_background(
        self: &Arc<Self>,
        caller: &str,
        task: &SubagentTask,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let Some(parent) = ctx.panel else {
            return Err(
                "a background task reports to an agent panel, and this agent runs in none; \
                 run it without `background`"
                    .into(),
            );
        };
        // Its answer starts the delegating agent anew, so it counts among the
        // messages agents pass one another.
        peers::may_hand_over(&self.project_root, parent)?;
        // Its answer is for the session that handed it over, wherever that
        // is by the time it comes.
        let parent_session = ctx.session.as_ref().and_then(|session| session.id.clone());
        let this = Arc::clone(self);
        let ctx = ctx.clone();
        let (caller, agent, prompt, call) = (
            caller.to_string(),
            task.agent.to_string(),
            task.prompt.to_string(),
            task.call.to_string(),
        );
        let thread_agent = agent.clone();
        std::thread::Builder::new()
            .name(format!("background-{agent}"))
            .spawn(move || {
                let task = SubagentTask {
                    agent: &thread_agent,
                    prompt: &prompt,
                    call: &call,
                    background: true,
                };
                // Nobody waits on the call: the user stops the task, from its
                // row or a panel following it, or by closing its panel.
                let outcome = this
                    .delegate(&caller, &task, &ctx, &CancelToken::new(), &mut |_| {})
                    .unwrap_or_else(|error| Err(error).into());
                // Parked at a limit: nothing is owed yet. The answer goes back
                // when the parked task has really run to its end.
                if outcome.parked {
                    return;
                }
                let held = termide_panel_agent::live::take_quiet(parent, &call);
                // Stopped by the agent that handed it over: it knows.
                if termide_panel_agent::live::take_withdrawn(parent, &call) {
                    return;
                }
                let text = background_report(&prompt, &thread_agent, &outcome);
                peers::report(
                    &this.project_root,
                    parent,
                    parent_session,
                    &thread_agent,
                    text,
                    held,
                );
            })
            .map_err(|error| format!("cannot start the task: {error}"))?;
        Ok(format!(
            "{agent} took the task and works on it out of sight; its id is {}. Carry on: its \
             answer comes to you later as a message of its own. Do not wait for it with tools, \
             and do not hand the same task over again. Should you no longer need it, stop it \
             with `jobs`.",
            task.call
        ))
    }

    /// Carry an unfinished delegated task on: run `agent` again out of its
    /// own session log, out of sight, under the `task` call `call`, asking
    /// through the panel `parent` and answering it when done. False when the
    /// log cannot be read: the task then stays where it is, in the lists.
    fn restart_background(
        self: &Arc<Self>,
        parent: u64,
        caller: &str,
        delegate: &termide_core::DelegateRestart,
    ) -> bool {
        let termide_core::DelegateRestart {
            agent,
            session,
            call,
            task,
        } = delegate;
        // A panel took the session up meanwhile: it carries the task on there.
        if termide_agent_core::Session::is_open(session) {
            log::warn!(
                "the delegate's session {} is open in a panel: not carried on twice",
                session.display()
            );
            return false;
        }
        let Ok(log) = termide_agent_core::Session::open(session) else {
            log::warn!(
                "cannot take up the delegate's session {}: no run",
                session.display()
            );
            return false;
        };
        // The task's own words, asked again of where its run stopped.
        let carried = format!(
            "Carry this task on from where you stopped. What you did before is in your log; \
             do not start it over.\n\n{task}"
        );
        let parent_session = log
            .header()
            .parent
            .as_ref()
            .and_then(|parent| parent.session.clone());
        let this = Arc::clone(self);
        let cwd = log.header().cwd.clone();
        let (caller, agent, carried, call, session, original) = (
            caller.to_string(),
            agent.to_string(),
            carried,
            call.to_string(),
            session.clone(),
            task.clone(),
        );
        let thread_agent = agent.clone();
        std::thread::Builder::new()
            .name(format!("carried-{thread_agent}"))
            .spawn(move || {
                // The agent that handed it over stopped it before this try
                // got going: nothing is run, nothing answered.
                if termide_panel_agent::live::take_withdrawn(parent, &call) {
                    return;
                }
                // Nobody waits on the call, as for any task handed over in the
                // background: its cards stay up until the user answers them.
                let (prompter, rx) = permission_channel(CancelToken::new());
                live::carry_cards(this.project_root.clone(), parent, rx);
                let ctx = ToolContext {
                    prompter: Some(prompter),
                    panel: Some(parent),
                    resume: Some(session),
                    ..ToolContext::new(cwd)
                };
                let task = SubagentTask {
                    agent: &thread_agent,
                    prompt: &carried,
                    call: &call,
                    background: true,
                };
                let outcome = this
                    .delegate(&caller, &task, &ctx, &CancelToken::new(), &mut |_| {})
                    .unwrap_or_else(|error| Err(error).into());
                // Parked at a limit again: the tick parks it afresh with a
                // longer wait, and the answer stays owed to this panel.
                if outcome.parked {
                    return;
                }
                // The report names the task as it was handed over, not the
                // words that carried it on.
                let held = termide_panel_agent::live::take_quiet(parent, &call);
                if termide_panel_agent::live::take_withdrawn(parent, &call) {
                    return;
                }
                let text = background_report(&original, &thread_agent, &outcome);
                peers::report(
                    &this.project_root,
                    parent,
                    parent_session,
                    &thread_agent,
                    text,
                    held,
                );
            })
            .map_err(|error| log::warn!("cannot carry the task on: {error}"))
            .is_ok()
    }

    /// Run `task`, which the agent `caller` handed over.
    fn run(
        &self,
        caller: &str,
        task: &SubagentTask,
        ctx: &ToolContext,
        cancel: &CancelToken,
        on_update: &mut dyn FnMut(ToolUpdate),
    ) -> SubagentOutcome {
        self.delegate(caller, task, ctx, cancel, on_update)
            .unwrap_or_else(|error| Err(error).into())
    }

    /// Run `task`, which the agent `caller` handed over and waits for, so
    /// that it can move to the background midway: when the user writes to
    /// the panel meanwhile (see [`termide_panel_agent::live::ask_aside`]), the call returns at
    /// once, saying where the task went, and the task runs on out of sight,
    /// answering as one handed over in the background does. The agent then
    /// reads the user's message, and may stop the task with `jobs`.
    fn run_detachable(
        self: &Arc<Self>,
        caller: &str,
        task: &SubagentTask,
        ctx: &ToolContext,
        cancel: &CancelToken,
        on_update: &mut dyn FnMut(ToolUpdate),
    ) -> SubagentOutcome {
        let Some(parent) = ctx.panel else {
            return self.run(caller, task, ctx, cancel, on_update);
        };
        // The run's stop reaches the task only while the run waits on it.
        let own = CancelToken::new();
        let attached = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (updates_tx, updates) = std::sync::mpsc::channel::<ToolUpdate>();
        let (done_tx, done) = std::sync::mpsc::channel::<SubagentOutcome>();
        // Taken by the call that lets go of the task: the outcome then has no
        // one to go to, and is reported as a background task's.
        let waiting = Arc::new(std::sync::Mutex::new(Some(done_tx)));
        let parent_session = ctx.session.as_ref().and_then(|session| session.id.clone());
        let (this, ctx_owned, thread_own) = (Arc::clone(self), ctx.clone(), own.clone());
        let thread_attached = Arc::clone(&attached);
        let thread_waiting = Arc::clone(&waiting);
        let (thread_caller, agent, prompt, call) = (
            caller.to_string(),
            task.agent.to_string(),
            task.prompt.to_string(),
            task.call.to_string(),
        );
        let spawned = std::thread::Builder::new()
            .name(format!("task-{agent}"))
            .spawn(move || {
                let task = SubagentTask {
                    agent: &agent,
                    prompt: &prompt,
                    call: &call,
                    background: false,
                };
                let outcome = this
                    .delegate(
                        &thread_caller,
                        &task,
                        &ctx_owned,
                        &thread_own,
                        &mut |update| {
                            if thread_attached.load(std::sync::atomic::Ordering::Acquire) {
                                let _ = updates_tx.send(update);
                            }
                        },
                    )
                    .unwrap_or_else(|error| Err(error).into());
                let waiter = thread_waiting
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                if let Some(waiter) = waiter {
                    let _ = waiter.send(outcome);
                    return;
                }
                // Moved aside: answered as a task handed over in the
                // background is, once it has really run to its end.
                if outcome.parked {
                    return;
                }
                let held = termide_panel_agent::live::take_quiet(parent, &call);
                if termide_panel_agent::live::take_withdrawn(parent, &call) {
                    return;
                }
                let text = background_report(&prompt, &agent, &outcome);
                peers::report(
                    &this.project_root,
                    parent,
                    parent_session,
                    &agent,
                    text,
                    held,
                );
            });
        if let Err(error) = spawned {
            return Err(format!("cannot start the task: {error}")).into();
        }
        loop {
            for update in updates.try_iter() {
                on_update(update);
            }
            match done.recv_timeout(DETACH_POLL) {
                Ok(outcome) => {
                    for update in updates.try_iter() {
                        on_update(update);
                    }
                    termide_panel_agent::live::take_aside(parent, task.call);
                    return outcome;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("the task's thread ended without an answer".to_string()).into();
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            if cancel.is_cancelled() {
                own.cancel();
                continue;
            }
            if !termide_panel_agent::live::take_aside(parent, task.call) {
                continue;
            }
            // Its answer starts the agent anew, so it counts among the
            // messages agents pass one another: past their limit, the task
            // stays waited for.
            if peers::may_hand_over(&self.project_root, parent).is_err() {
                continue;
            }
            // Mark it as in the background before the call lets go, so a
            // stop of the panel's run from here on keeps its answer for the
            // next request rather than dropping it.
            termide_panel_agent::live::move_to_background(parent, task.call);
            let let_go = waiting
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            drop(let_go);
            // It ended just before the call let go: its answer is the call's.
            if let Ok(outcome) = done.try_recv() {
                for update in updates.try_iter() {
                    on_update(update);
                }
                return outcome;
            }
            attached.store(false, std::sync::atomic::Ordering::Release);
            return Ok(format!(
                "The user wrote while {} worked on the task, so it goes on out of sight and you \
                 can read what they wrote; its id is {}. Its answer comes to you later as a \
                 message of its own: do not wait for it with tools, and do not hand the same \
                 task over again. Should you no longer need it, stop it with `jobs`.",
                task.agent, task.call
            ))
            .into();
        }
    }

    /// `[ai] on_limit` as it stands now: a change in the settings reaches a
    /// delegated task's next limit without the panel being reopened.
    fn on_limit(&self) -> LimitPolicy {
        applied_ai_settings(&self.settings).on_limit
    }

    /// What a delegated run's end means for the task, by `[ai] on_limit` as
    /// it stands now.
    fn after_run(
        &self,
        panel: Option<u64>,
        task: &SubagentTask,
        outcome: SubagentOutcome,
        connection: &str,
        session: Option<&Path>,
    ) -> SubagentOutcome {
        park_or_answer(self.on_limit(), panel, task, outcome, connection, session)
    }
}

/// What a delegated run's end means for the task. A limit that passes on its
/// own is not its answer: the task is parked, and goes again when its wait is
/// over — at the reset it named, or at once when the panel moves to another
/// connection, which is how a spent subscription is got past. An answer, a stop
/// and a failure waiting does not cure end the task, and the park it was in
/// with them.
fn park_or_answer(
    policy: LimitPolicy,
    panel: Option<u64>,
    task: &SubagentTask,
    mut outcome: SubagentOutcome,
    connection: &str,
    session: Option<&Path>,
) -> SubagentOutcome {
    let parking = panel.filter(|_| {
        outcome.answer.is_err()
            && policy != LimitPolicy::Stop
            && outcome.failure.is_some_and(|failure| failure.kind.passes())
    });
    let Some(panel) = parking else {
        if let Some(panel) = panel {
            termide_panel_agent::live::unpark(panel, task.call);
        }
        return outcome;
    };
    let failure = outcome.failure.unwrap_or_default();
    let now = unix_now();
    let attempts = termide_panel_agent::live::parked_of(panel)
        .into_iter()
        .find(|parked| parked.call == task.call)
        .map_or(1, |parked| parked.attempts + 1);
    // A task parked too many times is not going to answer: the limit's own
    // words go back to the delegating agent, with what can be done.
    if attempts > MAX_PARK_ATTEMPTS {
        termide_panel_agent::live::unpark(panel, task.call);
        return outcome;
    }
    let wake = wake_at(&failure, attempts - 1, now);
    termide_panel_agent::live::park(
        panel,
        termide_panel_agent::live::Parked {
            call: task.call.to_string(),
            agent: task.agent.to_string(),
            task: task.prompt.to_string(),
            session: session.map(Path::to_path_buf).or(outcome.session.clone()),
            failure,
            wake_at: wake,
            connection: connection.to_string(),
            attempts,
            since: now,
            asked: false,
            launching: false,
        },
    );
    outcome.parked = true;
    outcome.answer = Ok(parked_words(task.agent, &failure, connection, wake));
    outcome
}

impl Subagents {
    /// Run the task; an error is one that came before anything was spent.
    fn delegate(
        &self,
        caller: &str,
        task: &SubagentTask,
        ctx: &ToolContext,
        cancel: &CancelToken,
        on_update: &mut dyn FnMut(ToolUpdate),
    ) -> Result<SubagentOutcome, String> {
        let name = task.agent;
        let definition = self.dirs.agent(name);
        // What the delegate wants to know goes to the user its parent's
        // panel asks, headed with the delegate's name.
        let asker = ctx
            .asker
            .as_ref()
            .map(|asker| asker.for_delegate(name).for_task(task.call));
        let (tools, skills, system_prompt) = self.toolset(name, &definition, asker.is_some());
        let max_turns = definition.spec.max_turns.unwrap_or(SUBAGENT_MAX_TURNS);

        let active = self
            .active
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let settings = applied_ai_settings(&self.settings);
        let target = subagent_target(&settings, &active)?;
        // The session's connection says who reviews, wherever the task runs.
        let reviewer = reviewer_setup(&settings, &self.dirs, &active.connection);
        // A meter of its own: what its reviewer spends counts in the task,
        // and in its log, not in the panel's own reviews.
        let reviews = reviewer.spent.clone();

        let mut rules = self.rules.clone();
        rules.mode = subagent_mode(self.mode.get(), definition.spec.mode);
        let plan_guard = PlanGuard::new(ModeHandle::new(rules.mode)).with_refusals(&self.refusals);
        // Stops a run that will not stop itself; the parent's cancel stops
        // it too.
        let budget = CancelToken::new();
        // What the rules do not settle goes to the user of its parent's
        // panel, on a card headed with its name; with no one there it is
        // refused with a reason the model reads.
        // Its cards end with its own run, not the panel's: a stop of the
        // panel's run reaches a task the run waits on through `cancel`, which
        // ends its budget, and leaves one handed over in the background — or
        // moved there when the user wrote — with its cards up.
        let prompter = ctx.prompter.as_ref().map(|prompter| {
            prompter
                .for_delegate(name, budget.clone())
                .for_task(task.call)
                .detached()
        });
        let asker = asker.map(|asker| asker.detached(budget.clone()));
        let asks: Box<dyn termide_agent_core::PermissionPrompter> = match &prompter {
            Some(prompter) => Box::new(prompter.clone()),
            None => Box::new(AutoDenyPrompter::new(
                self.refusals.unattended_subagent.clone(),
            )),
        };
        let permissions = PermissionHooks::new(rules.clone(), asks)
            .with_classifier(Box::new(reviewer.classifier(budget.clone())))
            .with_refusals(self.refusals.clone());
        // The guard goes first, as in the panel's own chain: in plan mode a
        // session answer must not let a change through.
        // A delegate's reads and changes count as its parent panel's: a
        // change on a file another panel changed since waits for a read.
        let hooks = ChainedHooks::new(vec![
            Box::new(plan_guard),
            Box::new(termide_agent_core::ConflictGuard::new()),
            Box::new(permissions),
        ]);

        // The task reaches the delegate as its parent's words, never the
        // user's, and its log names where it came from.
        let parent_session = ctx.session.as_ref().and_then(|session| session.id.clone());
        let message = UserMessage::text(task.prompt).with_origin(MessageOrigin {
            session: parent_session.clone().unwrap_or_default(),
            agent: caller.to_string(),
            kind: OriginKind::Task,
            panel: ctx.panel,
            hops: 0,
        });
        let lineage = SessionParent {
            session: parent_session,
            agent: caller.to_string(),
            call: task.call.to_string(),
        };

        let intent = ctx
            .session
            .as_ref()
            .map(|session| IntentLog::delegated(&session.intent))
            .unwrap_or_default();
        // A task carried on runs out of the log it already made: what it did
        // stands there, and the run goes on from where it stopped, in the
        // directory it worked in then. It holds the log as a panel does: a
        // log open in a panel, or in another termide, is not run twice.
        let resumed = match ctx.resume.as_ref() {
            Some(path) => Some(Session::open_exclusive(path).map_err(|error| {
                format!("cannot carry the task on from {}: {error}", path.display())
            })?),
            None => None,
        };
        // An external agent runs its own program, whatever the session's
        // connection: out of sight as a Claude Code copy does, asking the
        // user of its parent's panel what its rules leave open. Carried on,
        // it goes on in its own session of the agent's, from its log.
        if let Some(config) = definition.spec.acp.clone() {
            let log = match resumed {
                Some(session) => DelegateLog::resume(session),
                None => DelegateLog::start(
                    self.session_dir.as_deref(),
                    &self.cwd,
                    lineage,
                    &DelegateRun {
                        connection: name,
                        provider: termide_agent_core::ACP_PROVIDER,
                        model: "",
                        context_window: None,
                        agent: name,
                    },
                ),
            };
            let run = CliSubagent {
                name: name.to_string(),
                connection: active.connection.clone(),
                max_turns,
                reviewer,
                config,
                model: String::new(),
                system_prompt,
                // It drives its own tools, as in a panel.
                tools,
                skills,
                hooks,
                rules,
                asker,
                prompter,
                panel: ctx.panel,
            };
            return self.run_cli(run, message, log, &budget, cancel, caller, task, on_update);
        }

        let (provider, requested, context_window, connection_name, kind) = match target {
            SubagentTarget::Cli(connection_name, connection) => {
                let config = cli_acp_config(&connection)
                    .ok_or_else(|| format!("{} is not a CLI agent", connection.provider))?;
                let log = match resumed {
                    Some(session) => DelegateLog::resume(session),
                    None => DelegateLog::start(
                        self.session_dir.as_deref(),
                        &self.cwd,
                        lineage,
                        &DelegateRun {
                            connection: &connection_name,
                            provider: &connection.provider,
                            model: connection.model.trim(),
                            context_window: None,
                            agent: name,
                        },
                    ),
                };
                let run = CliSubagent {
                    name: format!("{connection_name}:{name}"),
                    connection: connection_name.clone(),
                    max_turns,
                    reviewer,
                    config,
                    model: connection.model.trim().to_string(),
                    system_prompt,
                    tools,
                    skills,
                    hooks,
                    rules,
                    asker,
                    prompter,
                    panel: ctx.panel,
                };
                return self.run_cli(run, message, log, &budget, cancel, caller, task, on_update);
            }
            // On a connection of its own the subagent runs that connection's
            // model: the id its `AGENT.md` names is one of the session's
            // endpoint.
            SubagentTarget::Model(connection_name, connection) => (
                connection_provider(&connection_name, &connection),
                connection.model.clone(),
                connection.effective_context_window(),
                connection_name,
                connection.provider,
            ),
            SubagentTarget::Session(model) => (
                Arc::clone(&active.provider),
                definition
                    .spec
                    .model
                    .clone()
                    .or(model)
                    .unwrap_or_else(|| active.model.clone()),
                active.context_window,
                active.connection.clone(),
                active.kind.clone(),
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
        let cwd = resumed
            .as_ref()
            .map(|session| session.header().cwd.clone())
            .unwrap_or_else(|| self.cwd.clone());
        let mut log = match resumed {
            Some(session) => DelegateLog::resume(session),
            None => DelegateLog::start(
                self.session_dir.as_deref(),
                &self.cwd,
                lineage,
                &DelegateRun {
                    connection: &connection_name,
                    provider: &kind,
                    model: &model.id,
                    context_window: Some(model.context_window),
                    agent: name,
                },
            ),
        };
        let carried = log
            .0
            .as_ref()
            .map(|session| session.context_messages())
            .unwrap_or_default();
        // The reviewer judges the subagent's calls against what the user asked
        // the delegating agent; the task itself counts as that agent's words.
        let mut agent = Agent::new(provider, tools, model, cwd)
            .with_system_prompt(system_prompt)
            .with_compaction(self.compaction)
            .with_delegated_intent(intent);
        if !carried.is_empty() {
            agent = agent.with_messages(carried);
        }
        if let Some(asker) = asker {
            agent = agent.with_asker(asker);
        }
        if let Some(panel) = ctx.panel {
            agent = agent.with_panel(panel);
        }
        // The subagent's terminal sessions are its own, kept under its log's
        // id and ended with its task; its parent does not reach them.
        let terminals_owner = log.0.as_ref().map(|session| session.id().to_string());
        if terminals_owner.is_some() {
            let terminals: Arc<dyn termide_agent_core::TerminalSessions> =
                terminals::agent_terminals().clone();
            agent = agent
                .with_session_id(terminals_owner.clone())
                .with_context_note(termide_agent_core::terminals::context_note(terminals));
        }

        // Followed from its parent's panel while it runs.
        let live = register_live(ctx.panel, caller, task, &log);
        // The panel's pause holds this run at its step boundaries.
        agent = agent.with_pause_gate(live.control().gate());
        let user_stop = live.control().stop_token().clone();
        let mut noted = noting(&live, on_update);
        let on_update: &mut dyn FnMut(ToolUpdate) = &mut noted;
        let mut hooks = hooks;
        let mut report = SubagentReport::new(max_turns, &connection_name);
        let mut noted = false;
        let finished = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            // The parent's stop reaches the run at once, not at its next
            // event: a long prefill or tool sends none.
            scope.spawn(|| {
                while !finished.load(std::sync::atomic::Ordering::Acquire) {
                    if cancel.is_cancelled() || user_stop.is_cancelled() {
                        budget.cancel();
                        return;
                    }
                    std::thread::sleep(CLI_SUBAGENT_POLL);
                }
            });
            let mut emit = |event: AgentEvent| match &event {
                AgentEvent::MessageEnd(message) => {
                    let first = !noted;
                    noted = true;
                    log.note(message);
                    // The run's mark goes after its first message, as a
                    // panel's does; the task's words are that message.
                    if first {
                        log.note_run_start();
                    }
                    if let Message::Assistant(message) = message {
                        report.said(message, on_update);
                    }
                }
                AgentEvent::MessageStart { .. } => {
                    if report.turn() {
                        budget.cancel();
                    }
                }
                AgentEvent::MessageUpdate(StreamEvent::Queued { ahead }) => {
                    report.waiting(Some(*ahead), on_update);
                }
                AgentEvent::MessageUpdate(StreamEvent::Admitted) => {
                    report.waiting(None, on_update);
                }
                AgentEvent::ToolExecutionStart { call } => {
                    report.called(call, &self.cwd, on_update);
                }
                AgentEvent::ToolExecutionEnd { result } => {
                    report.call_ended(result, on_update);
                }
                _ => {}
            };
            agent.run(message, &mut hooks, &budget, &mut emit);
            finished.store(true, std::sync::atomic::Ordering::Release);
        });
        if let Some(owner) = &terminals_owner {
            termide_agent_core::TerminalSessions::release(
                terminals::agent_terminals().as_ref(),
                owner,
            );
        }
        if report.answer.is_none() && report.error.is_none() {
            report.error = run_error(agent.messages());
        }
        let answered = report.answer.is_some();
        let session = log.path();
        let mut ended = match live.control().ended_by_user() {
            Some(why) if !cancel.is_cancelled() => report.ended_by_user(why),
            _ => report.into_outcome(cancel.is_cancelled()),
        };
        ended.spent.add(&log.note_reviews(&reviews));
        let outcome = self.after_run(ctx.panel, task, ended, &connection_name, session.as_deref());
        // Held at its panel's pause, or parked at a limit that will pass: the
        // reopened session asks to carry the task on. One that answered is
        // over, whatever pause or limit came after its last step.
        log.note_run_end(!answered && (live.control().is_paused() || outcome.parked));
        Ok(log.into_outcome(outcome))
    }

    /// Run `run` as a session of its own of its CLI agent, which calls the
    /// subagent's tools through its hooks: the agent's model when the
    /// connection names one, nobody to ask — the agent's own permission
    /// requests are refused with the reason a subagent's calls get — and the
    /// process gone once the task is.
    #[allow(clippy::too_many_arguments)]
    fn run_cli(
        &self,
        run: CliSubagent,
        message: UserMessage,
        mut log: DelegateLog,
        budget: &CancelToken,
        cancel: &CancelToken,
        caller: &str,
        task: &SubagentTask,
        on_update: &mut dyn FnMut(ToolUpdate),
    ) -> Result<SubagentOutcome, String> {
        // Followed from its parent's panel while it runs.
        let live = register_live(run.panel, caller, task, &log);
        let mut noted = noting(&live, on_update);
        let on_update: &mut dyn FnMut(ToolUpdate) = &mut noted;
        let config = run.config;
        let reviews = run.reviewer.spent.clone();
        // The agent's own requests go where its parent's panel asks; with
        // no one there they come here, and are refused.
        let (prompter, permission_rx) = match run.prompter {
            Some(prompter) => (prompter, None),
            None => {
                let (prompter, rx) = permission_channel(budget.clone());
                (prompter, Some(rx))
            }
        };
        // Carried on, the task goes on in the agent's own session when it
        // still holds it, else the agent is told a recap of the log.
        let (resume, history) = log.external_resume();
        let cwd = log.cwd().unwrap_or_else(|| self.cwd.clone());
        let setup = BackendSetup {
            cwd: cwd.clone(),
            prompter,
            cancel: budget.clone(),
            mode: ModeHandle::new(run.rules.mode),
            rules: run.rules,
            persist: None,
            system_prompt: run.system_prompt,
            plan: self.dirs.plan_prompt(),
            plan_seen: false,
            goal: self.dirs.goal_prompt(),
            handoff: self.dirs.handoff_prompt(),
            reviewer: run.reviewer,
            host_tools: Some(HostTools {
                tools: run.tools,
                hooks: Box::new(run.hooks),
                context: ToolContext {
                    asker: run.asker,
                    panel: run.panel,
                    ..ToolContext::new(cwd.clone())
                },
                skills: run.skills,
                session_id: None,
            }),
            resume,
            history,
        };
        let runtime = AcpRuntime::start(&run.name, &config, setup)?;
        // The task's run is recorded in its own log, as a panel's is: cut
        // off by termide quitting, the session reads as unfinished and offers
        // to carry on when it is opened again.
        log.note_run_start();
        let drive = CliDrive {
            control: live.control(),
            name: &run.name,
            connection: &run.connection,
            model: &run.model,
            permission_rx: permission_rx.as_ref(),
            refusal: &self.refusals.unattended_subagent,
            max_turns: run.max_turns,
            cwd: &cwd,
            budget,
            cancel,
            held_at_end: std::cell::Cell::new(false),
        };
        let session = log.path();
        let panel = run.panel;
        let connection = run.connection.clone();
        // The process goes with the runtime, once the task is over.
        let outcome = drive.run(
            &runtime,
            &|| runtime.is_starting(),
            message,
            &mut log,
            on_update,
        );
        let outcome = match outcome {
            Ok(mut outcome) => {
                outcome.spent.add(&log.note_reviews(&reviews));
                Ok(self.after_run(panel, task, outcome, &connection, session.as_deref()))
            }
            Err(error) => {
                if let Some(panel) = panel {
                    termide_panel_agent::live::unpark(panel, task.call);
                }
                Err(error)
            }
        };
        // Parked at a limit that will pass, or held at its panel's pause when
        // the panel closed: the task stands unfinished, and a reopen of its
        // session offers to carry it on.
        let unfinished =
            drive.held_at_end.get() || outcome.as_ref().is_ok_and(|outcome| outcome.parked);
        log.note_run_end(unfinished);
        outcome.map(|outcome| log.into_outcome(outcome))
    }
}

impl App {
    /// The parked delegates whose wait is over go again: at the reset their
    /// limit named, when the panel that delegated them stands on another
    /// connection — which is how a spent subscription is got past — or when the
    /// user asks their row to. Each runs out of its own log, on the connection
    /// its panel is on now, and answers back there as any background task does.
    pub(in crate::app) fn wake_parked_delegates(&mut self) {
        if !termide_panel_agent::live::any_parked() {
            return;
        }
        let now = unix_now();
        // The panels are taken first: a restart reaches the runner behind the
        // panel, not the panel itself.
        let panels: Vec<(u64, String, String, Arc<dyn AgentCatalog>)> = self
            .layout_manager
            .iter_all_panels_mut()
            .filter_map(|panel| {
                let agent = panel.downcast_mut::<AgentPanel>()?;
                Some((
                    agent.panel_id(),
                    agent.agent_name().to_string(),
                    agent.connection_name().to_string(),
                    agent.catalog(),
                ))
            })
            .collect();
        let mut woke = false;
        for (panel, caller, connection, catalog) in panels {
            for parked in termide_panel_agent::live::parked_of(panel) {
                let due = parked.wake_at <= now || parked.asked;
                // The connection it stopped on is not the one in use: waiting
                // for its reset would wait out a limit that no longer applies.
                let switched = parked.connection != connection;
                if !(due || switched) {
                    continue;
                }
                let Some(entry) = termide_panel_agent::live::launch(panel, &parked.call) else {
                    continue;
                };
                let Some(session) = entry.session else {
                    // Nothing was logged to go on from: the task ends here, and
                    // its parent has already been told the limit stopped it.
                    termide_panel_agent::live::abandon(panel, &parked.call);
                    continue;
                };
                let item = termide_core::DelegateRestart {
                    agent: entry.agent.clone(),
                    session,
                    call: entry.call.clone(),
                    task: entry.task.clone(),
                };
                if !catalog.restart_delegate(panel, &caller, &item) {
                    // The log is open elsewhere or unreadable: it asks again
                    // shortly, rather than every tick.
                    termide_panel_agent::live::sleep_until(panel, &parked.call, now + 60);
                    continue;
                }
                log::info!("{}'s parked task goes again on {connection}", entry.agent);
                woke = true;
            }
        }
        if woke {
            self.state.needs_redraw = true;
        }
    }
}

/// The tasks a panel's agent handed over that run on without a call waiting
/// for them — in the background, or parked at a limit — for `jobs` to list
/// and stop. The task is the agent's own, so a stop asks nothing, and its
/// answer does not come.
struct PanelTasks;

impl OwnTasks for PanelTasks {
    fn list(&self, ctx: &ToolContext) -> Vec<String> {
        use termide_agent_tools::jobs_text::{duration, first_line};
        let Some(panel) = ctx.panel else {
            return Vec::new();
        };
        let mut lines: Vec<String> = termide_panel_agent::live::background_of(panel)
            .into_iter()
            .map(|task| {
                let state = if task.paused {
                    "held at the panel's pause".to_string()
                } else {
                    format!("running {}", duration(task.started.elapsed()))
                };
                let last = if task.status.is_empty() {
                    String::new()
                } else {
                    format!(" · last: {}", first_line(&task.status, 80))
                };
                format!(
                    "- {} · background task · {}: {} · {state}{last}",
                    task.call,
                    task.agent,
                    first_line(&task.task, 100)
                )
            })
            .collect();
        let now = unix_now();
        lines.extend(
            termide_panel_agent::live::parked_of(panel)
                .into_iter()
                .filter(|parked| !parked.launching)
                .map(|parked| {
                    let wait = std::time::Duration::from_secs(parked.wake_at.saturating_sub(now));
                    format!(
                        "- {} · task waiting out a limit · {}: {} · goes again in {}",
                        parked.call,
                        parked.agent,
                        first_line(&parked.task, 100),
                        duration(wait)
                    )
                }),
        );
        lines
    }

    fn stop(&self, id: &str, ctx: &ToolContext) -> Option<String> {
        use termide_panel_agent::live::{withdraw, Withdrawn};
        match withdraw(ctx.panel?, id) {
            Withdrawn::Running(agent) => Some(format!(
                "Stopped {agent}'s task {id}; no answer will come from it."
            )),
            Withdrawn::Parked(agent) => Some(format!(
                "Gave up {agent}'s task {id}, which waited out a limit; no answer will come from it."
            )),
            Withdrawn::None => None,
        }
    }
}

/// The message a background task's answer reaches the delegating agent as:
/// which task it answers, then the delegate's last words or what went wrong.
fn background_report(prompt: &str, agent: &str, outcome: &SubagentOutcome) -> String {
    let task: String = prompt
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(120)
        .collect();
    let ellipsis = if task.len() < prompt.trim_end().len() {
        "…"
    } else {
        ""
    };
    let head = format!(
        "The background task you handed to {agent} is over (\"{task}{ellipsis}\"); \
         do not reply to this message."
    );
    match &outcome.answer {
        Ok(answer) => format!("{head}\n\nIts answer:\n\n{answer}"),
        Err(error) => format!("{head}\n\nIt failed: {error}"),
    }
}

/// Register the task `task` the agent `caller` of panel `panel` handed
/// over, logged in `log`, for as long as it runs.
fn register_live(
    panel: Option<u64>,
    caller: &str,
    task: &SubagentTask,
    log: &DelegateLog,
) -> termide_panel_agent::live::Registration {
    termide_panel_agent::live::register(termide_panel_agent::live::LiveTask {
        panel,
        call: task.call,
        from: caller,
        agent: task.agent,
        task: task.prompt,
        session: log.path(),
        background: task.background,
    })
}

/// `on_update`, noting each update's last line in `live` for the task's row:
/// what the delegate did last.
fn noting<'a>(
    live: &'a termide_panel_agent::live::Registration,
    on_update: &'a mut dyn FnMut(ToolUpdate),
) -> impl FnMut(ToolUpdate) + 'a {
    move |update| {
        let ToolUpdate::Output(text) = &update;
        if let Some(last) = text.lines().rev().find(|line| !line.trim().is_empty()) {
            live.set_status(last.trim());
        }
        on_update(update);
    }
}

/// The run of a subagent on a CLI agent once its process is up.
struct CliDrive<'a> {
    /// What the user does to the task from a panel following it.
    control: &'a termide_panel_agent::DelegateControl,
    name: &'a str,
    /// The model to pick for the task, empty for the agent's own.
    model: &'a str,
    /// The agent's own permission requests, refused: nobody watches. `None`
    /// when they go to the user of its parent's panel instead.
    permission_rx: Option<&'a std::sync::mpsc::Receiver<termide_agent_core::PermissionEnvelope>>,
    refusal: &'a str,
    /// The model calls it may take before it is cut off.
    max_turns: usize,
    /// Where it works: its calls' paths are shown relative to it.
    cwd: &'a Path,
    /// The connection its agent runs on, named when the task stops there.
    connection: &'a str,
    budget: &'a CancelToken,
    cancel: &'a CancelToken,
    /// The run ended while its panel's pause held it — the panel closed —
    /// so its log reads as paused, to be carried on.
    held_at_end: std::cell::Cell<bool>,
}

impl CliDrive<'_> {
    /// Pick the model, hand `backend` the task and follow it to its end,
    /// reporting what it says; `starting` says whether its handshake is
    /// still under way.
    fn run(
        &self,
        backend: &dyn Backend,
        starting: &dyn Fn() -> bool,
        message: UserMessage,
        log: &mut DelegateLog,
        on_update: &mut dyn FnMut(ToolUpdate),
    ) -> Result<SubagentOutcome, String> {
        let stopped = || {
            self.cancel.is_cancelled()
                || self.budget.is_cancelled()
                || self.control.stop_token().is_cancelled()
        };
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
        let origin = message.origin.clone();
        backend
            .prompt(message)
            .map_err(|error| format!("the subagent did not start: {error:?}"))?;
        let mut report = SubagentReport::new(self.max_turns, self.connection);
        let mut aborted: Option<std::time::Instant> = None;
        let mut idle = 0;
        // The agent runs its own loop, which no pause reaches: held at its
        // panel's pause, its turn is cut short, and once the pause is lifted
        // the same session is asked to carry on.
        let mut holding: Option<std::time::Instant> = None;
        // Something came of the turn: an agent cut short before that may go
        // on with the cut prompt out of sight, so the cut waits for it.
        let mut heard = false;
        'run: loop {
            for envelope in self.permission_rx.into_iter().flat_map(|rx| rx.try_iter()) {
                let _ = envelope
                    .reply
                    .send(PermissionAnswer::DenyWithReason(self.refusal.to_string()));
            }
            let mut ended = false;
            for event in backend.drain() {
                heard = true;
                if let AgentEvent::MessageEnd(message) = &event {
                    log.note(message);
                }
                match event {
                    AgentEvent::MessageEnd(Message::Assistant(message)) => {
                        let over = report.turn();
                        report.said(&message, on_update);
                        if over {
                            self.budget.cancel();
                        }
                    }
                    AgentEvent::ToolExecutionStart { call } => {
                        report.called(&call, self.cwd, on_update);
                    }
                    AgentEvent::ToolExecutionEnd { result } => {
                        report.call_ended(&result, on_update);
                    }
                    AgentEvent::ExternalSession { agent, session_id } => {
                        log.note_external_session(&agent, &session_id);
                    }
                    AgentEvent::AgentEnd => {
                        ended = true;
                        break;
                    }
                    _ => {}
                }
            }
            // An agent that went away without saying it ended is over too,
            // as is one that does not end within a while of being stopped.
            idle = if backend.is_busy() { 0 } else { idle + 1 };
            // A cut that does not end the turn within a while is taken as
            // ended, as a stop's is.
            let over = ended
                || idle > 1
                || holding.is_some_and(|at| at.elapsed() > CLI_SUBAGENT_ABORT_WAIT);
            if over && holding.is_some() && !stopped() {
                // The cut turn is over: wait out the pause, then go on.
                while self.control.is_paused() && !stopped() {
                    std::thread::sleep(CLI_SUBAGENT_POLL);
                }
                if stopped() {
                    // A pause still on: its panel closed, holding it.
                    self.held_at_end.set(self.control.is_paused());
                    break 'run;
                }
                let mut carry_on = UserMessage::text(CLI_SUBAGENT_CARRY_ON);
                carry_on.origin.clone_from(&origin);
                if backend.prompt(carry_on).is_err() {
                    break 'run;
                }
                (holding, heard, idle) = (None, false, 0);
                continue;
            }
            if over || aborted.is_some_and(|at| at.elapsed() > CLI_SUBAGENT_ABORT_WAIT) {
                break;
            }
            if aborted.is_none() && stopped() {
                aborted = Some(std::time::Instant::now());
                // Stopped while its turn was being cut for the pause: the
                // pause still on means its panel closed, holding it.
                if holding.is_some() {
                    self.held_at_end.set(self.control.is_paused());
                }
                self.budget.cancel();
                backend.abort();
            } else if holding.is_none() && heard && self.control.is_paused() {
                holding = Some(std::time::Instant::now());
                backend.abort();
            }
            std::thread::sleep(CLI_SUBAGENT_POLL);
        }
        Ok(match self.control.ended_by_user() {
            Some(why) if !self.cancel.is_cancelled() => report.ended_by_user(why),
            _ => report.into_outcome(self.cancel.is_cancelled()),
        })
    }
}

/// What a subagent on a CLI agent runs with, built as for termide's loop.
struct CliSubagent {
    /// How its process is named in the logs.
    name: String,
    /// The connection it runs on, named when the task stops there.
    connection: String,
    /// The model calls it may take before it is cut off.
    max_turns: usize,
    /// Reviews its requests in `auto` mode.
    reviewer: ReviewerSetup,
    /// How its process runs.
    config: AcpConfig,
    /// The model to pick for the task, empty for the agent's own.
    model: String,
    system_prompt: String,
    tools: ToolRegistry,
    skills: Vec<termide_agent_core::SkillInfo>,
    hooks: ChainedHooks,
    rules: PermissionRules,
    /// Puts its questions to the user of its parent's panel.
    asker: Option<termide_agent_core::UserAsker>,
    /// Puts its permission requests to that user.
    prompter: Option<termide_agent_core::ChannelPrompter>,
    /// Its parent's panel, whose reads and changes its own count as.
    panel: Option<u64>,
}

/// The MCP servers of `dirs`, a project's held back unless approved as they
/// are configured now.
fn mcp_servers(
    dirs: &AgentDirs,
    approvals: &termide_agent_core::approvals::Approvals,
) -> std::collections::BTreeMap<String, termide_agent_core::McpServerConfig> {
    let mut servers = dirs.mcp_servers();
    approvals.withhold_mcp(&mut servers);
    servers
}

/// How often a subagent on a CLI agent is looked at while it runs.
const CLI_SUBAGENT_POLL: std::time::Duration = std::time::Duration::from_millis(50);
/// How often a call waiting on a task looks whether the user asked it aside.
const DETACH_POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// How long a stopped subagent on a CLI agent may take to end its turn.
const CLI_SUBAGENT_ABORT_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// What a subagent on a CLI agent is told once the pause that cut its turn
/// short is lifted.
const CLI_SUBAGENT_CARRY_ON: &str = "You were paused, not stopped: carry the task on from where \
     you were cut off. Do not start it over.";

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
    terminals::agent_terminals().set_limit(settings.max_terminal_sessions);

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
        session_dir: session_dir_of(&cwd),
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
    let loop_prompt = catalog.dirs.loop_prompt();
    let refusals = catalog.dirs.refusals();
    let hooks: Option<HooksFactory> = {
        let configured = !catalog.dirs.hooks().is_empty();
        let hook_cwd = cwd.clone();
        let dirs = catalog.dirs.clone();
        let approvals = catalog.approvals.clone();
        // Read at every run, so a hook approved meanwhile runs from the
        // next one on, and a project's not approved never does.
        configured.then(|| {
            Arc::new(move || {
                let (run, _) = approvals.split_hooks(dirs.hooks());
                Box::new(CommandHooks::new(run, hook_cwd.clone()))
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
    let provider_backend = cli_provider_backend(&connection, &agent);
    let backend = provider_backend.clone().or(profile.backend);

    let session_dir = session_dir_of(&cwd);

    // Built before `cwd` moves into the setup: a hand-run command runs where
    // the panel works.
    let shell_run = user_shell_runner(&catalog.dirs, &cwd);
    // The reviewer of a connection switched to reads its texts from here.
    let connection_dirs = catalog.dirs.clone();

    AgentPanelSetup {
        panel_id: None,
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
        connection_settings: connection.clone(),
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
        loop_prompt,
        reviewer,
        side_models: side_models(settings),
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
        terminals: Some(Arc::clone(terminals::agent_terminals()) as _),
        on_limit: settings.on_limit,
    }
}

/// The runner behind `!` in the agent panel's input and `[Run]` on a
/// `suggest_command` card: the same `shell` tool the agent calls, so timeouts,
/// output cleaning and the full-log file behave identically either way. It is
/// built here rather than taken from the agent's registry because a command
/// the user ran is not governed by the tools an agent profile was given —
/// `restrict_tools` may well have left `shell` out of those.
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
    let tool = ShellTool {
        shell: terminals::agent_terminals().shell(),
        shim_path: dirs.shims_dir(),
        ..ShellTool::default()
    };
    let cwd = cwd.to_path_buf();
    ShellRunner::new(move |command, cancel| {
        let call = ToolCall {
            id: "user-command".into(),
            name: "shell".into(),
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
fn cli_provider_backend(connection: &Connection, agent: &str) -> Option<BackendFactory> {
    let config = cli_acp_config(connection)?;
    let agent = agent.to_string();
    Some(Arc::new(move |setup: termide_agent_core::BackendSetup| {
        AcpRuntime::start(&agent, &config, setup)
            .map(|runtime| Box::new(runtime) as Box<dyn termide_agent_core::Backend>)
    }) as BackendFactory)
}

/// How the ACP adapter of a CLI-adapter connection runs; `None` for any
/// other provider.
fn cli_acp_config(connection: &Connection) -> Option<AcpConfig> {
    // Claude Code takes termide's prompt and tools in place of its own; Codex
    // and Gemini CLI keep their own and have termide's permission mode mapped
    // onto their modes.
    let (package, flag, flavor) = match connection.provider.as_str() {
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
    //
    // `--prefer-offline` takes what `@latest` meant at the last look from
    // npm's cache instead of asking the registry at every start, which cost
    // seconds per panel, side call and subagent, and over a minute with the
    // registry out of reach. The look is taken once per process, off the
    // thread, so the next start runs the new release.
    #[cfg(not(test))]
    refresh_adapter(package);
    Some(AcpConfig {
        command: "npx".to_string(),
        args: ["-y", "--prefer-offline", package]
            .into_iter()
            .chain(flag)
            .map(str::to_string)
            .collect(),
        env: context_limit_env(connection),
        timeout_secs: 120,
        flavor,
        context_limit: context_limit(connection),
    })
}

/// The window a CLI agent is limited to: the connection's
/// `context_window_limit`, raised to the least the agent takes; `None` for
/// none, or an agent that cannot be limited.
fn context_limit(connection: &Connection) -> Option<u64> {
    let limit = connection
        .context_window_limit
        .filter(|_| connection.takes_context_limit())?;
    Some(limit.max(connection.min_context_limit().unwrap_or(0)))
}

/// The environment that limits a CLI agent's context window, see
/// [`context_limit`]: Claude Code compacts at
/// `CLAUDE_CODE_AUTO_COMPACT_WINDOW`; Codex takes its window, and compacts
/// at nine tenths of it, from `CODEX_CONFIG`, which its adapter merges into
/// the session's config — over what the user's own `CODEX_CONFIG` holds.
fn context_limit_env(connection: &Connection) -> std::collections::BTreeMap<String, String> {
    let mut env = std::collections::BTreeMap::new();
    let Some(limit) = context_limit(connection) else {
        return env;
    };
    match connection.provider.as_str() {
        "claude_code" => {
            env.insert("CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(), limit.to_string());
        }
        "codex" => {
            let own = std::env::var("CODEX_CONFIG").ok();
            env.insert("CODEX_CONFIG".into(), codex_config(own.as_deref(), limit));
        }
        _ => {}
    }
    env
}

/// `CODEX_CONFIG` holding the user's own (`own`, when it is a JSON object)
/// with the window set to `limit`.
fn codex_config(own: Option<&str>, limit: u64) -> String {
    let mut config = own
        .and_then(|own| serde_json::from_str::<serde_json::Value>(own).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    config["model_context_window"] = limit.into();
    config["model_auto_compact_token_limit"] = (limit / 10 * 9).into();
    config.to_string()
}

/// Once per process for each adapter `package`, refresh npm's idea of its
/// `@latest` and fetch a newer release into npx's cache, for the starts that
/// [`cli_acp_config`] makes with `--prefer-offline`. Off the thread and after
/// a delay, so it neither holds up nor races the install of the start that
/// asked for it; a refresh that hangs is killed.
#[cfg(not(test))]
fn refresh_adapter(package: &'static str) {
    use std::collections::HashSet;
    use std::process::{Command, Stdio};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const DELAY: Duration = Duration::from_secs(60);
    const LIMIT: Duration = Duration::from_secs(300);
    static REFRESHED: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

    let first = REFRESHED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashSet::new)
        .insert(package);
    if !first {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("termide-adapter-refresh".into())
        .spawn(move || {
            std::thread::sleep(DELAY);
            // The spec must match the starts' so npx keeps one install;
            // `--version` makes the adapter exit once it is in place.
            let child = Command::new("npx")
                .args(["-y", "--prefer-online", package, "--version"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            let mut child = match child {
                Ok(child) => child,
                Err(error) => {
                    log::warn!("cannot refresh {package}: {error}");
                    return;
                }
            };
            let deadline = Instant::now() + LIMIT;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    _ => {
                        log::warn!("refreshing {package} took too long; stopped");
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }
                }
            }
        });
    if let Err(error) = spawned {
        log::warn!("cannot refresh {package}: {error}");
    }
}

/// Start an off-thread `list_models` for the settings modal's model dropdown
/// of `connection`. A CLI agent is started for it, as for a side call, and
/// lists the models of its login; the process ends with the fetch. The
/// receiver is polled by the app loop.
/// `api_key` is the key the app could find without asking anyone.
pub(crate) fn spawn_settings_model_fetch(
    connection: &Connection,
    api_key: Option<String>,
) -> Option<std::sync::mpsc::Receiver<Result<Vec<termide_agent_core::ModelInfo>, String>>> {
    let provider: Arc<dyn Provider> = if connection.is_cli() {
        let config = cli_acp_config(connection)?;
        let cwd = std::env::current_dir().unwrap_or_default();
        Arc::new(AcpProvider::new("settings", config, cwd))
    } else {
        build_provider(connection, api_key)
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(provider.list_models());
    });
    Some(rx)
}

/// The provider of connection `name` for a session or a side call: its
/// requests take the connection's slots, which every panel of the process
/// shares (see `max_concurrent_requests`). A CLI agent's is a placeholder
/// that serves no request, so it takes none. A model connection's key is
/// resolved for each call (see `api_keys`).
fn connection_provider(name: &str, connection: &Connection) -> Arc<dyn Provider> {
    if connection.is_cli() {
        return build_provider(connection, None);
    }
    if connection.api_key_env_looks_like_key() {
        log::warn!(
            "api_key_env of connection {name} holds what looks like a key, not a variable \
             name; it is not used"
        );
    }
    let provider: Arc<dyn Provider> = Arc::new(KeyedProvider::new(
        name,
        connection,
        build_provider,
        Broker::global(),
    ));
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
            anthropic = anthropic.with_attribution(settings.attribution());
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
                    .with_attribution(settings.attribution())
                    .with_compat({
                        let reasoning = reasoning_param(settings);
                        Compat {
                            reasoning,
                            send_reasoning: sends_reasoning(settings),
                            prefill_progress: prefill_progress(settings),
                            prompt_cache: asks_for_prompt_cache(settings),
                            ..Compat::default()
                        }
                    }),
            )
        }
    }
}

/// The field an OpenAI-compatible connection takes the reasoning level in.
/// A known hosted API — a fact about the service, not a guess from its
/// address — takes `reasoning_effort`; any other server is taken for one that
/// renders a chat template, whatever its address: llama.cpp on a rented
/// machine is as likely as one at home. A server that refuses the template's
/// `enable_thinking` is asked again without it (see the provider).
fn reasoning_param(connection: &Connection) -> ReasoningParam {
    if is_hosted_api(&connection.base_url) {
        ReasoningParam::Effort
    } else {
        ReasoningParam::EnableThinking
    }
}

/// Whether the model's reasoning goes back with the answers of the current
/// turn: to every server but the hosted APIs, which keep reasoning their own
/// way. A server that refuses the field is asked again without it.
fn sends_reasoning(connection: &Connection) -> bool {
    !is_hosted_api(&connection.base_url)
}

/// Whether a connection asks its server for prompt-processing progress
/// (llama.cpp's `return_progress`, for a live prefill bar): every server but
/// the hosted APIs, which have none to give. One that refuses the field is
/// asked again without it; others ignore it.
fn prefill_progress(connection: &Connection) -> bool {
    !is_hosted_api(&connection.base_url)
}

/// Whether a connection asks its gateway to cache the prompt of the models
/// that cache only on request (see `Compat::prompt_cache`): OpenRouter, a
/// known service whose Anthropic models need it. Elsewhere a model caches
/// by itself, or the server is not one to ask.
fn asks_for_prompt_cache(connection: &Connection) -> bool {
    let host = termide_config::url_host(&connection.base_url);
    host == "openrouter.ai" || host.ends_with(".openrouter.ai")
}

/// Whether `url` is one of the hosted APIs known to take `reasoning_effort`.
fn is_hosted_api(url: &str) -> bool {
    let host = termide_config::url_host(url);
    [
        "api.openai.com",
        "openrouter.ai",
        "generativelanguage.googleapis.com",
    ]
    .iter()
    .any(|known| host == *known || host.ends_with(&format!(".{known}")))
}

/// The shipped default base URL, used to tell "left at default" from "set on
/// purpose" when picking a provider.
fn default_openai_base_url() -> String {
    Connection::default().base_url
}

/// Persist an "allow always" rule as `[ai.permissions.<tool>]`: for the
/// project, in termide's data directory under the current project
/// (`termide_project::granted_rules_path`) — never in the repository, whose
/// config may not allow anything — or in the global configuration.
///
/// The rule is merged into the document rather than appended, so a repeated
/// grant updates it in place instead of writing a second `[ai.permissions.…]`
/// table (two tables of the same name are invalid TOML and would break the
/// whole file). `toml_edit` keeps the file's other settings, comments and
/// layout intact.
fn persist_rule(tool: &str, pattern: &str, decision: Decision, scope: PersistScope) {
    let path = match scope {
        // Not the project's own `.termide/config.toml`: the repository's
        // config may not allow anything, so the user's grants for it are
        // kept in termide's data instead.
        PersistScope::Project => match std::env::current_dir()
            .map_err(anyhow::Error::from)
            .and_then(|cwd| termide_project::granted_rules_path(&cwd))
        {
            Ok(path) => path,
            Err(error) => {
                log::warn!("cannot find where to keep the project's rules: {error}");
                return;
            }
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
mod tests;
