//! The coding agent panel: a transcript above a multi-line input, tool calls
//! collapsed to one line each, permission prompts routed through termide's
//! selection modal.
//!
//! The panel owns an [`AgentRuntime`] and mirrors its events into a
//! [`Transcript`] from `tick()`, so it never blocks the UI thread. Every
//! transcript change also goes to the JSONL [`Session`] when one is attached.

mod events;
mod export;
mod input;
mod mcp;
mod pending;
mod pickers;
mod render;
mod runtime;
mod select;
mod session_ops;
mod slash;
mod submit;
mod toolset;
mod transcript;

use std::any::Any;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use crossterm::event::MouseEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use termide_agent_core::{
    civil_date, Backend, BackendModel, BackendSetup, CancelToken, CheckpointStore, CommandScript,
    CompactionPolicy, CompactionPrompts, Decision, GoalPrompt, HandoffPrompt, Hooks, LateTools,
    McpReload, McpServerState, Mode, ModeHandle, ModelInfo, ModelSpec, PermissionEnvelope,
    PermissionRules, PersistScope, PlanPrompt, PromptError, PromptTemplate, Provider,
    QuestionEnvelope, Refusals, ReviewerSetup, Session, SessionSummary, ShellOutput, ShellRunner,
    SkillInfo, SuggestionEnvelope, Tool, ToolRegistry, DEFAULT_AGENT,
};
use termide_config::Config;
use termide_core::{
    CommandResult, InputAction, KeyChord, Panel, PanelCommand, PanelEvent, RenderContext,
    ScrollAxis, ScrollBars, SelectAction, StatusSegment, ThemeColors, TitleCut, WidthPreference,
};
use termide_theme::Theme;
use termide_ui::{ClickTracker, CompletionList, InputBar};

use crate::input::MentionSpan;
use crate::pending::Pending;
use crate::pickers::current_mark;
use crate::runtime::{
    checkpoint_store, session_agent, session_model, spawn_model_list, spawn_runtime, start_session,
    Spawned,
};
use crate::session_ops::discard_if_empty;
use crate::toolset::{Blocked, TOOLSET_ACTION};

pub use transcript::{FoldMode, Item, NoticeKind, Transcript};

/// A duration in whole milliseconds, saturated to fit a `u32`.
fn millis(duration: Duration) -> u32 {
    duration.as_millis().min(u128::from(u32::MAX)) as u32
}

/// Context-menu action that renames the session.
const RENAME_ACTION: &str = "agent_rename";
/// Context-menu action that deletes the session (behind a confirmation).
const DELETE_SESSION_ACTION: &str = "agent_delete_session";
/// Confirmation action that forks the session into a new panel (also `F5`).
const FORK_SESSION_ACTION: &str = "agent_fork_session";
/// Confirmation action that deletes the recent session picked in the banner.
const DELETE_RECENT_ACTION: &str = "agent_delete_recent_session";
/// Selection action for the F4 checkpoint-rollback picker.
const ROLLBACK_ACTION: &str = "agent_rollback";
/// Context-menu action that starts a fresh session.
const NEW_SESSION_ACTION: &str = "agent_new_session";
/// Context-menu action that opens the session picker.
const RESUME_ACTION: &str = "agent_resume";
/// What a click on a welcome-banner row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BannerHit {
    /// Runs the status action, as the matching status-bar chip does.
    Action(&'static str),
    /// Opens the recent session at this index of `recent_sessions`.
    Session(usize),
}
/// Status chip and context-menu action that opens the model picker.
const MODEL_ACTION: &str = "agent_model";
/// Status/banner action that switches the connection.
const CONNECTION_ACTION: &str = "agent_connection";
/// Input action carrying a model id typed by hand.
const MODEL_INPUT_ACTION: &str = "agent_model_input";
/// Status chip and context-menu action that opens the permission-mode picker.
const MODE_ACTION: &str = "agent_mode";
/// Status chip and picker that set how much the model is asked to reason.
const REASONING_ACTION: &str = "agent_reasoning";
/// Context-menu action that opens the assembled system prompt in a viewer.
const SHOW_PROMPT_ACTION: &str = "agent_show_prompt";
/// Context-menu action that opens the session-info modal (also F3, `/usage`).
const SESSION_INFO_ACTION: &str = "agent_session_info";
/// Status chip and context-menu action that opens the agent picker.
const AGENT_ACTION: &str = "agent_agent";
/// Context-menu action that opens the prompt-template picker.
const PROMPTS_ACTION: &str = "agent_prompts";
/// The built-in `/compact [focus]` command.
const COMPACT_COMMAND: &str = "compact";
/// The built-in `/undo` command.
const UNDO_COMMAND: &str = "undo";
/// The built-in `/new` command: start a fresh session, keeping the current one
/// in the list.
const NEW_COMMAND: &str = "new";
/// The built-in `/fork` command: copy the session log and carry on in a new
/// panel, leaving this one at its work.
const FORK_COMMAND: &str = "fork";
/// The built-in `/clear` command: discard the current session and start a fresh
/// one in its place.
const CLEAR_COMMAND: &str = "clear";
/// The built-in `/rename` and `/name` commands: rename the session, either from
/// an argument or through the same prompt as the menu.
const RENAME_COMMAND: &str = "rename";
const NAME_COMMAND: &str = "name";
/// The built-in `/pause` and `/continue` commands: stop the run gracefully
/// after the current step, and resume it.
const PAUSE_COMMAND: &str = "pause";
const CONTINUE_COMMAND: &str = "continue";
/// The built-in `/loop` command: re-run a prompt on an interval or back-to-back.
const LOOP_COMMAND: &str = "loop";
/// A `/loop` stops itself after this many iterations, so it cannot run away.
const LOOP_MAX_ITERATIONS: usize = 100;
/// The built-in `/goal` command: work autonomously toward a goal, a judge
/// deciding after each turn whether it is reached.
const GOAL_COMMAND: &str = "goal";
/// A `/goal` stops itself after this many work turns, so it cannot run away.
const GOAL_MAX_ITERATIONS: usize = 50;
/// The built-in `/handoff` command: distil the unfinished work into a brief for
/// a fresh session or another agent.
const HANDOFF_COMMAND: &str = "handoff";
/// The built-in `/usage` command: open the session-info modal (same as F3).
const USAGE_COMMAND: &str = "usage";
/// The built-in `/prompt` command: open the assembled system prompt in a viewer.
const PROMPT_COMMAND: &str = "prompt";
/// The built-in `/mcp` command: the MCP servers' status, a reload of their
/// configuration, a sign-in or a sign-out.
const MCP_COMMAND: &str = "mcp";
/// Every built-in `/name`, whatever the state; a template, script or skill
/// of the same name never runs under it (see `slash`).
const BUILTIN_COMMANDS: [&str; 15] = [
    UNDO_COMMAND,
    COMPACT_COMMAND,
    NEW_COMMAND,
    FORK_COMMAND,
    CLEAR_COMMAND,
    RENAME_COMMAND,
    NAME_COMMAND,
    PAUSE_COMMAND,
    CONTINUE_COMMAND,
    LOOP_COMMAND,
    GOAL_COMMAND,
    HANDOFF_COMMAND,
    USAGE_COMMAND,
    PROMPT_COMMAND,
    MCP_COMMAND,
];
/// Context-menu action (also `Ctrl+S`) that saves the chat as Markdown.
const SAVE_CHAT_ACTION: &str = "agent_save_chat";
/// Context-menu action that undoes the last request.
const UNDO_ACTION: &str = "agent_undo";

/// Everything the app resolves from configuration before opening the panel.
///
/// The panel keeps these so it can rebuild its agent when the user switches
/// to another session.
pub struct AgentPanelSetup {
    pub cwd: PathBuf,
    /// Name of the agent definition in use.
    pub agent: String,
    /// Resolves agent definitions when the user switches agents.
    pub catalog: Arc<dyn AgentCatalog>,
    /// Tools that arrive after the start (MCP servers connecting).
    pub late_tools: Option<Receiver<LateTools>>,
    /// Builds the hooks that run before the permission rules (command hooks);
    /// a factory, since every session switch spawns a fresh agent.
    pub hooks: Option<HooksFactory>,
    /// An external agent to drive instead of the built-in loop.
    pub backend: Option<BackendFactory>,
    /// The CLI agent (Claude Code, Codex, Gemini CLI) the connection drives
    /// over ACP, if it names one; it wins over an agent definition's own backend.
    pub provider_backend: Option<BackendFactory>,
    /// The connections a session can switch to; `None` offers none.
    pub connections: Option<Arc<dyn ConnectionCatalog>>,
    /// The connection in use.
    pub connection: String,
    pub provider: Arc<dyn Provider>,
    /// The provider's wire-protocol type (e.g. `openai_compatible`), recorded
    /// in the session log so a resume can rebuild the right provider.
    pub provider_kind: String,
    pub model: ModelSpec,
    pub tools: ToolRegistry,
    pub rules: PermissionRules,
    pub system_prompt: String,
    pub compaction: CompactionPolicy,
    /// The texts of a compaction, from the agent directory's `system/` files.
    pub compaction_prompts: CompactionPrompts,
    /// Plan mode's instructions and the request that carries a plan out.
    pub plan_prompt: PlanPrompt,
    /// The goal-judge texts, from the agent directory's `system/goal.md`.
    pub goal_prompt: GoalPrompt,
    /// The handoff-brief texts, from the agent directory's `system/handoff.md`.
    pub handoff_prompt: HandoffPrompt,
    /// The `auto` mode reviewer: its texts from `system/classify.md`, and a
    /// model of its own when one is configured.
    pub reviewer: ReviewerSetup,
    /// What the model reads when a call is refused, from
    /// `system/permissions.md`.
    pub refusals: Refusals,
    /// Where "allow always" rules go; a plain function so it survives a
    /// session switch. `None` keeps such rules in memory only.
    pub persist_rule: Option<PersistFn>,
    /// Directory holding this project's session logs; `None` runs without
    /// persistence and without the session picker.
    pub session_dir: Option<PathBuf>,
    /// Session to start in; `None` creates one in `session_dir`.
    pub session: Option<Session>,
    /// When reasoning and tool calls fold to their headline (the answer
    /// always shows).
    pub fold: FoldMode,
    /// How a command the user ran by hand (`$` in the input) or confirmed on
    /// a card reaches the shell; `None` when there is no way to run one, and
    /// then both are refused with a notice. Built by the app over the same
    /// `bash` tool the agent uses.
    pub shell_run: Option<ShellRunner>,
}

/// Records an "allow always" rule outside the panel, in the project's or the
/// global configuration.
pub type PersistFn = fn(&str, &str, Decision, PersistScope);

/// Makes the extra hooks of one agent (command hooks from `hooks.toml`).
pub type HooksFactory = Arc<dyn Fn() -> Box<dyn Hooks> + Send + Sync>;

/// Starts an external agent (ACP) in place of the built-in loop.
pub type BackendFactory =
    Arc<dyn Fn(BackendSetup) -> Result<Box<dyn Backend>, String> + Send + Sync>;

/// One connection the picker offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionEntry {
    pub name: String,
    /// Its wire protocol or CLI agent, as `[ai] provider` spells it.
    pub kind: String,
    pub model: String,
}

/// A connection made ready for an agent to run on.
pub struct ConnectionChoice {
    pub name: String,
    pub kind: String,
    pub provider: Arc<dyn Provider>,
    /// The connection's model and context window; the rest of the spec (output
    /// bound, reasoning) is the panel's own.
    pub model: String,
    pub context_window: u64,
    /// The CLI agent it drives over ACP instead of the built-in loop.
    pub backend: Option<BackendFactory>,
}

/// The app's connections (`[ai.connections.<name>]`); the
/// panel only chooses among them.
pub trait ConnectionCatalog: Send + Sync {
    fn list(&self) -> Vec<ConnectionEntry>;
    /// Connection `name` built for `agent`; `None` when it does not exist.
    fn build(&self, name: &str, agent: &str) -> Option<ConnectionChoice>;
    /// The panel switched to `choice`: what it hands off (a delegated task)
    /// follows. The default hands nothing off.
    fn activate(&self, _choice: &ConnectionChoice) {}
}

/// One agent the picker offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEntry {
    pub name: String,
    pub description: String,
}

/// What an agent definition changes about the panel's agent. `None` keeps
/// the current model or mode; the prompt and the tools always come from the
/// definition.
pub struct AgentProfile {
    pub system_prompt: String,
    pub tools: ToolRegistry,
    pub model: Option<String>,
    pub mode: Option<Mode>,
    /// Tools still connecting (MCP servers); they join `tools` as they come.
    pub late_tools: Option<Receiver<LateTools>>,
    /// An external agent to drive instead of the built-in loop.
    pub backend: Option<BackendFactory>,
    /// Every tool the agent has before a session switches any off, by name,
    /// in registry order.
    pub offered: Vec<String>,
    /// Every skill the agent has, by name.
    pub skills: Vec<String>,
}

/// The app's view of the agent definitions (`agents/<name>/` across the
/// agent directories); the panel only chooses among them.
pub trait AgentCatalog: Send + Sync {
    fn list(&self) -> Vec<AgentEntry>;
    fn resolve(&self, name: &str) -> Option<AgentProfile>;
    /// `name`'s profile with what a session switched off (`off`: tool names,
    /// `skill:<name>`) left out of its registry and its prompt. The default
    /// can only drop tools from the registry; a catalog that builds the
    /// prompt rebuilds it without them.
    fn resolve_without(&self, name: &str, off: &BTreeSet<String>) -> Option<AgentProfile> {
        let mut profile = self.resolve(name)?;
        let offered: Vec<String> = profile
            .tools
            .iter()
            .map(|tool| tool.name().to_string())
            .collect();
        for tool in off {
            profile.tools.remove(tool);
        }
        if profile.offered.is_empty() {
            profile.offered = offered;
        }
        Some(profile)
    }
    /// Prompt templates (`prompts/<name>.md`), for `/<name>` in the input.
    fn prompts(&self) -> Vec<PromptTemplate> {
        Vec::new()
    }
    /// Command scripts (`commands/<name>`), for `/<name>` in the input.
    fn commands(&self) -> Vec<CommandScript> {
        Vec::new()
    }
    /// Skills, for `/<name>` and `/skill:<name>` in the input.
    fn skills(&self) -> Vec<SkillInfo> {
        Vec::new()
    }
    /// The session's permission mode is now `mode`: what the catalog runs
    /// on the session's behalf (a delegated task) follows. The default runs
    /// nothing.
    fn set_mode(&self, _mode: Mode) {}
    /// Where each configured MCP server stands, for `/mcp` and the toolset
    /// list.
    fn mcp_status(&self) -> Vec<McpServerState> {
        Vec::new()
    }
    /// Connect the MCP server `server` again from the configuration as it
    /// is now, or let it go when it is no longer there.
    ///
    /// # Errors
    ///
    /// When there is no such server.
    fn mcp_reconnect(&self, server: &str) -> Result<McpReload, String> {
        Err(format!("no MCP server named {server}"))
    }
    /// Read the MCP configuration again. What changes reaches the panel as
    /// late tools; `None` when the catalog connects no servers.
    fn mcp_reload(&self) -> Option<McpReload> {
        None
    }
    /// Start a sign-in to the MCP server `server` in the browser; its outcome
    /// arrives as late tools.
    ///
    /// # Errors
    ///
    /// Why the sign-in cannot start.
    fn mcp_login(&self, server: &str) -> Result<(), String> {
        Err(format!("no MCP server named {server}"))
    }
    /// Forget the sign-in kept for `server` and reconnect it without one;
    /// `false` when none was kept.
    ///
    /// # Errors
    ///
    /// Why it cannot be forgotten.
    fn mcp_logout(&self, server: &str) -> Result<bool, String> {
        Err(format!("no MCP server named {server}"))
    }
}

/// What the agent is doing right now, for the live activity indicators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Waiting for the model's first token.
    Prefill,
    /// Streaming the model's answer.
    Generating,
    /// A tool is running.
    Tool,
    /// The conversation is being compacted.
    Compact,
}

/// A run control on the prompt box's top border.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunButton {
    /// `[‖]`: pause at the next step, like `/pause`.
    Pause,
    /// `[▶]`: resume a paused run, or withdraw a pause not reached yet, like
    /// `/continue`.
    Continue,
    /// `[■]`: stop the run, like `Esc`.
    Stop,
}

/// Live state of the current run: the phase, when it started, and enough to
/// estimate the generation speed until the authoritative `Usage` arrives.
#[derive(Debug, Clone, Copy)]
struct Activity {
    phase: Phase,
    /// When the current phase started (for its ticking elapsed time).
    since: Instant,
    /// Characters streamed in the current generation, for a rough live token
    /// count and speed (reconciled to `Usage` at `MessageEnd`).
    gen_chars: usize,
    /// When the current model message began (`MessageStart`), for the block's
    /// prefill/generation split in its cost footer.
    msg_start: Instant,
    /// When the first token of the current message arrived.
    first_token: Option<Instant>,
    /// The loop's estimate of the prompt the model is reading, for the live
    /// prefill line; `None` when the backend cannot tell.
    prompt_tokens: Option<u64>,
    /// How far the server has read the prompt, `(processed, total, cached)`
    /// in tokens, from a server that reports it.
    prefill: Option<(u64, u64, u64)>,
}

impl Activity {
    fn new(phase: Phase) -> Self {
        let now = Instant::now();
        Self {
            phase,
            since: now,
            gen_chars: 0,
            msg_start: now,
            first_token: None,
            prompt_tokens: None,
            prefill: None,
        }
    }

    fn enter(&mut self, phase: Phase) {
        self.phase = phase;
        self.since = Instant::now();
        self.gen_chars = 0;
    }

    /// Rough live token count from streamed characters (~4 chars per token).
    fn est_tokens(&self) -> u64 {
        (self.gen_chars / 4) as u64
    }

    /// The finished turn's cost from the phase timings and token `usage`:
    /// prefill (start→first token) and generation (first token→now).
    fn cost(&self, input: u64, output: u64) -> transcript::Cost {
        let ms = |d: Duration| d.as_millis() as u32;
        let prefill_ms = self.first_token.map_or(0, |ft| ms(ft - self.msg_start));
        let gen_ms = self
            .first_token
            .map_or(0, |ft| ms(ft.elapsed()))
            .min(ms(self.msg_start.elapsed()));
        transcript::Cost {
            prefill_ms,
            gen_ms,
            input,
            output,
        }
    }
}

/// A large paste kept out of the input: `placeholder` stands in the prompt
/// box, `text` is the full content spliced back in on submit.
struct Paste {
    placeholder: String,
    text: String,
}

/// A running `/loop`: re-submit `prompt` after each run finishes — on
/// `interval`, or back-to-back when `None` — until stopped or the cap is hit.
struct LoopTask {
    prompt: String,
    interval: Option<Duration>,
    /// When the next iteration is due; `None` while a run is in flight.
    next_at: Option<Instant>,
    iterations: usize,
}

/// A running `/goal`: work autonomously toward `goal`. After each work turn a
/// judge decides whether the goal is reached; if not, the panel sends the next
/// continuation turn. Stops when the judge says done, on an error, or at the
/// iteration cap.
struct GoalTask {
    goal: String,
    /// Work turns sent so far.
    iterations: usize,
    /// When the judge call is due (a work turn has finished); `None` while a
    /// work turn or the judge call is in flight.
    judge_at: Option<Instant>,
    /// A judge call is in flight; its verdict arrives as a `GoalJudged` event.
    judging: bool,
}

pub struct AgentPanel {
    runtime: Box<dyn Backend>,
    /// The runtime is an external agent: model and mode are not ours to set.
    external: bool,
    permission_rx: Receiver<PermissionEnvelope>,
    /// The model's questions to the user, from the `question` tool.
    question_rx: Receiver<QuestionEnvelope>,
    /// Commands the `suggest_command` tool offers, awaiting a card.
    suggestion_rx: Receiver<SuggestionEnvelope>,
    /// How a hand-run command and a confirmed card reach the shell; `None`
    /// refuses both.
    shell_run: Option<ShellRunner>,
    /// A command the user ran by hand, on a thread: the command with what it
    /// printed, or why it could not run, and how long it took in ms.
    shell_job: Option<Receiver<ShellJobDone>>,
    /// Stops the hand-run command in flight; `None` when none is running.
    shell_cancel: Option<CancelToken>,
    /// The input takes a shell command rather than a message: `$` typed into
    /// an empty input turns it on and is not kept in the text, the prompt
    /// marker becomes `$ `, and running the command, Esc, or Backspace in the
    /// empty input turns it off; in it the arrows walk the commands run
    /// before rather than the messages.
    shell_mode: bool,
    /// The question a card in the panel is asking, if any.
    pending: Option<Pending>,
    /// Command scripts the user let run for this session, by name.
    allowed_commands: HashSet<String>,
    /// A command script running on a thread, with the command as typed; its
    /// output becomes a request.
    command_run: Option<Receiver<(String, Result<String, String>)>>,
    /// What the files the agent changes looked like before each request,
    /// for `/undo`; shared with the hook that records them.
    checkpoints: Option<Arc<Mutex<CheckpointStore>>>,
    session: Option<Session>,
    /// The provider's wire-protocol type, recorded on model changes.
    provider_kind: String,
    session_dir: Option<PathBuf>,
    /// Sessions offered by the last picker, in the order they were shown.
    session_choices: Vec<SessionSummary>,
    /// This directory's other non-empty sessions, newest first, that the
    /// welcome banner of a fresh session offers to open. Read when the panel
    /// opens or switches session, not per frame; empty otherwise.
    recent_sessions: Vec<SessionSummary>,
    /// The recent session the keyboard cursor is on while the chat focus is
    /// in the banner's list, an index into `recent_sessions`.
    recent_selected: usize,
    /// First recent session the banner's list shows.
    recent_top: usize,
    /// Rows the banner's list had at the last render, for paging.
    recent_rows: usize,
    /// [`Session::open_generation`] when the list was read, to notice
    /// another panel opening or releasing a session.
    recent_generation: u64,
    /// The recent session a pending delete confirmation is about.
    recent_to_delete: Option<PathBuf>,
    cwd: PathBuf,
    agent: String,
    catalog: Arc<dyn AgentCatalog>,
    /// Agents offered by the last picker, in the order they were shown.
    agent_choices: Vec<String>,
    /// Prompt templates offered by the last picker, in the order shown.
    prompt_choices: Vec<PromptTemplate>,
    /// Tools still connecting, and those that arrived while a run was in
    /// flight and wait for the worker to be free.
    late_tools: Option<Receiver<LateTools>>,
    waiting_tools: Vec<Arc<dyn Tool>>,
    /// MCP tools whose server replaced or withdrew them, by name, still to
    /// leave the worker's registry once it is between runs.
    leaving_tools: Vec<String>,
    /// Under the banner, the transcript item that is each MCP server's line.
    mcp_lines: std::collections::HashMap<String, usize>,
    /// Servers asked to connect again, so their next set reads "reconnected"
    /// rather than "changed".
    mcp_reconnecting: BTreeSet<String>,
    /// Whether this panel is the one whose toolset checklist stands open.
    /// Only then does a tick ask the app to refresh it: with two agent panels
    /// open, the other one has nothing to say about this one's list. It is
    /// set when the panel raises the list and cleared when the list comes back
    /// — every way of closing it, `Enter`, `Esc` and a heading button alike,
    /// comes back as `PanelCommand::ChecklistDone`.
    toolset_list_open: bool,
    /// What the session switched off: tool names and `skill:<name>`.
    toolset_off: BTreeSet<String>,
    /// What the running profile (its prompt and registry) was built without.
    /// Switched off but not in it means still in the model's context, so
    /// refused rather than gone.
    context_off: BTreeSet<String>,
    /// `toolset_off` less `context_off`: what the guard refuses.
    blocked: Blocked,
    /// A compaction invalidated the prompt cache: the next moment between
    /// runs rebuilds the context without what is refused.
    context_stale: bool,
    /// Every tool and skill the agent offers, for the checklist.
    offered_tools: Vec<String>,
    offered_skills: Vec<String>,
    /// Every MCP tool that arrived, with its server, switched off or not.
    mcp_arrived: Vec<(String, Arc<dyn Tool>)>,
    model: ModelSpec,
    /// The model from the configuration: the base every session's model is
    /// built on, since the log records only an id and a context window.
    configured_model: ModelSpec,
    /// Live permission mode, shared with the hooks on the agent thread.
    mode: ModeHandle,
    /// Models offered by the last picker, in the order they were shown.
    model_choices: Vec<ModelInfo>,
    /// The external (ACP) agent's models, filled while its picker is open.
    acp_models: Vec<BackendModel>,
    /// Whether the external agent advertised any models (so a Model chip and
    /// picker are worth showing); latched once known.
    acp_has_models: bool,
    /// A model to pre-select on an external CLI agent (from `[ai].model` for a
    /// `claude_code`/`codex`/`gemini_cli` provider), applied once its models are known.
    /// Taken (set to `None`) after the one-shot attempt.
    pending_preferred_model: Option<String>,
    /// Background `list_models` call, polled from `tick()`.
    model_fetch: Option<Receiver<Result<Vec<ModelInfo>, String>>>,
    /// A silent `list_models` call started at construction to adopt the active
    /// model's real context window; polled and cleared in `tick()`. The
    /// configured window is only a fallback until this resolves (or when the
    /// provider reports none).
    context_probe: Option<Receiver<Result<Vec<ModelInfo>, String>>>,
    /// Events produced by a command handler, delivered on the next tick.
    pending_events: Vec<PanelEvent>,

    // Kept to rebuild the agent when switching sessions.
    hooks: Option<HooksFactory>,
    backend: Option<BackendFactory>,
    /// The connection's CLI agent, kept apart from `backend` so a
    /// rebuilt agent profile does not drop it.
    provider_backend: Option<BackendFactory>,
    connections: Option<Arc<dyn ConnectionCatalog>>,
    /// The connection in use.
    connection: String,
    /// The connections the picker last offered, in its order.
    connection_choices: Vec<ConnectionEntry>,
    provider: Arc<dyn Provider>,
    tools: ToolRegistry,
    rules: PermissionRules,
    /// "Allow for this session" grants, held for the panel's lifetime so a
    /// rebuild of the agent (undo, a model or agent switch) keeps them. They
    /// are never written to the configuration; "allow always" goes to `rules`.
    session_rules: PermissionRules,
    system_prompt: String,
    compaction: CompactionPolicy,
    compaction_prompts: CompactionPrompts,
    plan_prompt: PlanPrompt,
    /// The goal-judge texts, kept so `/goal` can start a judge call; passed to
    /// the agent so the judge prompt comes from the `system/` files.
    goal_prompt: GoalPrompt,
    /// The handoff-brief texts, passed to the agent for `/handoff`.
    handoff_prompt: HandoffPrompt,
    /// Builds the `auto` mode reviewer of each agent the panel spawns.
    reviewer: ReviewerSetup,
    /// The refusal texts every agent the panel spawns uses.
    refusals: Refusals,
    /// When blocks fold now; passed to each transcript. `Ctrl+O` switches
    /// it between `Never` and the configured mode, so fresh blocks follow
    /// what it last did to the finished ones.
    fold: FoldMode,
    /// When blocks fold as configured, restored by `Ctrl+O` folding all.
    fold_setting: FoldMode,
    /// The worker still has the prompt of the other plan-ness: a mode
    /// switch during a run could not update it, `AgentEnd` retries.
    prompt_stale: bool,
    /// The system prompt last shown as a `#` block, so a new one is surfaced
    /// (before the next message) only when it actually changed.
    shown_system: String,
    persist_rule: Option<PersistFn>,

    transcript: Transcript,
    /// The prompt box: one multi-line [`InputBar`] field, no border or
    /// controls — the panel draws its own separator above it.
    input: InputBar,
    /// Which earlier request the input shows while browsing history with
    /// the arrow keys; `None` while typing.
    history_pos: Option<usize>,
    /// What was being typed when browsing started, restored on the way back.
    draft: String,
    /// The `/command` completion list, while the input is a lone `/word`.
    completion: Option<CompletionList>,
    /// When the open completion is an `@`-file mention, the span it replaces;
    /// `None` for a `/`-command completion, which replaces the whole input.
    completion_span: Option<MentionSpan>,
    /// Consecutive clicks on a form row, so a single click selects and a
    /// double click confirms; keyed by the row index.
    form_clicks: ClickTracker<usize>,
    /// Where the left button went down in the transcript, until it is
    /// released: a release without a drag is a click on the block there.
    press: Option<select::Cell>,
    /// Text selected in the transcript with the mouse, copied by `Ctrl+C`.
    text_selection: Option<select::TextSelection>,
    /// Large pastes held out of the input as a short placeholder, expanded back
    /// inline on submit, so a big block does not swamp the prompt box.
    pastes: Vec<Paste>,
    /// Serial number for the next paste placeholder.
    paste_seq: usize,
    /// Keyboard focus is in the chat, not the input: `Tab` toggles it, then
    /// the arrows pick a block and Space/Enter fold it — or, while the welcome
    /// banner lists recent sessions, pick one and Enter opens it.
    chat_focus: bool,
    /// The block the chat focus is on, an index into the transcript items.
    selected: usize,
    /// First visible transcript line.
    top: usize,
    /// Keep the view pinned to the newest line while true.
    follow: bool,
    busy: bool,
    /// A run stopped early on `/pause` with work still pending, so `/continue`
    /// can resume it.
    paused: bool,
    /// An active `/loop`: re-runs its prompt after each turn until stopped.
    loop_task: Option<LoopTask>,
    /// A running `/goal`: autonomous work toward a goal with a judge; `None`
    /// when no goal is active.
    goal_task: Option<GoalTask>,
    /// The current goal work turn ended in an error, so the goal loop stops
    /// instead of judging and retrying. Reset at the start of each work turn.
    goal_errored: bool,
    /// When the current run started (`AgentStart`), for its closing line.
    run_start: Option<Instant>,
    /// The current run hit an error or was aborted, so its closing line is `✗`.
    run_failed: bool,
    /// A run ended or a question arrived since the panel was last rendered
    /// focused; its header is highlighted while it is unfocused.
    attention: bool,
    /// The panel asked for the bell since the user last saw it, so a second
    /// wait does not ring again.
    rung: bool,
    /// The terminal window has focus, as last reported while this panel was
    /// the active one.
    host_focused: bool,
    /// The current run stopped at a `/pause` (its closing line says so).
    run_paused: bool,
    /// A `/pause` was asked for and the run has not reached a step boundary
    /// yet; shown in the state strip.
    pause_requested: bool,
    /// A stop was asked for and the run has not ended yet; the stop control
    /// is red until it does.
    stop_requested: bool,
    /// When the current pause began, while the run is paused: its closing
    /// line ticks the pause's length until `/continue`.
    pause_start: Option<Instant>,
    /// A `/continue` resumed the paused run, so the next `AgentStart` keeps
    /// the run's start and its clock goes on from the request.
    resuming: bool,
    /// When the current permission question (or the model's question to the
    /// user) went up, and how long the
    /// running call had already waited before it: the wait is a pause of
    /// its own, shown on the call and kept out of its duration.
    permission_wait: Option<(Instant, u32)>,
    /// The screen row of the state strip's pause line, a click target that
    /// continues the run.
    pause_row: Option<u16>,
    /// The run controls last put on the prompt's border, in order, so a
    /// click maps back to one.
    run_buttons: Vec<RunButton>,
    /// Texts of the steering messages sent while the agent works, oldest
    /// first, shown in the state strip until the agent takes them. Kept in
    /// step with the runtime's steering count (`QueueUpdate`).
    queued_texts: VecDeque<String>,
    queued: (usize, usize),
    /// Tokens of the last reported context, for the status chip.
    context_tokens: u64,
    /// What the agent is doing right now; `None` when idle.
    activity: Option<Activity>,
    /// Session token totals from `Usage`: input (prefill) and output.
    session_input: u64,
    /// Prompt tokens the cache served this session.
    session_cached: u64,
    session_output: u64,
    /// Bytes of shell output before and after cleaning, summed over the
    /// session, for the "output cleaned" diagnostic in the summary.
    clean_raw_bytes: u64,
    clean_out_bytes: u64,
    /// When each running tool started, to report how long it took (`🕒`).
    tool_starts: HashMap<String, Instant>,
    /// Throttles the animation redraws requested while busy.
    last_anim: Instant,

    colors: ThemeColors,
    is_light: bool,
    transcript_area: Rect,
    input_area: Rect,
    scrollbars: ScrollBars,
    /// Clickable rows drawn in the welcome banner, each with what a click on
    /// it does (re-pick the model or the agent, open a recent session).
    /// Rebuilt every render; empty once the session has content and the
    /// banner is gone.
    banner_hits: Vec<(Rect, BannerHit)>,
}

impl AgentPanel {
    #[must_use]
    pub fn new(mut setup: AgentPanelSetup) -> Self {
        let session = setup.session.or_else(|| {
            start_session(
                setup.session_dir.as_deref(),
                &setup.cwd,
                &setup.connection,
                &setup.provider_kind,
                &setup.model,
                &setup.agent,
            )
        });
        let model = session_model(&setup.model, session.as_ref());
        let checkpoints = checkpoint_store(setup.session_dir.as_deref(), session.as_ref());
        let (mut agent, mut system_prompt, mut tools, mut late_tools, mut backend) = (
            setup.agent,
            setup.system_prompt,
            setup.tools,
            setup.late_tools,
            setup.backend,
        );
        let (toolset_off, resolved) = session_agent(
            setup.catalog.as_ref(),
            &agent,
            &BTreeSet::new(),
            session.as_ref(),
        );
        // What the running profile was built without: the session's set when
        // it was rebuilt for it, nothing when the set-up one runs.
        let context_off = if resolved.is_some() {
            toolset_off.clone()
        } else {
            BTreeSet::new()
        };
        let mut offered = None;
        if let Some((name, profile)) = resolved {
            agent = name;
            system_prompt = profile.system_prompt;
            tools = profile.tools;
            late_tools = profile.late_tools;
            backend = setup.provider_backend.clone().or(profile.backend);
            offered = Some((profile.offered, profile.skills));
            if let Some(mode) = profile.mode {
                setup.rules.mode = mode;
            }
        }
        // The full lists the checklist offers, the set-up profile's too.
        let (offered_tools, offered_skills) = offered.unwrap_or_else(|| {
            setup
                .catalog
                .resolve_without(&agent, &BTreeSet::new())
                .map(|profile| (profile.offered, profile.skills))
                .unwrap_or_default()
        });
        let blocked: Blocked = Arc::new(RwLock::new(
            toolset_off.difference(&context_off).cloned().collect(),
        ));
        let Spawned {
            runtime,
            permission_rx,
            question_rx,
            suggestion_rx,
            transcript,
            mode,
            external,
        } = spawn_runtime(
            &setup.provider,
            &tools,
            &model,
            &setup.cwd,
            &system_prompt,
            setup.rules.clone(),
            setup.compaction,
            &setup.compaction_prompts,
            &setup.plan_prompt,
            &setup.goal_prompt,
            &setup.handoff_prompt,
            &setup.reviewer,
            &setup.refusals,
            setup.persist_rule,
            setup.hooks.as_ref(),
            backend.as_ref(),
            checkpoints.clone(),
            setup.fold,
            session.as_ref(),
            &blocked,
            setup.shell_run.clone(),
        );
        // Learn the context window from the provider in the background and
        // adopt the active model's real `max_model_len`; the configured window
        // is only a fallback (an external agent has no such endpoint).
        let context_probe = (!external).then(|| spawn_model_list(Arc::clone(&setup.provider)));
        // A CLI provider carries the model to pre-select on its agent in
        // `[ai].model`; a wire-protocol or generic external agent does not.
        let pending_preferred_model = (external
            && termide_config::is_cli_provider(&setup.provider_kind)
            && !model.id.is_empty())
        .then(|| model.id.clone());
        setup.catalog.set_mode(mode.get());
        let mut panel = Self {
            runtime,
            external,
            permission_rx,
            question_rx,
            suggestion_rx,
            shell_run: setup.shell_run,
            shell_job: None,
            shell_cancel: None,
            shell_mode: false,
            pending: None,
            allowed_commands: HashSet::new(),
            command_run: None,
            checkpoints,
            session,
            session_dir: setup.session_dir,
            session_choices: Vec::new(),
            recent_sessions: Vec::new(),
            recent_selected: 0,
            recent_top: 0,
            recent_rows: 0,
            recent_generation: 0,
            recent_to_delete: None,
            cwd: setup.cwd,
            model,
            configured_model: setup.model,
            agent,
            catalog: setup.catalog,
            agent_choices: Vec::new(),
            prompt_choices: Vec::new(),
            late_tools,
            waiting_tools: Vec::new(),
            leaving_tools: Vec::new(),
            mcp_lines: std::collections::HashMap::new(),
            mcp_reconnecting: BTreeSet::new(),
            toolset_list_open: false,
            toolset_off,
            context_off,
            blocked,
            context_stale: false,
            offered_tools,
            offered_skills,
            mcp_arrived: Vec::new(),
            mode,
            model_choices: Vec::new(),
            acp_models: Vec::new(),
            acp_has_models: false,
            pending_preferred_model,
            model_fetch: None,
            context_probe,
            pending_events: Vec::new(),
            hooks: setup.hooks,
            backend,
            provider_backend: setup.provider_backend.clone(),
            connections: setup.connections.clone(),
            connection: setup.connection.clone(),
            connection_choices: Vec::new(),
            provider: setup.provider,
            provider_kind: setup.provider_kind,
            tools,
            rules: setup.rules,
            session_rules: PermissionRules::default(),
            system_prompt,
            compaction: setup.compaction,
            compaction_prompts: setup.compaction_prompts,
            plan_prompt: setup.plan_prompt,
            goal_prompt: setup.goal_prompt,
            handoff_prompt: setup.handoff_prompt,
            reviewer: setup.reviewer,
            refusals: setup.refusals,
            fold: setup.fold,
            fold_setting: setup.fold,
            prompt_stale: false,
            shown_system: String::new(),
            persist_rule: setup.persist_rule,
            transcript,
            input: InputBar::new(vec![])
                .with_multiline_field("")
                .with_placeholder(termide_i18n::t().agent_input_placeholder())
                .with_border(String::new(), String::new()),
            history_pos: None,
            draft: String::new(),
            completion: None,
            completion_span: None,
            form_clicks: ClickTracker::new(),
            press: None,
            text_selection: None,
            pastes: Vec::new(),
            paste_seq: 0,
            chat_focus: false,
            selected: 0,
            top: 0,
            follow: true,
            busy: false,
            paused: false,
            loop_task: None,
            goal_task: None,
            goal_errored: false,
            run_start: None,
            run_failed: false,
            attention: false,
            rung: false,
            host_focused: true,
            run_paused: false,
            pause_requested: false,
            stop_requested: false,
            pause_start: None,
            resuming: false,
            permission_wait: None,
            pause_row: None,
            run_buttons: Vec::new(),
            queued_texts: VecDeque::new(),
            queued: (0, 0),
            context_tokens: 0,
            activity: None,
            session_input: 0,
            session_cached: 0,
            session_output: 0,
            clean_raw_bytes: 0,
            clean_out_bytes: 0,
            tool_starts: HashMap::new(),
            last_anim: Instant::now(),
            colors: ThemeColors::default(),
            is_light: false,
            transcript_area: Rect::default(),
            input_area: Rect::default(),
            scrollbars: ScrollBars::default(),
            banner_hits: Vec::new(),
        };
        // Names defined twice are reported once, when the panel opens; under
        // a fresh session's banner the notices sit below it.
        panel.notice_slash_conflicts();
        panel.refresh_recent_sessions();
        panel
    }

    #[must_use]
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.busy || self.runtime.is_busy()
    }

    #[must_use]
    pub fn input_text(&self) -> String {
        self.input_area().text()
    }

    #[must_use]
    pub fn session_path(&self) -> Option<&std::path::Path> {
        self.session.as_ref().map(Session::path)
    }

    fn notice(&mut self, text: impl Into<String>, kind: NoticeKind) {
        self.transcript.push(Item::Notice {
            text: text.into(),
            kind,
        });
    }

    /// Whether the session has no conversation yet — no request sent, so the
    /// welcome banner is still showing. A model/agent switch then updates the
    /// banner's values in place instead of pushing a notice that would replace
    /// the banner with a near-empty transcript.
    /// Whether the welcome banner is up: nothing in the transcript but what
    /// the panel reported, which the banner shows below itself.
    fn banner_shown(&self) -> bool {
        self.transcript
            .items()
            .iter()
            .all(|item| matches!(item, Item::Notice { .. }))
    }

    fn is_fresh(&self) -> bool {
        !self
            .transcript
            .items()
            .iter()
            .any(|item| matches!(item, Item::User { .. } | Item::Assistant { .. }))
    }

    /// The rules the agent runs under: the configured and "always" rules, and
    /// apart from them this session's answers, which count in modes the
    /// configured rules do not. Rebuilt into every agent the panel spawns so
    /// an answer survives a rebuild.
    fn effective_rules(&self) -> PermissionRules {
        let mut rules = self.rules.clone();
        rules.session = self.session_rules.tools.clone();
        rules
    }
}

impl Drop for AgentPanel {
    /// Closing the panel discards its session when it was never used, so an
    /// empty session leaves nothing behind in the list or on disk.
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            discard_if_empty(session);
        }
    }
}

/// Longest text shown in a picker entry (the rollback steps) before it is cut.
const MAX_TITLE_CHARS: usize = 60;

/// A finished hand-run command: the command, what it printed or why it could
/// not run, and how long it took in ms.
pub(crate) type ShellJobDone = (String, Result<ShellOutput, String>, u32);

/// Local wall-clock time as `HH:MM:SS`, for a transcript block's byline.
pub(crate) fn now_hms() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// Upper-case the first character of `name`, leaving the rest as written
/// (so `reviewer` → `Reviewer`, `web-dev` → `Web-dev`).
pub(crate) fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `text` collapsed to one line, runs of whitespace becoming one space.
fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` collapsed to one line and cut with an ellipsis.
fn truncate_title(text: &str) -> String {
    let single_line = single_line(text);
    if single_line.chars().count() <= MAX_TITLE_CHARS {
        return single_line;
    }
    let cut: String = single_line.chars().take(MAX_TITLE_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// The provider's wire protocol as the status line names it.
fn provider_label(kind: &str) -> &str {
    match kind {
        "openai_compatible" => "OpenAI Compatible",
        "anthropic_compatible" => "Anthropic Compatible",
        "claude_code" => "Claude Code",
        "codex" => "Codex",
        "gemini_cli" => "Gemini CLI",
        other => other,
    }
}

/// A path for the welcome banner: the home directory shown as `~`, and, when
/// still wider than `max` columns, cut from the left so the tail (the part that
/// tells directories apart) stays visible.
fn shorten_path(path: &Path, max: usize) -> String {
    let full = path.display().to_string();
    let display = std::env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf))
        .map(|rest| {
            if rest.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", rest.display())
            }
        })
        .unwrap_or(full);
    let count = display.chars().count();
    if max <= 1 || count <= max {
        return display;
    }
    let tail: String = display.chars().skip(count - (max - 1)).collect();
    format!("…{tail}")
}

/// Token counts as the status line shows them: `32k`, `1.2M`.
pub(crate) fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        let millions = tokens as f64 / 1_000_000.0;
        if millions.fract() < 0.05 {
            format!("{millions:.0}M")
        } else {
            format!("{millions:.1}M")
        }
    } else if tokens >= 1000 {
        format!("{}k", (tokens + 500) / 1000)
    } else {
        tokens.to_string()
    }
}

impl Panel for AgentPanel {
    fn name(&self) -> &'static str {
        "agent"
    }

    /// `Agent: <name>` for a named conversation, else `Agent: <first
    /// prompt>`, else `Agent: <working directory>`. The `Agent` label is
    /// replaced by a custom agent's own name (capitalized), so parallel panels
    /// running different agents are told apart. Nothing is cut here: the
    /// header cuts the end to the panel's width, keeping the label.
    fn title(&self) -> String {
        let t = termide_i18n::t();
        let label = if self.agent == DEFAULT_AGENT {
            t.panel_agent().to_string()
        } else {
            capitalize(&self.agent)
        };
        let named = self
            .session
            .as_ref()
            .and_then(Session::name)
            .map(single_line);
        let subject = named
            .or_else(|| {
                self.transcript.items().iter().find_map(|item| match item {
                    Item::User { text, command, .. } => {
                        Some(single_line(command.as_ref().unwrap_or(text)))
                    }
                    _ => None,
                })
            })
            .unwrap_or_else(|| self.cwd.to_string_lossy().into_owned());
        format!("{label}: {subject}")
    }

    fn title_cut(&self) -> TitleCut {
        TitleCut::End
    }

    fn needs_attention(&self) -> bool {
        self.attention
    }

    fn context_menu_items(&self) -> Vec<(String, &'static str)> {
        let t = termide_i18n::t();
        // Only actions with no home elsewhere. New/switch/delete sessions also
        // have F-keys, sessions/prompts/agents live in the AI menu, and the
        // model/agent/mode pickers are status-bar chips — none is repeated here.
        // Session info also answers F3 and `/usage`; the assembled prompt is
        // reached with `/prompt`, so neither needs another menu slot beyond this.
        vec![
            (t.agent_session_info().to_string(), SESSION_INFO_ACTION),
            (t.agent_rename().to_string(), RENAME_ACTION),
            (t.agent_save_chat().to_string(), SAVE_CHAT_ACTION),
            (t.agent_fork_session().to_string(), FORK_SESSION_ACTION),
            (t.agent_delete_session().to_string(), DELETE_SESSION_ACTION),
        ]
    }

    fn handle_status_action(&mut self, action: &str) -> Vec<PanelEvent> {
        let t = termide_i18n::t();
        match action {
            RENAME_ACTION => vec![PanelEvent::ShowInput {
                prompt: t.agent_rename_prompt().to_string(),
                initial_value: self
                    .session
                    .as_ref()
                    .and_then(Session::name)
                    .unwrap_or_default()
                    .to_string(),
                on_submit: InputAction::Custom(RENAME_ACTION.to_string()),
            }],
            DELETE_SESSION_ACTION => self.ask_delete_session(),
            FORK_SESSION_ACTION => self.ask_fork_session(),
            SAVE_CHAT_ACTION => self.save_chat(),
            CONNECTION_ACTION => {
                let Some(connections) = &self.connections else {
                    return Vec::new();
                };
                self.connection_choices = connections.list();
                let options = self
                    .connection_choices
                    .iter()
                    .map(|entry| {
                        let mark = current_mark(entry.name == self.connection);
                        let model = if entry.model.is_empty() {
                            String::new()
                        } else {
                            format!(" · {}", entry.model)
                        };
                        format!(
                            "{mark}{} — {}{model}",
                            entry.name,
                            provider_label(&entry.kind)
                        )
                    })
                    .collect();
                vec![PanelEvent::ShowSelect {
                    title: t.agent_pick_connection().to_string(),
                    options,
                    on_select: SelectAction::Custom(CONNECTION_ACTION.to_string()),
                }]
            }
            // An external agent brings its own tools; nothing of ours to list.
            TOOLSET_ACTION if self.external => Vec::new(),
            TOOLSET_ACTION => {
                let groups = self.toolset_groups();
                let prompt = self.toolset_prompt(&groups);
                self.toolset_list_open = true;
                vec![PanelEvent::ShowChecklist {
                    title: t.agent_toolset_title().to_string(),
                    prompt,
                    items: self.toolset_items(),
                    groups,
                    action: TOOLSET_ACTION.to_string(),
                }]
            }
            NEW_SESSION_ACTION => {
                self.switch_session(None);
                vec![PanelEvent::NeedsRedraw]
            }
            RESUME_ACTION => {
                self.session_choices = self.session_list();
                if self.session_choices.is_empty() {
                    return vec![PanelEvent::SetStatusMessage {
                        message: t.agent_no_sessions().to_string(),
                        is_error: false,
                    }];
                }
                let current = self.session.as_ref().map(Session::path);
                let options = self
                    .session_choices
                    .iter()
                    .map(|summary| {
                        let mark = if current == Some(summary.path.as_path()) {
                            "● "
                        } else {
                            "  "
                        };
                        format!(
                            "{mark}{} · {}",
                            civil_date(summary.modified),
                            truncate_title(&summary.label())
                        )
                    })
                    .collect();
                vec![PanelEvent::ShowSelect {
                    title: t.agent_resume().to_string(),
                    options,
                    on_select: SelectAction::Custom(RESUME_ACTION.to_string()),
                }]
            }
            PROMPTS_ACTION => self.prompt_picker(),
            UNDO_ACTION => self.ask_undo(),
            AGENT_ACTION => vec![self.agent_picker()],
            MODEL_ACTION if self.external => self.acp_model_picker(),
            MODE_ACTION if self.external && !self.runtime.follows_mode() => {
                self.notice(PromptError::Unsupported.to_string(), NoticeKind::Warn);
                vec![PanelEvent::NeedsRedraw]
            }
            MODEL_ACTION => self.request_model_list(),
            MODE_ACTION => vec![self.mode_picker()],
            REASONING_ACTION => self.reasoning_action(),
            SHOW_PROMPT_ACTION => match self.write_system_prompt() {
                Ok(path) => vec![PanelEvent::ViewFile(path)],
                Err(error) => {
                    self.notice(
                        termide_i18n::t().agent_notice_cannot_write_prompt_fmt(&error.to_string()),
                        NoticeKind::Error,
                    );
                    vec![PanelEvent::NeedsRedraw]
                }
            },
            SESSION_INFO_ACTION => self.session_summary(),
            _ => vec![],
        }
    }

    fn icon(&self) -> Option<&'static str> {
        Some("🤖")
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferWide
    }

    fn prepare_render(&mut self, theme: &Theme, _config: &Arc<Config>) {
        self.colors = ThemeColors::from(theme);
        self.is_light = theme.is_light_theme();
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        self.render_panel(area, buf, ctx)
    }

    fn handle_key(&mut self, chord: KeyChord) -> Vec<PanelEvent> {
        self.on_key(chord)
    }

    fn captures_escape(&self) -> bool {
        // A hand-run command in flight counts: Esc stops it, and the key
        // must not reach the app to do something else while it runs.
        self.pending.is_some()
            || self.completion.is_some()
            || self.is_busy()
            || self.shell_job.is_some()
            || self.shell_mode
            || self.loop_task.is_some()
            || self.goal_task.is_some()
            || !self.input_area().is_empty()
    }

    fn handle_scroll(&mut self, delta: i32, _panel_area: Rect) -> Vec<PanelEvent> {
        if self.recent_list_shown() {
            self.scroll_recent(delta);
        } else {
            self.scroll_by(delta);
        }
        vec![PanelEvent::NeedsRedraw]
    }

    fn handle_mouse(&mut self, event: MouseEvent, _panel_area: Rect) -> Vec<PanelEvent> {
        self.on_mouse(event)
    }

    fn tick(&mut self) -> Vec<PanelEvent> {
        self.on_tick()
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            PanelCommand::Paste if !self.chat_focus => match self.paste_clipboard() {
                true => {
                    self.after_edit();
                    CommandResult::Handled(true)
                }
                false => CommandResult::Handled(false),
            },
            PanelCommand::PasteText { text } => {
                self.paste(&text);
                self.after_edit();
                CommandResult::NeedsRedraw(true)
            }
            // Copy takes the chat block while the chat holds focus, and the
            // prompt's selection while the input does.
            PanelCommand::Copy => {
                if self.copy_text_selection() {
                    CommandResult::Handled(true)
                } else if self.chat_focus {
                    match self.selected_block_text() {
                        Some(text) if !text.trim().is_empty() => {
                            self.copy_text(&text);
                            CommandResult::Handled(true)
                        }
                        _ => CommandResult::Handled(false),
                    }
                } else {
                    CommandResult::Handled(self.copy_input_selection())
                }
            }
            PanelCommand::Cut if !self.chat_focus => {
                CommandResult::Handled(self.cut_input_selection())
            }
            PanelCommand::ChecklistDone {
                action,
                checked,
                pressed,
            } if action == TOOLSET_ACTION => {
                // The ticks first, as `Enter` would have applied them; then
                // what the button asks for.
                self.toolset_list_open = false;
                self.apply_toolset(&checked);
                if let Some(id) = pressed {
                    self.press_toolset_button(&id);
                }
                self.pending_events.push(PanelEvent::NeedsRedraw);
                CommandResult::Handled(true)
            }
            PanelCommand::SelectionMade { action, index } if action == RESUME_ACTION => {
                CommandResult::Handled(self.resume_choice(index))
            }
            PanelCommand::SelectionMade { action, index } if action == ROLLBACK_ACTION => {
                let events = self.perform_rollback(index);
                self.pending_events.extend(events);
                CommandResult::Handled(true)
            }
            PanelCommand::SelectionMade { action, index } if action == PROMPTS_ACTION => {
                let choice = self.prompt_choices.get(index).cloned();
                self.prompt_choices.clear();
                if let Some(template) = choice {
                    self.set_input(&format!("/{} ", template.name));
                    self.after_edit();
                }
                CommandResult::Handled(true)
            }
            PanelCommand::SelectionMade { action, index } if action == AGENT_ACTION => {
                let choice = self.agent_choices.get(index).cloned();
                self.agent_choices.clear();
                CommandResult::Handled(choice.is_some_and(|name| self.switch_agent(&name)))
            }
            PanelCommand::SelectionMade { action, index } if action == REASONING_ACTION => {
                let level = self.thinking_levels().get(index).copied();
                CommandResult::Handled(level.is_some_and(|level| self.set_thinking(level)))
            }
            PanelCommand::SelectionMade { action, index } if action == MODE_ACTION => {
                if let Some(mode) = Mode::ALL.get(index).copied() {
                    let event = self.set_mode(mode);
                    self.pending_events.push(event);
                }
                CommandResult::Handled(true)
            }
            PanelCommand::SelectionMade { action, index }
                if action == MODEL_ACTION && self.external =>
            {
                let choice = self.acp_models.get(index).cloned();
                self.acp_models.clear();
                if let Some(model) = choice {
                    match self.runtime.select_model(model.id.clone()) {
                        Ok(()) => {
                            self.model.id = model.id.clone();
                            if !self.is_fresh() {
                                self.notice(
                                    termide_i18n::t().agent_notice_model_fmt(&model.id),
                                    NoticeKind::Info,
                                );
                            }
                        }
                        Err(error) => self.notice(
                            termide_i18n::t()
                                .agent_notice_cannot_switch_model_fmt(&error.to_string()),
                            NoticeKind::Warn,
                        ),
                    }
                }
                CommandResult::Handled(true)
            }
            PanelCommand::SelectionMade { action, index } if action == CONNECTION_ACTION => {
                if let Some(entry) = self.connection_choices.get(index).cloned() {
                    self.switch_connection(&entry.name);
                }
                self.connection_choices.clear();
                self.pending_events.push(PanelEvent::NeedsRedraw);
                CommandResult::Handled(true)
            }
            PanelCommand::SelectionMade { action, index } if action == MODEL_ACTION => {
                let choice = self.model_choices.get(index).cloned();
                self.model_choices.clear();
                match choice {
                    Some(model) => {
                        self.switch_model(&model.id, model.context_window);
                    }
                    // The entry after the list: type an id instead.
                    None => {
                        let event = self.model_input();
                        self.pending_events.push(event);
                    }
                }
                CommandResult::Handled(true)
            }
            PanelCommand::InputSubmitted { action, text } if action == MODEL_INPUT_ACTION => {
                CommandResult::Handled(self.switch_model(&text, None))
            }
            PanelCommand::InputSubmitted { action, text } if action == RENAME_ACTION => {
                CommandResult::Handled(self.rename_session(&text))
            }
            PanelCommand::Confirmed { action } if action == DELETE_SESSION_ACTION => {
                self.perform_delete_session();
                CommandResult::Handled(true)
            }
            PanelCommand::Confirmed { action } if action == FORK_SESSION_ACTION => {
                let events = self.perform_fork_session();
                self.pending_events.extend(events);
                CommandResult::Handled(true)
            }
            PanelCommand::Confirmed { action } if action == DELETE_RECENT_ACTION => {
                let events = self.perform_delete_recent_session();
                self.pending_events.extend(events);
                CommandResult::Handled(true)
            }
            PanelCommand::SetHostFocus { focused } => {
                self.host_focused = focused;
                CommandResult::None
            }
            PanelCommand::GetScrollBars => CommandResult::ScrollBars(self.scrollbars),
            PanelCommand::SetScrollOffset { axis, offset } => {
                if axis == ScrollAxis::Vertical {
                    self.top = offset.min(self.max_top());
                    self.follow = self.top >= self.max_top();
                }
                CommandResult::NeedsRedraw(true)
            }
            _ => CommandResult::None,
        }
    }

    fn status_segments(&self) -> Vec<StatusSegment> {
        self.segments()
    }

    fn has_running_processes(&self) -> bool {
        self.is_busy()
    }

    /// The working directory and the session log, which is all a restore
    /// needs: the model is in the log and the rest comes from the config.
    fn to_state(&self, _session_dir: &std::path::Path) -> Option<termide_core::PanelState> {
        Some(termide_core::PanelState::Agent {
            cwd: self.cwd.clone(),
            session: self.session_path().map(std::path::Path::to_path_buf),
            agent: (self.agent != DEFAULT_AGENT).then(|| self.agent.clone()),
        })
    }

    fn kill_processes(&mut self) {
        self.runtime.abort();
    }

    fn get_working_directory(&self) -> Option<PathBuf> {
        Some(self.cwd.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests;
