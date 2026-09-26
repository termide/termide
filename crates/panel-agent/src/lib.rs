//! The coding agent panel: a transcript above a multi-line input, tool calls
//! collapsed to one line each, permission prompts routed through termide's
//! selection modal.
//!
//! The panel owns an [`AgentRuntime`] and mirrors its events into a
//! [`Transcript`] from `tick()`, so it never blocks the UI thread. Every
//! transcript change also goes to the JSONL [`Session`] when one is attached.

mod select;
mod transcript;

use std::any::Any;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use termide_agent_core::{
    civil_date, now_millis, permission_channel, Agent, AgentEvent, Backend, BackendModel,
    BackendSetup, CancelToken, ChainedHooks, CheckpointHooks, CheckpointStore, CommandScript,
    CompactionPolicy, CompactionPrompts, Decision, EntryKind, GoalPrompt, HandoffPrompt, Hooks,
    LateTools, LoggedMessage, Message, Mode, ModeHandle, ModelInfo, ModelSpec, PermissionAnswer,
    PermissionEnvelope, PermissionHooks, PermissionRules, PersistRule, PersistScope, PlanGuard,
    PlanPrompt, PromptTemplate, Provider, Session, SessionSummary, StopReason, StreamEvent, Timing,
    Tool, ToolCall, ToolContext, ToolDecision, ToolRegistry, ToolResultMessage, ToolUpdate,
    UserMessage, DEFAULT_AGENT,
};
use termide_agent_core::{AgentRuntime, PromptError};
use termide_config::Config;
use termide_core::{
    ChecklistItem, CommandResult, ConfirmAction, InputAction, KeyChord, Panel, PanelCommand,
    PanelEvent, RenderContext, ScrollAxis, ScrollBars, SegmentKind, SelectAction, StatusSegment,
    ThemeColors, WidthPreference,
};
use termide_theme::Theme;
use termide_ui::textarea::TextArea;
use termide_ui::{
    ChoiceAction, ChoiceForm, ClickTracker, CompletionAction, CompletionItem, CompletionList,
    FieldEdit, InputBar, ScrollBar,
};

pub use transcript::{FoldMode, Item, NoticeKind, Transcript};

/// A paste past either bound is held as a short placeholder rather than
/// inlined, so a big block does not swamp the prompt box.
const PASTE_MAX_CHARS: usize = 2000;
const PASTE_MAX_LINES: usize = 5;
/// A duration in whole milliseconds, saturated to fit a `u32`.
fn millis(duration: Duration) -> u32 {
    duration.as_millis().min(u128::from(u32::MAX)) as u32
}

/// Context-menu action that renames the session.
const RENAME_ACTION: &str = "agent_rename";
/// Context-menu action that deletes the session (behind a confirmation).
const DELETE_SESSION_ACTION: &str = "agent_delete_session";
/// Selection action for the F4 checkpoint-rollback picker.
const ROLLBACK_ACTION: &str = "agent_rollback";
/// Context-menu action that starts a fresh session.
const NEW_SESSION_ACTION: &str = "agent_new_session";
/// Context-menu action that opens the session picker.
const RESUME_ACTION: &str = "agent_resume";
/// Status chip and context-menu action that opens the model picker.
const MODEL_ACTION: &str = "agent_model";
/// Status/banner action that switches the connection.
const CONNECTION_ACTION: &str = "agent_connection";
/// Input action carrying a model id typed by hand.
const MODEL_INPUT_ACTION: &str = "agent_model_input";
/// Status chip and context-menu action that opens the permission-mode picker.
const MODE_ACTION: &str = "agent_mode";
/// Status chip that toggles whether the model is asked to reason.
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
/// Context-menu action that undoes the last request.
const UNDO_ACTION: &str = "agent_undo";

/// What a card in the panel is asking: the agent's permission request, or
/// whether a command script that came with the project may run.
enum Pending {
    Permission {
        envelope: PermissionEnvelope,
        form: ChoiceForm,
        /// What each of the form's rows answers, in their order.
        answers: Vec<PermissionAnswer>,
    },
    Command {
        script: CommandScript,
        args: String,
        form: ChoiceForm,
    },
    Undo {
        form: ChoiceForm,
    },
    /// Plan mode: the agent answered, carry the plan out or keep planning?
    Plan {
        form: ChoiceForm,
    },
    /// A `/handoff` brief is ready: save it to a file, or start a new session
    /// from it. The brief is kept until the choice is made.
    Handoff {
        form: ChoiceForm,
        brief: String,
    },
}

impl Pending {
    fn form(&self) -> &ChoiceForm {
        match self {
            Pending::Permission { form, .. }
            | Pending::Command { form, .. }
            | Pending::Undo { form }
            | Pending::Plan { form }
            | Pending::Handoff { form, .. } => form,
        }
    }

    fn form_mut(&mut self) -> &mut ChoiceForm {
        match self {
            Pending::Permission { form, .. }
            | Pending::Command { form, .. }
            | Pending::Undo { form }
            | Pending::Plan { form }
            | Pending::Handoff { form, .. } => form,
        }
    }
}

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
    /// The CLI agent (Claude Code, Codex) the connection drives over
    /// ACP, if it names one; it wins over an agent definition's own backend.
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
    /// The session's permission mode is now `mode`: what the catalog runs
    /// on the session's behalf (a delegated task) follows. The default runs
    /// nothing.
    fn set_mode(&self, _mode: Mode) {}
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

/// What the session switched off but the model still has in its context,
/// shared with the guard that refuses it.
type Blocked = Arc<RwLock<BTreeSet<String>>>;

/// The checklist of the session's tools, skills and MCP tools.
const TOOLSET_ACTION: &str = "agent_toolset";

/// Refuses what the session switched off while it is still in the model's
/// context: a tool by its name, a skill by the name the `skill` tool loads.
struct ToolsetGuard {
    blocked: Blocked,
}

impl Hooks for ToolsetGuard {
    fn before_tool_call(&mut self, call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
        let blocked = self.blocked.read().unwrap_or_else(PoisonError::into_inner);
        let skill = (call.name == "skill")
            .then(|| call.arguments.get("name").and_then(|v| v.as_str()))
            .flatten()
            .map(|name| format!("skill:{name}"));
        if blocked.contains(&call.name) || skill.is_some_and(|key| blocked.contains(&key)) {
            return ToolDecision::Block {
                reason: "The user switched this off for the session; do not call it again."
                    .to_string(),
            };
        }
        ToolDecision::Allow
    }
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
    /// The question a card in the panel is asking, if any.
    pending: Option<Pending>,
    /// Command scripts the user let run for this session, by name.
    allowed_commands: HashSet<String>,
    /// A command script running on a thread; its output becomes a request.
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
    /// `claude_code`/`codex` provider), applied once its models are known.
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
    /// When blocks fold; passed to each transcript.
    fold: FoldMode,
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
    /// the arrows pick a block and Space/Enter fold it.
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
    /// The current run stopped at a `/pause` (its closing line says so).
    run_paused: bool,
    /// A `/pause` was asked for and the run has not reached a step boundary
    /// yet; shown in the state strip.
    pause_requested: bool,
    /// When the current pause began, while the run is paused: its closing
    /// line ticks the pause's length until `/continue`.
    pause_start: Option<Instant>,
    /// A `/continue` resumed the paused run, so the next `AgentStart` keeps
    /// the run's start and its clock goes on from the request.
    resuming: bool,
    /// When the current permission question went up, and how long the
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
    /// Clickable fields drawn in the welcome banner, each with the status
    /// action a click on it triggers (re-pick the model, the agent). Rebuilt
    /// every render; empty once the session has content and the banner is gone.
    banner_hits: Vec<(Rect, &'static str)>,
}

impl AgentPanel {
    #[must_use]
    pub fn new(mut setup: AgentPanelSetup) -> Self {
        let session = setup.session.or_else(|| {
            start_session(
                setup.session_dir.as_deref(),
                &setup.cwd,
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
            setup.persist_rule,
            setup.hooks.as_ref(),
            backend.as_ref(),
            checkpoints.clone(),
            setup.fold,
            session.as_ref(),
            &blocked,
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
        Self {
            runtime,
            external,
            permission_rx,
            pending: None,
            allowed_commands: HashSet::new(),
            command_run: None,
            checkpoints,
            session,
            session_dir: setup.session_dir,
            session_choices: Vec::new(),
            cwd: setup.cwd,
            model,
            configured_model: setup.model,
            agent,
            catalog: setup.catalog,
            agent_choices: Vec::new(),
            prompt_choices: Vec::new(),
            late_tools,
            waiting_tools: Vec::new(),
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
            fold: setup.fold,
            prompt_stale: false,
            shown_system: String::new(),
            persist_rule: setup.persist_rule,
            transcript,
            input: InputBar::new(vec![])
                .with_multiline_field("")
                .with_placeholder("Ask the agent…")
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
            run_paused: false,
            pause_requested: false,
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
        }
    }

    /// Replace the running agent with one continuing `session` (or a fresh
    /// one when `None`). Refuses while a run is in flight.
    pub fn switch_session(&mut self, session: Option<Session>) -> bool {
        if self.is_busy() {
            self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
            return false;
        }
        let session = session.or_else(|| {
            start_session(
                self.session_dir.as_deref(),
                &self.cwd,
                &self.provider_kind,
                &self.model,
                &self.agent,
            )
        });
        let model = session_model(&self.configured_model, session.as_ref());
        self.checkpoints = checkpoint_store(self.session_dir.as_deref(), session.as_ref());
        let (mut agent, mut system_prompt, mut tools) = (
            self.agent.clone(),
            self.system_prompt.clone(),
            self.tools.clone(),
        );
        let (toolset_off, resolved) = session_agent(
            self.catalog.as_ref(),
            &agent,
            &self.context_off,
            session.as_ref(),
        );
        if let Some((name, profile)) = resolved {
            agent = name;
            system_prompt = profile.system_prompt;
            tools = profile.tools;
            self.late_tools = profile.late_tools;
            self.backend = self.provider_backend.clone().or(profile.backend);
            self.waiting_tools.clear();
            self.mcp_arrived.clear();
            self.offered_tools = profile.offered;
            self.offered_skills = profile.skills;
            self.context_off = toolset_off.clone();
            if let Some(mode) = profile.mode {
                self.rules.mode = mode;
            }
        }
        self.toolset_off = toolset_off;
        self.sync_blocked();
        let blocked = Arc::clone(&self.blocked);
        let Spawned {
            runtime,
            permission_rx,
            transcript,
            mode,
            external,
        } = spawn_runtime(
            &self.provider,
            &tools,
            &model,
            &self.cwd,
            &system_prompt,
            self.effective_rules(),
            self.compaction,
            &self.compaction_prompts,
            &self.plan_prompt,
            &self.goal_prompt,
            &self.handoff_prompt,
            self.persist_rule,
            self.hooks.as_ref(),
            self.backend.as_ref(),
            self.checkpoints.clone(),
            self.fold,
            session.as_ref(),
            &blocked,
        );
        // Dropping the old runtime cancels it and asks its worker to stop.
        self.runtime = runtime;
        self.external = external;
        self.permission_rx = permission_rx;
        self.pending = None;
        self.transcript = transcript;
        // Leaving the current session: if it was never used, delete it so an
        // empty session does not clutter the list or the disk. On a
        // same-session rebuild (switch agent, undo) the caller has already
        // taken the session out, so there is nothing to leave here.
        if let Some(old) = self.session.take() {
            discard_if_empty(old);
        }
        self.session = session;
        self.model = model;
        self.agent = agent;
        self.system_prompt = system_prompt;
        self.tools = tools;
        self.catalog.set_mode(mode.get());
        self.mode = mode;
        self.model_choices.clear();
        self.model_fetch = None;
        self.clear_input();
        self.history_pos = None;
        self.draft.clear();
        self.completion = None;
        self.top = 0;
        self.follow = true;
        self.queued = (0, 0);
        self.queued_texts.clear();
        self.pause_requested = false;
        self.context_tokens = 0;
        true
    }

    /// Sessions of this project, newest first.
    #[must_use]
    pub fn session_list(&self) -> Vec<SessionSummary> {
        self.session_dir
            .as_ref()
            .and_then(|dir| Session::list(dir).ok())
            .unwrap_or_default()
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

    /// The prompt box's text area. The input bar holds exactly one multi-line
    /// field, so both accessors always resolve.
    fn input_area(&self) -> &TextArea {
        self.input
            .multiline(0)
            .expect("agent input is a multiline field")
    }

    fn input_area_mut(&mut self) -> &mut TextArea {
        self.input
            .multiline_mut(0)
            .expect("agent input is a multiline field")
    }

    /// Clear the prompt box.
    fn clear_input(&mut self) {
        self.input.set_field_text(0, "");
        self.pastes.clear();
        self.paste_seq = 0;
    }

    /// Insert pasted `text` at the cursor: a small paste inline, a large one as
    /// a short `[#n pasted …]` placeholder whose full content is spliced back
    /// in on [`AgentPanel::submit`].
    fn paste(&mut self, text: &str) {
        let lines = text.lines().count();
        let large = text.chars().count() > PASTE_MAX_CHARS || lines > PASTE_MAX_LINES;
        if !large {
            self.input_area_mut().insert_str(text);
            return;
        }
        // Pasting the same block again unmasks it: the placeholder the first
        // paste left gives way to the full text, to read or edit in place.
        let input = self.input_text();
        if let Some(last) = self.pastes.last() {
            if last.text == text && input.contains(&last.placeholder) {
                let unmasked = input.replacen(&last.placeholder, text, 1);
                self.pastes.pop();
                self.set_input(&unmasked);
                return;
            }
        }
        self.paste_seq += 1;
        let label = if lines > 1 {
            format!("{lines} lines")
        } else {
            format!("{} chars", text.chars().count())
        };
        let placeholder = format!("[#{} pasted {label}]", self.paste_seq);
        self.input_area_mut().insert_str(&placeholder);
        self.pastes.push(Paste {
            placeholder,
            text: text.to_string(),
        });
    }

    /// Copy the prompt's selection to the clipboard. Returns whether there was
    /// one to copy, so the caller knows whether the key was the prompt's.
    fn copy_input_selection(&mut self) -> bool {
        match self.input_area().selected_text() {
            Some(text) => {
                self.copy_text(&text);
                true
            }
            None => false,
        }
    }

    /// The transcript cell under screen position (`column`, `row`), clamped
    /// into the transcript.
    fn cell_at(&self, column: u16, row: u16) -> select::Cell {
        let area = self.transcript_area;
        let row = row.clamp(area.y, (area.y + area.height).saturating_sub(1));
        let col = column
            .saturating_sub(area.x)
            .min(area.width.saturating_sub(2));
        select::Cell {
            line: self.top + (row - area.y) as usize,
            col: col as usize,
        }
    }

    /// A click on transcript line `line`: focus the chat and select the block
    /// there; a second click on the block already selected folds/unfolds it.
    fn click_line(&mut self, line: usize) -> Vec<PanelEvent> {
        // A pause's ticking line resumes the run.
        if self.paused && self.pause_start.is_some() && self.transcript.is_live_pause_line(line) {
            self.resume();
            return vec![PanelEvent::NeedsRedraw];
        }
        // A click on a run's closing line selects the block above it.
        let Some(index) = self
            .transcript
            .item_at_line(line)
            .and_then(|index| self.transcript.selectable_near(index))
        else {
            return vec![PanelEvent::NeedsRedraw];
        };
        if self.chat_focus && self.selected == index {
            self.transcript.toggle_expanded(index);
        } else {
            self.chat_focus = true;
            self.selected = index;
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// Copy the text selected in the transcript with the mouse. Returns
    /// whether there was any.
    fn copy_text_selection(&mut self) -> bool {
        let Some(selection) = self.text_selection else {
            return false;
        };
        let width = self.transcript_area.width.saturating_sub(1).max(1) as usize;
        let text = selection.text(self.transcript.rendered(), width);
        if text.trim().is_empty() {
            return false;
        }
        self.copy_text(&text);
        true
    }

    /// Copy the prompt's selection and delete it.
    fn cut_input_selection(&mut self) -> bool {
        match self.input_area().selected_text() {
            Some(text) => {
                self.copy_text(&text);
                self.input_area_mut().delete_selection();
                true
            }
            None => false,
        }
    }

    /// Paste the clipboard into the prompt; a large paste is held as a
    /// placeholder rather than flooding the input.
    fn paste_clipboard(&mut self) -> bool {
        match termide_ui::clipboard::paste() {
            Some(text) => {
                self.paste(&text);
                true
            }
            None => false,
        }
    }

    /// Put `text` on the clipboard, telling the user when the clipboard refused.
    fn copy_text(&mut self, text: &str) {
        if let Err(error) = termide_ui::clipboard::copy(text) {
            log::warn!("agent copy failed: {error}");
            self.notice(
                termide_i18n::t().agent_notice_clipboard_failed(),
                NoticeKind::Warn,
            );
        }
    }

    /// Splice every held paste's full content back in place of its placeholder.
    fn expand_pastes(&self, text: &str) -> String {
        let mut out = text.to_string();
        for paste in &self.pastes {
            out = out.replace(&paste.placeholder, &paste.text);
        }
        out
    }

    #[must_use]
    pub fn session_path(&self) -> Option<&std::path::Path> {
        self.session.as_ref().map(Session::path)
    }

    /// Send the input box: a new run when idle, a steering message while
    /// the agent works.
    pub fn submit(&mut self) -> Vec<PanelEvent> {
        // Held pastes are spliced back in before anything reads the message, so
        // the full content is what a command, a template or the model sees.
        let text = self.expand_pastes(&self.input_area().text());
        let text = text.trim().to_string();
        if text.is_empty() {
            return vec![];
        }
        self.completion = None;
        self.history_pos = None;
        self.draft.clear();
        let text = match slash_command(&text) {
            Some((UNDO_COMMAND, _)) => {
                self.clear_input();
                return self.ask_undo();
            }
            Some((COMPACT_COMMAND, focus)) => {
                // Built in: summarise the older part of the session now.
                let focus = (!focus.is_empty()).then(|| focus.to_string());
                match self.runtime.compact(focus) {
                    Ok(()) => self.clear_input(),
                    Err(PromptError::Busy) => {
                        self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn)
                    }
                    Err(error) => self.notice(error.to_string(), NoticeKind::Warn),
                }
                return vec![PanelEvent::NeedsRedraw];
            }
            Some((NEW_COMMAND, _)) => {
                // Start fresh, leaving the current session in the list (empty
                // ones are still dropped by `switch_session`).
                self.clear_input();
                self.switch_session(None);
                return vec![PanelEvent::NeedsRedraw];
            }
            Some((CLEAR_COMMAND, _)) => {
                // Like `/new`, but the current session is deleted rather than
                // kept, so there is nothing to resume back to.
                if self.is_busy() {
                    self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
                    return vec![PanelEvent::NeedsRedraw];
                }
                self.clear_input();
                if let Some(old) = self.session.take() {
                    discard(old);
                }
                self.switch_session(None);
                return vec![PanelEvent::NeedsRedraw];
            }
            Some((RENAME_COMMAND | NAME_COMMAND, args)) => {
                // With a name, rename now; without one, open the same prompt as
                // the `[≡]` menu and F2.
                self.clear_input();
                if args.is_empty() {
                    return self.handle_status_action(RENAME_ACTION);
                }
                if !self.rename_session(args) {
                    self.notice(
                        termide_i18n::t().agent_notice_no_log_to_name(),
                        NoticeKind::Warn,
                    );
                }
                return vec![PanelEvent::NeedsRedraw];
            }
            Some((PAUSE_COMMAND, _)) => {
                self.clear_input();
                if !self.request_pause() {
                    self.notice(
                        termide_i18n::t().agent_notice_nothing_to_pause(),
                        NoticeKind::Info,
                    );
                }
                return vec![PanelEvent::NeedsRedraw];
            }
            Some((CONTINUE_COMMAND, _)) => {
                self.clear_input();
                if self.paused {
                    self.resume();
                } else if self.pause_requested {
                    // The run has not reached the pause yet: withdraw it.
                    self.cancel_pause();
                } else if self.is_busy() {
                    self.notice(
                        termide_i18n::t().agent_notice_already_running(),
                        NoticeKind::Info,
                    );
                } else {
                    self.notice(
                        termide_i18n::t().agent_notice_nothing_to_continue(),
                        NoticeKind::Info,
                    );
                }
                return vec![PanelEvent::NeedsRedraw];
            }
            Some((LOOP_COMMAND, args)) => {
                self.clear_input();
                let args = args.trim();
                if args.is_empty() || args == "stop" || args == "off" {
                    if self.loop_task.take().is_some() {
                        self.notice(
                            termide_i18n::t().agent_notice_loop_stopped(),
                            NoticeKind::Info,
                        );
                    } else {
                        self.notice(
                            termide_i18n::t().agent_notice_loop_usage(),
                            NoticeKind::Info,
                        );
                    }
                    return vec![PanelEvent::NeedsRedraw];
                }
                let (interval, prompt) = parse_loop_args(args);
                if prompt.is_empty() {
                    self.notice(
                        termide_i18n::t().agent_notice_loop_usage(),
                        NoticeKind::Info,
                    );
                    return vec![PanelEvent::NeedsRedraw];
                }
                let t = termide_i18n::t();
                self.notice(
                    match interval {
                        Some(d) => t.agent_notice_looping_every_fmt(&fmt_secs(d.as_secs())),
                        None => t.agent_notice_looping().to_string(),
                    },
                    NoticeKind::Info,
                );
                self.loop_task = Some(LoopTask {
                    prompt: prompt.to_string(),
                    interval,
                    next_at: None,
                    iterations: 0,
                });
                return self.loop_step();
            }
            Some((GOAL_COMMAND, args)) => {
                self.clear_input();
                let args = args.trim();
                if args.is_empty() || args == "stop" || args == "off" {
                    if self.goal_task.take().is_some() {
                        self.notice(
                            termide_i18n::t().agent_notice_goal_stopped(),
                            NoticeKind::Info,
                        );
                    } else {
                        self.notice(
                            termide_i18n::t().agent_notice_goal_usage(),
                            NoticeKind::Info,
                        );
                    }
                    return vec![PanelEvent::NeedsRedraw];
                }
                if self.is_busy() {
                    self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
                    return vec![PanelEvent::NeedsRedraw];
                }
                return self.start_goal(args.to_string());
            }
            Some((HANDOFF_COMMAND, _)) => {
                self.clear_input();
                return self.start_handoff();
            }
            Some((USAGE_COMMAND, _)) => {
                self.clear_input();
                return self.session_summary();
            }
            Some((PROMPT_COMMAND, _)) => {
                self.clear_input();
                return self.handle_status_action(SHOW_PROMPT_ACTION);
            }
            Some((name, args)) => {
                let prompts = self.catalog.prompts();
                if let Some(template) = prompts.iter().find(|p| p.name == name) {
                    template.expand(args)
                } else if let Some(script) =
                    self.catalog.commands().into_iter().find(|c| c.name == name)
                {
                    // A command script: its output becomes the request, once
                    // it has run (and, for a project's script, been allowed).
                    self.clear_input();
                    self.run_command(script, args.to_string());
                    return vec![PanelEvent::NeedsRedraw];
                } else {
                    let mut names: Vec<String> = prompts.iter().map(|p| p.name.clone()).collect();
                    names.extend(self.catalog.commands().into_iter().map(|c| c.name));
                    names.push(COMPACT_COMMAND.to_string());
                    if self.session_dir.is_some() {
                        names.push(NEW_COMMAND.to_string());
                        names.push(CLEAR_COMMAND.to_string());
                        names.push(RENAME_COMMAND.to_string());
                        names.push(NAME_COMMAND.to_string());
                    }
                    if self.is_busy() {
                        names.push(PAUSE_COMMAND.to_string());
                    }
                    if self.paused || self.pause_requested {
                        names.push(CONTINUE_COMMAND.to_string());
                    }
                    names.push(LOOP_COMMAND.to_string());
                    names.push(GOAL_COMMAND.to_string());
                    names.push(HANDOFF_COMMAND.to_string());
                    names.push(USAGE_COMMAND.to_string());
                    names.push(PROMPT_COMMAND.to_string());
                    self.notice(
                        termide_i18n::t().agent_notice_no_command_fmt(name, &names.join(", ")),
                        NoticeKind::Warn,
                    );
                    return vec![PanelEvent::NeedsRedraw];
                }
            }
            None => text,
        };
        // A model left to the provider is known once its list arrives; until
        // then there is nothing to send to, and the text stays to send later.
        if !self.external && self.model.id.is_empty() {
            self.notice(
                termide_i18n::t().agent_notice_model_pending(),
                NoticeKind::Warn,
            );
            return vec![PanelEvent::NeedsRedraw];
        }
        self.clear_input();
        self.send(text)
    }

    /// Send `text` as the next request: a new run when idle, a steering
    /// message while the agent works.
    fn send(&mut self, text: String) -> Vec<PanelEvent> {
        self.follow = true;
        let message = UserMessage::text(text);
        if self.is_busy() {
            // The message waits in the state strip until the agent takes it,
            // then shows as a user block.
            self.queued_texts.push_back(message.plain_text());
            self.runtime.steer(message);
            self.set_queued(self.runtime.queue_lens());
        } else {
            // A fresh turn starts: surface the system prompt as a folded `#`
            // block when it is new or has changed since it was last shown, so it
            // sits just above this message.
            let system = self.effective_system_prompt();
            if system != self.shown_system {
                self.transcript.push(Item::System {
                    text: system.clone(),
                });
                self.shown_system = system;
            }
            // A request starts: from here on the files it touches are kept
            // for /undo, together with where the conversation stood.
            if let Some(store) = &self.checkpoints {
                let leaf = self
                    .session
                    .as_ref()
                    .and_then(Session::leaf_id)
                    .map(str::to_string);
                store.lock().unwrap().begin_run(leaf);
            }
            match self.runtime.prompt(message) {
                Ok(()) => self.busy = true,
                Err(error) => self.notice(
                    termide_i18n::t().agent_notice_cannot_start_fmt(&error.to_string()),
                    NoticeKind::Error,
                ),
            }
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// Run the next `/loop` iteration: send the loop's prompt as a fresh run,
    /// unless the safety cap has been reached.
    fn loop_step(&mut self) -> Vec<PanelEvent> {
        if self
            .loop_task
            .as_ref()
            .is_some_and(|t| t.iterations >= LOOP_MAX_ITERATIONS)
        {
            self.loop_task = None;
            self.notice(
                termide_i18n::t().agent_notice_loop_stopped_max_fmt(LOOP_MAX_ITERATIONS),
                NoticeKind::Warn,
            );
            return vec![PanelEvent::NeedsRedraw];
        }
        let Some(task) = self.loop_task.as_mut() else {
            return vec![PanelEvent::NeedsRedraw];
        };
        task.iterations += 1;
        task.next_at = None;
        let prompt = task.prompt.clone();
        self.send(prompt)
    }

    /// Start a `/goal`: work autonomously toward `goal`, a judge deciding after
    /// each turn whether it is reached. The first work turn is the goal itself.
    fn start_goal(&mut self, goal: String) -> Vec<PanelEvent> {
        self.notice(
            termide_i18n::t().agent_notice_goal_working_fmt(&goal),
            NoticeKind::Info,
        );
        self.goal_task = Some(GoalTask {
            goal: goal.clone(),
            iterations: 0,
            judge_at: None,
            judging: false,
        });
        self.send_goal_turn(goal)
    }

    /// Send one work turn of the active goal as a fresh run and count it;
    /// stops the goal when the safety cap is reached.
    fn send_goal_turn(&mut self, prompt: String) -> Vec<PanelEvent> {
        let over_cap = match self.goal_task.as_mut() {
            Some(task) => {
                task.iterations += 1;
                task.judge_at = None;
                task.iterations > GOAL_MAX_ITERATIONS
            }
            None => return vec![PanelEvent::NeedsRedraw],
        };
        if over_cap {
            self.goal_task = None;
            self.notice(
                termide_i18n::t().agent_notice_goal_stopped_max_fmt(GOAL_MAX_ITERATIONS),
                NoticeKind::Warn,
            );
            return vec![PanelEvent::NeedsRedraw];
        }
        self.goal_errored = false;
        self.send(prompt)
    }

    /// Ask the judge whether the active goal is reached; the verdict arrives as
    /// a `GoalJudged` event, applied in [`AgentPanel::on_goal_verdict`].
    fn run_goal_judge(&mut self) -> Vec<PanelEvent> {
        let goal = match self.goal_task.as_mut() {
            Some(task) => {
                task.judge_at = None;
                task.judging = true;
                task.goal.clone()
            }
            None => return vec![PanelEvent::NeedsRedraw],
        };
        match self.runtime.judge(goal) {
            Ok(()) => self.notice(
                termide_i18n::t().agent_notice_goal_checking(),
                NoticeKind::Info,
            ),
            Err(error) => {
                self.goal_task = None;
                self.notice(
                    termide_i18n::t().agent_notice_cannot_check_goal_fmt(&error.to_string()),
                    NoticeKind::Warn,
                );
            }
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// Apply the judge's verdict: finish when the goal is reached, otherwise
    /// send the next work turn with what is still missing.
    fn on_goal_verdict(&mut self, done: bool, reason: &str) {
        let goal = match self.goal_task.as_mut() {
            Some(task) => {
                task.judging = false;
                task.goal.clone()
            }
            None => return,
        };
        if done {
            self.goal_task = None;
            let reason = reason.trim();
            let t = termide_i18n::t();
            let msg = if reason.is_empty() {
                t.agent_notice_goal_reached().to_string()
            } else {
                t.agent_notice_goal_reached_reason_fmt(reason)
            };
            self.notice(msg, NoticeKind::Info);
            return;
        }
        let prompt = goal_continuation(&goal, reason);
        let _ = self.send_goal_turn(prompt);
    }

    /// Start a `/handoff`: a read-only model call that distils the unfinished
    /// work into a brief; the verdict arrives as an `AgentEvent::Handoff`.
    fn start_handoff(&mut self) -> Vec<PanelEvent> {
        match self.runtime.handoff() {
            Ok(()) => self.notice(
                termide_i18n::t().agent_notice_handoff_preparing(),
                NoticeKind::Info,
            ),
            Err(PromptError::Busy) => {
                self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn)
            }
            Err(error) => self.notice(
                termide_i18n::t().agent_notice_cannot_handoff_fmt(&error.to_string()),
                NoticeKind::Warn,
            ),
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// Write the handoff brief to `HANDOFF.md` in the panel's working directory,
    /// where a fresh session or another agent (including an external one that
    /// reads files) can pick it up.
    fn save_handoff(&mut self, brief: &str) {
        let path = self.cwd.join("HANDOFF.md");
        match std::fs::write(&path, brief) {
            Ok(()) => {
                self.notice(
                    termide_i18n::t().agent_notice_handoff_written_fmt(&path.display().to_string()),
                    NoticeKind::Info,
                );
                self.pending_events
                    .push(PanelEvent::FileChangedOnDisk(path));
            }
            Err(error) => self.notice(
                termide_i18n::t().agent_notice_cannot_write_handoff_fmt(&error.to_string()),
                NoticeKind::Warn,
            ),
        }
    }

    /// Discard the current session and start a fresh one seeded with the
    /// handoff brief as its first request, so work continues from it.
    fn handoff_to_new_session(&mut self, brief: String) -> Vec<PanelEvent> {
        if let Some(old) = self.session.take() {
            discard(old);
        }
        self.switch_session(None);
        self.send(format!(
            "Continue the work described in this handoff brief:\n\n{brief}"
        ))
    }

    pub fn abort(&mut self) {
        // Stopping also ends any running loop or goal.
        self.loop_task = None;
        self.goal_task = None;
        if self.is_busy() {
            self.runtime.abort();
            self.notice(termide_i18n::t().agent_notice_stopping(), NoticeKind::Warn);
        }
    }

    /// Record the runtime's queue lengths and drop the steering texts the
    /// agent has taken (it takes them oldest first).
    fn set_queued(&mut self, queued: (usize, usize)) {
        self.queued = queued;
        while self.queued_texts.len() > queued.0 {
            self.queued_texts.pop_front();
        }
    }

    /// Ask the running agent to pause at its next step boundary. Returns
    /// whether a run was there to pause.
    fn request_pause(&mut self) -> bool {
        if !self.is_busy() {
            return false;
        }
        // The state strip shows the pending pause until the run reaches a
        // step boundary.
        self.runtime.pause();
        self.pause_requested = true;
        true
    }

    /// Withdraw a pause asked for that the run has not reached yet.
    fn cancel_pause(&mut self) {
        self.runtime.cancel_pause();
        self.pause_requested = false;
    }

    /// Resume the paused run. Its clock goes on from the request, and the
    /// pause's line keeps how long the pause lasted.
    fn resume(&mut self) {
        match self.runtime.resume() {
            Ok(()) => {
                self.busy = true;
                self.paused = false;
                self.resuming = true;
                self.end_pause();
            }
            Err(error) => self.notice(
                termide_i18n::t().agent_notice_cannot_continue_fmt(&error.to_string()),
                NoticeKind::Warn,
            ),
        }
    }

    /// Give up a paused run: nothing is running, so there is nothing to
    /// abort; the pause ends where it stood, and so do a loop or goal it was
    /// part of. The calls it left unrun are closed by the next request.
    fn stop_paused(&mut self) {
        self.paused = false;
        self.run_start = None;
        self.loop_task = None;
        self.goal_task = None;
        self.end_pause();
    }

    /// Freeze the pause's line at the pause's length, once it is over.
    fn end_pause(&mut self) {
        if let Some(start) = self.pause_start.take() {
            self.transcript.finish_pause(millis(start.elapsed()));
        }
    }

    /// The state strip above the input: what holds right now rather than what
    /// happened — a pause asked for but not reached yet, and the queued
    /// messages. Empty when there is nothing to show. A pause that took
    /// effect is the transcript's `‖` line, not a line here.
    fn state_lines(&self, width: u16) -> Vec<Line<'static>> {
        let pause = self
            .pause_requested
            .then(|| termide_i18n::t().agent_notice_will_pause());
        state_strip(
            self.queued_texts.iter().map(String::as_str),
            pause,
            width,
            &self.colors,
        )
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
    fn is_fresh(&self) -> bool {
        !self
            .transcript
            .items()
            .iter()
            .any(|item| matches!(item, Item::User { .. } | Item::Assistant { .. }))
    }

    /// Apply one runtime event to the transcript and the session log.
    /// Note streamed output: enter the generating phase on the first token,
    /// then count characters for the live token estimate and speed.
    fn note_generation(&mut self, chars: usize) {
        let activity = self
            .activity
            .get_or_insert_with(|| Activity::new(Phase::Generating));
        activity.first_token.get_or_insert_with(Instant::now);
        if activity.phase != Phase::Generating {
            activity.enter(Phase::Generating);
        }
        activity.gen_chars += chars;
    }

    /// Switch the current activity to `phase` (starting one if idle).
    fn set_phase(&mut self, phase: Phase) {
        match &mut self.activity {
            Some(activity) => activity.enter(phase),
            None => self.activity = Some(Activity::new(phase)),
        }
    }

    /// The streaming block's live meta line: the same right-aligned zone a
    /// finished block shows, but with the block glyph, the ticking elapsed time
    /// (and a live token estimate while generating) and, in place of the status
    /// check, an animated spinner. Sits after the last block, animating on the
    /// panel's ~10 fps redraw while busy; `None` when idle.
    fn live_footer_lines(&self, width: u16) -> Vec<Line<'static>> {
        let Some(activity) = self.activity.as_ref() else {
            return Vec::new();
        };
        let dim = Style::default().fg(self.colors.disabled);
        let mut lines: Vec<Line<'static>> = Vec::new();
        // While tokens stream (an answer or reasoning), a generation line in the
        // same shape as a finished block's `✍️` meta, with the live estimate.
        // Input tokens are only known once the turn ends, so the `⏫` prefill
        // line waits for the finished block. Only while tokens stream: once a
        // tool runs the message's first token is still known (the cost needs
        // it), but nothing is being generated.
        // An external agent sends its text in bursts and reports no tokens, so
        // there is no generation to time: none is shown for it.
        if let (Phase::Generating, Some(first_token), false) =
            (activity.phase, activity.first_token, self.external)
        {
            let gen_ms = first_token.elapsed().as_millis() as u32;
            let tokens = activity.est_tokens();
            lines.push(transcript::right_meta(
                width,
                vec![Span::styled(
                    format!(
                        "✍\u{fe0f} {} (↓{}, {})",
                        transcript::fmt_dur(gen_ms),
                        format_tokens(tokens),
                        transcript::fmt_speed(tokens, gen_ms)
                    ),
                    dim,
                )],
            ));
        }
        // The run clock: an animated glyph and the time since the request.
        // Its own glyph keeps it apart from a block's `🕒`, and when the run
        // ends it freezes on the answer (or on the run's closing line).
        let elapsed = self
            .run_start
            .map_or_else(|| activity.msg_start.elapsed(), |start| start.elapsed());
        let frames = transcript::RUN_FRAMES;
        let frame = (elapsed.as_millis() / 120) as usize % frames.len();
        lines.push(transcript::right_meta(
            width,
            vec![
                Span::styled(
                    format!("{} ", frames[frame]),
                    Style::default()
                        .fg(self.colors.info)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(transcript::fmt_dur(elapsed.as_millis() as u32), dim),
            ],
        ));
        lines
    }

    fn apply(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::AgentStart => {
                self.busy = true;
                self.paused = false;
                // A resumed run keeps its start, so its clock counts from the
                // request; anything else (a new prompt over a pause) starts
                // afresh.
                self.end_pause();
                if !std::mem::take(&mut self.resuming) || self.run_start.is_none() {
                    self.run_start = Some(Instant::now());
                    self.run_failed = false;
                }
                self.run_paused = false;
            }
            AgentEvent::Paused => {
                // The run's closing line records the pause; the state strip
                // shows it until `/continue`.
                self.paused = true;
                self.run_paused = true;
                self.pause_requested = false;
            }
            AgentEvent::AgentEnd => {
                self.busy = false;
                self.activity = None;
                if self.run_paused {
                    // The run waits at a pause: its line ticks the pause's
                    // length (no time of day), and the run's own clock stays
                    // for `/continue`.
                    self.pause_start = Some(Instant::now());
                    self.transcript.end_run(0, "", !self.run_failed, true);
                } else if let Some(start) = self.run_start.take() {
                    self.transcript.end_run(
                        millis(start.elapsed()),
                        &now_hms(),
                        !self.run_failed,
                        false,
                    );
                }
                self.pause_requested = false;
                self.set_queued(self.runtime.queue_lens());
                if let Some(store) = &self.checkpoints {
                    store.lock().unwrap().end_run();
                }
                if self.prompt_stale {
                    self.sync_system_prompt();
                }
                self.offer_plan();
                // A loop schedules its next iteration once the run ends, unless
                // it was paused (then it waits for `/continue`) or a card is up.
                if !self.paused && self.pending.is_none() {
                    if let Some(task) = self.loop_task.as_mut() {
                        task.next_at =
                            Some(Instant::now() + task.interval.unwrap_or(Duration::ZERO));
                    }
                }
                // A goal judges the finished work turn next, unless it was
                // paused or a card is up; a turn that errored stops the goal
                // rather than looping on the failure.
                if self.goal_task.is_some() && self.goal_errored {
                    self.goal_task = None;
                    self.notice(
                        termide_i18n::t().agent_notice_goal_stopped_failed(),
                        NoticeKind::Warn,
                    );
                } else if !self.paused && self.pending.is_none() {
                    if let Some(task) = self.goal_task.as_mut() {
                        task.judge_at = Some(Instant::now());
                    }
                }
            }
            AgentEvent::TurnStart | AgentEvent::TurnEnd => {}
            AgentEvent::MessageStart => {
                // The reasoning and answer blocks are created lazily on their
                // first delta, so the reasoning lands above the answer and a
                // prefill with neither shows only the spinner.
                self.activity = Some(Activity::new(Phase::Prefill));
            }
            AgentEvent::MessageUpdate(StreamEvent::TextDelta(delta)) => {
                self.note_generation(delta.chars().count());
                self.transcript.stream_answer(&delta);
            }
            AgentEvent::MessageUpdate(StreamEvent::ThinkingDelta(delta)) => {
                self.note_generation(delta.chars().count());
                self.transcript.stream_thinking(&delta);
            }
            AgentEvent::MessageUpdate(StreamEvent::Retry {
                attempt,
                max_attempts,
                delay_ms,
                error,
            }) => self.notice(
                termide_i18n::t().agent_notice_retry_fmt(
                    attempt as usize,
                    max_attempts as usize,
                    delay_ms,
                    &error.to_string(),
                ),
                NoticeKind::Warn,
            ),
            AgentEvent::MessageUpdate(_) => {}
            AgentEvent::MessageEnd(message) => {
                // How long the message took, logged with it so a reopened
                // session shows the same figures.
                let timing = match &message {
                    Message::User(user) => {
                        self.transcript.push(Item::User {
                            text: user.plain_text(),
                            at: now_hms(),
                        });
                        None
                    }
                    Message::Assistant(assistant) => {
                        if assistant.usage.total() > 0 {
                            self.context_tokens = assistant.usage.total();
                        }
                        self.session_input += assistant.usage.input;
                        self.session_output += assistant.usage.output;
                        // A call that failed before its first token (no network,
                        // say) went through no prefill or generation: it has
                        // no cost to show, only its time and failure. Nor has
                        // an external agent's message: its first text arrives
                        // with the message's start and it reports no tokens, so
                        // prefill, generation and speed would all be made up.
                        let cost = self
                            .activity
                            .as_ref()
                            .filter(|_| !self.external)
                            .filter(|a| a.first_token.is_some() || assistant.usage.total() > 0)
                            .map(|a| a.cost(assistant.usage.input, assistant.usage.output));
                        let at = now_hms();
                        let error = assistant.error_message.clone();
                        // A goal work turn that errored must not be judged and
                        // retried on the failure; note it for `AgentEnd`.
                        if error.is_some() && self.goal_task.is_some() {
                            self.goal_errored = true;
                        }
                        if error.is_some()
                            || matches!(
                                assistant.stop_reason,
                                StopReason::Error | StopReason::Aborted
                            )
                        {
                            self.run_failed = true;
                        }
                        // The answer always carries the wall-clock time; a
                        // reasoning block, if any, carries the prefill/generation
                        // indicators (else the answer does). A tool-only turn
                        // (reasoning, no answer text) leaves no answer block.
                        let had_thinking = self.transcript.finish_thinking(&at, cost);
                        let answer_cost = if had_thinking { None } else { cost };
                        self.transcript.finish_assistant(
                            assistant.plain_text(),
                            error,
                            answer_cost,
                            at,
                            had_thinking,
                        );
                        cost.map(|cost| Timing::Turn {
                            prefill_ms: cost.prefill_ms,
                            gen_ms: cost.gen_ms,
                        })
                    }
                    // The call's end came first and timed it.
                    Message::ToolResult(result) => self
                        .transcript
                        .tool_duration(&result.tool_call_id)
                        .map(|duration_ms| Timing::Tool {
                            duration_ms,
                            waited_ms: self.transcript.tool_wait(&result.tool_call_id),
                        }),
                };
                if let Some(session) = &mut self.session {
                    if let Err(error) = session.append_timed_message(&message, timing) {
                        log::warn!("agent session write failed: {error}");
                    }
                }
            }
            AgentEvent::ToolExecutionStart { call } => {
                self.set_phase(Phase::Tool);
                self.tool_starts.insert(call.id.clone(), Instant::now());
                self.transcript.push(Item::Tool {
                    call,
                    result: None,
                    live: None,
                    at: String::new(),
                    duration_ms: None,
                    waited_ms: None,
                    waiting: false,
                });
            }
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                update: ToolUpdate::Output(output),
            } => {
                self.transcript.with_tool(&tool_call_id, |item| {
                    if let Item::Tool { live, .. } = item {
                        *live = Some(output);
                    }
                });
            }
            AgentEvent::ToolExecutionEnd { result } => {
                if let Some(path) = changed_file(&result) {
                    self.pending_events
                        .push(PanelEvent::FileChangedOnDisk(path));
                }
                // Tally how much the output cleaning saved (bash reports the
                // raw and cleaned byte counts in its details).
                if let Some(details) = result.details.as_ref() {
                    if let (Some(raw), Some(clean)) = (
                        details.get("raw_bytes").and_then(|v| v.as_u64()),
                        details.get("cleaned_bytes").and_then(|v| v.as_u64()),
                    ) {
                        self.clean_raw_bytes += raw;
                        self.clean_out_bytes += clean;
                    }
                }
                let id = result.tool_call_id.clone();
                let finished = now_hms();
                // A wait on a permission answer is the call's pause, not its
                // run time.
                self.end_permission_wait();
                let waited = self.transcript.tool_wait(&id).unwrap_or(0);
                let elapsed = self
                    .tool_starts
                    .remove(&id)
                    .map(|start| millis(start.elapsed()).saturating_sub(waited));
                self.transcript.with_tool(&id, |item| {
                    if let Item::Tool {
                        result: slot,
                        live,
                        at,
                        duration_ms,
                        ..
                    } = item
                    {
                        *slot = Some(result);
                        *live = None;
                        *at = finished;
                        *duration_ms = elapsed;
                    }
                });
            }
            AgentEvent::QueueUpdate {
                steering,
                follow_up,
            } => self.set_queued((steering, follow_up)),
            AgentEvent::CompactionStart { .. } => {
                self.set_phase(Phase::Compact);
                self.notice(
                    termide_i18n::t().agent_notice_compacting(),
                    NoticeKind::Info,
                )
            }
            AgentEvent::Compacted {
                summary,
                kept,
                tokens_before,
            } => {
                self.notice(
                    termide_i18n::t().agent_notice_compacted_fmt(tokens_before, kept),
                    NoticeKind::Info,
                );
                if let Some(session) = &mut self.session {
                    if let Err(error) = session.append_compaction(&summary, tokens_before, kept) {
                        log::warn!("agent session write failed: {error}");
                    }
                }
                // The conversation is re-read anyway: what is refused can leave
                // the context almost for free once the run is between turns.
                if self.toolset_off != self.context_off {
                    self.context_stale = true;
                }
            }
            AgentEvent::CompactionFailed { error } => self.notice(
                termide_i18n::t().agent_notice_compaction_failed_fmt(&error.to_string()),
                NoticeKind::Warn,
            ),
            AgentEvent::GoalJudged { done, reason } => self.on_goal_verdict(done, &reason),
            AgentEvent::GoalJudgeFailed { error } => {
                self.goal_task = None;
                self.notice(
                    termide_i18n::t().agent_notice_goal_check_failed_fmt(&error.to_string()),
                    NoticeKind::Warn,
                );
            }
            AgentEvent::Handoff { brief } => match brief {
                Ok(text) => {
                    // Offer the brief, with what to do with it; the text is kept
                    // on the card until the choice is made.
                    let t = termide_i18n::t();
                    let form = ChoiceForm::new(
                        t.agent_handoff_ready_title(),
                        vec![
                            t.agent_handoff_save().to_string(),
                            t.agent_handoff_new_session().to_string(),
                        ],
                    )
                    .with_detail(text.clone())
                    .with_cancel(t.agent_handoff_dismiss());
                    self.pending = Some(Pending::Handoff { form, brief: text });
                }
                Err(error) => self.notice(
                    termide_i18n::t().agent_notice_handoff_failed_fmt(&error.to_string()),
                    NoticeKind::Warn,
                ),
            },
        }
    }

    fn poll_permissions(&mut self) -> Vec<PanelEvent> {
        let mut events = Vec::new();
        while let Ok(envelope) = self.permission_rx.try_recv() {
            if self.pending.is_some() {
                // Prompts are sequential on the agent thread; a second one
                // cannot arrive before the first is answered. Deny defensively.
                let _ = envelope.reply.send(PermissionAnswer::Deny);
                continue;
            }
            // The question is asked in the panel, not in an app-wide modal:
            // with several panels open a modal does not say who is asking.
            // The status line still announces it for an unfocused panel.
            let request = &envelope.request;
            // The card shows the intent in its title and what exactly the agent
            // wants to do in the detail block below. MCP tools and others
            // without a path or command have no subject, so no detail.
            let has_subject = !request.subject.is_empty();
            let t = termide_i18n::t();
            let base = t.agent_permission_run_fmt(&request.tool);
            let title = if has_subject {
                format!("{base}:")
            } else {
                base.clone()
            };
            // The status line, for an unfocused panel, still names the subject.
            let status = if has_subject {
                format!("{base}: {}", request.subject)
            } else {
                base
            };
            // The answers the form offers, each with its row. "Always" is on
            // offer only where the configured rules count; the rows that
            // outlast this call name the pattern they record.
            let pattern = &request.suggested_pattern;
            let mut rows: Vec<(PermissionAnswer, String)> = vec![
                (
                    PermissionAnswer::AllowOnce,
                    t.agent_perm_allow_once().to_string(),
                ),
                (
                    PermissionAnswer::AllowSession,
                    format!("{} ({pattern})", t.agent_perm_allow_session()),
                ),
            ];
            if request.can_persist {
                rows.push((
                    PermissionAnswer::AllowAlways,
                    format!("{} ({pattern})", t.agent_perm_allow_always()),
                ));
                rows.push((
                    PermissionAnswer::AllowAlwaysGlobal,
                    format!("{} ({pattern})", t.agent_perm_allow_always_global()),
                ));
            }
            rows.push((PermissionAnswer::Deny, t.agent_perm_deny().to_string()));
            rows.push((
                PermissionAnswer::DenySession,
                format!("{} ({pattern})", t.agent_perm_deny_session()),
            ));
            let (answers, options): (Vec<_>, Vec<_>) = rows.into_iter().unzip();
            let mut form = ChoiceForm::new(title, options)
                .with_custom(t.agent_perm_deny_reason())
                .with_cancel(t.agent_perm_stop());
            if has_subject {
                form = form.with_detail(request.subject.clone());
            }
            events.push(PanelEvent::SetStatusMessage {
                message: status,
                is_error: false,
            });
            self.pending = Some(Pending::Permission {
                envelope,
                form,
                answers,
            });
            // The question pauses the running call until it is answered.
            let before = self.running_tool_wait();
            self.permission_wait = Some((Instant::now(), before));
            self.transcript.set_tool_wait(before, true);
        }
        events
    }

    /// How long the running call has waited on permission answers so far.
    fn running_tool_wait(&self) -> u32 {
        self.transcript
            .items()
            .iter()
            .rev()
            .find_map(|item| match item {
                Item::Tool {
                    result: None,
                    waited_ms,
                    ..
                } => Some(waited_ms.unwrap_or(0)),
                _ => None,
            })
            .unwrap_or(0)
    }

    /// The permission question is gone (answered, or dropped by a stop):
    /// the call's wait keeps its length and rests.
    fn end_permission_wait(&mut self) {
        if let Some((start, before)) = self.permission_wait.take() {
            self.transcript
                .set_tool_wait(before.saturating_add(millis(start.elapsed())), false);
        }
    }

    /// Name the conversation, so the panel title shows it instead of the
    /// first prompt. `false` when there is no session log to record it in.
    pub fn rename_session(&mut self, name: &str) -> bool {
        let Some(session) = &mut self.session else {
            return false;
        };
        match session.set_name(name) {
            Ok(_) => true,
            Err(error) => {
                log::warn!("cannot rename the agent conversation: {error}");
                false
            }
        }
    }

    /// Open the session the picker offered at `index`.
    fn resume_choice(&mut self, index: usize) -> bool {
        let Some(summary) = self.session_choices.get(index).cloned() else {
            return false;
        };
        self.session_choices.clear();
        if self.session.as_ref().map(Session::path) == Some(summary.path.as_path()) {
            return true; // already open
        }
        match Session::open_exclusive(&summary.path) {
            Ok(session) => {
                self.switch_session(Some(session));
            }
            Err(error) => {
                log::warn!("cannot open {}: {error}", summary.path.display());
                self.notice(
                    termide_i18n::t().agent_notice_cannot_open_session_fmt(&error.to_string()),
                    NoticeKind::Error,
                );
            }
        }
        true
    }

    /// Answer the outstanding question; `false` when there is none.
    pub fn answer_permission(&mut self, answer: PermissionAnswer) -> bool {
        let Some(Pending::Permission { envelope, .. }) = self.pending.take() else {
            return false;
        };
        // Mirror a lasting grant into the panel's own rules so rebuilding the
        // agent (undo, a model or agent switch) carries it, not just the hooks
        // on the worker thread. "Always" is also written to the configuration
        // by the persist callback, where it is on offer; "for this session"
        // lives only here.
        let request = &envelope.request;
        let pattern = &request.suggested_pattern;
        match answer {
            PermissionAnswer::AllowAlways | PermissionAnswer::AllowAlwaysGlobal
                if request.can_persist =>
            {
                self.rules.add(&request.tool, pattern, Decision::Allow);
            }
            PermissionAnswer::AllowAlways
            | PermissionAnswer::AllowAlwaysGlobal
            | PermissionAnswer::AllowSession => {
                self.session_rules
                    .add(&request.tool, pattern, Decision::Allow);
            }
            PermissionAnswer::DenySession => {
                self.session_rules
                    .add(&request.tool, pattern, Decision::Deny);
            }
            _ => {}
        }
        self.end_permission_wait();
        let _ = envelope.reply.send(answer);
        true
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

    /// The system prompt as the agent receives it, written next to the
    /// session logs (or to the temp directory without them) so it can be
    /// opened in a viewer.
    fn write_system_prompt(&self) -> std::io::Result<PathBuf> {
        let dir = self.session_dir.clone().unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("system-prompt.md");
        std::fs::write(&path, self.effective_system_prompt())?;
        Ok(path)
    }

    /// Offer the endpoint's models. The list is fetched off the UI thread
    /// and the picker opens from `tick()` when it arrives; an endpoint that
    /// cannot list models falls back to a typed id.
    fn request_model_list(&mut self) -> Vec<PanelEvent> {
        if self.is_busy() {
            self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
            return vec![PanelEvent::NeedsRedraw];
        }
        if self.model_fetch.is_some() {
            return vec![];
        }
        self.model_fetch = Some(spawn_model_list(Arc::clone(&self.provider)));
        vec![PanelEvent::SetStatusMessage {
            message: termide_i18n::t().agent_models_loading().to_string(),
            is_error: false,
        }]
    }

    fn model_picker(&mut self, result: Result<Vec<ModelInfo>, String>) -> PanelEvent {
        let t = termide_i18n::t();
        let mut models = match result {
            Ok(models) => models,
            Err(error) => {
                self.notice(
                    termide_i18n::t().agent_notice_model_list_unavailable_fmt(&error.to_string()),
                    NoticeKind::Info,
                );
                Vec::new()
            }
        };
        if models.is_empty() {
            return self.model_input();
        }
        if !models.iter().any(|m| m.id == self.model.id) {
            models.insert(
                0,
                ModelInfo {
                    id: self.model.id.clone(),
                    context_window: None,
                },
            );
        }
        let mut options: Vec<String> = models
            .iter()
            .map(|m| format!("{}{}", current_mark(m.id == self.model.id), m.id))
            .collect();
        options.push(format!("  {}", t.agent_model_other()));
        self.model_choices = models;
        PanelEvent::ShowSelect {
            title: t.agent_change_model().to_string(),
            options,
            on_select: SelectAction::Custom(MODEL_ACTION.to_string()),
        }
    }

    /// The model picker for an external (ACP) agent: the models it advertised,
    /// current marked, switched over ACP rather than through the built-in loop.
    fn acp_model_picker(&mut self) -> Vec<PanelEvent> {
        let t = termide_i18n::t();
        let models = self.runtime.available_models();
        if models.is_empty() {
            self.notice(
                termide_i18n::t().agent_notice_no_model_choices(),
                NoticeKind::Info,
            );
            return vec![PanelEvent::NeedsRedraw];
        }
        let current = self.runtime.current_model();
        let options = models
            .iter()
            .map(|m| {
                let mark = current_mark(current.as_deref() == Some(m.id.as_str()));
                if m.name == m.id {
                    format!("{mark}{}", m.id)
                } else {
                    format!("{mark}{} · {}", m.name, m.id)
                }
            })
            .collect();
        self.acp_models = models;
        vec![PanelEvent::ShowSelect {
            title: t.agent_change_model().to_string(),
            options,
            on_select: SelectAction::Custom(MODEL_ACTION.to_string()),
        }]
    }

    fn model_input(&self) -> PanelEvent {
        PanelEvent::ShowInput {
            prompt: termide_i18n::t().agent_model_prompt().to_string(),
            initial_value: self.model.id.clone(),
            on_submit: InputAction::Custom(MODEL_INPUT_ACTION.to_string()),
        }
    }

    fn mode_picker(&self) -> PanelEvent {
        let t = termide_i18n::t();
        let current = self.mode.get();
        let options = Mode::ALL
            .iter()
            .map(|mode| {
                let text = match mode {
                    Mode::Ask => t.agent_mode_ask(),
                    Mode::Plan => t.agent_mode_plan(),
                    Mode::Edit => t.agent_mode_edit(),
                    Mode::Configured => t.agent_mode_configured(),
                    Mode::All => t.agent_mode_all(),
                };
                format!("{}{text}", current_mark(*mode == current))
            })
            .collect();
        PanelEvent::ShowSelect {
            title: t.agent_change_mode().to_string(),
            options,
            on_select: SelectAction::Custom(MODE_ACTION.to_string()),
        }
    }

    /// Switch the permission mode. The handle is shared with the hooks, so
    /// a run in flight sees the new mode at its next tool call.
    fn set_mode(&mut self, mode: Mode) -> PanelEvent {
        let was_plan = self.mode.get() == Mode::Plan;
        self.mode.set(mode);
        self.rules.mode = mode;
        self.catalog.set_mode(mode);
        if was_plan != (mode == Mode::Plan) {
            self.sync_system_prompt();
        }
        PanelEvent::SetStatusMessage {
            message: format!(
                "{}: {}",
                termide_i18n::t().agent_change_mode(),
                mode.label()
            ),
            is_error: false,
        }
    }

    /// The prompt the worker runs on: the agent's, plus the plan-mode
    /// instructions while that mode is on.
    fn effective_system_prompt(&self) -> String {
        if self.mode.get() == Mode::Plan {
            self.plan_prompt.apply(&self.system_prompt)
        } else {
            self.system_prompt.clone()
        }
    }

    /// Hand the worker the current effective prompt. During a run the
    /// update is refused; it is retried when the run ends.
    fn sync_system_prompt(&mut self) {
        let prompt = self.effective_system_prompt();
        match self
            .runtime
            .update(Box::new(move |agent| agent.set_system_prompt(prompt)))
        {
            Ok(()) => self.prompt_stale = false,
            Err(PromptError::Busy) => self.prompt_stale = true,
            // An external agent has no prompt of ours to update.
            Err(_) => self.prompt_stale = false,
        }
    }

    /// Refuse what is switched off but still in the model's context.
    fn sync_blocked(&self) {
        *self.blocked.write().unwrap_or_else(PoisonError::into_inner) = self
            .toolset_off
            .difference(&self.context_off)
            .cloned()
            .collect();
    }

    /// Rebuild the prompt and the registry without what the session switched
    /// off, so it leaves the model's context. Only worth it where the prompt
    /// cache is lost anyway (before the first request, after a compaction,
    /// on an agent or model switch): elsewhere it would cost the cache.
    /// During a run it waits for the run to end.
    fn refresh_context(&mut self) {
        if self.external {
            return;
        }
        if self.is_busy() {
            self.context_stale = true;
            return;
        }
        let Some(profile) = self.catalog.resolve_without(&self.agent, &self.toolset_off) else {
            return;
        };
        let mut tools = profile.tools;
        // The MCP tools that already arrived stay, save those switched off;
        // the profile's own subscription is not taken, so they do not arrive
        // (and announce themselves) twice.
        for (_, tool) in &self.mcp_arrived {
            if !self.toolset_off.contains(tool.name()) {
                tools.insert(Arc::clone(tool));
            }
        }
        let prompt = if self.mode.get() == Mode::Plan {
            self.plan_prompt.apply(&profile.system_prompt)
        } else {
            profile.system_prompt.clone()
        };
        let worker_tools = tools.clone();
        match self.runtime.update(Box::new(move |agent| {
            agent.set_system_prompt(prompt);
            *agent.tools_mut() = worker_tools;
        })) {
            Ok(()) => {
                self.system_prompt = profile.system_prompt;
                self.tools = tools;
                self.waiting_tools.clear();
                self.context_off = self.toolset_off.clone();
                self.context_stale = false;
                self.sync_blocked();
            }
            Err(PromptError::Busy) => self.context_stale = true,
            Err(_) => {}
        }
    }

    /// The checklist of what the session may use: the built-in tools, the
    /// skills, each MCP server's tools. Before the first request anything
    /// toggles freely; after it, what is in the context toggles between
    /// allowed and refused, and what is out of it stays out.
    fn toolset_items(&self) -> Vec<ChecklistItem> {
        let t = termide_i18n::t();
        let fresh = self.is_fresh();
        let item = |key: String, label: String, group: String| {
            let off = self.toolset_off.contains(&key);
            let in_context = !self.context_off.contains(&key);
            let enabled = fresh || in_context;
            let note = if !enabled {
                t.agent_toolset_note_new_session()
            } else if off && !fresh {
                t.agent_toolset_note_refused()
            } else {
                ""
            };
            ChecklistItem {
                key,
                label,
                group,
                checked: !off,
                enabled,
                note: note.to_string(),
            }
        };
        let mut items: Vec<ChecklistItem> = self
            .offered_tools
            .iter()
            // The skill loader goes with the skills, which have their own items.
            .filter(|name| name.as_str() != "skill")
            .map(|name| {
                item(
                    name.clone(),
                    name.clone(),
                    t.agent_toolset_builtin().to_string(),
                )
            })
            .collect();
        items.extend(self.offered_skills.iter().map(|name| {
            item(
                format!("skill:{name}"),
                name.clone(),
                t.agent_toolset_skills().to_string(),
            )
        }));
        items.extend(self.mcp_arrived.iter().map(|(server, tool)| {
            item(
                tool.name().to_string(),
                tool.name().to_string(),
                t.agent_toolset_mcp_fmt(server),
            )
        }));
        items
    }

    /// Apply the checklist: what is left unchecked is switched off. Before
    /// the first request that takes it out of the context at once; later it
    /// is refused until a compaction takes it out.
    fn apply_toolset(&mut self, checked: &[String]) {
        let fresh = self.is_fresh();
        let mut off = self.toolset_off.clone();
        for item in self.toolset_items() {
            if !item.enabled {
                continue;
            }
            if checked.contains(&item.key) {
                off.remove(&item.key);
            } else {
                off.insert(item.key);
            }
        }
        if off == self.toolset_off {
            return;
        }
        self.toolset_off = off;
        if let Some(session) = &mut self.session {
            let disabled: Vec<String> = self.toolset_off.iter().cloned().collect();
            if let Err(error) = session.append_toolset(&disabled) {
                log::warn!("agent session write failed: {error}");
            }
        }
        if fresh {
            self.refresh_context();
        } else {
            self.sync_blocked();
        }
    }

    /// `on/all` of what the session offers, for the banner and the chip.
    fn toolset_counts(&self) -> (usize, usize) {
        let all = self
            .offered_tools
            .iter()
            .filter(|name| name.as_str() != "skill")
            .count()
            + self.offered_skills.len()
            + self.mcp_arrived.len();
        let off = self.toolset_off.len();
        (all.saturating_sub(off), all)
    }

    /// In plan mode, once the agent has answered: offer to carry the plan
    /// out, in accept-edits or asking, or to keep planning.
    fn offer_plan(&mut self) {
        if self.external || self.mode.get() != Mode::Plan || self.pending.is_some() {
            return;
        }
        // The run's closing line sits after the answer; look past it.
        let last = self
            .transcript
            .items()
            .iter()
            .rev()
            .find(|item| !matches!(item, Item::RunEnd { .. }));
        let answered = matches!(
            last,
            Some(Item::Assistant { text, error: None, .. }) if !text.trim().is_empty()
        );
        if !answered {
            return;
        }
        let t = termide_i18n::t();
        let form = ChoiceForm::new(
            t.agent_plan_carry_title(),
            vec![
                t.agent_plan_accept_edits().to_string(),
                t.agent_plan_configured().to_string(),
            ],
        )
        .with_cancel(t.agent_plan_keep());
        self.pending = Some(Pending::Plan { form });
    }

    /// The plan was accepted: leave plan mode for `mode` and send the
    /// request that carries it out.
    fn carry_out_plan(&mut self, mode: Mode) -> Vec<PanelEvent> {
        let mut events = vec![self.set_mode(mode)];
        let request = self.plan_prompt.request.trim().to_string();
        if request.is_empty() {
            self.notice(
                termide_i18n::t().agent_notice_plan_no_request(),
                NoticeKind::Warn,
            );
        } else {
            events.extend(self.send(request));
        }
        events.push(PanelEvent::NeedsRedraw);
        events
    }

    /// Take in tools that finished connecting and hand them to the worker as
    /// soon as it is between runs. `true` when something was shown.
    fn poll_late_tools(&mut self) -> bool {
        let mut arrivals = Vec::new();
        let mut disconnected = false;
        if let Some(rx) = &self.late_tools {
            loop {
                match rx.try_recv() {
                    Ok(event) => arrivals.push(event),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if disconnected {
            self.late_tools = None;
        }
        let changed = !arrivals.is_empty();
        for event in arrivals {
            match event {
                LateTools::Ready { source, tools } => {
                    self.notice(
                        termide_i18n::t().agent_notice_mcp_connected_fmt(&source, tools.len()),
                        NoticeKind::Info,
                    );
                    // Every one is listed in the checklist; one switched off
                    // stays out of the registry, and so out of the context.
                    for tool in tools {
                        let name = tool.name().to_string();
                        self.mcp_arrived.push((source.clone(), Arc::clone(&tool)));
                        if self.toolset_off.contains(&name) {
                            self.context_off.insert(name);
                        } else {
                            self.waiting_tools.push(tool);
                        }
                    }
                    self.sync_blocked();
                }
                LateTools::Failed { source, error } => {
                    self.notice(
                        termide_i18n::t().agent_notice_mcp_error_fmt(&source, &error.to_string()),
                        NoticeKind::Warn,
                    );
                }
            }
        }
        if !self.waiting_tools.is_empty() && !self.is_busy() {
            let batch = std::mem::take(&mut self.waiting_tools);
            let for_worker = batch.clone();
            match self.runtime.update(Box::new(move |agent| {
                for tool in for_worker {
                    agent.tools_mut().insert(tool);
                }
            })) {
                Ok(()) => {
                    for tool in batch {
                        self.tools.insert(tool);
                    }
                }
                Err(_) => self.waiting_tools = batch,
            }
        }
        changed
    }

    /// Offer the prompt templates; choosing one puts `/<name> ` into the
    /// input so arguments can follow.
    fn prompt_picker(&mut self) -> Vec<PanelEvent> {
        let t = termide_i18n::t();
        let prompts = self.catalog.prompts();
        if prompts.is_empty() {
            return vec![PanelEvent::SetStatusMessage {
                message: t.agent_no_prompts().to_string(),
                is_error: false,
            }];
        }
        let options = prompts
            .iter()
            .map(|p| {
                let mut line = format!("/{}", p.name);
                if !p.argument_hint.is_empty() {
                    line.push(' ');
                    line.push_str(&p.argument_hint);
                }
                if !p.description.is_empty() {
                    line.push_str(" · ");
                    line.push_str(&p.description);
                }
                line
            })
            .collect();
        self.prompt_choices = prompts;
        vec![PanelEvent::ShowSelect {
            title: t.agent_prompts().to_string(),
            options,
            on_select: SelectAction::Custom(PROMPTS_ACTION.to_string()),
        }]
    }

    fn agent_picker(&mut self) -> PanelEvent {
        let t = termide_i18n::t();
        let entries = self.catalog.list();
        let options = entries
            .iter()
            .map(|entry| {
                let mark = current_mark(entry.name == self.agent);
                if entry.description.is_empty() {
                    format!("{mark}{}", entry.name)
                } else {
                    format!("{mark}{} · {}", entry.name, entry.description)
                }
            })
            .collect();
        self.agent_choices = entries.into_iter().map(|entry| entry.name).collect();
        PanelEvent::ShowSelect {
            title: t.agent_change_agent().to_string(),
            options,
            on_select: SelectAction::Custom(AGENT_ACTION.to_string()),
        }
    }

    /// Continue the session as another agent: its prompt and tools, and its
    /// model and mode when the definition names them. Refused while a run
    /// is in flight.
    fn switch_agent(&mut self, name: &str) -> bool {
        if name == self.agent {
            return true;
        }
        let Some(profile) = self.catalog.resolve_without(name, &self.toolset_off) else {
            self.notice(
                termide_i18n::t().agent_notice_no_agent_fmt(name),
                NoticeKind::Warn,
            );
            return false;
        };
        // An external agent, or leaving one: the runtime is rebuilt on the
        // same session log, which is replayed into the transcript only.
        if profile.backend.is_some() || self.external {
            if self.is_busy() {
                self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
                return false;
            }
            if let Some(session) = &mut self.session {
                if let Err(error) = session.append_agent_change(name) {
                    log::warn!("agent session write failed: {error}");
                }
            }
            self.agent = name.to_string();
            self.system_prompt = profile.system_prompt;
            self.tools = profile.tools;
            self.late_tools = profile.late_tools;
            self.backend = profile.backend;
            if let Some(mode) = profile.mode {
                self.rules.mode = mode;
            }
            if let Some(id) = profile.model {
                self.model.id = id;
            }
            let session = self.session.take();
            self.switch_session(session);
            if !self.is_fresh() {
                self.notice(
                    termide_i18n::t().agent_notice_agent_fmt(name),
                    NoticeKind::Info,
                );
            }
            return true;
        }
        let model = match profile.model {
            Some(id) if id != self.model.id => ModelSpec {
                id,
                ..self.model.clone()
            },
            _ => self.model.clone(),
        };
        let mode_after = profile.mode.unwrap_or(self.mode.get());
        let prompt = if mode_after == Mode::Plan {
            self.plan_prompt.apply(&profile.system_prompt)
        } else {
            profile.system_prompt.clone()
        };
        let tools = profile.tools.clone();
        let worker_model = model.clone();
        if let Err(error) = self.runtime.update(Box::new(move |agent| {
            agent.set_system_prompt(prompt);
            *agent.tools_mut() = tools;
            agent.set_model(worker_model);
        })) {
            self.notice(
                termide_i18n::t().agent_notice_cannot_switch_agent_fmt(&error.to_string()),
                NoticeKind::Warn,
            );
            return false;
        }
        if model.id != self.model.id {
            if let Some(session) = &mut self.session {
                if let Err(error) = session.append_model_change(
                    self.provider_kind.as_str(),
                    &model.id,
                    Some(model.context_window),
                ) {
                    log::warn!("agent session write failed: {error}");
                }
            }
        }
        self.model = model;
        self.system_prompt = profile.system_prompt;
        self.tools = profile.tools;
        self.late_tools = profile.late_tools;
        self.waiting_tools.clear();
        // The new agent's prompt is built without what the session switched
        // off (the cache is lost anyway), and it offers its own lists.
        self.mcp_arrived.clear();
        self.offered_tools = profile.offered;
        self.offered_skills = profile.skills;
        self.context_off = self.toolset_off.clone();
        self.sync_blocked();
        if let Some(session) = &mut self.session {
            if let Err(error) = session.append_agent_change(name) {
                log::warn!("agent session write failed: {error}");
            }
        }
        if let Some(mode) = profile.mode {
            self.mode.set(mode);
            self.rules.mode = mode;
            self.catalog.set_mode(mode);
        }
        self.agent = name.to_string();
        if !self.is_fresh() {
            self.notice(
                termide_i18n::t().agent_notice_agent_fmt(name),
                NoticeKind::Info,
            );
        }
        true
    }

    /// The model as the banner and the chip show it: `auto` while it is left
    /// to the provider and not known yet.
    fn model_display(&self) -> String {
        if self.model.id.is_empty() {
            "auto".to_string()
        } else {
            self.model.id.clone()
        }
    }

    /// The connection as the banner and the chip show it: its name beside
    /// the protocol.
    fn connection_display(&self) -> String {
        let kind = provider_label(&self.provider_kind);
        if self.connection.is_empty() {
            kind.to_string()
        } else {
            format!("{} · {kind}", self.connection)
        }
    }

    /// Run the session on connection `name`: its endpoint and its
    /// model, the rest of the session kept. The agent restarts on the same
    /// log, so a built-in loop carries the conversation over; a CLI agent
    /// does not, so switching to or from one is refused once it has begun.
    fn switch_connection(&mut self, name: &str) -> bool {
        if name == self.connection {
            return true;
        }
        let t = termide_i18n::t();
        let Some(connections) = self.connections.clone() else {
            return false;
        };
        if self.is_busy() {
            self.notice(t.agent_notice_busy(), NoticeKind::Warn);
            return false;
        }
        let Some(choice) = connections.build(name, &self.agent) else {
            self.notice(t.agent_notice_no_connection_fmt(name), NoticeKind::Warn);
            return false;
        };
        if (choice.backend.is_some() || self.external) && !self.is_fresh() {
            self.notice(t.agent_notice_connection_before_first(), NoticeKind::Warn);
            return false;
        }
        connections.activate(&choice);
        self.provider = Arc::clone(&choice.provider);
        self.provider_kind = choice.kind.clone();
        self.connection = choice.name.clone();
        // The connection's model replaces the one in use: another endpoint seldom
        // serves the same id.
        let model = ModelSpec {
            id: choice.model.clone(),
            context_window: choice.context_window,
            ..self.model.clone()
        };
        self.configured_model = model.clone();
        self.model = model;
        self.model_choices.clear();
        self.provider_backend = choice.backend.clone();
        self.backend = self.provider_backend.clone().or_else(|| {
            self.catalog
                .resolve(&self.agent)
                .and_then(|profile| profile.backend)
        });
        if let Some(session) = &mut self.session {
            let written = session.append_connection_change(name).and_then(|_| {
                session.append_model_change(
                    &choice.kind,
                    &self.model.id,
                    Some(self.model.context_window),
                )
            });
            if let Err(error) = written {
                log::warn!("agent session write failed: {error}");
            }
        }
        let fresh = self.is_fresh();
        let session = self.session.take();
        self.switch_session(session);
        // The silent window probe asks the new endpoint.
        self.context_probe = (!self.external).then(|| spawn_model_list(Arc::clone(&self.provider)));
        if !fresh {
            self.notice(t.agent_notice_connection_fmt(name), NoticeKind::Info);
        }
        true
    }

    /// Continue the session on another model of the same endpoint. The
    /// context window follows the endpoint's figure when it gave one and
    /// stays as configured otherwise; the token limit is always the
    /// configured one. Refused while a run is in flight.
    fn switch_model(&mut self, id: &str, context_window: Option<u64>) -> bool {
        let id = id.trim();
        if id.is_empty() {
            return false;
        }
        let new_window = context_window.unwrap_or(self.model.context_window);
        let id_changed = id != self.model.id;
        // Re-selecting the same model still adopts a newly-known context window
        // (a provider's `max_model_len`); nothing to do only when both match.
        if !id_changed && new_window == self.model.context_window {
            return true;
        }
        let model = ModelSpec {
            id: id.to_string(),
            context_window: new_window,
            ..self.model.clone()
        };
        let worker_model = model.clone();
        if let Err(error) = self
            .runtime
            .update(Box::new(move |agent| agent.set_model(worker_model)))
        {
            self.notice(
                termide_i18n::t().agent_notice_cannot_switch_model_fmt(&error.to_string()),
                NoticeKind::Warn,
            );
            return false;
        }
        self.model = model;
        if let Some(session) = &mut self.session {
            if let Err(error) = session.append_model_change(
                self.provider_kind.as_str(),
                id,
                Some(self.model.context_window),
            ) {
                log::warn!("agent session write failed: {error}");
            }
        }
        // A silent window adoption (same id) leaves no notice; nor does a fresh
        // session, where the banner shows the new model instead.
        if id_changed && !self.is_fresh() {
            self.notice(
                termide_i18n::t().agent_notice_model_fmt(id),
                NoticeKind::Info,
            );
        }
        // Another model has no cache of this prompt: what is refused can
        // leave the context for free.
        if id_changed && self.toolset_off != self.context_off {
            self.refresh_context();
        }
        true
    }

    /// Adopt what a `list_models` result says: with the model left to the
    /// provider, its first model — for this session and the next ones the
    /// panel starts; otherwise the active model's real context window, when
    /// it is known and differs. Returns whether anything changed.
    fn adopt_listed_models(&mut self, models: &[ModelInfo]) -> bool {
        if self.is_busy() {
            return false;
        }
        if self.model.id.is_empty() {
            let Some(first) = models.first() else {
                return false;
            };
            let adopted = self.switch_model(&first.id, first.context_window);
            if adopted && self.configured_model.id.is_empty() {
                self.configured_model.id = self.model.id.clone();
                self.configured_model.context_window = self.model.context_window;
            }
            return adopted;
        }
        let Some(window) = models
            .iter()
            .find(|m| m.id == self.model.id)
            .and_then(|m| m.context_window)
        else {
            return false;
        };
        if window == self.model.context_window {
            return false;
        }
        let id = self.model.id.clone();
        self.switch_model(&id, Some(window))
    }

    /// Toggle whether the model is asked to reason (extended thinking /
    /// `reasoning_effort`). Applies to the next request and is remembered in
    /// the session log so a resume comes back with the same choice.
    fn toggle_reasoning(&mut self) -> bool {
        if self.external {
            return false;
        }
        if self.is_busy() {
            self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
            return false;
        }
        let reasoning = !self.model.reasoning;
        let mut model = self.model.clone();
        model.reasoning = reasoning;
        let worker_model = model.clone();
        if let Err(error) = self
            .runtime
            .update(Box::new(move |agent| agent.set_model(worker_model)))
        {
            self.notice(
                termide_i18n::t().agent_notice_cannot_change_reasoning_fmt(&error.to_string()),
                NoticeKind::Warn,
            );
            return false;
        }
        self.model = model;
        if let Some(session) = &mut self.session {
            if let Err(error) = session.append_reasoning_change(reasoning) {
                log::warn!("agent session write failed: {error}");
            }
        }
        let t = termide_i18n::t();
        self.notice(
            if reasoning {
                t.agent_notice_reasoning_on()
            } else {
                t.agent_notice_reasoning_off()
            },
            NoticeKind::Info,
        );
        true
    }

    /// Earlier requests of this session, oldest first, repeats collapsed.
    fn history(&self) -> Vec<String> {
        let mut history: Vec<String> = Vec::new();
        for item in self.transcript.items() {
            if let Item::User { text, .. } = item {
                if history.last() != Some(text) {
                    history.push(text.clone());
                }
            }
        }
        history
    }

    /// Show an earlier (`older`) or later request in the input, the way a
    /// shell recalls its history; past the newest, the draft comes back.
    /// Take the messages still waiting in the queue back into the input, to
    /// edit them before they go: ahead of what is typed, as they were sent
    /// first. Returns whether there were any, so `↑` walks history only once
    /// the queue is empty.
    fn unqueue(&mut self) -> bool {
        if self.history_pos.is_some() || self.queued_texts.is_empty() {
            return false;
        }
        let taken = self.runtime.take_queued();
        self.set_queued(self.runtime.queue_lens());
        self.queued_texts.clear();
        let Some(queued) = UserMessage::merge(taken) else {
            return false;
        };
        let typed = self.input_area().text();
        let text = if typed.trim().is_empty() {
            queued.plain_text()
        } else {
            format!("{}\n\n{typed}", queued.plain_text())
        };
        self.set_input(&text);
        true
    }

    fn recall(&mut self, older: bool) -> bool {
        let history = self.history();
        let next = match (self.history_pos, older) {
            (None, true) if !history.is_empty() => {
                self.draft = self.input_area().text();
                Some(history.len() - 1)
            }
            (None, _) => return false,
            (Some(pos), true) => Some(pos.saturating_sub(1)),
            (Some(pos), false) if pos + 1 < history.len() => Some(pos + 1),
            (Some(_), false) => None,
        };
        self.history_pos = next;
        let text = match next {
            Some(pos) => history[pos].clone(),
            None => std::mem::take(&mut self.draft),
        };
        self.set_input(&text);
        true
    }

    /// Replace the input with `text`, cursor at its end.
    fn set_input(&mut self, text: &str) {
        self.input.set_field_text(0, text);
        let area = self.input_area_mut();
        while area.move_down() {}
        area.move_end();
    }

    /// Recompute the `/command` popup after the input changed: it shows
    /// while the input is a single `/word` with no space yet.
    /// The plain text of the block under the chat cursor, for `Copy`, trimmed of
    /// the stray leading/trailing blank lines models and tools produce.
    fn selected_block_text(&self) -> Option<String> {
        let text = match self.transcript.items().get(self.selected)? {
            Item::User { text, .. }
            | Item::Assistant { text, .. }
            | Item::Thinking { text, .. }
            | Item::System { text } => text.clone(),
            Item::Notice { text, .. } => text.clone(),
            Item::RunEnd { elapsed_ms, at, .. } => transcript::run_end_text(*elapsed_ms, at),
            Item::Tool {
                result, live, call, ..
            } => result
                .as_ref()
                .map(ToolResultMessage::plain_text)
                .or_else(|| live.clone())
                .unwrap_or_else(|| call.name.clone()),
        };
        Some(text.trim().to_string())
    }

    /// Open the selected block's full output as a read-only panel, for a
    /// bigger view than the inline preview. A tool with a saved raw log opens
    /// that file; anything else is written to a temporary file first. Focus
    /// stays in the chat.
    fn open_selected_in_panel(&mut self) -> Vec<PanelEvent> {
        let Some(item) = self.transcript.items().get(self.selected) else {
            return vec![];
        };
        let (content, name) = match item {
            Item::Tool {
                call, result, live, ..
            } => {
                if let Some(path) = result.as_ref().and_then(full_log_path) {
                    if path.exists() {
                        return vec![PanelEvent::ViewFile(path)];
                    }
                }
                let body = result
                    .as_ref()
                    .map(ToolResultMessage::plain_text)
                    .or_else(|| live.clone())
                    .unwrap_or_default();
                (body, format!("{}-output.txt", call.name))
            }
            Item::Assistant { text, .. } => (text.clone(), "agent-answer.md".to_string()),
            Item::Thinking { text, .. } => (text.clone(), "agent-thinking.md".to_string()),
            Item::System { text } => (text.clone(), "system-prompt.md".to_string()),
            Item::User { text, .. } => (text.clone(), "message.txt".to_string()),
            Item::Notice { .. } | Item::RunEnd { .. } => return vec![PanelEvent::NeedsRedraw],
        };
        if content.trim().is_empty() {
            self.notice(
                termide_i18n::t().agent_notice_nothing_to_open(),
                NoticeKind::Info,
            );
            return vec![PanelEvent::NeedsRedraw];
        }
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let path = std::env::temp_dir().join(format!("termide-agent-{}-{safe}", now_millis()));
        match std::fs::write(&path, content) {
            Ok(()) => vec![PanelEvent::ViewFile(path), PanelEvent::NeedsRedraw],
            Err(error) => {
                self.notice(
                    termide_i18n::t().agent_notice_cannot_open_block_fmt(&error.to_string()),
                    NoticeKind::Error,
                );
                vec![PanelEvent::NeedsRedraw]
            }
        }
    }

    /// The `@`-file mention under the cursor: the span from the `@` to the
    /// cursor and the text typed after it. `@` counts only at the start of a
    /// word (line start or after whitespace), and the mention ends at the
    /// first space, so it is one path.
    fn mention_at_cursor(&self) -> Option<(MentionSpan, String)> {
        let cursor = self.input_area().cursor();
        let chars: Vec<char> = self.input_area().lines().get(cursor.row)?.chars().collect();
        if cursor.col > chars.len() {
            return None;
        }
        let mut i = cursor.col;
        while i > 0 {
            let c = chars[i - 1];
            if c == '@' {
                let starts_word = i == 1 || chars[i - 2].is_whitespace();
                if !starts_word {
                    return None;
                }
                let prefix: String = chars[i..cursor.col].iter().collect();
                return Some((
                    MentionSpan {
                        row: cursor.row,
                        start: i - 1,
                        end: cursor.col,
                    },
                    prefix,
                ));
            }
            if c.is_whitespace() {
                return None;
            }
            i -= 1;
        }
        None
    }

    fn refresh_completion(&mut self) {
        self.completion_span = None;
        let text = self.input_area().text();
        let word = text.strip_prefix('/').filter(|rest| {
            self.input_area().line_count() <= 1 && !rest.contains(char::is_whitespace)
        });
        let Some(prefix) = word else {
            return self.refresh_file_completion();
        };
        let mut items: Vec<CompletionItem> = self
            .catalog
            .prompts()
            .into_iter()
            .filter(|template| template.name.starts_with(prefix))
            .map(|template| {
                CompletionItem::new(template.name.clone())
                    .with_label(format!("/{}", template.name))
                    .with_hint(template.argument_hint)
                    .with_description(template.description)
            })
            .collect();
        let taken: Vec<String> = items.iter().map(|i| i.value.clone()).collect();
        let scripts: Vec<CommandScript> = self
            .catalog
            .commands()
            .into_iter()
            .filter(|c| c.name.starts_with(prefix) && !taken.contains(&c.name))
            .collect();
        for script in scripts {
            let description = if script.trusted {
                script.description
            } else if script.description.is_empty() {
                "project command".to_string()
            } else {
                format!("{} (project)", script.description)
            };
            items.push(
                CompletionItem::new(script.name.clone())
                    .with_label(format!("/{}", script.name))
                    .with_hint(script.argument_hint)
                    .with_description(description),
            );
        }
        if UNDO_COMMAND.starts_with(prefix) && !self.external {
            items.push(
                CompletionItem::new(UNDO_COMMAND)
                    .with_label(format!("/{UNDO_COMMAND}"))
                    .with_description(termide_i18n::t().agent_cmd_desc_undo()),
            );
        }
        if COMPACT_COMMAND.starts_with(prefix) && !self.external {
            items.push(
                CompletionItem::new(COMPACT_COMMAND)
                    .with_label(format!("/{COMPACT_COMMAND}"))
                    .with_hint("[focus]")
                    .with_description(termide_i18n::t().agent_cmd_desc_compact()),
            );
        }
        if self.session_dir.is_some() {
            if NEW_COMMAND.starts_with(prefix) {
                items.push(
                    CompletionItem::new(NEW_COMMAND)
                        .with_label(format!("/{NEW_COMMAND}"))
                        .with_description(termide_i18n::t().agent_cmd_desc_new()),
                );
            }
            if CLEAR_COMMAND.starts_with(prefix) {
                items.push(
                    CompletionItem::new(CLEAR_COMMAND)
                        .with_label(format!("/{CLEAR_COMMAND}"))
                        .with_description(termide_i18n::t().agent_cmd_desc_clear()),
                );
            }
            if RENAME_COMMAND.starts_with(prefix) {
                items.push(
                    CompletionItem::new(RENAME_COMMAND)
                        .with_label(format!("/{RENAME_COMMAND}"))
                        .with_hint("[name]")
                        .with_description(termide_i18n::t().agent_cmd_desc_rename()),
                );
            }
            if NAME_COMMAND.starts_with(prefix) {
                items.push(
                    CompletionItem::new(NAME_COMMAND)
                        .with_label(format!("/{NAME_COMMAND}"))
                        .with_hint("[name]")
                        .with_description(termide_i18n::t().agent_cmd_desc_rename()),
                );
            }
        }
        // Run control is offered only when it applies.
        if self.is_busy() && PAUSE_COMMAND.starts_with(prefix) {
            items.push(
                CompletionItem::new(PAUSE_COMMAND)
                    .with_label(format!("/{PAUSE_COMMAND}"))
                    .with_description(termide_i18n::t().agent_cmd_desc_pause()),
            );
        }
        if (self.paused || self.pause_requested) && CONTINUE_COMMAND.starts_with(prefix) {
            items.push(
                CompletionItem::new(CONTINUE_COMMAND)
                    .with_label(format!("/{CONTINUE_COMMAND}"))
                    .with_description(termide_i18n::t().agent_cmd_desc_continue()),
            );
        }
        if LOOP_COMMAND.starts_with(prefix) {
            items.push(
                CompletionItem::new(LOOP_COMMAND)
                    .with_label(format!("/{LOOP_COMMAND}"))
                    .with_hint("[interval] <prompt>")
                    .with_description(termide_i18n::t().agent_cmd_desc_loop()),
            );
        }
        if GOAL_COMMAND.starts_with(prefix) {
            items.push(
                CompletionItem::new(GOAL_COMMAND)
                    .with_label(format!("/{GOAL_COMMAND}"))
                    .with_hint("<what to achieve>")
                    .with_description(termide_i18n::t().agent_cmd_desc_goal()),
            );
        }
        if HANDOFF_COMMAND.starts_with(prefix) && !self.external {
            items.push(
                CompletionItem::new(HANDOFF_COMMAND)
                    .with_label(format!("/{HANDOFF_COMMAND}"))
                    .with_description(termide_i18n::t().agent_cmd_desc_handoff()),
            );
        }
        if USAGE_COMMAND.starts_with(prefix) {
            items.push(
                CompletionItem::new(USAGE_COMMAND)
                    .with_label(format!("/{USAGE_COMMAND}"))
                    .with_description(termide_i18n::t().agent_cmd_desc_usage()),
            );
        }
        if PROMPT_COMMAND.starts_with(prefix) {
            items.push(
                CompletionItem::new(PROMPT_COMMAND)
                    .with_label(format!("/{PROMPT_COMMAND}"))
                    .with_description(termide_i18n::t().agent_cmd_desc_prompt()),
            );
        }
        if items.is_empty() {
            self.completion = None;
            return;
        }
        // Prompts, command scripts and built-ins are gathered in different
        // groups; show the whole `/` list in one alphabetical order.
        items.sort_by(|a, b| a.value.cmp(&b.value));
        match &mut self.completion {
            Some(list) => list.set_items(items),
            None => self.completion = Some(CompletionList::new(items)),
        }
    }

    /// The `@`-file popup: files and directories under the panel's directory
    /// matching the text after `@`, so a path is a few keystrokes and a
    /// selection. A directory ends with `/` and reopens the popup for its
    /// contents; a file inserts the path and a space. Reuses the same
    /// completion widget as `/`.
    fn refresh_file_completion(&mut self) {
        let Some((span, prefix)) = self.mention_at_cursor() else {
            self.completion = None;
            return;
        };
        let items = file_completions(&self.cwd, &prefix);
        if items.is_empty() {
            self.completion = None;
            return;
        }
        self.completion_span = Some(span);
        match &mut self.completion {
            Some(list) => list.set_items(items),
            None => self.completion = Some(CompletionList::new(items)),
        }
    }

    /// Put the highlighted completion into the input. A `/`-command replaces
    /// the whole input; an `@`-file mention replaces just its span.
    fn accept_completion(&mut self) -> bool {
        let Some(list) = self.completion.take() else {
            return false;
        };
        let Some(item) = list.selected_item().cloned() else {
            return false;
        };
        match self.completion_span.take() {
            None => {
                let text = format!("/{} ", item.value);
                self.set_input(&text);
            }
            Some(span) => {
                let is_dir = item.value.ends_with('/');
                // Delete the `@`+prefix typed so far.
                self.input_area_mut().set_cursor(span.row, span.end);
                for _ in span.start..span.end {
                    self.input_area_mut().backspace();
                }
                if is_dir {
                    // Keep the `@` so the popup reopens for the directory's
                    // contents and the user can drill in.
                    self.input_area_mut().insert('@');
                    self.input_area_mut().insert_str(&item.value);
                    self.refresh_completion();
                } else {
                    // A chosen file becomes a plain path the agent can read.
                    self.input_area_mut().insert_str(&item.value);
                    self.input_area_mut().insert(' ');
                }
            }
        }
        true
    }

    /// Turn what the card reported into an answer. For a permission,
    /// `Cancelled` denies and stops the run: the user wants out, not just a
    /// "no" to this one call. For a command script, the rows are run once,
    /// run for the session, run always (a rule is written) and don't run.
    /// `false` for `NotHandled`.
    fn apply_form_action(&mut self, action: ChoiceAction) -> bool {
        match (&self.pending, action) {
            (_, ChoiceAction::Handled) => {}
            (_, ChoiceAction::NotHandled) => return false,
            (Some(Pending::Permission { answers, .. }), ChoiceAction::Chosen(index)) => {
                let answer = answers
                    .get(index)
                    .cloned()
                    .unwrap_or(PermissionAnswer::Deny);
                self.answer_permission(answer);
            }
            (Some(Pending::Permission { .. }), ChoiceAction::Custom(reason)) => {
                self.answer_permission(PermissionAnswer::DenyWithReason(reason));
            }
            (Some(Pending::Permission { .. }), ChoiceAction::Cancelled) => {
                self.answer_permission(PermissionAnswer::Deny);
                self.abort();
            }
            (Some(Pending::Command { .. }), ChoiceAction::Chosen(index)) => {
                let Some(Pending::Command { script, args, .. }) = self.pending.take() else {
                    return true;
                };
                match index {
                    1 => {
                        self.allowed_commands.insert(script.name.clone());
                    }
                    2 => {
                        self.rules.add("command", &script.name, Decision::Allow);
                        if let Some(persist) = self.persist_rule {
                            persist(
                                "command",
                                &script.name,
                                Decision::Allow,
                                PersistScope::Project,
                            );
                        }
                    }
                    3 => return true,
                    _ => {}
                }
                self.start_command(script, args);
            }
            (Some(Pending::Command { .. }), ChoiceAction::Cancelled | ChoiceAction::Custom(_)) => {
                self.pending = None;
            }
            (Some(Pending::Undo { .. }), ChoiceAction::Chosen(_)) => {
                self.pending = None;
                let events = self.perform_undo();
                self.pending_events.extend(events);
            }
            (Some(Pending::Undo { .. }), ChoiceAction::Cancelled | ChoiceAction::Custom(_)) => {
                self.pending = None;
            }
            (Some(Pending::Plan { .. }), ChoiceAction::Chosen(index)) => {
                self.pending = None;
                let mode = if index == 0 {
                    Mode::Edit
                } else {
                    Mode::Configured
                };
                let events = self.carry_out_plan(mode);
                self.pending_events.extend(events);
            }
            (Some(Pending::Plan { .. }), ChoiceAction::Cancelled | ChoiceAction::Custom(_)) => {
                self.pending = None;
            }
            (Some(Pending::Handoff { .. }), ChoiceAction::Chosen(index)) => {
                let brief = match self.pending.take() {
                    Some(Pending::Handoff { brief, .. }) => brief,
                    _ => return true,
                };
                if index == 0 {
                    self.save_handoff(&brief);
                } else {
                    let events = self.handoff_to_new_session(brief);
                    self.pending_events.extend(events);
                }
            }
            (Some(Pending::Handoff { .. }), ChoiceAction::Cancelled | ChoiceAction::Custom(_)) => {
                self.pending = None;
            }
            (None, _) => {}
        }
        true
    }

    /// Offer to undo the last request: its files go back and the
    /// conversation is rewound to before it.
    fn ask_undo(&mut self) -> Vec<PanelEvent> {
        if self.is_busy() {
            self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
            return vec![PanelEvent::NeedsRedraw];
        }
        let files = self
            .checkpoints
            .as_ref()
            .map(|store| store.lock().unwrap().last_files())
            .unwrap_or_default();
        if files.is_empty() {
            self.notice(
                termide_i18n::t().agent_notice_nothing_to_undo(),
                NoticeKind::Info,
            );
            return vec![PanelEvent::NeedsRedraw];
        }
        let names: Vec<String> = files
            .iter()
            .map(|path| {
                path.strip_prefix(&self.cwd)
                    .unwrap_or(path)
                    .display()
                    .to_string()
            })
            .collect();
        let t = termide_i18n::t();
        let changed = if names.len() == 1 {
            names[0].clone()
        } else {
            t.agent_undo_changed_files_fmt(names.len(), &names.join(", "))
        };
        let form = ChoiceForm::new(
            t.agent_undo_confirm_fmt(&changed),
            vec![t.agent_undo_restore().to_string()],
        )
        .with_cancel(t.agent_undo_keep());
        self.pending = Some(Pending::Undo { form });
        vec![PanelEvent::NeedsRedraw]
    }

    /// Offer to delete the current session (F8, or the panel's `[≡]` menu):
    /// a confirmation modal, since it removes the log for good. The accepted
    /// answer comes back as `PanelCommand::Confirmed(DELETE_SESSION_ACTION)`.
    fn ask_delete_session(&mut self) -> Vec<PanelEvent> {
        if self.is_busy() {
            self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
            return vec![PanelEvent::NeedsRedraw];
        }
        let t = termide_i18n::t();
        let name = self
            .session
            .as_ref()
            .and_then(Session::name)
            .map(str::to_string);
        let id = self
            .session
            .as_ref()
            .map(Session::path)
            .and_then(Path::file_stem)
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let label = name
            .clone()
            .unwrap_or_else(|| t.agent_delete_this_session().to_string());
        let confirm = t.agent_delete_confirm_fmt(&label);
        // The log's id under the question, after the name when it has one.
        let message = match (name, id.is_empty()) {
            (_, true) => confirm,
            (Some(name), false) => format!("{confirm}\n{name} · {id}"),
            (None, false) => format!("{confirm}\n{id}"),
        };
        vec![PanelEvent::ShowConfirm {
            message,
            on_confirm: ConfirmAction::Custom(DELETE_SESSION_ACTION.to_string()),
        }]
    }

    /// Discard the current session and open a fresh one in its place — the
    /// confirmed F8 delete, the same effect as `/clear`.
    fn perform_delete_session(&mut self) -> Vec<PanelEvent> {
        if let Some(old) = self.session.take() {
            discard(old);
        }
        self.switch_session(None);
        vec![PanelEvent::NeedsRedraw]
    }

    /// A read-only summary of the current session, shown in an info modal
    /// (F3, the `[≡]` menu's "Session info", or `/usage`).
    fn session_summary(&self) -> Vec<PanelEvent> {
        let t = termide_i18n::t();
        let name = self
            .session
            .as_ref()
            .and_then(Session::name)
            .map(str::to_string)
            .unwrap_or_else(|| "untitled".to_string());
        let messages = self
            .transcript
            .items()
            .iter()
            .filter(|item| matches!(item, Item::User { .. } | Item::Assistant { .. }))
            .count();
        let mut rows: Vec<(String, String)> = vec![("Session".into(), name)];
        if let Some(session) = self.session.as_ref() {
            if let Some(id) = session
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            {
                rows.push(("Log".into(), id));
            }
        }
        rows.push(("Agent".into(), self.agent.clone()));
        rows.push(("Provider".into(), self.provider_kind.clone()));
        rows.push(("Model".into(), self.model.id.clone()));
        rows.push(("Mode".into(), self.mode.get().label().to_string()));
        rows.push(("Directory".into(), shorten_path(&self.cwd, usize::MAX)));
        if let Some(session) = self.session.as_ref() {
            rows.push(("Created".into(), civil_date(session.header().created)));
            if let Some(last) = session.entries().last().map(|e| e.timestamp) {
                rows.push(("Last active".into(), civil_date(last)));
            }
            let compactions = session
                .entries()
                .iter()
                .filter(|e| matches!(e.kind, EntryKind::Compaction { .. }))
                .count();
            rows.push(("Compactions".into(), compactions.to_string()));
        }
        rows.push(("Messages".into(), messages.to_string()));
        rows.push((
            "Tokens".into(),
            format!(
                "↑{} ↓{}",
                format_tokens(self.session_input),
                format_tokens(self.session_output)
            ),
        ));
        rows.push((
            "Context".into(),
            format!(
                "{} / {}",
                format_tokens(self.context_tokens),
                format_tokens(self.model.context_window)
            ),
        ));
        // How much the clean mechanism has shrunk shell output this session,
        // when any ran: raw → cleaned and the percentage saved.
        if self.clean_raw_bytes > 0 {
            let saved = self.clean_raw_bytes.saturating_sub(self.clean_out_bytes);
            let percent = saved * 100 / self.clean_raw_bytes;
            rows.push((
                "Output cleaned".into(),
                format!(
                    "{} → {} (−{percent}%)",
                    format_bytes(self.clean_raw_bytes),
                    format_bytes(self.clean_out_bytes),
                ),
            ));
        }
        vec![PanelEvent::ShowInfo {
            title: t.agent_session_info().to_string(),
            rows,
        }]
    }

    /// Offer the undoable checkpoints (F4), newest first, to roll the session
    /// back to before a chosen change.
    fn ask_rollback(&mut self) -> Vec<PanelEvent> {
        if self.is_busy() {
            self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
            return vec![PanelEvent::NeedsRedraw];
        }
        let Some(store) = self.checkpoints.clone() else {
            self.notice(
                termide_i18n::t().agent_notice_nothing_to_rollback(),
                NoticeKind::Info,
            );
            return vec![PanelEvent::NeedsRedraw];
        };
        let checkpoints = store.lock().unwrap().checkpoints();
        if checkpoints.is_empty() {
            self.notice(
                termide_i18n::t().agent_notice_nothing_to_rollback(),
                NoticeKind::Info,
            );
            return vec![PanelEvent::NeedsRedraw];
        }
        let options = checkpoints
            .iter()
            .enumerate()
            .map(|(i, files)| {
                let names: Vec<String> = files
                    .iter()
                    .map(|p| p.strip_prefix(&self.cwd).unwrap_or(p).display().to_string())
                    .collect();
                let changed = if names.len() == 1 {
                    names[0].clone()
                } else {
                    format!("{} files: {}", names.len(), names.join(", "))
                };
                let step = if i == 0 {
                    "last request".to_string()
                } else {
                    format!("{} requests back", i + 1)
                };
                truncate_title(&format!("{step} — {changed}"))
            })
            .collect();
        vec![PanelEvent::ShowSelect {
            title: termide_i18n::t().agent_rollback_title().to_string(),
            options,
            on_select: SelectAction::Custom(ROLLBACK_ACTION.to_string()),
        }]
    }

    /// Undo every request from the newest down to the one the user picked
    /// (`steps_from_newest` = 0 is the last request), putting the files back and
    /// rewinding the conversation to before the oldest of them.
    fn perform_rollback(&mut self, steps_from_newest: usize) -> Vec<PanelEvent> {
        if self.is_busy() {
            self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
            return vec![PanelEvent::NeedsRedraw];
        }
        let Some(store) = self.checkpoints.clone() else {
            return vec![PanelEvent::NeedsRedraw];
        };
        let mut events = Vec::new();
        let mut restored = 0usize;
        let mut leaf = None;
        {
            let mut store = store.lock().unwrap();
            for _ in 0..=steps_from_newest {
                match store.undo_last() {
                    Ok(undone) => {
                        for path in &undone.files {
                            events.push(PanelEvent::FileChangedOnDisk(path.clone()));
                        }
                        restored += undone.files.len();
                        leaf = undone.leaf_before;
                    }
                    Err(_) => break,
                }
            }
        }
        if let Some(session) = &mut self.session {
            if let Err(error) = session.rewind_to(leaf.as_deref()) {
                log::warn!("agent session rewind failed: {error}");
            }
        }
        let session = self.session.take();
        self.switch_session(session);
        let t = termide_i18n::t();
        self.notice(
            t.agent_notice_rolled_back_fmt(restored, t.pluralize(restored, "file")),
            NoticeKind::Info,
        );
        events.push(PanelEvent::NeedsRedraw);
        events
    }

    /// Put the last request's files back, rewind the session to before it
    /// and rebuild the agent from there.
    fn perform_undo(&mut self) -> Vec<PanelEvent> {
        let Some(store) = self.checkpoints.clone() else {
            return vec![];
        };
        let undone = store.lock().unwrap().undo_last();
        let undone = match undone {
            Ok(undone) => undone,
            Err(error) => {
                self.notice(
                    termide_i18n::t().agent_notice_cannot_undo_fmt(&error.to_string()),
                    NoticeKind::Error,
                );
                return vec![PanelEvent::NeedsRedraw];
            }
        };
        let mut events: Vec<PanelEvent> = undone
            .files
            .iter()
            .map(|path| PanelEvent::FileChangedOnDisk(path.clone()))
            .collect();
        if let Some(session) = &mut self.session {
            if let Err(error) = session.rewind_to(undone.leaf_before.as_deref()) {
                log::warn!("agent session rewind failed: {error}");
            }
        }
        let count = undone.files.len();
        let session = self.session.take();
        self.switch_session(session);
        let t = termide_i18n::t();
        self.notice(
            t.agent_notice_undid_fmt(count, t.pluralize(count, "file")),
            NoticeKind::Info,
        );
        events.push(PanelEvent::NeedsRedraw);
        events
    }

    /// `/name args` names a command script: run it, or ask first when it
    /// came with the project and no rule or session grant covers it.
    fn run_command(&mut self, script: CommandScript, args: String) {
        let verdict = self.rules.evaluate("command", &script.name);
        if verdict == Some(Decision::Deny) {
            self.notice(
                termide_i18n::t().agent_notice_command_denied_fmt(&script.name),
                NoticeKind::Warn,
            );
            return;
        }
        let allowed = script.trusted
            || verdict == Some(Decision::Allow)
            || self.allowed_commands.contains(&script.name);
        if allowed {
            self.start_command(script, args);
            return;
        }
        let t = termide_i18n::t();
        let title = t.agent_command_run_title_fmt(&script.name, &script.path.display().to_string());
        let form = ChoiceForm::new(
            title.clone(),
            vec![
                t.agent_cmd_run_once().to_string(),
                t.agent_cmd_run_session().to_string(),
                t.agent_cmd_run_always().to_string(),
                t.agent_cmd_dont_run().to_string(),
            ],
        );
        self.pending_events.push(PanelEvent::SetStatusMessage {
            message: title,
            is_error: false,
        });
        self.pending = Some(Pending::Command { script, args, form });
    }

    /// Run the script on a thread; `tick` sends its output as the request.
    fn start_command(&mut self, script: CommandScript, args: String) {
        if self.command_run.is_some() {
            self.notice(
                termide_i18n::t().agent_notice_command_running(),
                NoticeKind::Warn,
            );
            return;
        }
        let (tx, rx) = mpsc::channel();
        let cwd = self.cwd.clone();
        let name = script.name.clone();
        let reported = name.clone();
        std::thread::spawn(move || {
            let outcome = script.run(&args, &cwd);
            let _ = tx.send((reported, outcome));
        });
        self.command_run = Some(rx);
        self.pending_events.push(PanelEvent::SetStatusMessage {
            message: format!("running /{}…", name),
            is_error: false,
        });
    }

    /// Take in a finished command script: its output goes out as a request.
    fn poll_command(&mut self) -> bool {
        let outcome = self.command_run.as_ref().map(Receiver::try_recv);
        match outcome {
            Some(Ok((_, Ok(text)))) => {
                self.command_run = None;
                self.send(text);
                true
            }
            Some(Ok((_, Err(error)))) => {
                self.command_run = None;
                self.notice(error, NoticeKind::Error);
                true
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.command_run = None;
                self.notice(
                    termide_i18n::t().agent_notice_command_dropped(),
                    NoticeKind::Error,
                );
                true
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => false,
        }
    }

    /// The input changed by typing: history browsing ends, the popup follows.
    fn after_edit(&mut self) {
        self.history_pos = None;
        self.refresh_completion();
    }

    fn viewport_height(&self) -> usize {
        self.transcript_area.height as usize
    }

    fn max_top(&self) -> usize {
        self.transcript
            .line_count()
            .saturating_sub(self.viewport_height())
    }

    fn scroll_by(&mut self, delta: i32) {
        let max_top = self.max_top();
        let next = if delta < 0 {
            self.top.saturating_sub(delta.unsigned_abs() as usize)
        } else {
            self.top.saturating_add(delta as usize)
        };
        self.top = next.min(max_top);
        self.follow = self.top >= max_top;
    }

    /// Bring the selected block into view after the selection moves. Scrolling
    /// is otherwise free, so this runs only from block navigation, not on every
    /// frame — a block scrolled off screen stays off until the selection moves.
    /// Uses the geometry of the last render (viewport height, flat-line layout).
    fn scroll_selected_into_view(&mut self) {
        let height = self.viewport_height();
        if height == 0 {
            return;
        }
        let Some(first) = self.transcript.first_line_of(self.selected) else {
            return;
        };
        let mut last = first;
        while self.transcript.item_at_line(last + 1) == Some(self.selected) {
            last += 1;
        }
        if first < self.top {
            // The block starts above the viewport: show it from its start.
            self.top = first;
        } else if last >= self.top + height {
            // It ends below the viewport: reveal its end, or, when it is taller
            // than the viewport, its start so it reads from the top.
            self.top = if last - first < height {
                last + 1 - height
            } else {
                first
            };
        }
        self.top = self.top.min(self.max_top());
        self.follow = self.top >= self.max_top();
    }

    fn input_rows(&self, available: u16, width: u16) -> u16 {
        // Size by wrapped (visual) rows so a long prompt grows the box instead
        // of being clipped; the `› ` prompt takes two columns. It grows up to
        // half the panel, leaving the other half to the conversation, and
        // scrolls beyond that.
        let text_width = width.saturating_sub(2).max(1) as usize;
        let rows =
            termide_ui::input_bar::wrapped_row_count(&self.input_text(), text_width).max(1) as u16;
        rows.min((available / 2).max(1))
            .min(available.saturating_sub(2).max(1))
    }

    /// The welcome banner shown while the session is empty: a logo on the left
    /// and what the agent is set up with (provider, model, agent, directory) on
    /// the right, each column centred in the transcript area. On a narrow panel
    /// the logo is dropped and only the details show.
    fn render_welcome(&mut self, area: Rect, buf: &mut Buffer, colors: &ThemeColors) {
        const LOGO: [&str; 5] = [
            "╭───────╮",
            "│       │",
            "│  ›_   │",
            "│       │",
            "╰───────╯",
        ];
        self.banner_hits.clear();
        if area.width < 14 || area.height == 0 {
            return;
        }
        let logo_w = LOGO
            .iter()
            .map(|l| termide_ui::str_display_width(l))
            .max()
            .unwrap_or(0) as u16;
        let gap = 3u16;
        let show_logo = area.width >= logo_w + gap + 22;
        let info_x = area.x + 2 + if show_logo { logo_w + gap } else { 0 };
        let info_w = (area.x + area.width).saturating_sub(info_x + 1);

        let accent = Style::default()
            .fg(colors.info)
            .add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(colors.disabled);
        let fg = Style::default().fg(colors.fg);
        // A re-pickable value (model, agent, tools) is drawn bold in the
        // accent colour, so it reads as clickable; a fixed one (cwd)
        // is plain. The click itself is wired through `banner_hits` below.
        let link = Style::default()
            .fg(colors.info)
            .add_modifier(Modifier::BOLD);
        let field = |name: &str, value: String, clickable: bool| -> Line<'static> {
            Line::from(vec![
                Span::styled(format!("{name:<12}"), dim),
                Span::styled(value, if clickable { link } else { fg }),
            ])
        };
        let cwd = shorten_path(&self.cwd, (info_w as usize).saturating_sub(12));
        // Each entry is a line and, when it names a choice that can be re-picked
        // by clicking, the status action that click triggers.
        let info: Vec<(Line<'static>, Option<&'static str>)> = vec![
            (Line::styled("termide", accent), None),
            (Line::styled("coding agent", dim), None),
            (Line::from(""), None),
            (
                field(
                    "connection",
                    self.connection_display(),
                    self.connections.is_some(),
                ),
                self.connections.is_some().then_some(CONNECTION_ACTION),
            ),
            (
                field("model", self.model_display(), true),
                Some(MODEL_ACTION),
            ),
            (field("agent", self.agent.clone(), true), Some(AGENT_ACTION)),
        ];
        // What the session may use, re-pickable before the first request,
        // when switching it off keeps it out of the context altogether.
        let mut info = info;
        if !self.external {
            let (on, all) = self.toolset_counts();
            info.push((
                field("tools", format!("{on}/{all}"), true),
                Some(TOOLSET_ACTION),
            ));
        }
        info.push((field("cwd", cwd, false), None));

        let banner_h = info.len().max(LOGO.len()) as u16;
        let bottom = area.y + area.height;
        let top = area.y + area.height.saturating_sub(banner_h) / 2;
        if show_logo {
            let logo_top = top + (banner_h - LOGO.len() as u16) / 2;
            for (i, line) in LOGO.iter().enumerate() {
                let y = logo_top + i as u16;
                if y >= bottom {
                    break;
                }
                buf.set_stringn(
                    area.x + 2,
                    y,
                    line,
                    logo_w as usize,
                    Style::default().fg(colors.info),
                );
            }
        }
        let info_top = top + (banner_h - info.len() as u16) / 2;
        for (i, (line, action)) in info.iter().enumerate() {
            let y = info_top + i as u16;
            if y >= bottom {
                break;
            }
            buf.set_line(info_x, y, line, info_w);
            // The whole field row is the click target, so the label is as good
            // as the value; an external agent still routes the click, and its
            // action answers with the "unsupported" notice.
            if let Some(action) = action {
                self.banner_hits.push((
                    Rect {
                        x: info_x,
                        y,
                        width: info_w,
                        height: 1,
                    },
                    action,
                ));
            }
        }
    }

    /// The run controls the current state offers: pause and stop while the
    /// agent works, continue in place of pause once a pause is asked for or
    /// has taken effect, none while idle.
    fn run_buttons(&self) -> Vec<RunButton> {
        let paused = self.paused && !self.is_busy();
        if paused || (self.is_busy() && self.pause_requested) {
            vec![RunButton::Continue, RunButton::Stop]
        } else if self.is_busy() {
            vec![RunButton::Pause, RunButton::Stop]
        } else {
            Vec::new()
        }
    }

    fn render_input(&mut self, area: Rect, buf: &mut Buffer, focused: bool) {
        let colors = self.colors;
        // The run controls sit at the right end of the top border, always in
        // view whatever the transcript's scroll.
        self.run_buttons = self.run_buttons();
        let buttons = self
            .run_buttons
            .iter()
            .map(|button| {
                let (label, color) = match button {
                    RunButton::Pause => ("[‖]", colors.info),
                    RunButton::Continue => ("[▶]", colors.success),
                    RunButton::Stop => ("[■]", colors.error),
                };
                (label.to_string(), Style::default().fg(color))
            })
            .collect();
        self.input.set_border_buttons(buttons);
        // The bar's top border is the divider from the content above and
        // brightens while the input is focused; the agent's name lives in the
        // panel title, not here.
        self.input.render(area, buf, &colors, focused);
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

/// Longest prompt shown in the panel title before it is cut.
const MAX_TITLE_CHARS: usize = 60;

/// Start a background `list_models` call, returning the receiver to poll from
/// `tick()`. Used both by the model picker and the silent context-window probe.
fn spawn_model_list(provider: Arc<dyn Provider>) -> Receiver<Result<Vec<ModelInfo>, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(provider.list_models());
    });
    rx
}

/// Local wall-clock time as `HH:MM:SS`, for a transcript block's byline.
/// Queued messages the state strip shows before folding the rest into a count.
const STATE_QUEUED_ROWS: usize = 3;

/// The state strip's lines: a dim dashed rule, then a pause row (`pause`,
/// when one is pending or active) and a row per queued message (its first
/// line, cut to the width), at most [`STATE_QUEUED_ROWS`] of them before a
/// "… N more" row. Empty when there is nothing to show.
fn state_strip<'a>(
    queued: impl ExactSizeIterator<Item = &'a str>,
    pause: Option<&str>,
    width: u16,
    colors: &ThemeColors,
) -> Vec<Line<'static>> {
    let t = termide_i18n::t();
    let dim = Style::default().fg(colors.disabled);
    let total = queued.len();
    if total == 0 && pause.is_none() {
        return Vec::new();
    }
    let width = width as usize;
    let cut = |text: &str, room: usize| termide_ui::path_utils::truncate_right(text, room);
    let mut lines = vec![transcript::separator(width as u16, colors)];
    if let Some(pause) = pause {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", transcript::PAUSED_GLYPH),
                Style::default()
                    .fg(colors.warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(cut(pause, width.saturating_sub(2)), dim),
        ]));
    }
    let label = format!(" {}", t.agent_state_queued());
    let label_width = termide_ui::str_display_width(&label);
    for (i, text) in queued.take(STATE_QUEUED_ROWS).enumerate() {
        let first = text.trim().lines().next().unwrap_or("");
        // The first row carries the "queued" label at the right edge, one
        // column short of the scrollbar gutter, like a block's meta.
        let room = width.saturating_sub(3 + if i == 0 { label_width } else { 0 });
        let body = cut(first, room);
        let mut spans = vec![
            Span::styled("› ", Style::default().fg(colors.info)),
            Span::styled(body.clone(), dim),
        ];
        if i == 0 {
            let used = 2 + termide_ui::str_display_width(&body) + label_width;
            spans.push(Span::raw(" ".repeat(width.saturating_sub(used + 1))));
            spans.push(Span::styled(label.clone(), dim));
        }
        lines.push(Line::from(spans));
    }
    if total > STATE_QUEUED_ROWS {
        lines.push(Line::styled(
            format!("  {}", t.agent_state_queued_more(total - STATE_QUEUED_ROWS)),
            dim,
        ));
    }
    lines
}

fn now_hms() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// Upper-case the first character of `name`, leaving the rest as written
/// (so `reviewer` → `Reviewer`, `web-dev` → `Web-dev`).
fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// First line of `text`, collapsed to one line and cut with an ellipsis.
fn truncate_title(text: &str) -> String {
    let single_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() <= MAX_TITLE_CHARS {
        return single_line;
    }
    let cut: String = single_line.chars().take(MAX_TITLE_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// The file a successful `edit` or `write` changed, from the result details,
/// so open editors can follow it without waiting for the watcher.
fn changed_file(result: &ToolResultMessage) -> Option<PathBuf> {
    if result.is_error || !matches!(result.tool_name.as_str(), "edit" | "write") {
        return None;
    }
    result
        .details
        .as_ref()?
        .get("path")?
        .as_str()
        .map(PathBuf::from)
}

/// The provider's wire protocol as the status line names it.
fn provider_label(kind: &str) -> &str {
    match kind {
        "openai_compatible" => "OpenAI Compatible",
        "anthropic_compatible" => "Anthropic Compatible",
        "claude_code" => "Claude Code",
        "codex" => "Codex",
        other => other,
    }
}

/// An eight-cell fill bar for a 0–100 percentage, e.g. `▰▰▱▱▱▱▱▱` at 20%.
fn context_bar(percent: u64) -> String {
    const CELLS: u64 = 8;
    let filled = (percent * CELLS).div_ceil(100).min(CELLS);
    let mut bar = String::with_capacity(CELLS as usize * 3);
    for i in 0..CELLS {
        bar.push(if i < filled { '▰' } else { '▱' });
    }
    bar
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

/// The work turn a `/goal` sends when the judge says the goal is not yet
/// reached: the goal restated, plus the one thing the judge found still
/// missing, so the agent keeps working from where it fell short.
fn goal_continuation(goal: &str, reason: &str) -> String {
    let reason = reason.trim();
    if reason.is_empty() {
        format!("The goal is not reached yet. Keep working toward it.\nGoal: {goal}")
    } else {
        format!(
            "The goal is not reached yet. Keep working toward it.\nGoal: {goal}\nStill missing: {reason}"
        )
    }
}

/// Split `/loop` arguments into an optional interval and the prompt. When the
/// first word is a duration (`30s`, `5m`, `2h`, or a bare count of seconds) it
/// is the interval and the rest is the prompt; otherwise the whole thing is the
/// prompt (a self-paced loop).
fn parse_loop_args(args: &str) -> (Option<Duration>, &str) {
    match args.split_once(char::is_whitespace) {
        // A duration as the first word is the interval; the rest is the prompt.
        Some((first, rest)) if parse_duration(first).is_some() => {
            (parse_duration(first), rest.trim())
        }
        // No interval (a lone word, or the first word is not a duration): the
        // whole thing is the prompt, a self-paced loop.
        _ => (None, args),
    }
}

/// Parse a duration like `30s`, `5m`, `2h`, or a bare count of seconds.
fn parse_duration(token: &str) -> Option<Duration> {
    let (digits, unit) = match token.chars().last() {
        Some('s') => (&token[..token.len() - 1], 1),
        Some('m') => (&token[..token.len() - 1], 60),
        Some('h') => (&token[..token.len() - 1], 3600),
        _ => (token, 1),
    };
    let n: u64 = digits.parse().ok()?;
    (n > 0).then(|| Duration::from_secs(n * unit))
}

/// A short human duration for the loop notice: `45s`, `5m`, `1m30s`.
fn fmt_secs(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{}m{}s", secs / 60, secs % 60)
    }
}

/// A byte count as `B`/`KB`/`MB`, for the output-cleaning diagnostic.
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1}MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1000 {
        format!("{}KB", (bytes + 500) / 1000)
    } else {
        format!("{bytes}B")
    }
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

/// `/<name> args` at the start of a message: the template name and the
/// rest. A word with further slashes (`/usr/bin`) is text, not a command.
fn slash_command(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some((name, args.trim()))
}

/// Delete a session the panel is leaving when it holds no conversation, so
/// empty sessions do not clutter the list or the disk. A session with any
/// message or a user-given name is kept.
fn discard_if_empty(session: Session) {
    if session.is_empty() {
        discard(session);
    }
}

/// Delete `session` from disk unconditionally (the `/clear` path). A failure is
/// logged rather than surfaced: the session is being abandoned regardless.
fn discard(session: Session) {
    if let Err(error) = session.discard() {
        log::warn!("could not remove agent session: {error}");
    }
}

/// The checkpoint store of `session`, under the session directory.
fn checkpoint_store(
    session_dir: Option<&std::path::Path>,
    session: Option<&Session>,
) -> Option<Arc<Mutex<CheckpointStore>>> {
    let dir = session_dir?;
    let session = session?;
    Some(Arc::new(Mutex::new(CheckpointStore::for_session(
        dir,
        session.id(),
    ))))
}

/// The path a tool result saved its full raw log to, if it did.
fn full_log_path(result: &ToolResultMessage) -> Option<std::path::PathBuf> {
    result
        .details
        .as_ref()?
        .get("full_output_path")?
        .as_str()
        .map(std::path::PathBuf::from)
}

/// The span an `@`-file mention occupies on one input line, from the `@`
/// (`start`) to the cursor (`end`), in character columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MentionSpan {
    row: usize,
    start: usize,
    end: usize,
}

/// Files and directories under `root` matching `prefix` (the text after `@`),
/// as completion items: a relative path each, directories ending in `/`. A
/// shallow, budgeted walk that skips version-control and build noise, so it
/// stays cheap on every keystroke even in a large tree.
fn file_completions(root: &std::path::Path, prefix: &str) -> Vec<CompletionItem> {
    const MAX_RESULTS: usize = 50;
    const MAX_VISITED: usize = 4000;
    /// Directory names never worth offering.
    const SKIP: [&str; 4] = [".git", "target", "node_modules", ".termide"];

    let needle = prefix.to_ascii_lowercase();
    let wants_hidden = prefix.starts_with('.');
    let mut out: Vec<(bool, String)> = Vec::new(); // (name_starts_with, path)
    let mut stack = vec![root.to_path_buf()];
    let mut visited = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if visited >= MAX_VISITED {
                break;
            }
            visited += 1;
            let name = entry.file_name().to_string_lossy().to_string();
            if SKIP.contains(&name.as_str()) {
                continue;
            }
            if name.starts_with('.') && !wants_hidden {
                continue;
            }
            let path = entry.path();
            let is_dir = path.is_dir();
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let mut rel = relative.to_string_lossy().replace('\\', "/");
            if is_dir {
                rel.push('/');
                if stack.len() < MAX_VISITED {
                    stack.push(path.clone());
                }
            }
            let hay = rel.to_ascii_lowercase();
            let name_match = name.to_ascii_lowercase().starts_with(&needle);
            if needle.is_empty() || name_match || hay.contains(&needle) {
                out.push((name_match, rel));
            }
        }
        if visited >= MAX_VISITED {
            break;
        }
    }
    // Name-prefix matches first, then shortest paths, then alphabetical.
    out.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.len().cmp(&b.1.len()))
            .then_with(|| a.1.cmp(&b.1))
    });
    out.truncate(MAX_RESULTS);
    out.into_iter()
        .map(|(_, path)| CompletionItem::new(path.clone()).with_label(path))
        .collect()
}

/// Picker prefix: `●` on the current entry, blank otherwise.
fn current_mark(current: bool) -> &'static str {
    if current {
        "● "
    } else {
        "  "
    }
}

/// Create a session log in `dir` and record the model it starts on, so a
/// later resume comes back on the same model.
fn start_session(
    dir: Option<&std::path::Path>,
    cwd: &std::path::Path,
    provider: &str,
    model: &ModelSpec,
    agent: &str,
) -> Option<Session> {
    let mut session = match Session::create_exclusive(dir?, cwd) {
        Ok(session) => session,
        Err(error) => {
            log::warn!("cannot start an agent session log: {error}");
            return None;
        }
    };
    if let Err(error) = session.append_model_change(provider, &model.id, Some(model.context_window))
    {
        log::warn!("agent session write failed: {error}");
    }
    if let Err(error) = session.append_agent_change(agent) {
        log::warn!("agent session write failed: {error}");
    }
    Some(session)
}

/// What `session` switched off, and the profile to run it with when that
/// differs from what is running: another agent than `current`, or another
/// set switched off than `built_without` (what the running profile was built
/// without). `None` keeps the running profile.
fn session_agent(
    catalog: &dyn AgentCatalog,
    current: &str,
    built_without: &BTreeSet<String>,
    session: Option<&Session>,
) -> (BTreeSet<String>, Option<(String, AgentProfile)>) {
    let off: BTreeSet<String> = session
        .and_then(Session::current_toolset)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let recorded = session
        .and_then(Session::current_agent)
        .filter(|name| name != current);
    if recorded.is_none() && off == *built_without {
        return (off, None);
    }
    let name = recorded.unwrap_or_else(|| current.to_string());
    match catalog.resolve_without(&name, &off) {
        Some(profile) => (off, Some((name, profile))),
        None => {
            log::warn!("session ran as agent {name}, which no longer exists; using {current}");
            (off, None)
        }
    }
}

/// The configured model with the id and context window `session` last ran
/// on, when it recorded them: a resumed conversation continues on its own
/// model.
fn session_model(configured: &ModelSpec, session: Option<&Session>) -> ModelSpec {
    let mut model = match session.and_then(Session::current_model) {
        Some(recorded) if !recorded.id.is_empty() => ModelSpec {
            id: recorded.id,
            context_window: recorded.context_window.unwrap_or(configured.context_window),
            ..configured.clone()
        },
        _ => configured.clone(),
    };
    // A reasoning choice made in this session (the status-bar toggle) outlives
    // a resume, overriding the configured default.
    if let Some(reasoning) = session.and_then(Session::current_reasoning) {
        model.reasoning = reasoning;
    }
    model
}

/// What [`spawn_runtime`] hands back.
struct Spawned {
    runtime: Box<dyn Backend>,
    permission_rx: Receiver<PermissionEnvelope>,
    transcript: Transcript,
    mode: ModeHandle,
    external: bool,
}

/// Spawn the agent — the built-in loop on a worker thread, or the external
/// agent `backend` makes — with its permission channel, a transcript
/// mirroring `session`'s history and the live mode handle. An external agent
/// that cannot start is reported in the transcript and the built-in loop
/// runs instead.
#[allow(clippy::too_many_arguments)]
fn spawn_runtime(
    provider: &Arc<dyn Provider>,
    tools: &ToolRegistry,
    model: &ModelSpec,
    cwd: &std::path::Path,
    system_prompt: &str,
    rules: PermissionRules,
    compaction: CompactionPolicy,
    compaction_prompts: &CompactionPrompts,
    plan_prompt: &PlanPrompt,
    goal_prompt: &GoalPrompt,
    handoff_prompt: &HandoffPrompt,
    persist_rule: Option<PersistFn>,
    extra_hooks: Option<&HooksFactory>,
    backend: Option<&BackendFactory>,
    checkpoints: Option<Arc<Mutex<CheckpointStore>>>,
    fold: FoldMode,
    session: Option<&Session>,
    blocked: &Blocked,
) -> Spawned {
    let cancel = CancelToken::new();
    let (prompter, permission_rx) = permission_channel(cancel.clone());
    let system_prompt = if rules.mode == Mode::Plan {
        plan_prompt.apply(system_prompt)
    } else {
        system_prompt.to_string()
    };
    let system_prompt = system_prompt.as_str();
    // The external backend, if one is used, is handed the same rules so its
    // permission requests get the built-in agent's treatment (read-only
    // commands and matching rules pass without a prompt).
    let backend_rules = rules.clone();
    let mut hooks = PermissionHooks::new(rules, Box::new(prompter));
    let mode = hooks.mode_handle();
    if let Some(persist) = persist_rule {
        hooks = hooks.with_persist(Box::new(persist) as PersistRule);
    }

    let mut transcript = Transcript::default();
    transcript.set_fold(fold);
    let history = session
        .map(|s| s.context_messages_with_times(compaction_prompts))
        .unwrap_or_default();
    for logged in &history {
        push_history(&mut transcript, logged);
    }
    let messages: Vec<Message> = history.into_iter().map(|logged| logged.message).collect();

    if let Some(factory) = backend {
        // The external agent gets its own prompter on a channel of its own,
        // and the same rules, so it builds permission hooks that decide its
        // requests exactly as the built-in agent's do.
        let (external_prompter, external_rx) = permission_channel(cancel.clone());
        match factory(BackendSetup {
            cwd: cwd.to_path_buf(),
            prompter: external_prompter,
            cancel: cancel.clone(),
            rules: backend_rules,
            persist: persist_rule.map(|f| Box::new(f) as PersistRule),
        }) {
            Ok(runtime) => {
                if !messages.is_empty() {
                    transcript.push(Item::Notice {
                        text: "earlier messages are shown but not known to the external agent"
                            .into(),
                        kind: NoticeKind::Info,
                    });
                }
                return Spawned {
                    runtime,
                    permission_rx: external_rx,
                    transcript,
                    mode,
                    external: true,
                };
            }
            Err(error) => transcript.push(Item::Notice {
                text: format!("cannot start the external agent: {error}; using the built-in one"),
                kind: NoticeKind::Error,
            }),
        }
    }

    let agent = Agent::new(
        Arc::clone(provider),
        tools.clone(),
        model.clone(),
        cwd.to_path_buf(),
    )
    .with_system_prompt(system_prompt)
    .with_compaction(compaction)
    .with_compaction_prompts(compaction_prompts.clone())
    .with_goal_prompt(goal_prompt.clone())
    .with_handoff_prompt(handoff_prompt.clone())
    .with_messages(messages);
    // What the session switched off goes first: it is refused whatever else
    // would allow it. Then plan mode's guard: nothing, not even a hook's
    // approval, changes a file while it is on. Then the checkpoint recorder,
    // so no call that runs is missed; then the command hooks, which may block
    // or approve before anyone is asked, and whose rewritten arguments are
    // what the rules then judge.
    let mut chain: Vec<Box<dyn Hooks>> = vec![
        Box::new(ToolsetGuard {
            blocked: Arc::clone(blocked),
        }),
        Box::new(PlanGuard::new(mode.clone())),
    ];
    if let Some(store) = checkpoints {
        chain.push(Box::new(CheckpointHooks::new(store)));
    }
    if let Some(factory) = extra_hooks {
        chain.push(factory());
    }
    chain.push(Box::new(hooks));
    let hooks: Box<dyn Hooks> = Box::new(ChainedHooks::new(chain));
    let runtime = AgentRuntime::spawn_with_cancel(agent, hooks, cancel);
    Spawned {
        runtime: Box::new(runtime),
        permission_rx,
        transcript,
        mode,
        external: false,
    }
}

/// Mirror a session's message into transcript items when a session is
/// reopened. The log's timestamp restores when each block was written; its
/// recorded timing, with the turn's own token usage, restores the `⏫`/`✍️`
/// cost and a tool's `🕒` duration, as the live run showed them.
fn push_history(transcript: &mut Transcript, logged: &LoggedMessage) {
    let at = hms_from_millis(logged.timestamp);
    match &logged.message {
        Message::User(user) => transcript.push(Item::User {
            text: user.plain_text(),
            at,
        }),
        Message::Assistant(assistant) => {
            let cost = match logged.timing {
                Some(Timing::Turn { prefill_ms, gen_ms }) => Some(transcript::Cost {
                    prefill_ms,
                    gen_ms,
                    input: assistant.usage.input,
                    output: assistant.usage.output,
                }),
                _ => None,
            };
            // Reasoning is restored as its own block above the tools and answer.
            // When it is present it carries the turn's cost, and the answer is
            // left with its time alone, matching a live turn.
            let thinking = assistant.thinking_text();
            let has_thinking = !thinking.trim().is_empty();
            if has_thinking {
                transcript.push(Item::Thinking {
                    text: thinking,
                    streaming: false,
                    at: at.clone(),
                    cost,
                });
            }
            for call in assistant.tool_calls() {
                transcript.push(Item::Tool {
                    call: call.clone(),
                    result: None,
                    live: None,
                    at: at.clone(),
                    duration_ms: None,
                    waited_ms: None,
                    waiting: false,
                });
            }
            // The answer keeps its wall-clock time; skip an empty answer block
            // when the reasoning already stands for the turn (a tool-only turn),
            // so no phantom block is left behind.
            let answer = assistant.plain_text();
            let error = assistant.error_message.clone();
            if !answer.trim().is_empty() || !has_thinking || error.is_some() {
                transcript.push(Item::Assistant {
                    text: answer,
                    streaming: false,
                    error,
                    at,
                    cost: if has_thinking { None } else { cost },
                    run_ms: None,
                });
            }
        }
        Message::ToolResult(result) => {
            let id = result.tool_call_id.clone();
            let (elapsed, wait) = match logged.timing {
                Some(Timing::Tool {
                    duration_ms,
                    waited_ms,
                }) => (Some(duration_ms), waited_ms),
                _ => (None, None),
            };
            transcript.with_tool(&id, |item| {
                if let Item::Tool {
                    result: slot,
                    at: tool_at,
                    duration_ms,
                    waited_ms,
                    ..
                } = item
                {
                    *slot = Some(result.clone());
                    *tool_at = at;
                    *duration_ms = elapsed;
                    *waited_ms = wait;
                }
            });
        }
    }
}

/// Format an epoch-millis timestamp as the local `HH:MM:SS`, matching
/// [`now_hms`] so restored blocks read the same as live ones.
fn hms_from_millis(ms: u64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ms as i64) {
        chrono::offset::LocalResult::Single(dt) => dt.format("%H:%M:%S").to_string(),
        _ => String::new(),
    }
}

impl Panel for AgentPanel {
    fn name(&self) -> &'static str {
        "agent"
    }

    /// `Agent: <name>` for a named conversation, else `Agent: <first
    /// prompt>`, else `Agent: <working directory>`. The `Agent` label is
    /// replaced by a custom agent's own name (capitalized), so parallel panels
    /// running different agents are told apart. The renderer shortens further
    /// from the left when the panel is narrow, so only a long name or prompt is
    /// cut here.
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
            .map(truncate_title);
        let subject = named
            .or_else(|| {
                self.transcript.items().iter().find_map(|item| match item {
                    Item::User { text, .. } => Some(truncate_title(text)),
                    _ => None,
                })
            })
            .unwrap_or_else(|| self.cwd.to_string_lossy().into_owned());
        format!("{label}: {subject}")
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
            TOOLSET_ACTION => vec![PanelEvent::ShowChecklist {
                title: t.agent_toolset_title().to_string(),
                prompt: t.agent_toolset_prompt().to_string(),
                items: self.toolset_items(),
                action: TOOLSET_ACTION.to_string(),
            }],
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
            MODE_ACTION if self.external => {
                self.notice(PromptError::Unsupported.to_string(), NoticeKind::Warn);
                vec![PanelEvent::NeedsRedraw]
            }
            MODEL_ACTION => self.request_model_list(),
            MODE_ACTION => vec![self.mode_picker()],
            REASONING_ACTION => {
                self.toggle_reasoning();
                vec![PanelEvent::NeedsRedraw]
            }
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
        if area.width == 0 || area.height == 0 {
            return;
        }
        buf.set_style(area, Style::default().fg(self.colors.fg).bg(self.colors.bg));

        let input_rows = self.input_rows(area.height, area.width);
        // The input bar carries its own titled top border, which divides it
        // from the content above, so the box is one row taller than its text.
        let bar_rows = input_rows + 1;
        // The agent's question sits above the input; when a card is present a
        // plain separator divides it from the transcript (the bar's own border
        // divides the card from the input). When the panel is too short for the
        // card the keys still answer.
        let form_rows = self
            .pending
            .as_ref()
            .map_or(0, |pending| pending.form().height(area.width))
            .min(area.height.saturating_sub(bar_rows + 1));
        let has_separator = form_rows > 0 && area.height > bar_rows + form_rows;
        // The state strip (a pending pause, queued messages) sits between the
        // transcript and the card, leaving the transcript at least one row.
        let text_width = area.width.saturating_sub(1).max(1);
        let mut state = self.state_lines(text_width);
        let room = area
            .height
            .saturating_sub(bar_rows + form_rows + u16::from(has_separator) + 1);
        state.truncate(room as usize);
        let state_rows = state.len() as u16;
        let transcript_height = area
            .height
            .saturating_sub(bar_rows + form_rows + state_rows)
            .saturating_sub(u16::from(has_separator));
        self.transcript_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: transcript_height,
        };
        self.input_area = Rect {
            x: area.x,
            y: area.y + area.height - bar_rows,
            width: area.width,
            height: bar_rows,
        };
        let form_area = Rect {
            x: area.x,
            y: self.input_area.y - form_rows,
            width: area.width,
            height: form_rows,
        };

        // The rightmost column is the scrollbar gutter (`text_width` above), so
        // wrapped text never sits under the bar.
        let colors = self.colors;
        let is_light = self.is_light;
        // The streaming block's live meta (ticking time + spinner) sits after
        // the last block while the agent works, animating on the ~10 fps redraw.
        let footer = self.live_footer_lines(text_width);
        self.transcript.set_live_footer(footer);
        let total = self.transcript.lines(text_width, &colors, is_light).len();
        let max_top = total.saturating_sub(transcript_height as usize);
        if self.follow {
            self.top = max_top;
        } else {
            self.top = self.top.min(max_top);
        }
        // Keep the chat selection valid, on screen, and note the flat-line
        // range to tint — computed now, before `lines` borrows the transcript.
        let item_count = self.transcript.items().len();
        let mut selected_range: Option<(usize, usize)> = None;
        if self.chat_focus && item_count > 0 {
            self.selected = self
                .transcript
                .selectable_near(self.selected)
                .unwrap_or(item_count - 1);
            // Scrolling stays free while a block is selected: the view is
            // brought to a block only when the selection moves (see
            // `scroll_selected_into_view`), not on every frame. Here we only
            // note the block's flat-line range to tint.
            // The rule or gap around a block stays out of the highlight.
            selected_range = self.transcript.content_lines_of(self.selected);
        }
        let lines = self.transcript.lines(text_width, &colors, is_light);
        // The block under the chat cursor is shown inverted (text and
        // background swapped), so the selection reads as one solid block.
        let selected_style = Style::default().fg(colors.bg).bg(colors.fg);
        if lines.is_empty() {
            // A fresh session shows a welcome banner in place of the (empty)
            // transcript: the logo and what the agent is set up with.
            let welcome = Rect {
                height: transcript_height,
                ..area
            };
            self.render_welcome(welcome, buf, &colors);
        } else {
            // No banner while the session has content, so its click targets go.
            self.banner_hits.clear();
            for row in 0..transcript_height as usize {
                let Some(line) = lines.get(self.top + row) else {
                    break;
                };
                buf.set_line(area.x, area.y + row as u16, line, text_width);
                if selected_range.is_some_and(|(f, l)| self.top + row >= f && self.top + row <= l) {
                    for dx in 0..text_width {
                        let cell = &mut buf[(area.x + dx, area.y + row as u16)];
                        // Success and error keep their hue under the
                        // selection, inverted like the rest: an edit's diff,
                        // a status glyph or a failure still reads as one.
                        let style = if cell.fg == colors.success || cell.fg == colors.error {
                            Style::default().fg(colors.bg).bg(cell.fg)
                        } else {
                            selected_style
                        };
                        cell.set_style(style);
                    }
                }
                // A mouse selection over the text, as a terminal shows one.
                if let Some((start, end)) = self
                    .text_selection
                    .and_then(|sel| sel.columns_on(self.top + row, text_width as usize))
                {
                    for dx in start..end {
                        buf[(area.x + dx as u16, area.y + row as u16)].set_style(
                            Style::default()
                                .fg(colors.selection_fg)
                                .bg(colors.selection_bg),
                        );
                    }
                }
            }
        }
        self.scrollbars.vertical = ScrollBar::render_tracked(
            buf,
            ctx.border_right_x.unwrap_or(area.x + area.width - 1),
            area.y,
            transcript_height,
            self.top,
            transcript_height as usize,
            total,
            &self.colors,
            ctx.is_focused,
        );

        let state_y = area.y + transcript_height;
        for (row, line) in state.iter().enumerate() {
            buf.set_line(area.x, state_y + row as u16, line, text_width);
        }
        // The strip's pending-pause line (after its rule) withdraws the pause
        // on a click.
        self.pause_row = (self.pause_requested && state.len() > 1).then_some(state_y + 1);
        if has_separator {
            let y = form_area.y - 1;
            let style = Style::default().fg(if ctx.is_focused {
                self.colors.border_focused
            } else {
                self.colors.disabled
            });
            for dx in 0..area.width {
                buf[(area.x + dx, y)].set_symbol("─").set_style(style);
            }
        }
        let input_area = self.input_area;
        self.render_input(input_area, buf, ctx.is_focused && !self.chat_focus);
        if form_rows >= 3 {
            if let Some(pending) = &mut self.pending {
                pending
                    .form_mut()
                    .render(form_area, buf, &colors, ctx.is_focused);
            }
        }
        if ctx.is_focused && transcript_height > 0 {
            // The completion list overlays the bottom of the transcript,
            // right above the input bar.
            let above = Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: transcript_height,
            };
            if let Some(list) = &mut self.completion {
                list.render(above, buf, &colors);
            }
        }
    }

    fn handle_key(&mut self, chord: KeyChord) -> Vec<PanelEvent> {
        // A shortcut — a Ctrl/Alt chord, or any key while the chat has focus
        // and nothing is being typed — matches on the canonical form, so it
        // works on a non-Latin layout too (`Ctrl+щ` is `Ctrl+O`). Typing into
        // the input or a pending question keeps the raw key.
        let raw = chord.raw;
        let shortcut = raw
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            || (self.chat_focus && self.pending.is_none());
        let key = if shortcut { chord.canonical } else { raw };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let page = (self.viewport_height() as i32 - 1).max(1);

        // A pending question takes the keys first: the arrows, Enter, a
        // digit or Esc answer it; only scrolling passes by.
        if let Some(pending) = &mut self.pending {
            let action = if ctrl || alt {
                ChoiceAction::NotHandled
            } else {
                pending.form_mut().handle_key(key)
            };
            let scroll_key = matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
                || (ctrl
                    && matches!(
                        key.code,
                        KeyCode::Up
                            | KeyCode::Down
                            | KeyCode::Home
                            | KeyCode::End
                            | KeyCode::Char('o')
                    ));
            if self.apply_form_action(action.clone()) {
                return vec![PanelEvent::NeedsRedraw];
            }
            if action == ChoiceAction::NotHandled && !scroll_key {
                return vec![];
            }
        }

        // F2 renames the session, wherever the focus sits in the panel — the
        // same prompt as the `[≡]` menu's Rename.
        if key.code == KeyCode::F(2) && !ctrl && !alt && !shift {
            return self.handle_status_action(RENAME_ACTION);
        }
        // F3 shows a summary of the session, F4 offers a checkpoint to roll back
        // to.
        if key.code == KeyCode::F(3) && !ctrl && !alt && !shift {
            return self.session_summary();
        }
        if key.code == KeyCode::F(4) && !ctrl && !alt && !shift {
            return self.ask_rollback();
        }
        // F6 switches session (the picker), F7 starts a new one, F8 deletes the
        // current one behind a confirmation card.
        if key.code == KeyCode::F(6) && !ctrl && !alt && !shift {
            return self.handle_status_action(RESUME_ACTION);
        }
        if key.code == KeyCode::F(7) && !ctrl && !alt && !shift {
            return self.handle_status_action(NEW_SESSION_ACTION);
        }
        if key.code == KeyCode::F(8) && !ctrl && !alt && !shift {
            return self.ask_delete_session();
        }

        // The completion list gets the navigation keys while it is open, except
        // the `Shift`-held ones: those extend the prompt's selection.
        let selecting = shift
            && matches!(
                key.code,
                KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Home
                    | KeyCode::End
            );
        let completion_action = match &mut self.completion {
            Some(list) if !ctrl && !alt && !selecting => list.handle_key(key),
            _ => CompletionAction::NotHandled,
        };
        match completion_action {
            CompletionAction::Handled => return vec![PanelEvent::NeedsRedraw],
            CompletionAction::Dismiss => {
                self.completion = None;
                return vec![PanelEvent::NeedsRedraw];
            }
            CompletionAction::Accept => {
                // Enter on the command already typed in full sends it; on a
                // partial one, or on Tab, it completes, like a shell.
                let typed = self.input_area().text();
                let exact = self.completion_span.is_none()
                    && key.code == KeyCode::Enter
                    && self
                        .completion
                        .as_ref()
                        .and_then(CompletionList::selected_item)
                        .is_some_and(|item| format!("/{}", item.value) == typed.trim());
                if exact {
                    return self.submit();
                }
                self.accept_completion();
                return vec![PanelEvent::NeedsRedraw];
            }
            CompletionAction::NotHandled => {}
        }

        // Chat focus: the arrows walk the blocks, Space/Enter fold the one
        // under the cursor, Tab or Esc hands focus back to the input.
        if self.chat_focus {
            let count = self.transcript.items().len();
            match key.code {
                KeyCode::Tab | KeyCode::Esc => {
                    self.chat_focus = false;
                    return vec![PanelEvent::NeedsRedraw];
                }
                KeyCode::Up if !ctrl => {
                    if let Some(prev) = (0..self.selected)
                        .rev()
                        .find(|&i| self.transcript.is_selectable(i))
                    {
                        self.selected = prev;
                    }
                    self.follow = false;
                    self.scroll_selected_into_view();
                    return vec![PanelEvent::NeedsRedraw];
                }
                KeyCode::Down if !ctrl => {
                    if let Some(next) =
                        (self.selected + 1..count).find(|&i| self.transcript.is_selectable(i))
                    {
                        self.selected = next;
                    }
                    self.follow = false;
                    self.scroll_selected_into_view();
                    return vec![PanelEvent::NeedsRedraw];
                }
                KeyCode::Char(' ') | KeyCode::Enter => {
                    self.transcript.toggle_expanded(self.selected);
                    return vec![PanelEvent::NeedsRedraw];
                }
                KeyCode::Char('o') if !ctrl => {
                    return self.open_selected_in_panel();
                }
                KeyCode::Char('o') if ctrl => {
                    let expand = !self.transcript.any_expanded();
                    self.transcript.set_all_expanded(expand);
                    return vec![PanelEvent::NeedsRedraw];
                }
                KeyCode::PageUp => {
                    self.scroll_by(-page);
                    return vec![PanelEvent::NeedsRedraw];
                }
                KeyCode::PageDown => {
                    self.scroll_by(page);
                    return vec![PanelEvent::NeedsRedraw];
                }
                // Everything else is swallowed so it does not type into the
                // (unfocused) input.
                _ => return vec![],
            }
        }
        // From the input, Tab moves focus into the chat when it has a block
        // (annotations alone give the cursor nowhere to stop).
        let last_block = self
            .transcript
            .items()
            .len()
            .checked_sub(1)
            .and_then(|last| self.transcript.selectable_near(last));
        if let (KeyCode::Tab, Some(last_block)) = (key.code, last_block) {
            self.chat_focus = true;
            self.follow = false;
            self.selected = last_block;
            return vec![PanelEvent::NeedsRedraw];
        }

        match key.code {
            KeyCode::Esc => {
                if self.is_busy() {
                    self.abort();
                } else if self.goal_task.take().is_some() {
                    self.notice(
                        termide_i18n::t().agent_notice_goal_stopped(),
                        NoticeKind::Info,
                    );
                } else if self.loop_task.take().is_some() {
                    self.notice(
                        termide_i18n::t().agent_notice_loop_stopped(),
                        NoticeKind::Info,
                    );
                } else if !self.input_area().is_empty() {
                    self.clear_input();
                    self.after_edit();
                } else {
                    return vec![];
                }
            }
            KeyCode::Enter if shift || alt => {
                self.input_area_mut().insert_newline();
                self.after_edit();
            }
            KeyCode::Char('j') if ctrl => {
                self.input_area_mut().insert_newline();
                self.after_edit();
            }
            KeyCode::Enter => return self.submit(),
            KeyCode::Char('o') if ctrl => {
                let expand = !self.transcript.any_expanded();
                self.transcript.set_all_expanded(expand);
            }
            KeyCode::BackTab if !self.external => {
                let next = self.mode.get().next();
                return vec![self.set_mode(next), PanelEvent::NeedsRedraw];
            }
            KeyCode::PageUp => self.scroll_by(-page),
            KeyCode::PageDown => self.scroll_by(page),
            KeyCode::Home if ctrl => {
                self.top = 0;
                self.follow = false;
            }
            KeyCode::End if ctrl => self.follow = true,
            KeyCode::Up if ctrl => self.scroll_by(-1),
            KeyCode::Down if ctrl => self.scroll_by(1),
            KeyCode::Up | KeyCode::Down => {
                // Past the first or last line, `↑` first takes back what is
                // still queued, then the arrows walk through what was asked
                // before, as in a shell; with Shift held the selection takes
                // the arrow and history waits.
                let up = key.code == KeyCode::Up;
                let handled = self.input.edit_field(0, key) != FieldEdit::NotHandled
                    || shift
                    || (up && self.unqueue())
                    || self.recall(up);
                if !handled {
                    return vec![];
                }
            }
            // Prompt clipboard: the panel owns these so a large paste keeps its
            // placeholder handling and a failure can raise a notice.
            KeyCode::Char('c') if ctrl => {
                if !self.copy_input_selection() && !self.copy_text_selection() {
                    return vec![];
                }
            }
            KeyCode::Char('x') if ctrl => {
                if !self.cut_input_selection() {
                    return vec![];
                }
                self.after_edit();
            }
            KeyCode::Char('v') if ctrl => {
                if !self.paste_clipboard() {
                    return vec![];
                }
                self.after_edit();
            }
            // Everything the prompt edits with: typing, deletion, character and
            // word navigation and selection.
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Char(_)
                if !alt =>
            {
                let edit = self.input.edit_field(0, key);
                if edit == FieldEdit::NotHandled {
                    return vec![];
                }
                if edit == FieldEdit::Edited {
                    self.after_edit();
                }
            }
            _ => return vec![],
        }
        vec![PanelEvent::NeedsRedraw]
    }

    fn captures_escape(&self) -> bool {
        self.pending.is_some()
            || self.completion.is_some()
            || self.is_busy()
            || self.loop_task.is_some()
            || self.goal_task.is_some()
            || !self.input_area().is_empty()
    }

    fn handle_scroll(&mut self, delta: i32, _panel_area: Rect) -> Vec<PanelEvent> {
        self.scroll_by(delta);
        vec![PanelEvent::NeedsRedraw]
    }

    fn handle_mouse(&mut self, event: MouseEvent, _panel_area: Rect) -> Vec<PanelEvent> {
        // The prompt box claims its own presses and drags: a press places the
        // cursor, a drag selects the text under it. It is asked first because
        // the bar sits below the transcript, whose rows would otherwise take
        // every click, and because a release must reach the bar to end a drag
        // that started in it — even after the pointer has been dragged up into
        // the transcript. A pending question keeps its clicks to itself.
        // A run control on the prompt's border acts on the press.
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            let button = self
                .input
                .border_button_at(event.column, event.row)
                .and_then(|index| self.run_buttons.get(index).copied());
            if let Some(button) = button {
                match button {
                    RunButton::Pause => {
                        self.request_pause();
                    }
                    RunButton::Continue if self.paused && !self.is_busy() => self.resume(),
                    RunButton::Continue => self.cancel_pause(),
                    RunButton::Stop if self.paused && !self.is_busy() => self.stop_paused(),
                    RunButton::Stop => self.abort(),
                }
                return vec![PanelEvent::NeedsRedraw];
            }
        }
        if self.pending.is_none() && self.press.is_none() && self.input.mouse_hits(event) {
            self.input.handle_mouse(event);
            match event.kind {
                MouseEventKind::Up(_) => return vec![],
                _ => {
                    self.chat_focus = false;
                    return vec![PanelEvent::NeedsRedraw];
                }
            }
        }
        match event.kind {
            MouseEventKind::ScrollDown => self.scroll_by(3),
            MouseEventKind::ScrollUp => self.scroll_by(-3),
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(anchor) = self.press else {
                    return vec![];
                };
                // Dragged past an edge, the transcript scrolls under it.
                let area = self.transcript_area;
                if event.row < area.y {
                    self.scroll_by(-1);
                } else if event.row >= area.y + area.height {
                    self.scroll_by(1);
                }
                let head = self.cell_at(event.column, event.row);
                self.text_selection = Some(select::TextSelection { anchor, head });
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(press) = self.press.take() else {
                    return vec![];
                };
                if self.text_selection.is_some_and(|sel| !sel.is_empty()) {
                    return vec![PanelEvent::NeedsRedraw];
                }
                self.text_selection = None;
                return self.click_line(press.line);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.pending.is_some() {
                    // A click on a row selects it, and only a second click (a
                    // double click) on the same row confirms — so a misplaced
                    // click cannot answer. A click on the detail folds it.
                    let hit = self
                        .pending
                        .as_ref()
                        .unwrap()
                        .form()
                        .hit(event.column, event.row);
                    if let Some(index) = hit {
                        if self.form_clicks.click(index) >= 2 {
                            self.form_clicks.reset();
                            let action =
                                self.pending.as_mut().unwrap().form_mut().activate_at(index);
                            if self.apply_form_action(action) {
                                return vec![PanelEvent::NeedsRedraw];
                            }
                        } else {
                            self.pending.as_mut().unwrap().form_mut().select(index);
                        }
                        return vec![PanelEvent::NeedsRedraw];
                    }
                    if self
                        .pending
                        .as_mut()
                        .unwrap()
                        .form_mut()
                        .click_select(event.column, event.row)
                    {
                        self.form_clicks.reset();
                        return vec![PanelEvent::NeedsRedraw];
                    }
                }
                if let Some(list) = &mut self.completion {
                    if let Some(index) = list.hit(event.column, event.row) {
                        list.select(index);
                        self.accept_completion();
                        return vec![PanelEvent::NeedsRedraw];
                    }
                }
                // A click on a re-pickable field in the welcome banner opens its
                // picker — the same one its status-bar chip opens.
                let banner_hit = self
                    .banner_hits
                    .iter()
                    .find(|(rect, _)| {
                        event.column >= rect.x
                            && event.column < rect.x + rect.width
                            && event.row == rect.y
                    })
                    .map(|(_, action)| *action);
                if let Some(action) = banner_hit {
                    return self.handle_status_action(action);
                }
                let area = self.transcript_area;
                let inside = event.column >= area.x
                    && event.column < area.x + area.width
                    && event.row >= area.y
                    && event.row < area.y + area.height;
                // The state strip's pending-pause line withdraws the pause.
                if self.pause_requested && self.pause_row == Some(event.row) {
                    self.cancel_pause();
                    return vec![PanelEvent::NeedsRedraw];
                }
                if !inside {
                    // A click below the transcript lands on the input: hand focus
                    // back to it so typing resumes.
                    if self.chat_focus {
                        self.chat_focus = false;
                        return vec![PanelEvent::NeedsRedraw];
                    }
                    return vec![];
                }
                // The press may start a text selection; only a release
                // without a drag clicks the block under it.
                self.press = Some(self.cell_at(event.column, event.row));
                self.text_selection = None;
            }
            _ => return vec![],
        }
        vec![PanelEvent::NeedsRedraw]
    }

    fn tick(&mut self) -> Vec<PanelEvent> {
        let mut changed = false;
        // A pause's line ticks its length, redrawn once a second; so does a
        // call's wait on a permission question, until the question is gone.
        if let Some(start) = self.pause_start {
            changed |= self.transcript.set_pause_length(millis(start.elapsed()));
        }
        if self.context_stale && !self.is_busy() {
            self.refresh_context();
            changed = true;
        }
        if let Some((start, before)) = self.permission_wait {
            if matches!(self.pending, Some(Pending::Permission { .. })) {
                changed |= self
                    .transcript
                    .set_tool_wait(before.saturating_add(millis(start.elapsed())), true);
            } else {
                self.end_permission_wait();
                changed = true;
            }
        }
        for event in self.runtime.drain() {
            self.apply(event);
            changed = true;
        }
        let mut events = self.poll_permissions();
        events.append(&mut self.pending_events);
        changed |= self.poll_late_tools();
        changed |= self.poll_command();
        let fetched = self.model_fetch.as_ref().map(Receiver::try_recv);
        match fetched {
            Some(Ok(result)) => {
                self.model_fetch = None;
                events.push(self.model_picker(result));
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.model_fetch = None;
                events.push(self.model_picker(Err("the request was dropped".to_string())));
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
        // The silent context-window probe: adopt the active model's real
        // window when it arrives, and stay quiet on failure.
        match self.context_probe.as_ref().map(Receiver::try_recv) {
            Some(Ok(Ok(models))) => {
                self.context_probe = None;
                changed |= self.adopt_listed_models(&models);
            }
            Some(Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected)) => {
                self.context_probe = None;
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
        // An external agent's model becomes known once its handshake finishes
        // (its adapter starts asynchronously): adopt the current model for the
        // banner and the Model chip, and note whether it offers a choice.
        if self.external {
            if !self.acp_has_models && !self.runtime.available_models().is_empty() {
                self.acp_has_models = true;
                changed = true;
                // Apply the configured pre-selected model once, now that the
                // agent's models are known.
                if let Some(pref) = self.pending_preferred_model.take() {
                    if self.runtime.current_model().as_deref() != Some(pref.as_str()) {
                        match self.runtime.select_model(pref.clone()) {
                            Ok(()) => self.model.id = pref,
                            Err(error) => {
                                log::warn!("cannot pre-select the model: {error}");
                            }
                        }
                    }
                }
            }
            if let Some(id) = self.runtime.current_model() {
                if id != self.model.id {
                    self.model.id = id;
                    changed = true;
                }
            }
        }
        // A loop whose wait has elapsed starts its next iteration once the
        // panel is free (no run in flight, no card waiting for an answer).
        let due = self
            .loop_task
            .as_ref()
            .and_then(|t| t.next_at)
            .is_some_and(|at| at <= Instant::now());
        if due && !self.is_busy() && self.pending.is_none() {
            events.extend(self.loop_step());
            changed = true;
        }
        // A goal whose work turn has finished runs the judge once the panel is
        // free and no judge call is already in flight.
        let judge_due = self
            .goal_task
            .as_ref()
            .is_some_and(|t| !t.judging && t.judge_at.is_some_and(|at| at <= Instant::now()));
        if judge_due && !self.is_busy() && self.pending.is_none() {
            events.extend(self.run_goal_judge());
            changed = true;
        }
        // While the agent works, keep the ticking timer and the block's
        // spinner moving without waiting for an event (throttled to ~10 fps).
        if self.is_busy() && self.last_anim.elapsed() >= Duration::from_millis(100) {
            self.last_anim = Instant::now();
            changed = true;
        }
        if changed || !events.is_empty() {
            events.push(PanelEvent::NeedsRedraw);
        }
        events
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
            PanelCommand::ChecklistDone { action, checked } if action == TOOLSET_ACTION => {
                self.apply_toolset(&checked);
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
        // Separators are the panel's job: the status bar concatenates the
        // segments as given. The knobs sit on the left, the figures flush
        // right; a narrow bar cuts the knobs, never the figures. The live
        // phase is not repeated here: each chat block carries its own byline.
        let sep = || StatusSegment::new(" │ ", SegmentKind::Label);
        let mut segments = vec![
            StatusSegment::new(" ", SegmentKind::Label),
            StatusSegment::clickable("Agent: ", SegmentKind::Label, AGENT_ACTION),
            StatusSegment::clickable(self.agent.clone(), SegmentKind::Active, AGENT_ACTION),
        ];
        if self.external {
            // An external agent has its own model and permission model.
            segments.push(StatusSegment::new(" (acp)", SegmentKind::Label));
            // A CLI agent is a connection too: the way back is here.
            if self.connections.is_some() {
                segments.extend([
                    sep(),
                    StatusSegment::clickable("Connection: ", SegmentKind::Label, CONNECTION_ACTION),
                    StatusSegment::clickable(
                        self.connection_display(),
                        SegmentKind::Active,
                        CONNECTION_ACTION,
                    ),
                ]);
            }
        } else {
            segments.extend([
                sep(),
                StatusSegment::clickable("Mode: ", SegmentKind::Label, MODE_ACTION),
                StatusSegment::clickable(self.mode.get().label(), SegmentKind::Active, MODE_ACTION),
                sep(),
                StatusSegment::clickable("Reasoning: ", SegmentKind::Label, REASONING_ACTION),
                StatusSegment::clickable(
                    if self.model.reasoning { "on" } else { "off" },
                    SegmentKind::Active,
                    REASONING_ACTION,
                ),
                sep(),
                StatusSegment::clickable("Tools: ", SegmentKind::Label, TOOLSET_ACTION),
                StatusSegment::clickable(
                    {
                        let (on, all) = self.toolset_counts();
                        format!("{on}/{all}")
                    },
                    SegmentKind::Active,
                    TOOLSET_ACTION,
                ),
                sep(),
                StatusSegment::clickable("Connection: ", SegmentKind::Label, CONNECTION_ACTION),
                StatusSegment::clickable(
                    self.connection_display(),
                    SegmentKind::Active,
                    CONNECTION_ACTION,
                ),
            ]);
        }
        // The agent's model over ACP, when it advertised any: clickable to
        // switch, like the built-in loop's Model chip.
        if !self.external || self.acp_has_models {
            segments.extend([
                sep(),
                StatusSegment::clickable("Model: ", SegmentKind::Label, MODEL_ACTION),
                StatusSegment::clickable(self.model_display(), SegmentKind::Active, MODEL_ACTION),
            ]);
        }
        segments.push(StatusSegment::spacer());
        let queued = self.queued.0 + self.queued.1;
        if queued > 0 {
            segments.push(StatusSegment::new(
                format!("{queued} queued "),
                SegmentKind::Inactive,
            ));
        }
        // Session token totals: ↑ input (prefill), ↓ output (generated).
        if !self.external && (self.session_input > 0 || self.session_output > 0) {
            segments.push(StatusSegment::new(
                format!(
                    "↑{} ↓{} ",
                    format_tokens(self.session_input),
                    format_tokens(self.session_output)
                ),
                SegmentKind::Value,
            ));
        }
        if !self.external && self.model.context_window > 0 {
            let percent = ((self.context_tokens * 100) / self.model.context_window).min(100);
            let kind = if percent >= 80 {
                SegmentKind::Warn
            } else {
                SegmentKind::Value
            };
            segments.push(StatusSegment::new(
                format!(
                    "{}/{} {} ",
                    format_tokens(self.context_tokens),
                    format_tokens(self.model.context_window),
                    context_bar(percent)
                ),
                kind,
            ));
        }
        segments
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
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use termide_agent_core::PermissionPrompter;
    use termide_agent_core::{
        AssistantContent, AssistantMessage, Request, StopReason, ToolCall, Usage,
    };
    use termide_core::PanelConfig;

    /// Replays one scripted assistant message per model call and records
    /// which model each call asked for.
    struct Scripted {
        replies: Mutex<Vec<AssistantMessage>>,
        models: Result<Vec<ModelInfo>, String>,
        seen_models: Mutex<Vec<String>>,
    }

    impl Scripted {
        fn new(replies: Vec<AssistantMessage>) -> Self {
            Self {
                replies: Mutex::new(replies),
                models: Ok(vec![
                    ModelInfo {
                        id: "big".into(),
                        context_window: Some(64_000),
                    },
                    ModelInfo {
                        id: "m".into(),
                        context_window: None,
                    },
                ]),
                seen_models: Mutex::new(Vec::new()),
            }
        }
    }

    impl Provider for Scripted {
        fn name(&self) -> &str {
            "scripted"
        }
        fn stream(
            &self,
            request: &Request<'_>,
            on_event: &mut dyn FnMut(StreamEvent),
            _cancel: &CancelToken,
        ) -> AssistantMessage {
            self.seen_models
                .lock()
                .unwrap()
                .push(request.model.id.clone());
            let mut replies = self.replies.lock().unwrap();
            if replies.is_empty() {
                return AssistantMessage::failed("scripted", "m", StopReason::Error, "exhausted");
            }
            let reply = replies.remove(0);
            let thinking = reply.thinking_text();
            if !thinking.is_empty() {
                on_event(StreamEvent::ThinkingDelta(thinking));
            }
            on_event(StreamEvent::TextDelta(reply.plain_text()));
            reply
        }
        fn list_models(&self) -> Result<Vec<ModelInfo>, String> {
            self.models.clone()
        }
    }

    fn reply_thinking(text: &str, thinking: &str) -> AssistantMessage {
        let mut message = reply(text);
        message.content.insert(
            0,
            AssistantContent::Thinking {
                text: thinking.into(),
            },
        );
        message
    }

    fn reply(text: &str) -> AssistantMessage {
        AssistantMessage {
            content: vec![AssistantContent::Text { text: text.into() }],
            stop_reason: StopReason::Stop,
            usage: Usage {
                input: 100,
                output: 20,
                cache_read: 0,
                cache_write: 0,
            },
            provider: "scripted".into(),
            model: "m".into(),
            error_message: None,
            timestamp: 0,
        }
    }

    fn panel(replies: Vec<AssistantMessage>) -> AgentPanel {
        AgentPanel::new(setup(replies))
    }

    fn setup(replies: Vec<AssistantMessage>) -> AgentPanelSetup {
        setup_with(Arc::new(Scripted::new(replies)))
    }

    /// Command scripts the test catalog offers; tests fill it.
    static COMMANDS: Mutex<Vec<CommandScript>> = Mutex::new(Vec::new());

    /// Two agents: the default one and a terse reviewer on another model.
    struct Agents;

    impl AgentCatalog for Agents {
        fn list(&self) -> Vec<AgentEntry> {
            vec![
                AgentEntry {
                    name: "default".into(),
                    description: String::new(),
                },
                AgentEntry {
                    name: "review".into(),
                    description: "Reviews diffs".into(),
                },
                AgentEntry {
                    name: "outside".into(),
                    description: "An external agent".into(),
                },
            ]
        }
        fn prompts(&self) -> Vec<PromptTemplate> {
            vec![PromptTemplate {
                name: "review".into(),
                description: "Review a file".into(),
                argument_hint: "<path>".into(),
                body: "Review $1 carefully.".into(),
            }]
        }
        fn commands(&self) -> Vec<CommandScript> {
            COMMANDS.lock().unwrap().clone()
        }
        fn resolve(&self, name: &str) -> Option<AgentProfile> {
            match name {
                "default" => Some(AgentProfile {
                    system_prompt: "default prompt".into(),
                    tools: ToolRegistry::new(),
                    model: None,
                    mode: None,
                    late_tools: None,
                    backend: None,
                    offered: Vec::new(),
                    skills: Vec::new(),
                }),
                "review" => Some(AgentProfile {
                    system_prompt: "You review diffs.".into(),
                    tools: ToolRegistry::new(),
                    model: Some("big".into()),
                    mode: Some(Mode::Edit),
                    late_tools: None,
                    backend: None,
                    offered: Vec::new(),
                    skills: Vec::new(),
                }),
                "outside" => Some(AgentProfile {
                    system_prompt: String::new(),
                    tools: ToolRegistry::new(),
                    model: None,
                    mode: None,
                    late_tools: None,
                    backend: Some(Arc::new(|setup: BackendSetup| {
                        Ok(Box::new(External::new(setup)) as Box<dyn Backend>)
                    })),
                    offered: Vec::new(),
                    skills: Vec::new(),
                }),
                _ => None,
            }
        }
    }

    fn setup_with(provider: Arc<Scripted>) -> AgentPanelSetup {
        AgentPanelSetup {
            cwd: PathBuf::from("/tmp"),
            agent: "default".into(),
            catalog: Arc::new(Agents),
            late_tools: None,
            hooks: None,
            backend: None,
            provider_backend: None,
            connections: None,
            connection: "local".into(),
            provider,
            provider_kind: "openai_compatible".into(),
            model: ModelSpec {
                provider: "scripted".into(),
                id: "m".into(),
                context_window: 1000,
                max_tokens: Some(100),
                reasoning: false,
            },
            tools: ToolRegistry::new(),
            rules: PermissionRules::default(),
            system_prompt: String::new(),
            compaction: CompactionPolicy::default(),
            compaction_prompts: CompactionPrompts::default(),
            plan_prompt: PlanPrompt::default(),
            goal_prompt: GoalPrompt::default(),
            handoff_prompt: HandoffPrompt::default(),
            persist_rule: None,
            session_dir: None,
            session: None,
            fold: FoldMode::OnFinish,
        }
    }

    fn chord(code: KeyCode, modifiers: KeyModifiers) -> KeyChord {
        let event = KeyEvent::new(code, modifiers);
        KeyChord {
            raw: event,
            canonical: event,
        }
    }

    fn strip_text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn the_state_strip_shows_queued_messages_until_the_agent_takes_them() {
        let mut panel = AgentPanel::new(setup(vec![]));
        assert!(panel.state_lines(40).is_empty());
        panel.apply(AgentEvent::AgentStart);
        for text in ["first\nsecond line", "two", "three", "four", "five"] {
            panel.send(text.to_string());
        }
        // Queued while busy: nothing in the transcript, all of it in the strip.
        assert!(panel
            .transcript()
            .items()
            .iter()
            .all(|i| !matches!(i, Item::Notice { .. } | Item::User { .. })));
        let lines = strip_text(&panel.state_lines(40));
        assert!(lines[0].starts_with('╌'));
        assert!(lines[1].starts_with("› first") && lines[1].ends_with("queued"));
        assert!(!lines[1].contains("second line"));
        assert_eq!(lines[2], "› two");
        assert!(lines[4].contains("2 more queued"), "{lines:?}");
        assert!(lines.iter().all(|l| termide_ui::str_display_width(l) <= 40));
        // The agent takes the two oldest; the strip follows its count.
        panel.apply(AgentEvent::QueueUpdate {
            steering: 3,
            follow_up: 0,
        });
        let lines = strip_text(&panel.state_lines(40));
        assert!(lines[1].starts_with("› three"), "{lines:?}");
        assert_eq!(lines.len(), 4);
        panel.apply(AgentEvent::QueueUpdate {
            steering: 0,
            follow_up: 0,
        });
        assert!(panel.state_lines(40).is_empty());
    }

    #[test]
    fn the_state_strip_sits_between_the_transcript_and_the_input() {
        let mut panel = AgentPanel::new(setup(vec![]));
        panel.apply(AgentEvent::AgentStart);
        panel.send("waiting its turn".to_string());
        let rows = render_text(&mut panel, 40, 12);
        let at = rows
            .iter()
            .position(|r| r.starts_with("› waiting its turn"))
            .expect("queued row");
        assert!(rows[at - 1].starts_with('╌'), "{rows:?}");
        // The input bar's titled border follows the strip directly.
        assert!(
            !rows[at + 1].trim().is_empty() && !rows[at + 1].starts_with('›'),
            "{rows:?}"
        );
        assert_eq!(at, rows.len() - 1 - panel.input_area.height as usize);
    }

    #[test]
    fn a_pending_pause_lives_in_the_state_strip_and_a_pause_in_the_transcript() {
        let mut panel = AgentPanel::new(setup(vec![]));
        panel.apply(AgentEvent::AgentStart);
        type_text(&mut panel, "/pause");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        let t = termide_i18n::t();
        let lines = strip_text(&panel.state_lines(60));
        assert!(lines[1].contains(t.agent_notice_will_pause()), "{lines:?}");
        panel.apply(AgentEvent::Paused);
        panel.apply(AgentEvent::AgentEnd);
        // Once the pause takes effect the strip clears: the transcript's `‖`
        // line stands for it.
        assert!(panel.state_lines(80).is_empty());
        // History keeps only the event: the run's closing line, marked paused.
        let items = panel.transcript().items();
        assert!(items.iter().all(|i| !matches!(i, Item::Notice { .. })));
        assert!(matches!(
            items.last(),
            Some(Item::RunEnd {
                paused: true,
                ok: true,
                ..
            })
        ));
    }

    #[test]
    fn a_pause_ticks_its_own_length_and_the_run_clock_resumes_from_the_request() {
        let mut panel = AgentPanel::new(setup(vec![]));
        panel.apply(AgentEvent::AgentStart);
        let start = panel.run_start.expect("the run started");
        panel.apply(AgentEvent::Paused);
        panel.apply(AgentEvent::AgentEnd);
        // Paused: the run keeps its start, and the pause's line counts the
        // pause itself.
        assert_eq!(panel.run_start, Some(start));
        panel.pause_start = Some(Instant::now() - Duration::from_secs(3));
        panel.tick();
        let lines = strip_text(panel.transcript.lines(40, &panel.colors, false));
        // The duration alone, no time of day.
        assert_eq!(lines.last().map(|l| l.trim()), Some("‖ 3s"));
        // Resumed: the pause's length stays on its line, the run's clock goes
        // on from the request.
        panel.paused = false;
        panel.resuming = true;
        panel.apply(AgentEvent::AgentStart);
        assert_eq!(panel.run_start, Some(start));
        assert!(panel.pause_start.is_none());
        assert!(matches!(
            panel.transcript.items().last(),
            Some(Item::RunEnd { paused: true, elapsed_ms, .. }) if *elapsed_ms >= 3000
        ));
        // A fresh run (not a resume) starts its own clock.
        panel.apply(AgentEvent::AgentEnd);
        panel.apply(AgentEvent::AgentStart);
        assert_ne!(panel.run_start, Some(start));
    }

    #[test]
    fn the_prompt_border_carries_the_run_controls() {
        let mut panel = AgentPanel::new(setup(vec![]));
        let (width, height) = (40, 12);
        // The controls on the border, as text, and a click on one by glyph.
        let border = |panel: &mut AgentPanel| {
            let rows = render_text(panel, width, height);
            rows[panel.input_area.y as usize].clone()
        };
        let click = |panel: &mut AgentPanel, glyph: &str| {
            let row = border(panel);
            let col = row[..row.find(glyph).expect("the control is on the border")]
                .chars()
                .count() as u16;
            panel.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: col,
                    row: panel.input_area.y,
                    modifiers: KeyModifiers::NONE,
                },
                Rect::new(0, 0, width, height),
            );
        };
        // Idle: no controls.
        assert!(!border(&mut panel).contains('['));
        // Working: pause and stop.
        panel.apply(AgentEvent::AgentStart);
        let row = border(&mut panel);
        assert!(row.ends_with("[‖][■]─"), "{row:?}");
        click(&mut panel, "[‖]");
        assert!(panel.pause_requested, "the pause control asks to pause");
        // A pause asked for: continue withdraws it.
        assert!(border(&mut panel).contains("[▶][■]"));
        click(&mut panel, "[▶]");
        assert!(
            !panel.pause_requested,
            "continue withdraws the pending pause"
        );
        // The strip's pending-pause line withdraws it too.
        click(&mut panel, "[‖]");
        let _ = render_text(&mut panel, width, height);
        let row = panel.pause_row.expect("the strip shows the pending pause");
        panel.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 2,
                row,
                modifiers: KeyModifiers::NONE,
            },
            Rect::new(0, 0, width, height),
        );
        assert!(!panel.pause_requested);
        // `/continue` withdraws a pending pause as well.
        click(&mut panel, "[‖]");
        type_text(&mut panel, "/continue");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            !panel.pause_requested,
            "/continue withdraws the pending pause"
        );
        // Paused: continue alone, and the pause's line resumes too.
        click(&mut panel, "[‖]");
        panel.apply(AgentEvent::Paused);
        panel.apply(AgentEvent::AgentEnd);
        assert!(border(&mut panel).contains("[▶][■]"));
        let line = panel.transcript.line_count() - 1;
        assert!(panel.transcript.is_live_pause_line(line));
        // Stop while paused gives the run up: no controls, the pause rests.
        click(&mut panel, "[■]");
        assert!(!panel.paused);
        assert!(panel.pause_start.is_none());
        assert!(!border(&mut panel).contains('['));
        assert!(!panel.transcript.is_live_pause_line(line));
    }

    fn type_text(panel: &mut AgentPanel, text: &str) {
        for c in text.chars() {
            panel.handle_key(chord(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    fn settle(panel: &mut AgentPanel) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            panel.tick();
            if !panel.is_busy() {
                return;
            }
            assert!(Instant::now() < deadline, "agent did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Tick until an event matching `wanted` arrives, returning it.
    fn wait_for(panel: &mut AgentPanel, wanted: fn(&PanelEvent) -> bool) -> PanelEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(event) = panel.tick().into_iter().find(&wanted) {
                return event;
            }
            assert!(Instant::now() < deadline, "event did not arrive");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Text of the bold chip that carries `action`.
    fn chip(panel: &AgentPanel, action: &str) -> String {
        panel
            .status_segments()
            .into_iter()
            .find(|s| s.action == Some(action) && s.kind == SegmentKind::Active)
            .map(|s| s.text)
            .expect("chip present")
    }

    fn select(panel: &mut AgentPanel, event: &PanelEvent, index: usize) -> CommandResult {
        let PanelEvent::ShowSelect {
            on_select: SelectAction::Custom(action),
            ..
        } = event
        else {
            panic!("expected a picker, got {event:?}");
        };
        panel.handle_command(PanelCommand::SelectionMade {
            action: action.clone(),
            index,
        })
    }

    fn render_text(panel: &mut AgentPanel, width: u16, height: u16) -> Vec<String> {
        let buf = render_buf(panel, width, height);
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn shortcuts_work_on_a_cyrillic_layout() {
        // As the dispatcher builds it: the raw Cyrillic key, and its Latin
        // canonical form.
        let key = |c, latin, modifiers| KeyChord {
            raw: KeyEvent::new(KeyCode::Char(c), modifiers),
            canonical: KeyEvent::new(KeyCode::Char(latin), modifiers),
        };
        let mut panel = panel(vec![]);
        let long = (1..=8)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let call = ToolCall {
            id: "t1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({ "command": "ls" }),
        };
        panel.transcript.push(Item::Tool {
            call: call.clone(),
            result: Some(ToolResultMessage::text(&call, long)),
            live: None,
            at: "12:00:00".into(),
            duration_ms: Some(10),
            waited_ms: None,
            waiting: false,
        });
        // `Ctrl+щ` is `Ctrl+O`: unfold everything.
        assert!(!panel.transcript.any_expanded());
        panel.handle_key(key('щ', 'o', KeyModifiers::CONTROL));
        assert!(panel.transcript.any_expanded());
        // Typed into the input, the same letter stays Cyrillic.
        panel.handle_key(key('щ', 'o', KeyModifiers::NONE));
        assert_eq!(panel.input_text(), "щ");
    }

    #[test]
    fn a_drag_selects_transcript_text_for_copy() {
        let mut panel = panel(vec![reply("Hello from the model")]);
        type_text(&mut panel, "hi there");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        let rows = render_text(&mut panel, 40, 12);
        let area = panel.transcript_area;
        let y = rows
            .iter()
            .position(|r| r.contains("Hello from"))
            .expect("the answer is on screen") as u16;
        let row = &rows[y as usize];
        let x = row[..row.find("Hello").unwrap()].chars().count() as u16;
        let mouse = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        panel.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y), area);
        panel.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), x + 9, y),
            area,
        );
        panel.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x + 9, y), area);
        // A drag is a selection, not a click: the chat keeps its focus state.
        assert!(!panel.chat_focus);
        let width = panel.transcript_area.width.saturating_sub(1) as usize;
        let selection = panel.text_selection.expect("a selection");
        assert_eq!(
            selection.text(panel.transcript.rendered(), width),
            "Hello from"
        );
        // It shows in the selection colours.
        let buf = render_buf(&mut panel, 40, 12);
        assert_eq!(buf[(x, y)].bg, ThemeColors::default().selection_bg);
        // A plain click clears it.
        panel.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y), area);
        panel.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x, y), area);
        assert!(panel.text_selection.is_none());
    }

    #[test]
    fn the_input_grows_to_half_the_panel() {
        let mut panel = panel(vec![]);
        let text = (1..=12)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        panel.input_area_mut().insert_str(&text);
        // A 40-row panel lets the prompt take 20 rows; 12 lines fit in full.
        assert_eq!(panel.input_rows(40, 60), 12);
        // A 16-row one stops it at 8, and the prompt scrolls inside.
        assert_eq!(panel.input_rows(16, 60), 8);
    }

    #[test]
    fn pasting_the_same_block_twice_unmasks_it() {
        let mut panel = panel(vec![]);
        let block = "1\n2\n3\n4\n5\n6";
        panel.paste(block);
        assert_eq!(panel.input_text(), "[#1 pasted 6 lines]");
        // The same text again: the placeholder turns into the text itself.
        panel.paste(block);
        assert_eq!(panel.input_text(), block);
        assert!(panel.pastes.is_empty());
        // Another block is masked as before.
        panel.paste("a\nb\nc\nd\ne\nf");
        assert!(panel.input_text().ends_with("[#2 pasted 6 lines]"));
    }

    #[test]
    fn a_paste_of_more_than_five_lines_is_masked() {
        let mut panel = panel(vec![]);
        panel.paste("a\nb\nc\nd\ne");
        assert_eq!(
            panel.input_text(),
            "a\nb\nc\nd\ne",
            "five lines stay inline"
        );
        let mut panel = self::panel(vec![]);
        panel.paste("1\n2\n3\n4\n5\n6");
        assert_eq!(panel.input_text(), "[#1 pasted 6 lines]");
    }

    #[test]
    fn a_selected_diff_keeps_its_colors() {
        let mut panel = panel(vec![]);
        let edit = |args| ToolCall {
            id: "e1".into(),
            name: "edit".into(),
            arguments: args,
        };
        let call = edit(serde_json::json!({ "path": "a.rs" }));
        panel.transcript.push(Item::Tool {
            call: call.clone(),
            result: Some(ToolResultMessage::text(
                &call,
                "Edited a.rs.\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new",
            )),
            live: None,
            at: "12:00:00".into(),
            duration_ms: Some(100),
            waited_ms: None,
            waiting: false,
        });
        assert!(panel.transcript.toggle_expanded(0));
        panel.chat_focus = true;
        panel.selected = 0;
        let (width, height) = (40, 16);
        let buf = render_buf(&mut panel, width, height);
        let colors = ThemeColors::default();
        let row_of = |needle: &str| {
            (0..height)
                .find(|&y| {
                    (0..width)
                        .map(|x| buf[(x, y)].symbol())
                        .collect::<String>()
                        .contains(needle)
                })
                .unwrap_or_else(|| panic!("no row {needle:?}"))
        };
        // Selected, the diff rows invert onto their own hue, the rest onto
        // the plain selection.
        assert_eq!(buf[(4, row_of("+new"))].bg, colors.success);
        assert_eq!(buf[(4, row_of("-old"))].bg, colors.error);
        assert_eq!(buf[(4, row_of("a.rs"))].bg, colors.fg);
    }

    fn render_buf(panel: &mut AgentPanel, width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let colors = ThemeColors::default();
        let config = PanelConfig {
            tab_size: 4,
            word_wrap: false,
            show_line_numbers: false,
            show_hidden_files: false,
        };
        let ctx = RenderContext {
            theme: &colors,
            config: &config,
            is_focused: true,
            panel_index: 0,
            terminal_width: width,
            terminal_height: height,
            border_right_x: None,
            border_bottom_y: None,
        };
        panel.render(area, &mut buf, &ctx);
        buf
    }

    #[test]
    fn typing_enter_runs_a_turn_and_renders_it() {
        let mut panel = panel(vec![reply("Hello from the model")]);
        type_text(&mut panel, "hi there");
        assert_eq!(panel.input_text(), "hi there");
        assert!(panel.captures_escape(), "non-empty input keeps Esc");

        let events = panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        assert!(events.iter().any(|e| matches!(e, PanelEvent::NeedsRedraw)));
        assert!(panel.input_text().is_empty());
        settle(&mut panel);

        let items = panel.transcript().items();
        assert!(matches!(&items[0], Item::User { text, .. } if text == "hi there"));
        assert!(items.iter().any(|item| matches!(
            item,
            Item::Assistant { text, streaming: false, .. } if text == "Hello from the model"
        )));
        let rows = render_text(&mut panel, 40, 16);
        assert!(rows.iter().any(|r| r.contains("› hi there")));
        assert!(rows.iter().any(|r| r.contains("Hello from the model")));
        assert!(
            rows.last().unwrap().starts_with("›"),
            "input box at the bottom"
        );
        assert!(
            rows[rows.len() - 2].starts_with("─"),
            "separator above the input"
        );

        // The knobs on the left, the figures flush right after the spacer.
        let segments = panel.status_segments();
        let split = segments
            .iter()
            .position(|s| s.kind == SegmentKind::Spacer)
            .expect("a spacer");
        let text =
            |segs: &[StatusSegment]| segs.iter().map(|s| s.text.as_str()).collect::<String>();
        assert_eq!(
            text(&segments[..split]),
            " Agent: default │ Mode: configured │ Reasoning: off │ Tools: 0/0 │ Connection: local · OpenAI Compatible │ Model: m"
        );
        assert_eq!(text(&segments[split + 1..]), "↑100 ↓20 120/1k ▰▱▱▱▱▱▱▱ ");
    }

    #[test]
    fn title_follows_the_first_prompt() {
        let mut fresh = panel(vec![reply("ok")]);
        // Empty conversation: the working directory.
        assert_eq!(fresh.title(), "Agent: /tmp");

        type_text(&mut fresh, "  make the   timeout configurable  ");
        fresh.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut fresh);
        assert_eq!(fresh.title(), "Agent: make the timeout configurable");

        // A long prompt is cut with an ellipsis.
        let mut wordy = panel(vec![reply("ok")]);
        type_text(&mut wordy, &"word ".repeat(30));
        wordy.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut wordy);
        let title = wordy.title();
        assert!(title.ends_with('…'), "{title}");
        assert_eq!(title.chars().count(), "Agent: ".len() + MAX_TITLE_CHARS);
    }

    #[test]
    fn the_context_window_is_learned_from_the_provider() {
        let mut panel = panel(vec![reply("ok")]);
        // The configured window is only a fallback until the provider is known.
        assert_eq!(panel.model.context_window, 1000);
        // The provider reports the active model's real window; the panel always
        // adopts it, overriding the fallback.
        let models = vec![ModelInfo {
            id: panel.model.id.clone(),
            context_window: Some(48_000),
        }];
        assert!(panel.adopt_listed_models(&models));
        assert_eq!(panel.model.context_window, 48_000);
        // A second identical report is a no-op.
        assert!(!panel.adopt_listed_models(&models));
    }

    #[test]
    fn a_model_left_to_the_provider_is_its_first_listed() {
        let mut base = setup(vec![reply("ok")]);
        base.model.id = String::new();
        let mut panel = AgentPanel::new(base);
        assert_eq!(panel.model_display(), "auto");
        // Before the list arrives there is nothing to send to: the text stays.
        type_text(&mut panel, "hello");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(panel.input_text(), "hello");
        assert!(panel.transcript.items().iter().any(|item| matches!(
            item,
            Item::Notice { text, .. } if text == termide_i18n::t().agent_notice_model_pending()
        )));
        let models = vec![
            ModelInfo {
                id: "first".into(),
                context_window: Some(64_000),
            },
            ModelInfo {
                id: "second".into(),
                context_window: None,
            },
        ];
        assert!(panel.adopt_listed_models(&models));
        assert_eq!(panel.model.id, "first");
        assert_eq!(panel.model.context_window, 64_000);
        // A new session in the panel starts on it too.
        assert_eq!(panel.configured_model.id, "first");
    }

    #[test]
    fn a_model_without_a_reported_window_keeps_the_fallback() {
        // When the provider reports no window for the active model, the
        // configured fallback stays.
        let mut panel = panel(vec![reply("ok")]);
        let models = vec![ModelInfo {
            id: panel.model.id.clone(),
            context_window: None,
        }];
        assert!(!panel.adopt_listed_models(&models));
        assert_eq!(panel.model.context_window, 1000);
    }

    #[test]
    fn an_empty_session_is_discarded_on_close() {
        let dir = tempfile::tempdir().unwrap();
        let path = {
            let panel = AgentPanel::new(AgentPanelSetup {
                session_dir: Some(dir.path().to_path_buf()),
                ..setup(vec![reply("ok")])
            });
            let path = panel.session.as_ref().unwrap().path().to_path_buf();
            assert!(path.exists());
            path
        };
        assert!(!path.exists(), "an unused session is removed on close");
    }

    #[test]
    fn a_session_with_messages_survives_close() {
        let dir = tempfile::tempdir().unwrap();
        let path = {
            let mut panel = AgentPanel::new(AgentPanelSetup {
                session_dir: Some(dir.path().to_path_buf()),
                ..setup(vec![reply("ok")])
            });
            let path = panel.session.as_ref().unwrap().path().to_path_buf();
            type_text(&mut panel, "do something");
            panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
            settle(&mut panel);
            assert!(path.exists());
            path
        };
        assert!(path.exists(), "a session with a conversation is kept");
    }

    #[test]
    fn switching_away_from_an_empty_session_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("ok")])
        });
        let empty = panel.session.as_ref().unwrap().path().to_path_buf();
        assert!(empty.exists());
        // A new session: the empty one we leave is discarded, not listed.
        assert!(panel.switch_session(None));
        assert!(!empty.exists(), "the empty session is removed on switch");
        let fresh = panel.session.as_ref().unwrap().path().to_path_buf();
        assert!(fresh.exists());
        assert_ne!(empty, fresh);
    }

    #[test]
    fn the_context_bar_fills_with_the_percentage() {
        assert_eq!(context_bar(0), "▱▱▱▱▱▱▱▱");
        assert_eq!(context_bar(12), "▰▱▱▱▱▱▱▱");
        assert_eq!(context_bar(50), "▰▰▰▰▱▱▱▱");
        assert_eq!(context_bar(100), "▰▰▰▰▰▰▰▰");
        assert_eq!(context_bar(200), "▰▰▰▰▰▰▰▰");
    }

    #[test]
    fn activity_follows_the_events_and_totals_accumulate() {
        let mut panel = panel(vec![reply("hi")]);
        assert!(panel.activity.is_none());
        panel.apply(AgentEvent::AgentStart);
        panel.apply(AgentEvent::MessageStart);
        assert_eq!(panel.activity.map(|a| a.phase), Some(Phase::Prefill));
        panel.apply(AgentEvent::MessageUpdate(StreamEvent::TextDelta(
            "hello".into(),
        )));
        assert_eq!(panel.activity.map(|a| a.phase), Some(Phase::Generating));
        panel.apply(AgentEvent::MessageEnd(Message::Assistant(reply("hello"))));
        // reply()'s usage is input 100 / output 20.
        assert_eq!((panel.session_input, panel.session_output), (100, 20));
        panel.apply(AgentEvent::ToolExecutionStart {
            call: termide_agent_core::ToolCall {
                id: "1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
            },
        });
        assert_eq!(panel.activity.map(|a| a.phase), Some(Phase::Tool));
        panel.apply(AgentEvent::AgentEnd);
        assert!(panel.activity.is_none());
    }

    #[test]
    fn the_live_footer_shows_generation_meta_and_a_clock() {
        let text_of = |panel: &AgentPanel| -> Vec<String> {
            panel
                .live_footer_lines(40)
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
                .collect()
        };
        let mut panel = panel(vec![]);
        panel.apply(AgentEvent::AgentStart);
        panel.apply(AgentEvent::MessageStart);
        // Prefill: the run clock alone — an animated glyph and the time since
        // the request, with no dividing rule and no generation line yet.
        let lines = text_of(&panel);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            transcript::RUN_FRAMES
                .iter()
                .any(|f| lines[0].trim_start().starts_with(f)),
            "{lines:?}"
        );
        assert!(!lines[0].contains("🕒") && !lines[0].contains('╌'));

        // Once tokens stream, the `✍️` generation line joins the clock. The
        // `⏫` prefill line does not appear live (input tokens are only known
        // at the end).
        panel.apply(AgentEvent::MessageUpdate(StreamEvent::TextDelta(
            "hello there".into(),
        )));
        let lines = text_of(&panel);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains('✍') && lines[0].contains('↓'));
        assert!(lines.iter().all(|l| !l.contains('⏫') && !l.contains('╌')));

        // A tool running after the reply: the clock alone, no generation.
        panel.apply(AgentEvent::ToolExecutionStart {
            call: termide_agent_core::ToolCall {
                id: "t1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({ "command": "sleep 1" }),
            },
        });
        let lines = text_of(&panel);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(!lines[0].contains('✍'), "{lines:?}");
    }

    #[test]
    fn a_tall_focused_block_scrolls_to_its_end() {
        // A reply taller than the viewport.
        let long = (1..=40)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut panel = panel(vec![reply(&long)]);
        type_text(&mut panel, "go");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        // Render short so the answer cannot fit, then focus its block.
        let _ = render_text(&mut panel, 40, 10);
        panel.chat_focus = true;
        panel.selected = panel.transcript().items().len() - 1;

        // Scrolling to the bottom must reach it: the focus keeps the block in
        // view without snapping the viewport back to the block's first line.
        panel.scroll_by(1000);
        let _ = render_text(&mut panel, 40, 10);
        assert!(panel.max_top() > 0, "the block is taller than the viewport");
        assert_eq!(
            panel.top,
            panel.max_top(),
            "a tall focused block still scrolls to its end"
        );
    }

    #[test]
    fn a_selected_block_does_not_trap_scrolling() {
        let long = (1..=40)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut panel = panel(vec![reply(&long)]);
        type_text(&mut panel, "go");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        let _ = render_text(&mut panel, 40, 10);
        // Select the first block (the user message at the very top).
        panel.chat_focus = true;
        panel.selected = 0;
        let _ = render_text(&mut panel, 40, 10);

        // The selection near the top does not stop scrolling to the bottom.
        panel.scroll_by(1000);
        let _ = render_text(&mut panel, 40, 10);
        assert!(
            panel.max_top() > 0,
            "the content is taller than the viewport"
        );
        assert_eq!(panel.top, panel.max_top(), "scrolling down stays free");

        // And scrolling back to the top is equally free.
        panel.scroll_by(-1000);
        let _ = render_text(&mut panel, 40, 10);
        assert_eq!(panel.top, 0, "scrolling up stays free");
    }

    #[test]
    fn arrow_navigation_brings_the_selected_block_into_view() {
        let long = (1..=40)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut panel = panel(vec![reply(&long)]);
        type_text(&mut panel, "go");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        let _ = render_text(&mut panel, 40, 10);
        // Scroll to the top and focus the chat on the first block.
        panel.scroll_by(-1000);
        panel.chat_focus = true;
        panel.selected = 0;
        let _ = render_text(&mut panel, 40, 10);
        assert_eq!(panel.top, 0);

        // Arrowing down to the tall answer scrolls it into view.
        panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
        let _ = render_text(&mut panel, 40, 10);
        assert!(panel.top > 0, "the answer below is scrolled into view");
    }

    #[test]
    fn the_session_records_the_provider_kind() {
        let dir = tempfile::tempdir().unwrap();
        let panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            provider_kind: "anthropic_compatible".into(),
            ..setup(vec![reply("ok")])
        });
        let recorded = panel.session.as_ref().unwrap().current_model().unwrap();
        assert_eq!(recorded.provider, "anthropic_compatible");
    }

    #[test]
    fn reasoning_toggles_from_the_chip_and_persists_in_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("ok")])
        });
        assert!(!panel.model.reasoning);
        let path = panel.session.as_ref().unwrap().path().to_path_buf();

        panel.handle_status_action(REASONING_ACTION);
        assert!(panel.model.reasoning, "the chip turns reasoning on");

        // The choice is written to the session, so a resume brings it back.
        let reopened = Session::open(&path).unwrap();
        assert_eq!(reopened.current_reasoning(), Some(true));
        assert!(session_model(&panel.configured_model, Some(&reopened)).reasoning);
    }

    #[test]
    fn a_click_focuses_the_chat_and_selects_a_block() {
        let mut panel = panel(vec![reply("Hello from the model")]);
        type_text(&mut panel, "hi there");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        // Render so the transcript area and its lines exist.
        let _ = render_text(&mut panel, 40, 12);
        assert!(!panel.chat_focus, "starts on the input");
        let area = panel.transcript_area;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x + 1,
            row: area.y,
            modifiers: KeyModifiers::NONE,
        };
        panel.handle_mouse(click, area);
        // The click lands on release, once it is clear no drag follows.
        assert!(!panel.chat_focus, "a press alone does not click");
        let release = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            ..click
        };
        panel.handle_mouse(release, area);
        assert!(panel.chat_focus, "a click focuses the chat");
        assert!(panel
            .selected_block_text()
            .is_some_and(|t| !t.trim().is_empty()));

        // A click below the transcript (on the input) hands focus back.
        let below = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x + 1,
            row: area.y + area.height + 1,
            modifiers: KeyModifiers::NONE,
        };
        panel.handle_mouse(below, area);
        assert!(
            !panel.chat_focus,
            "a click on the input returns focus to it"
        );
    }

    /// Select the tail of the prompt with Shift+arrows.
    fn select_back(panel: &mut AgentPanel, steps: usize) {
        for _ in 0..steps {
            panel.handle_key(chord(KeyCode::Left, KeyModifiers::SHIFT));
        }
    }

    #[test]
    fn shift_arrows_select_the_prompt_and_ctrl_a_takes_it_all() {
        let mut panel = panel(vec![]);
        type_text(&mut panel, "fix the flaky test");
        assert!(!panel.input_area().has_selection());
        select_back(&mut panel, 4);
        assert_eq!(
            panel.input_area().selected_text(),
            Some("test".to_string()),
            "Shift+Left extends the selection back over what was typed"
        );
        // The selection survives a redraw and shows inverted in the prompt.
        let rows = render_text(&mut panel, 30, 10);
        assert!(
            rows.iter().any(|row| row.contains("test")),
            "the prompt still shows its text: {rows:?}"
        );
        // A plain arrow drops the selection instead of extending it.
        panel.handle_key(chord(KeyCode::Left, KeyModifiers::NONE));
        assert!(!panel.input_area().has_selection());
        // Ctrl+A selects the whole prompt.
        panel.handle_key(chord(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(
            panel.input_area().selected_text(),
            Some("fix the flaky test".to_string())
        );
    }

    /// The one test that reaches the real system clipboard, so it is also the
    /// one that pays for opening it (tens of seconds on some hosts). It puts
    /// the clipboard back as it found it.
    #[test]
    fn ctrl_x_cuts_the_prompt_selection_and_ctrl_v_puts_it_back() {
        let before = termide_ui::clipboard::paste();
        let mut panel = panel(vec![]);
        type_text(&mut panel, "fix the flaky test");
        select_back(&mut panel, 5);
        let events = panel.handle_key(chord(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert!(events.iter().any(|e| matches!(e, PanelEvent::NeedsRedraw)));
        assert_eq!(panel.input_text(), "fix the flaky");
        assert!(!panel.input_area().has_selection());

        // With nothing selected, the clipboard keys are the prompt's to ignore.
        let events = panel.handle_key(chord(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert!(events.is_empty(), "nothing selected, nothing cut");

        // A machine without a display (CI) copies over OSC 52, which cannot be
        // read back, so the paste half runs only where the clipboard reads.
        if termide_ui::clipboard::paste().is_some() {
            let events = panel.handle_key(chord(KeyCode::Char('v'), KeyModifiers::CONTROL));
            assert!(events.iter().any(|e| matches!(e, PanelEvent::NeedsRedraw)));
            assert_eq!(panel.input_text(), "fix the flaky test");
        }
        if let Some(text) = before {
            let _ = termide_ui::clipboard::copy(&text);
        }
    }

    /// The routing of the clipboard commands, kept off the real clipboard: what
    /// is answered here is which side takes the key, not what it writes.
    #[test]
    fn the_clipboard_commands_answer_to_the_input_not_the_chat() {
        let mut panel = panel(vec![reply("an answer")]);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        let _ = render_text(&mut panel, 40, 12);

        // Nothing selected in the prompt: the panel declines, so the key falls
        // through to whatever the app has for it.
        assert!(matches!(
            panel.handle_command(PanelCommand::Copy),
            CommandResult::Handled(false)
        ));
        assert!(matches!(
            panel.handle_command(PanelCommand::Cut),
            CommandResult::Handled(false)
        ));

        // A bracketed paste carries its own text, so no clipboard is read; it
        // belongs to the prompt.
        assert!(matches!(
            panel.handle_command(PanelCommand::PasteText {
                text: "pasted".into()
            }),
            CommandResult::NeedsRedraw(true)
        ));
        assert_eq!(panel.input_text(), "pasted");

        // While the chat holds focus its block keeps the clipboard: `Copy`
        // there is the block's, and the prompt is not asked.
        panel.chat_focus = true;
        assert!(matches!(
            panel.handle_command(PanelCommand::Cut),
            CommandResult::None
        ));
        assert!(matches!(
            panel.handle_command(PanelCommand::Paste),
            CommandResult::None
        ));
    }

    #[test]
    fn dragging_in_the_prompt_selects_and_hands_focus_back_from_the_chat() {
        let mut panel = panel(vec![reply("Hello from the model")]);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        let _ = render_text(&mut panel, 40, 12);
        // A draft in the prompt to drag a selection through, typed before the
        // chat takes focus (which swallows plain characters).
        type_text(&mut panel, "one two three");
        let _ = render_text(&mut panel, 40, 12);
        panel.chat_focus = true;
        let bar = panel.input_area;
        let at = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let press = at(
            MouseEventKind::Down(MouseButton::Left),
            bar.x + 4,
            bar.y + bar.height - 1,
        );
        panel.handle_mouse(press, bar);
        assert!(!panel.chat_focus, "a press on the prompt focuses the input");
        // The prompt box is the bar's last row, so the drag stays on it.
        let drag = at(
            MouseEventKind::Drag(MouseButton::Left),
            bar.x + 8,
            bar.y + bar.height - 1,
        );
        let events = panel.handle_mouse(drag, bar);
        assert!(events.iter().any(|e| matches!(e, PanelEvent::NeedsRedraw)));
        assert!(
            panel.input_area().has_selection(),
            "the drag selected prompt text"
        );
    }

    #[test]
    fn a_resumed_block_shows_its_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), dir.path()).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("earlier question")))
            .unwrap();
        session
            .append_message(&Message::Assistant(reply("earlier answer")))
            .unwrap();
        let path = session.path().to_path_buf();
        // Reopen: the restored blocks carry the wall-clock time from the log.
        let session = Session::open(&path).unwrap();
        let mut transcript = Transcript::default();
        for logged in &session.context_messages_with_times(&CompactionPrompts::default()) {
            push_history(&mut transcript, logged);
        }
        let has_time = transcript
            .items()
            .iter()
            .any(|item| matches!(item, Item::Assistant { at, .. } if !at.is_empty()));
        assert!(has_time, "a resumed answer keeps its time");
    }

    #[test]
    fn a_resumed_session_keeps_its_timing() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), dir.path()).unwrap();
        let tool_call = ToolCall {
            id: "t1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({ "command": "ls" }),
        };
        let mut turn = reply("");
        turn.content = vec![
            AssistantContent::Thinking {
                text: "let me look".into(),
            },
            AssistantContent::ToolCall(tool_call.clone()),
        ];
        session
            .append_timed_message(
                &Message::Assistant(turn),
                Some(Timing::Turn {
                    prefill_ms: 1000,
                    gen_ms: 2000,
                }),
            )
            .unwrap();
        session
            .append_timed_message(
                &Message::ToolResult(ToolResultMessage::text(&tool_call, "a\nb")),
                Some(Timing::Tool {
                    duration_ms: 4000,
                    waited_ms: Some(9000),
                }),
            )
            .unwrap();
        let session = Session::open(session.path()).unwrap();
        let mut transcript = Transcript::default();
        for logged in &session.context_messages_with_times(&CompactionPrompts::default()) {
            push_history(&mut transcript, logged);
        }
        // The turn's cost comes back from the timing and the turn's usage.
        assert!(transcript.items().iter().any(|item| matches!(
            item,
            Item::Thinking { cost: Some(cost), .. }
                if cost.prefill_ms == 1000 && cost.gen_ms == 2000 && cost.input == 100
        )));
        assert_eq!(transcript.tool_duration("t1"), Some(4000));
        assert_eq!(transcript.tool_wait("t1"), Some(9000));
    }

    #[test]
    fn a_custom_agent_names_the_title() {
        let mut panel = panel(vec![reply("ok")]);
        // The default agent shows the localized "Agent" label.
        assert_eq!(panel.title(), "Agent: /tmp");
        // A custom agent replaces the label with its own capitalized name.
        assert!(panel.switch_agent("review"));
        assert_eq!(panel.title(), "Review: /tmp");
    }

    #[test]
    fn a_named_session_titles_the_panel() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("ok")])
        });

        type_text(&mut panel, "first request");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        assert_eq!(panel.title(), "Agent: first request");

        // The context menu raises an input prompt carrying our action.
        let (label, action) = panel
            .context_menu_items()
            .into_iter()
            .find(|(_, action)| *action == RENAME_ACTION)
            .expect("a rename entry in the menu");
        assert_eq!(label, "Rename session");
        let events = panel.handle_status_action(action);
        let Some(PanelEvent::ShowInput { on_submit, .. }) = events.first() else {
            panic!("expected an input prompt, got {events:?}");
        };
        let InputAction::Custom(submit_action) = on_submit else {
            panic!("expected a custom action");
        };
        assert_eq!(submit_action, RENAME_ACTION);

        // The submitted name wins over the first prompt and survives a reopen.
        let result = panel.handle_command(PanelCommand::InputSubmitted {
            action: submit_action.clone(),
            text: "  timeout work  ".into(),
        });
        assert!(matches!(result, CommandResult::Handled(true)));
        assert_eq!(panel.title(), "Agent: timeout work");
        let path = panel.session_path().unwrap().to_path_buf();
        drop(panel);
        assert_eq!(Session::open(&path).unwrap().name(), Some("timeout work"));

        // Without a session log there is nothing to record the name in.
        let mut logless = AgentPanel::new(setup(vec![]));
        assert!(!logless.rename_session("x"));
    }

    #[test]
    fn sessions_can_be_listed_switched_and_resumed() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("one"), reply("two")])
        });
        // A session is created eagerly, so the log exists before the first
        // prompt.
        let first_path = panel.session_path().unwrap().to_path_buf();
        type_text(&mut panel, "first task");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);

        // The panel menu is trimmed to the actions with no home elsewhere;
        // "New session" starts an empty one and leaves the old log alone.
        let items = panel.context_menu_items();
        let labels: Vec<&str> = items.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["Session info", "Rename session", "Delete session"]
        );
        panel.handle_status_action(NEW_SESSION_ACTION);
        assert!(panel.transcript().items().is_empty());
        assert_ne!(panel.session_path().unwrap(), first_path);
        assert_eq!(panel.title(), "Agent: /tmp");

        // The picker lists both, newest first, marking the current one.
        let events = panel.handle_status_action(RESUME_ACTION);
        let Some(PanelEvent::ShowSelect {
            options, on_select, ..
        }) = events.first()
        else {
            panic!("expected a picker, got {events:?}");
        };
        assert_eq!(options.len(), 2);
        assert!(options[0].starts_with("● "), "{:?}", options[0]);
        assert!(options[1].contains("first task"), "{:?}", options[1]);
        let SelectAction::Custom(action) = on_select else {
            panic!("expected a custom action");
        };

        // Choosing the older one replays its transcript into the panel.
        let result = panel.handle_command(PanelCommand::SelectionMade {
            action: action.clone(),
            index: 1,
        });
        assert!(matches!(result, CommandResult::Handled(true)));
        assert_eq!(panel.session_path().unwrap(), first_path);
        assert_eq!(panel.title(), "Agent: first task");
        let items = panel.transcript().items();
        assert!(matches!(&items[0], Item::User { text, .. } if text == "first task"));
        assert!(items
            .iter()
            .any(|item| matches!(item, Item::Assistant { text, .. } if text == "one")));

        // The resumed agent keeps the old messages as context.
        type_text(&mut panel, "second task");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        let reopened = Session::open(&first_path).unwrap();
        assert_eq!(
            roles(&reopened.context_messages()),
            vec!["user", "assistant", "user", "assistant"]
        );
    }

    #[test]
    fn slash_new_starts_a_fresh_session_and_keeps_the_old() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("one")])
        });
        let first_path = panel.session_path().unwrap().to_path_buf();
        type_text(&mut panel, "first task");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);

        // /new opens a fresh session; the used one is left on disk to resume.
        type_text(&mut panel, "/new");
        panel.submit();
        assert!(panel.transcript().items().is_empty());
        assert_ne!(panel.session_path().unwrap(), first_path);
        assert!(first_path.exists(), "the previous session log is kept");
        assert_eq!(panel.session_list().len(), 2);
    }

    #[test]
    fn slash_clear_discards_the_current_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("one")])
        });
        let first_path = panel.session_path().unwrap().to_path_buf();
        type_text(&mut panel, "first task");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);

        // /clear deletes the current session and starts a fresh one, so there
        // is nothing to resume back to: only the new empty session remains.
        type_text(&mut panel, "/clear");
        panel.submit();
        assert!(panel.transcript().items().is_empty());
        assert_ne!(panel.session_path().unwrap(), first_path);
        assert!(!first_path.exists(), "the previous session log is removed");
        assert_eq!(panel.session_list().len(), 1);
    }

    #[test]
    fn slash_rename_and_name_set_the_session_title() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("ok")])
        });
        // With an argument, both /rename and /name set the title directly.
        type_text(&mut panel, "/rename my work");
        panel.submit();
        assert_eq!(panel.title(), "Agent: my work");
        type_text(&mut panel, "/name other");
        panel.submit();
        assert_eq!(panel.title(), "Agent: other");
        assert!(panel.input_text().is_empty());

        // Without an argument, it opens the rename prompt instead of sending.
        type_text(&mut panel, "/rename");
        let events = panel.submit();
        assert!(events
            .iter()
            .any(|e| matches!(e, PanelEvent::ShowInput { .. })));
        assert!(panel.transcript().items().is_empty(), "nothing was sent");
    }

    #[test]
    fn loop_args_split_interval_from_prompt() {
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_duration("90"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("x"), None);
        assert_eq!(parse_duration("0"), None);
        assert_eq!(
            parse_loop_args("5m run the tests"),
            (Some(Duration::from_secs(300)), "run the tests")
        );
        assert_eq!(parse_loop_args("keep improving"), (None, "keep improving"));
    }

    #[test]
    fn slash_loop_starts_and_stops() {
        let mut panel = panel(vec![reply("a")]);
        type_text(&mut panel, "/loop keep going");
        panel.submit();
        assert!(panel.loop_task.is_some(), "the loop started");

        type_text(&mut panel, "/loop stop");
        panel.submit();
        assert!(panel.loop_task.is_none(), "/loop stop ended it");

        // Esc also ends a waiting loop.
        type_text(&mut panel, "/loop 5m again");
        panel.submit();
        assert!(panel.loop_task.is_some());
        settle(&mut panel);
        panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
        assert!(panel.loop_task.is_none(), "Esc ended the loop");
    }

    #[test]
    fn slash_completion_is_alphabetical() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("ok")])
        });
        type_text(&mut panel, "/c");
        let list = panel.completion.as_ref().expect("a completion popup");
        let names: Vec<&str> = list.items().iter().map(|i| i.value.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "the /-command list should be alphabetical");
        assert!(names.contains(&"clear") && names.contains(&"compact"));
    }

    #[test]
    fn slash_pause_and_continue_report_when_idle() {
        let mut panel = panel(vec![]);
        type_text(&mut panel, "/pause");
        panel.submit();
        assert!(panel.transcript().items().iter().any(
            |i| matches!(i, Item::Notice { text, .. } if text.contains("nothing is running"))
        ));
        type_text(&mut panel, "/continue");
        panel.submit();
        assert!(panel.transcript().items().iter().any(
            |i| matches!(i, Item::Notice { text, .. } if text.contains("nothing to continue"))
        ));
    }

    /// Drive the panel until the active goal finishes, or fail on a deadline.
    fn settle_goal(panel: &mut AgentPanel) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while panel.goal_task.is_some() {
            panel.tick();
            assert!(Instant::now() < deadline, "goal did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn goal_works_turn_by_turn_until_the_judge_says_done() {
        // Provider calls, in order: work turn, judge (continue), work turn,
        // judge (done).
        let mut panel = panel(vec![
            reply("starting the work"),
            reply("CONTINUE\ntests are still red"),
            reply("more work done"),
            reply("DONE\neverything is green"),
        ]);
        type_text(&mut panel, "/goal get the build green");
        panel.submit();
        assert!(panel.goal_task.is_some());
        settle_goal(&mut panel);

        // Two work turns were sent (the goal, then a continuation).
        let users = panel
            .transcript()
            .items()
            .iter()
            .filter(|i| matches!(i, Item::User { .. }))
            .count();
        assert_eq!(users, 2);
        // The judge ran and the goal ended with the success reason.
        assert!(panel.transcript().items().iter().any(
            |i| matches!(i, Item::Notice { text, .. } if text.contains("checking whether the goal"))
        ));
        assert!(panel.transcript().items().iter().any(
            |i| matches!(i, Item::Notice { text, .. } if text.contains("goal reached: everything is green"))
        ));
    }

    #[test]
    fn goal_stop_ends_an_active_goal() {
        let mut panel = panel(vec![reply("working")]);
        type_text(&mut panel, "/goal do the thing");
        panel.submit();
        settle(&mut panel);
        assert!(panel.goal_task.is_some());
        // Stop it before the judge would send another turn.
        type_text(&mut panel, "/goal stop");
        panel.submit();
        assert!(panel.goal_task.is_none());
        assert!(panel
            .transcript()
            .items()
            .iter()
            .any(|i| matches!(i, Item::Notice { text, .. } if text.contains(termide_i18n::t().agent_notice_goal_stopped()))));
    }

    fn roles(messages: &[Message]) -> Vec<&'static str> {
        messages
            .iter()
            .map(|m| match m {
                Message::User(_) => "user",
                Message::Assistant(_) => "assistant",
                Message::ToolResult(_) => "tool_result",
            })
            .collect()
    }

    #[test]
    fn shift_enter_adds_a_line_and_esc_clears() {
        let mut panel = panel(vec![]);
        type_text(&mut panel, "one");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::SHIFT));
        type_text(&mut panel, "two");
        assert_eq!(panel.input_text(), "one\ntwo");
        let rows = render_text(&mut panel, 30, 6);
        assert!(rows[4].starts_with("› one"));
        assert!(rows[5].starts_with("  two"));

        panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
        assert!(panel.input_text().is_empty());
        assert!(!panel.captures_escape());
        assert!(panel
            .handle_key(chord(KeyCode::Esc, KeyModifiers::NONE))
            .is_empty());
    }

    #[test]
    fn a_large_paste_is_held_as_a_placeholder_and_expanded() {
        let mut panel = panel(vec![]);
        let big = (1..=40)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");

        // A large paste shows a short placeholder, not the whole block.
        panel.handle_command(PanelCommand::PasteText { text: big.clone() });
        assert_eq!(panel.input_text(), "[#1 pasted 40 lines]");
        // Its full text is spliced back for the message.
        assert_eq!(panel.expand_pastes(&panel.input_text()), big);

        // A small paste is inlined as-is, beside the placeholder.
        panel.handle_command(PanelCommand::PasteText {
            text: " review this".into(),
        });
        assert_eq!(panel.input_text(), "[#1 pasted 40 lines] review this");
        assert_eq!(
            panel.expand_pastes(&panel.input_text()),
            format!("{big} review this")
        );

        // Submitting sends the expanded text and drops the held paste.
        panel.submit();
        assert!(panel.input_text().is_empty());
        assert!(panel.pastes.is_empty());
        assert_eq!(panel.paste_seq, 0);
    }

    #[test]
    fn f2_opens_the_rename_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("ok")])
        });
        let events = panel.handle_key(chord(KeyCode::F(2), KeyModifiers::NONE));
        let Some(PanelEvent::ShowInput { on_submit, .. }) = events.first() else {
            panic!("F2 should open the rename prompt, got {events:?}");
        };
        assert!(
            matches!(on_submit, InputAction::Custom(a) if a == RENAME_ACTION),
            "{on_submit:?}"
        );
    }

    #[test]
    fn f8_confirms_in_a_modal_then_deletes_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("ok")])
        });
        type_text(&mut panel, "first task");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        let first_path = panel.session_path().unwrap().to_path_buf();

        // F8 asks through an app confirmation modal — nothing is deleted yet,
        // and the panel raises no in-panel card of its own.
        let events = panel.handle_key(chord(KeyCode::F(8), KeyModifiers::NONE));
        let Some(PanelEvent::ShowConfirm {
            on_confirm,
            message,
        }) = events.first()
        else {
            panic!("F8 should raise a confirmation modal, got {events:?}");
        };
        // An unnamed session is named in the UI language, its id below.
        let t = termide_i18n::t();
        let id = first_path.file_stem().unwrap().to_str().unwrap();
        assert_eq!(
            *message,
            format!(
                "{}\n{id}",
                t.agent_delete_confirm_fmt(t.agent_delete_this_session())
            )
        );
        assert!(
            matches!(on_confirm, ConfirmAction::Custom(a) if a == DELETE_SESSION_ACTION),
            "{on_confirm:?}"
        );
        assert!(panel.pending.is_none());
        assert!(first_path.exists());

        // The confirmed answer comes back as a Confirmed command: the log is
        // removed and a fresh session starts.
        panel.handle_command(PanelCommand::Confirmed {
            action: DELETE_SESSION_ACTION.to_string(),
        });
        settle(&mut panel);
        assert!(!first_path.exists(), "the session log is removed");
        assert_ne!(panel.session_path().unwrap(), first_path);
        assert!(panel.transcript().items().is_empty());

        // Cancelling the modal (no Confirmed command) keeps the session.
        type_text(&mut panel, "more");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        let path = panel.session_path().unwrap().to_path_buf();
        panel.handle_key(chord(KeyCode::F(8), KeyModifiers::NONE));
        assert!(path.exists(), "cancelled delete keeps the log");
    }

    #[test]
    fn f7_starts_a_new_session_and_f6_opens_the_switcher() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("ok")])
        });
        type_text(&mut panel, "task one");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        let first = panel.session_path().unwrap().to_path_buf();

        // F7 opens a fresh session, keeping the used one.
        panel.handle_key(chord(KeyCode::F(7), KeyModifiers::NONE));
        assert!(panel.transcript().items().is_empty());
        assert_ne!(panel.session_path().unwrap(), first);

        // F6 opens the session switcher.
        let events = panel.handle_key(chord(KeyCode::F(6), KeyModifiers::NONE));
        assert!(events
            .iter()
            .any(|e| matches!(e, PanelEvent::ShowSelect { .. })));
    }

    #[test]
    fn f3_shows_a_session_summary() {
        let mut panel = panel(vec![]);
        let events = panel.handle_key(chord(KeyCode::F(3), KeyModifiers::NONE));
        let Some(PanelEvent::ShowInfo { rows, .. }) = events.first() else {
            panic!("F3 should show a summary modal, got {events:?}");
        };
        for label in ["Provider", "Model", "Agent", "Directory", "Tokens"] {
            assert!(
                rows.iter().any(|(key, _)| key == label),
                "missing {label}: {rows:?}"
            );
        }
        assert!(
            rows.iter()
                .any(|(key, value)| key == "Model" && value == panel.model.id.as_str()),
            "{rows:?}"
        );
    }

    #[test]
    fn f4_offers_a_rollback_picker_when_there_is_a_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("ok")])
        });
        // Nothing changed yet: F4 says there is nothing to roll back.
        panel.handle_key(chord(KeyCode::F(4), KeyModifiers::NONE));
        assert!(panel
            .transcript()
            .items()
            .iter()
            .any(|i| matches!(i, Item::Notice { text, .. } if text.contains("roll back"))));

        // Record a checkpoint, then F4 offers it in a picker.
        let file = dir.path().join("x.txt");
        std::fs::write(&file, "v1").unwrap();
        {
            let store = panel.checkpoints.clone().expect("a checkpoint store");
            let mut store = store.lock().unwrap();
            store.begin_run(None);
            store.save(&file).unwrap();
            store.end_run();
        }
        let events = panel.handle_key(chord(KeyCode::F(4), KeyModifiers::NONE));
        assert!(events
            .iter()
            .any(|e| matches!(e, PanelEvent::ShowSelect { .. })));
    }

    #[test]
    fn an_empty_session_shows_a_welcome_banner() {
        let mut panel = panel(vec![]);
        let all = render_text(&mut panel, 60, 16).join("\n");
        assert!(all.contains("termide"), "{all}");
        for label in ["connection", "model", "agent", "cwd"] {
            assert!(all.contains(label), "missing {label}: {all}");
        }
        // The banner is the empty-state: once a turn runs, real content shows.
        type_text(&mut panel, "hello");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        let all = render_text(&mut panel, 60, 16).join("\n");
        assert!(
            !all.contains("coding agent"),
            "banner gone once used: {all}"
        );
        // Once the banner is gone it leaves no clickable fields behind.
        assert!(panel.banner_hits.is_empty());
    }

    #[test]
    fn the_summary_reports_output_cleaning_savings() {
        use termide_agent_core::ToolCall;
        let mut panel = panel(vec![]);
        let call = ToolCall {
            id: "b1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({ "command": "ls" }),
        };
        panel.apply(AgentEvent::ToolExecutionStart { call: call.clone() });
        panel.apply(AgentEvent::ToolExecutionEnd {
            result: ToolResultMessage::text(&call, "out").with_details(
                serde_json::json!({ "raw_bytes": 1000_u64, "cleaned_bytes": 250_u64 }),
            ),
        });
        let events = panel.session_summary();
        let Some(PanelEvent::ShowInfo { rows, .. }) = events.first() else {
            panic!("expected a summary modal, got {events:?}");
        };
        let cleaned = rows
            .iter()
            .find(|(key, _)| key == "Output cleaned")
            .map(|(_, value)| value.as_str())
            .unwrap_or_else(|| panic!("no cleaning row: {rows:?}"));
        assert!(cleaned.contains("−75%"), "{cleaned}");
    }

    #[test]
    fn slash_usage_opens_the_session_info_modal() {
        let mut panel = panel(vec![]);
        type_text(&mut panel, "/usage");
        let events = panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, PanelEvent::ShowInfo { .. })),
            "expected an info modal, got {events:?}"
        );
    }

    #[test]
    fn slash_prompt_opens_the_system_prompt() {
        let mut panel = panel(vec![]);
        type_text(&mut panel, "/prompt");
        let events = panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            events.iter().any(|e| matches!(e, PanelEvent::ViewFile(_))),
            "expected the prompt file to open, got {events:?}"
        );
    }

    #[test]
    fn handoff_offers_to_save_or_start_a_new_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            cwd: dir.path().to_path_buf(),
            ..setup(vec![reply("# Handoff\n\n## Remaining\nFinish the parser.")])
        });
        type_text(&mut panel, "/handoff");
        panel.submit();

        // The brief is produced off-thread; a card offers what to do with it.
        let deadline = Instant::now() + Duration::from_secs(5);
        while panel.pending.is_none() {
            panel.tick();
            assert!(Instant::now() < deadline, "no handoff card");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(panel.pending, Some(Pending::Handoff { .. })));

        // Choosing "Save to HANDOFF.md" writes the brief to the panel's dir.
        panel.apply_form_action(ChoiceAction::Chosen(0));
        let written = std::fs::read_to_string(dir.path().join("HANDOFF.md")).unwrap();
        assert!(written.contains("Finish the parser."), "{written}");
        assert!(panel.pending.is_none());
    }

    #[test]
    fn a_model_switch_in_a_fresh_session_updates_the_banner_not_the_transcript() {
        let mut panel = panel(vec![]);
        assert!(render_text(&mut panel, 60, 16)
            .join("\n")
            .contains("coding agent"));

        // No prompt yet: switching the model updates the banner in place and
        // pushes no notice, so the banner stays.
        assert!(panel.switch_model("gpt-5-brand-new", None));
        assert!(panel.is_fresh());
        assert!(!panel
            .transcript()
            .items()
            .iter()
            .any(|i| matches!(i, Item::Notice { .. })));
        let all = render_text(&mut panel, 60, 16).join("\n");
        assert!(all.contains("coding agent"), "banner stays: {all}");
        assert!(all.contains("gpt-5-brand-new"), "banner shows it: {all}");

        // Once the conversation has begun, a switch is announced as before.
        type_text(&mut panel, "hi");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        assert!(!panel.is_fresh());
        assert!(panel.switch_model("gpt-6-next", None));
        assert!(panel
            .transcript()
            .items()
            .iter()
            .any(|i| matches!(i, Item::Notice { text, .. } if text.contains("gpt-6-next"))));
    }

    #[test]
    fn a_re_pickable_banner_value_is_bold_not_underlined() {
        let mut panel = panel(vec![]);
        let buf = render_buf(&mut panel, 60, 16);
        let (x, y) = (0..16u16)
            .find_map(|y| {
                let row: String = (0..60u16).map(|x| buf[(x, y)].symbol()).collect();
                row.find("default")
                    .map(|at| (row[..at].chars().count() as u16, y))
            })
            .expect("the agent's name is on the banner");
        let cell = &buf[(x, y)];
        assert!(cell.modifier.contains(Modifier::BOLD));
        assert!(!cell.modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn clicking_a_banner_field_reopens_its_picker() {
        let mut panel = panel(vec![]);
        // Rendering the empty-state banner records its clickable fields.
        let _ = render_text(&mut panel, 60, 16);
        assert_eq!(
            panel.banner_hits.len(),
            3,
            "the model, the agent and the tools are re-pickable"
        );
        let (rect, _) = panel
            .banner_hits
            .iter()
            .find(|(_, action)| *action == AGENT_ACTION)
            .copied()
            .expect("the agent field is clickable");
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 1,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        };
        let events = panel.handle_mouse(click, Rect::new(0, 0, 60, 16));
        assert!(
            events.iter().any(|e| matches!(
                e,
                PanelEvent::ShowSelect { on_select: SelectAction::Custom(a), .. } if a == AGENT_ACTION
            )),
            "clicking the agent field opens the agent picker, got {events:?}"
        );
    }

    #[test]
    fn permission_prompt_is_answered_in_the_panel() {
        let mut panel = panel(vec![]);
        let (mut prompter, rx) = permission_channel(CancelToken::new());
        panel.permission_rx = rx;
        let worker = std::thread::spawn(move || {
            prompter.ask(&termide_agent_core::PermissionRequest {
                tool: "bash".into(),
                subject: "git push".into(),
                call: termide_agent_core::ToolCall {
                    id: "c".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({ "command": "git push" }),
                },
                suggested_pattern: "git push *".into(),
                can_persist: true,
            })
        });

        // The question arrives as a form in the panel and a status line.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let events = panel.tick();
            if panel.pending.is_some() {
                assert!(events.iter().any(|e| matches!(
                    e,
                    PanelEvent::SetStatusMessage { message, .. } if message.contains("git push")
                )));
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        {
            let form = panel.pending.as_ref().unwrap().form();
            assert_eq!(form.title(), "Agent wants to run bash:");
            assert_eq!(form.detail(), Some("git push"));
            assert_eq!(
                form.options()[2],
                "Allow always in this project (git push *)"
            );
            assert_eq!(form.options()[3], "Allow always everywhere (git push *)");
            assert_eq!(form.options()[5], "Deny for this session (git push *)");
        }
        assert!(panel.captures_escape());
        let rows = render_text(&mut panel, 60, 14);
        assert!(
            rows.iter().any(|r| r.contains("2. Allow for this session")),
            "{rows:?}"
        );
        // Typing goes nowhere while the question is open; Down + Enter answer it.
        type_text(&mut panel, "x");
        assert_eq!(panel.input_text(), "");
        panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        assert!(panel.pending.is_none());
        assert_eq!(worker.join().unwrap(), PermissionAnswer::AllowSession);

        // A digit answers at once; Esc declines.
        let (mut prompter, rx) = permission_channel(CancelToken::new());
        panel.permission_rx = rx;
        let request = termide_agent_core::PermissionRequest {
            tool: "edit".into(),
            subject: "src/x.rs".into(),
            call: termide_agent_core::ToolCall {
                id: "c".into(),
                name: "edit".into(),
                arguments: serde_json::json!({ "path": "src/x.rs" }),
            },
            suggested_pattern: "src/x.rs".into(),
            can_persist: true,
        };
        let asked = request.clone();
        let worker = std::thread::spawn(move || {
            let first = prompter.ask(&asked);
            let second = prompter.ask(&asked);
            (first, second)
        });
        let wait = |panel: &mut AgentPanel| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while panel.pending.is_none() {
                panel.tick();
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        wait(&mut panel);
        panel.handle_key(chord(KeyCode::Char('3'), KeyModifiers::NONE));
        // The fifth row takes a reason the model gets to read.
        wait(&mut panel);
        {
            let form = panel.pending.as_ref().unwrap().form();
            assert_eq!(
                form.height(60),
                12,
                "a detail line and divider, six answers, a reason row and a stop row"
            );
        }
        panel.handle_key(chord(KeyCode::Char('7'), KeyModifiers::NONE));
        type_text(&mut panel, "edit the test instead");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            worker.join().unwrap(),
            (
                PermissionAnswer::AllowAlways,
                PermissionAnswer::DenyWithReason("edit the test instead".into())
            )
        );
        let _ = request;
    }

    #[test]
    fn always_is_not_offered_where_the_configured_rules_do_not_count() {
        let mut panel = panel(vec![]);
        let (mut prompter, rx) = permission_channel(CancelToken::new());
        panel.permission_rx = rx;
        let worker = std::thread::spawn(move || {
            prompter.ask(&termide_agent_core::PermissionRequest {
                tool: "bash".into(),
                subject: "make".into(),
                call: termide_agent_core::ToolCall {
                    id: "c".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({ "command": "make" }),
                },
                suggested_pattern: "make *".into(),
                can_persist: false,
            })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while panel.pending.is_none() {
            panel.tick();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let options = panel.pending.as_ref().unwrap().form().options().to_vec();
        assert_eq!(options.len(), 4, "{options:?}");
        assert!(options.iter().all(|o| !o.contains("always")), "{options:?}");
        // The last row refuses for the session.
        panel.handle_key(chord(KeyCode::Char('4'), KeyModifiers::NONE));
        assert_eq!(worker.join().unwrap(), PermissionAnswer::DenySession);
        assert_eq!(
            panel.effective_rules().evaluate_session("bash", "make all"),
            Some(Decision::Deny)
        );
    }

    #[test]
    fn a_single_click_selects_and_a_double_click_answers_a_permission() {
        let mut panel = panel(vec![]);
        let (mut prompter, rx) = permission_channel(CancelToken::new());
        panel.permission_rx = rx;
        let worker = std::thread::spawn(move || {
            prompter.ask(&termide_agent_core::PermissionRequest {
                tool: "bash".into(),
                subject: "git push".into(),
                call: termide_agent_core::ToolCall {
                    id: "c".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({ "command": "git push" }),
                },
                suggested_pattern: "git push *".into(),
                can_persist: true,
            })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while panel.pending.is_none() {
            panel.tick();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        // Render so the form's row geometry exists, then find the second option.
        let rows = render_text(&mut panel, 60, 16);
        let y = rows
            .iter()
            .position(|r| r.contains("2. Allow for this session"))
            .expect("the second option is on screen") as u16;
        let area = Rect::new(0, 0, 60, 16);
        let click = |panel: &mut AgentPanel, y: u16| {
            panel.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: 5,
                    row: y,
                    modifiers: KeyModifiers::NONE,
                },
                area,
            );
        };

        // A single click only moves the selection; the question stays open.
        click(&mut panel, y);
        assert!(panel.pending.is_some(), "a single click does not answer");
        assert_eq!(panel.pending.as_ref().unwrap().form().selected(), 1);

        // A second click on the same row (a double click) answers it.
        click(&mut panel, y);
        assert!(panel.pending.is_none());
        assert_eq!(worker.join().unwrap(), PermissionAnswer::AllowSession);
    }

    /// Three connections: a local endpoint, a hosted one, and a CLI agent.
    struct Connections;

    impl ConnectionCatalog for Connections {
        fn list(&self) -> Vec<ConnectionEntry> {
            [
                ("local", "openai_compatible", "m"),
                ("cloud", "anthropic_compatible", "claude-x"),
                ("cli", "codex", ""),
            ]
            .into_iter()
            .map(|(name, kind, model)| ConnectionEntry {
                name: name.into(),
                kind: kind.into(),
                model: model.into(),
            })
            .collect()
        }

        fn build(&self, name: &str, _agent: &str) -> Option<ConnectionChoice> {
            let entry = self.list().into_iter().find(|entry| entry.name == name)?;
            let backend: Option<BackendFactory> = (entry.kind == "codex").then(|| {
                Arc::new(|setup: BackendSetup| {
                    Ok(Box::new(External::new(setup)) as Box<dyn Backend>)
                }) as BackendFactory
            });
            Some(ConnectionChoice {
                name: entry.name,
                kind: entry.kind,
                provider: Arc::new(Scripted::new(vec![])),
                model: entry.model,
                context_window: 200_000,
                backend,
            })
        }
    }

    fn connected_panel(dir: &std::path::Path) -> AgentPanel {
        AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.to_path_buf()),
            connections: Some(Arc::new(Connections)),
            ..setup(vec![])
        })
    }

    #[test]
    fn the_connection_picker_switches_the_endpoint_and_its_model() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = connected_panel(dir.path());
        let events = panel.handle_status_action(CONNECTION_ACTION);
        let Some(PanelEvent::ShowSelect { options, .. }) = events.first() else {
            panic!("a picker, got {events:?}");
        };
        assert!(options[0].starts_with("● local"), "{options:?}");
        assert!(options[1].contains("cloud") && options[1].contains("claude-x"));
        panel.handle_command(PanelCommand::SelectionMade {
            action: CONNECTION_ACTION.to_string(),
            index: 1,
        });
        // The connection's endpoint and model replace the ones in use.
        assert_eq!(panel.connection, "cloud");
        assert_eq!(panel.provider_kind, "anthropic_compatible");
        assert_eq!(panel.model.id, "claude-x");
        assert_eq!(panel.model.context_window, 200_000);
        assert!(!panel.external);
        // The chip names the connection beside its protocol.
        assert_eq!(panel.connection_display(), "cloud · Anthropic Compatible");
        // The log keeps it, so a reopened session reconnects there.
        let session = Session::open(panel.session_path().unwrap()).unwrap();
        assert_eq!(session.current_connection(), Some("cloud".to_string()));
        assert_eq!(
            session.current_model().map(|m| m.id),
            Some("claude-x".to_string())
        );
    }

    #[test]
    fn a_cli_agent_connection_is_taken_on_only_before_the_first_request() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = connected_panel(dir.path());
        panel.transcript.push(Item::User {
            text: "go".into(),
            at: String::new(),
        });
        // Mid-conversation the CLI agent would not see it: refused.
        assert!(!panel.switch_connection("cli"));
        assert_eq!(panel.connection, "local");
        assert!(panel.transcript.items().iter().any(|item| matches!(
            item,
            Item::Notice { text, .. } if text == termide_i18n::t().agent_notice_connection_before_first()
        )));
        // In a fresh session it drives the CLI agent over ACP.
        let dir = tempfile::tempdir().unwrap();
        let mut fresh = connected_panel(dir.path());
        assert!(fresh.switch_connection("cli"));
        assert!(fresh.external);
        assert_eq!(fresh.provider_kind, "codex");
    }

    #[test]
    fn the_toolset_guard_refuses_what_is_switched_off_in_context() {
        let blocked: Blocked = Arc::new(RwLock::new(
            ["bash".to_string(), "skill:review".to_string()].into(),
        ));
        let mut guard = ToolsetGuard { blocked };
        let ctx = ToolContext {
            cwd: PathBuf::from("/tmp"),
        };
        let call = |name: &str, args: serde_json::Value| ToolCall {
            id: "c".into(),
            name: name.into(),
            arguments: args,
        };
        let refused = |d: ToolDecision| matches!(d, ToolDecision::Block { .. });
        assert!(refused(
            guard.before_tool_call(&call("bash", serde_json::json!({})), &ctx)
        ));
        assert!(refused(guard.before_tool_call(
            &call("skill", serde_json::json!({ "name": "review" })),
            &ctx
        )));
        assert!(!refused(guard.before_tool_call(
            &call("skill", serde_json::json!({ "name": "deploy" })),
            &ctx
        )));
        assert!(!refused(
            guard.before_tool_call(&call("read", serde_json::json!({})), &ctx)
        ));
    }

    #[test]
    fn switching_off_before_the_first_request_keeps_it_out_of_the_context() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![])
        });
        panel.offered_tools = vec!["read".into(), "bash".into()];
        panel.offered_skills = vec!["review".into()];
        assert!(panel.is_fresh());
        panel.apply_toolset(&["read".to_string()]);
        let off: BTreeSet<String> = ["bash".to_string(), "skill:review".to_string()].into();
        assert_eq!(panel.toolset_off, off);
        // Rebuilt at once without them: out of the context, nothing to refuse.
        assert_eq!(panel.context_off, off);
        assert!(panel.blocked.read().unwrap().is_empty());
        // The log keeps the set for a reopened session.
        let path = panel.session_path().unwrap().to_path_buf();
        let session = Session::open(&path).unwrap();
        assert_eq!(
            session.current_toolset(),
            Some(vec!["bash".to_string(), "skill:review".to_string()])
        );
        // Reopened, the session comes back with it, built without it: a new
        // worker has no cache to keep.
        drop(panel);
        let reopened = AgentPanel::new(AgentPanelSetup {
            session: Some(session),
            ..setup(vec![])
        });
        assert_eq!(reopened.toolset_off, off);
        assert_eq!(reopened.context_off, off);
        assert!(reopened.blocked.read().unwrap().is_empty());
    }

    #[test]
    fn switching_off_mid_session_refuses_until_a_compaction_takes_it_out() {
        let mut panel = panel(vec![]);
        panel.offered_tools = vec!["read".into(), "bash".into()];
        panel.transcript.push(Item::User {
            text: "go".into(),
            at: String::new(),
        });
        assert!(!panel.is_fresh());
        panel.apply_toolset(&["read".to_string()]);
        // Still in the context (the prompt cache stays): refused, not gone.
        assert!(panel.context_off.is_empty());
        assert!(panel.blocked.read().unwrap().contains("bash"));
        let items = panel.toolset_items();
        let bash = items.iter().find(|i| i.key == "bash").unwrap();
        assert!(
            bash.enabled && !bash.checked && !bash.note.is_empty(),
            "{bash:?}"
        );
        // A compaction lets the next moment between runs rebuild without it.
        panel.apply(AgentEvent::Compacted {
            summary: "earlier".into(),
            kept: 1,
            tokens_before: 1000,
        });
        assert!(panel.context_stale);
        panel.tick();
        assert!(panel.context_off.contains("bash"));
        assert!(panel.blocked.read().unwrap().is_empty());
        // Out of the context now, it cannot come back in this session.
        let items = panel.toolset_items();
        let bash = items.iter().find(|i| i.key == "bash").unwrap();
        assert!(!bash.enabled);
        panel.apply_toolset(&["read".to_string(), "bash".to_string()]);
        assert!(
            panel.toolset_off.contains("bash"),
            "a locked item keeps its state"
        );
    }

    #[test]
    fn up_takes_the_queue_back_into_the_input() {
        let mut panel = panel(vec![]);
        panel.apply(AgentEvent::AgentStart);
        for text in ["fix the test", "and the docs"] {
            type_text(&mut panel, text);
            panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        }
        assert_eq!(panel.runtime.queue_lens(), (2, 0));
        type_text(&mut panel, "also");
        // Up at the top of the input takes both back, ahead of what is typed.
        for _ in 0..2 {
            panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
        }
        assert_eq!(panel.input_text(), "fix the test\n\nand the docs\n\nalso");
        assert_eq!(panel.runtime.queue_lens(), (0, 0));
        assert!(panel.queued_texts.is_empty());
        assert!(
            panel.state_lines(60).is_empty(),
            "the strip no longer lists them"
        );
    }

    #[test]
    fn a_call_that_fails_before_its_first_token_shows_no_cost() {
        let mut panel = AgentPanel::new(setup(vec![]));
        panel.apply(AgentEvent::AgentStart);
        panel.apply(AgentEvent::MessageStart);
        let failed = AssistantMessage::failed("p", "m", StopReason::Error, "connection refused");
        panel.apply(AgentEvent::MessageEnd(Message::Assistant(failed)));
        let answer = panel
            .transcript
            .items()
            .iter()
            .find(|item| matches!(item, Item::Assistant { .. }))
            .expect("the failure shows on an answer block");
        assert!(matches!(
            answer,
            Item::Assistant {
                cost: None,
                error: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn a_permission_wait_is_timed_apart_from_the_call() {
        let mut panel = panel(vec![]);
        let call = termide_agent_core::ToolCall {
            id: "c".into(),
            name: "bash".into(),
            arguments: serde_json::json!({ "command": "cat notes.txt" }),
        };
        panel.apply(AgentEvent::AgentStart);
        panel.apply(AgentEvent::ToolExecutionStart { call: call.clone() });
        // The call started 7s ago and its question has been up for 5s.
        panel
            .tool_starts
            .insert("c".into(), Instant::now() - Duration::from_secs(7));
        let (reply, _rx) = std::sync::mpsc::channel();
        panel.pending = Some(Pending::Permission {
            envelope: PermissionEnvelope {
                id: 1,
                request: termide_agent_core::PermissionRequest {
                    tool: "bash".into(),
                    subject: "cat notes.txt".into(),
                    call: call.clone(),
                    suggested_pattern: "cat *".into(),
                    can_persist: true,
                },
                reply,
            },
            form: ChoiceForm::new("", vec![]),
            answers: Vec::new(),
        });
        panel.permission_wait = Some((Instant::now() - Duration::from_secs(5), 0));
        panel.tick();
        // While the question is up, the call's wait ticks as a pause.
        let rows = strip_text(panel.transcript.lines(60, &panel.colors, false));
        assert!(rows.iter().any(|r| r.contains("‖ 5s")), "{rows:?}");
        assert!(matches!(
            panel.transcript.items().last(),
            Some(Item::Tool { waiting: true, .. })
        ));
        // Answered: the wait rests, and the call's duration leaves it out.
        assert!(panel.answer_permission(PermissionAnswer::AllowOnce));
        panel.apply(AgentEvent::ToolExecutionEnd {
            result: ToolResultMessage::text(&call, "notes"),
        });
        let Some(Item::Tool {
            waited_ms: Some(waited),
            waiting: false,
            duration_ms: Some(duration),
            ..
        }) = panel.transcript.items().last().cloned()
        else {
            panic!("the call keeps its wait and duration");
        };
        assert!((5000..6000).contains(&waited), "{waited}");
        assert!((1500..2500).contains(&duration), "{duration}");
        let rows = strip_text(panel.transcript.lines(60, &panel.colors, false));
        assert!(rows.iter().any(|r| r.contains("‖ 5s 🕒 2s")), "{rows:?}");
    }

    #[test]
    fn a_grant_reaches_the_panel_rules_so_it_survives_a_rebuild() {
        let pending = |panel: &mut AgentPanel| {
            let (reply, _rx) = std::sync::mpsc::channel();
            panel.pending = Some(Pending::Permission {
                envelope: PermissionEnvelope {
                    id: 1,
                    request: termide_agent_core::PermissionRequest {
                        tool: "bash".into(),
                        subject: "cat notes.txt".into(),
                        call: termide_agent_core::ToolCall {
                            id: "c".into(),
                            name: "bash".into(),
                            arguments: serde_json::json!({ "command": "cat notes.txt" }),
                        },
                        suggested_pattern: "cat *".into(),
                        can_persist: true,
                    },
                    reply,
                },
                form: ChoiceForm::new("", vec![]),
                answers: Vec::new(),
            });
        };

        // "Allow for the session" lands in the session rules, which
        // `effective_rules` — what every rebuilt agent starts from — carries
        // apart from the persistent rules; "deny for the session" too.
        let mut session = panel(vec![]);
        pending(&mut session);
        assert!(session.answer_permission(PermissionAnswer::AllowSession));
        assert_eq!(
            session.effective_rules().evaluate_session("bash", "cat x"),
            Some(Decision::Allow)
        );
        assert_eq!(session.effective_rules().evaluate("bash", "cat x"), None);
        pending(&mut session);
        assert!(session.answer_permission(PermissionAnswer::DenySession));
        assert_eq!(
            session.effective_rules().evaluate_session("bash", "cat x"),
            Some(Decision::Deny)
        );

        // "Allow always" lands in the persistent rules.
        let mut always = panel(vec![]);
        pending(&mut always);
        assert!(always.answer_permission(PermissionAnswer::AllowAlways));
        assert_eq!(
            always.rules.evaluate("bash", "cat x"),
            Some(Decision::Allow)
        );
    }

    /// A catalog that records the modes the panel reports.
    struct ModeWatcher(Arc<Mutex<Vec<Mode>>>);

    impl AgentCatalog for ModeWatcher {
        fn list(&self) -> Vec<AgentEntry> {
            Agents.list()
        }
        fn resolve(&self, name: &str) -> Option<AgentProfile> {
            Agents.resolve(name)
        }
        fn set_mode(&self, mode: Mode) {
            self.0.lock().unwrap().push(mode);
        }
    }

    #[test]
    fn delegated_tasks_follow_the_session_mode() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut panel = AgentPanel::new(AgentPanelSetup {
            catalog: Arc::new(ModeWatcher(Arc::clone(&seen))),
            ..setup(vec![])
        });
        // Told the starting mode, then every change.
        panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert!(panel.switch_agent("review"));
        assert_eq!(
            *seen.lock().unwrap(),
            vec![Mode::Configured, Mode::All, Mode::Edit]
        );
    }

    #[test]
    fn mode_switches_from_the_chip_and_with_shift_tab() {
        let mut panel = panel(vec![]);
        let hooks_mode = panel.mode.clone();
        assert_eq!(chip(&panel, MODE_ACTION), "configured");

        // Shift+Tab cycles and reports the new mode in the status line.
        let events = panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert!(events.iter().any(|e| matches!(
            e,
            PanelEvent::SetStatusMessage { message, .. } if message.ends_with("all")
        )));
        assert_eq!(chip(&panel, MODE_ACTION), "all");
        assert_eq!(hooks_mode.get(), Mode::All);

        // The chip opens a picker with the current mode marked.
        let events = panel.handle_status_action(MODE_ACTION);
        let picker = events.first().expect("picker");
        let PanelEvent::ShowSelect { options, .. } = picker else {
            panic!("expected a picker, got {picker:?}");
        };
        assert_eq!(options.len(), 5);
        assert!(options[4].starts_with("● all"), "{:?}", options[4]);
        assert!(options[1].starts_with("  plan"), "{:?}", options[1]);
        assert!(matches!(
            select(&mut panel, picker, 2),
            CommandResult::Handled(true)
        ));
        assert_eq!(chip(&panel, MODE_ACTION), "edit");
        assert_eq!(hooks_mode.get(), Mode::Edit);
        let events = panel.tick();
        assert!(events.iter().any(|e| matches!(
            e,
            PanelEvent::SetStatusMessage { message, .. } if message.ends_with("edit")
        )));

        // Cycling wraps from all to ask, and a rebuilt agent starts in the
        // chosen mode.
        panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
        panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(chip(&panel, MODE_ACTION), "all");
        panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(chip(&panel, MODE_ACTION), "ask");
        panel.switch_session(None);
        assert_eq!(chip(&panel, MODE_ACTION), "ask");
        assert_eq!(panel.mode.get(), Mode::Ask);
    }

    #[test]
    fn model_switches_are_recorded_and_followed_on_resume() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(Scripted::new(vec![reply("one"), reply("two")]));
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup_with(Arc::clone(&provider))
        });
        let first_path = panel.session_path().unwrap().to_path_buf();
        // A fresh session records the model it starts on.
        assert_eq!(
            Session::open(&first_path).unwrap().current_model(),
            Some(termide_agent_core::SessionModel {
                provider: "openai_compatible".into(),
                id: "m".into(),
                context_window: Some(1000),
            })
        );
        assert_eq!(chip(&panel, MODEL_ACTION), "m");

        // The chip fetches the list off-thread; the picker marks the current
        // model and ends with the typed-id entry.
        let events = panel.handle_status_action(MODEL_ACTION);
        assert!(matches!(
            events.first(),
            Some(PanelEvent::SetStatusMessage { .. })
        ));
        let picker = wait_for(&mut panel, |e| matches!(e, PanelEvent::ShowSelect { .. }));
        let PanelEvent::ShowSelect { options, .. } = &picker else {
            unreachable!()
        };
        assert_eq!(options, &["  big", "● m", "  Enter a model id…"]);
        select(&mut panel, &picker, 0);
        assert_eq!(chip(&panel, MODEL_ACTION), "big");
        // The endpoint's context window comes along with the id.
        assert_eq!(panel.model.context_window, 64_000);
        // A fresh session updates the banner in place: the switch leaves no
        // notice, but it is still recorded in the session log.
        assert!(!panel
            .transcript()
            .items()
            .iter()
            .any(|item| matches!(item, Item::Notice { .. })));
        assert_eq!(
            Session::open(&first_path)
                .unwrap()
                .current_model()
                .map(|m| m.id),
            Some("big".to_string())
        );

        // The next run goes to the new model.
        type_text(&mut panel, "go");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        assert_eq!(
            *provider.seen_models.lock().unwrap(),
            vec!["big".to_string()]
        );

        // A new session starts on the current model; the last picker entry
        // asks for an id by hand.
        panel.handle_status_action(NEW_SESSION_ACTION);
        assert_eq!(chip(&panel, MODEL_ACTION), "big");
        panel.handle_status_action(MODEL_ACTION);
        let picker = wait_for(&mut panel, |e| matches!(e, PanelEvent::ShowSelect { .. }));
        select(&mut panel, &picker, 2);
        let input = wait_for(&mut panel, |e| matches!(e, PanelEvent::ShowInput { .. }));
        let PanelEvent::ShowInput {
            initial_value,
            on_submit: InputAction::Custom(action),
            ..
        } = input
        else {
            panic!("expected an input prompt, got {input:?}");
        };
        assert_eq!(initial_value, "big");
        panel.handle_command(PanelCommand::InputSubmitted {
            action,
            text: " typed ".to_string(),
        });
        assert_eq!(chip(&panel, MODEL_ACTION), "typed");
        // A typed id keeps the window of the model it replaced.
        assert_eq!(panel.model.context_window, 64_000);

        // Reopening the first session returns to the model it last used.
        panel.session_choices = panel.session_list();
        let index = panel
            .session_choices
            .iter()
            .position(|s| s.path == first_path)
            .unwrap();
        panel.resume_choice(index);
        assert_eq!(chip(&panel, MODEL_ACTION), "big");
        // The window comes back from the log too.
        assert_eq!(panel.model.context_window, 64_000);
    }

    #[test]
    fn model_picker_falls_back_to_a_typed_id() {
        let mut provider = Scripted::new(vec![]);
        provider.models = Err("HTTP 404: no such route".into());
        let mut panel = AgentPanel::new(setup_with(Arc::new(provider)));
        panel.handle_status_action(MODEL_ACTION);
        let input = wait_for(&mut panel, |e| matches!(e, PanelEvent::ShowInput { .. }));
        assert!(
            matches!(input, PanelEvent::ShowInput { initial_value, .. } if initial_value == "m")
        );
        assert!(panel.transcript().items().iter().any(|item| matches!(
            item,
            Item::Notice { text, .. } if text.contains("HTTP 404")
        )));
    }
    #[test]
    fn saved_state_names_the_directory_and_the_session_log() {
        let no_log = panel(vec![]);
        assert_eq!(
            no_log.to_state(Path::new("/unused")),
            Some(termide_core::PanelState::Agent {
                cwd: PathBuf::from("/tmp"),
                session: None,
                agent: None,
            })
        );

        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("done")])
        });
        type_text(&mut panel, "task");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        let Some(termide_core::PanelState::Agent { cwd, session, .. }) =
            panel.to_state(Path::new("/unused"))
        else {
            panic!("agent state expected");
        };
        assert_eq!(cwd, PathBuf::from("/tmp"));
        let session = session.expect("session path");
        assert_eq!(panel.session_path(), Some(session.as_path()));

        // Rebuilding from that state brings the conversation back.
        let restored = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            session: Some(Session::open(&session).unwrap()),
            ..setup(vec![])
        });
        assert_eq!(restored.title(), "Agent: task");
        assert_eq!(restored.session_path(), Some(session.as_path()));
    }

    #[test]
    fn a_successful_edit_reports_the_changed_file() {
        use termide_agent_core::ToolCall;
        let mut panel = panel(vec![]);
        let call = |name: &str| ToolCall {
            id: format!("{name}-1"),
            name: name.into(),
            arguments: serde_json::json!({}),
        };
        let changed = |events: &[PanelEvent]| -> Vec<PathBuf> {
            events
                .iter()
                .filter_map(|e| match e {
                    PanelEvent::FileChangedOnDisk(path) => Some(path.clone()),
                    _ => None,
                })
                .collect()
        };

        let edit = call("edit");
        panel.apply(AgentEvent::ToolExecutionStart { call: edit.clone() });
        panel.apply(AgentEvent::ToolExecutionEnd {
            result: ToolResultMessage::text(&edit, "Edited")
                .with_details(serde_json::json!({ "path": "/tmp/f.rs", "replacements": 1 })),
        });
        assert_eq!(changed(&panel.tick()), vec![PathBuf::from("/tmp/f.rs")]);

        // A failed edit, a read and a shell command report nothing.
        panel.apply(AgentEvent::ToolExecutionEnd {
            result: ToolResultMessage::error(&edit, "no match")
                .with_details(serde_json::json!({ "path": "/tmp/f.rs" })),
        });
        let read = call("read");
        panel.apply(AgentEvent::ToolExecutionEnd {
            result: ToolResultMessage::text(&read, "…")
                .with_details(serde_json::json!({ "path": "/tmp/g.rs" })),
        });
        let bash = call("bash");
        panel.apply(AgentEvent::ToolExecutionEnd {
            result: ToolResultMessage::text(&bash, "ok"),
        });
        assert!(changed(&panel.tick()).is_empty());
    }
    #[test]
    fn the_system_prompt_can_be_opened_as_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            system_prompt: "You are terse.\n".into(),
            ..setup(vec![])
        });
        // It has no menu slot; `/prompt` is its entry point.
        type_text(&mut panel, "/prompt");
        let events = panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        let Some(PanelEvent::ViewFile(path)) = events.first() else {
            panic!("expected a viewer, got {events:?}");
        };
        assert_eq!(path, &dir.path().join("system-prompt.md"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "You are terse.\n");
        // The prompt file is not mistaken for a session.
        assert!(panel.session_list().iter().all(|s| s.path != *path));
    }

    #[test]
    fn a_system_prompt_block_shows_once_and_on_change() {
        let mut panel = AgentPanel::new(AgentPanelSetup {
            system_prompt: "Base prompt.".into(),
            ..setup(vec![reply("a"), reply("b"), reply("c")])
        });
        let count = |p: &AgentPanel| {
            p.transcript()
                .items()
                .iter()
                .filter(|i| matches!(i, Item::System { .. }))
                .count()
        };

        type_text(&mut panel, "hi");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        assert_eq!(count(&panel), 1, "shown with the first message");

        // Unchanged prompt: not repeated.
        type_text(&mut panel, "again");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        assert_eq!(count(&panel), 1, "not repeated when unchanged");

        // A different agent has a different prompt: shown again.
        assert!(panel.switch_agent("review"));
        type_text(&mut panel, "more");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        assert_eq!(count(&panel), 2, "shown again after it changed");
    }

    #[test]
    fn switching_agents_changes_prompt_model_and_mode_and_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(Scripted::new(vec![reply("ok")]));
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup_with(Arc::clone(&provider))
        });
        assert_eq!(chip(&panel, AGENT_ACTION), "default");

        let events = panel.handle_status_action(AGENT_ACTION);
        let picker = events.first().expect("picker");
        let PanelEvent::ShowSelect { options, .. } = picker else {
            panic!("expected a picker, got {picker:?}");
        };
        assert_eq!(
            options,
            &[
                "● default",
                "  review · Reviews diffs",
                "  outside · An external agent"
            ]
        );
        assert!(matches!(
            select(&mut panel, picker, 1),
            CommandResult::Handled(true)
        ));

        assert_eq!(chip(&panel, AGENT_ACTION), "review");
        assert_eq!(chip(&panel, MODEL_ACTION), "big");
        assert_eq!(chip(&panel, MODE_ACTION), "edit");
        assert_eq!(panel.system_prompt, "You review diffs.");
        // A fresh session keeps its banner: the switch leaves no notice (it is
        // still applied and recorded, checked below on resume).
        assert!(!panel
            .transcript()
            .items()
            .iter()
            .any(|item| matches!(item, Item::Notice { .. })));

        // The next run goes to the reviewer's model, and the layout state
        // names the agent so a restore comes back as it.
        type_text(&mut panel, "go");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        assert_eq!(
            *provider.seen_models.lock().unwrap(),
            vec!["big".to_string()]
        );
        let Some(termide_core::PanelState::Agent { agent, .. }) =
            panel.to_state(Path::new("/unused"))
        else {
            panic!("agent state expected");
        };
        assert_eq!(agent.as_deref(), Some("review"));
        assert_eq!(
            Session::open(panel.session_path().unwrap())
                .unwrap()
                .current_model()
                .unwrap()
                .id,
            "big"
        );

        // A new session starts as the current agent; reopening the first
        // one comes back as the agent it last ran as, prompt and tools too.
        let first_path = panel.session_path().unwrap().to_path_buf();
        panel.handle_status_action(NEW_SESSION_ACTION);
        assert_eq!(chip(&panel, AGENT_ACTION), "review");
        panel.switch_agent("default");
        assert_eq!(panel.system_prompt, "default prompt");
        assert_eq!(chip(&panel, MODEL_ACTION), "big", "the model stays");
        assert_eq!(chip(&panel, MODE_ACTION), "edit", "the mode stays");
        panel.session_choices = panel.session_list();
        let index = panel
            .session_choices
            .iter()
            .position(|s| s.path == first_path)
            .unwrap();
        panel.resume_choice(index);
        assert_eq!(chip(&panel, AGENT_ACTION), "review");
        assert_eq!(panel.system_prompt, "You review diffs.");
        assert_eq!(
            Session::open(&first_path)
                .unwrap()
                .current_agent()
                .as_deref(),
            Some("review")
        );
        assert!(!panel.switch_agent("missing"));
    }
    #[test]
    fn o_opens_the_selected_block_in_a_read_only_panel() {
        let mut panel = AgentPanel::new(setup(vec![reply("the answer here")]));
        type_text(&mut panel, "do it");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE)); // chat focus, last block (assistant)
        let events = panel.handle_key(chord(KeyCode::Char('o'), KeyModifiers::NONE));
        let path = events.iter().find_map(|e| match e {
            PanelEvent::ViewFile(path) => Some(path.clone()),
            _ => None,
        });
        let path = path.expect("o should open a ViewFile panel");
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("the answer here"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn tab_moves_focus_to_the_chat_and_arrows_fold_blocks() {
        // The answer folds only when its reasoning is long enough to be worth
        // hiding.
        let thinking = "l1\nl2\nl3\nl4\nl5\nl6";
        let mut panel = AgentPanel::new(setup(vec![reply_thinking("the answer", thinking)]));
        type_text(&mut panel, "do it");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        // A user block and an assistant block exist.
        assert!(panel.transcript().items().len() >= 2);
        assert!(!panel.chat_focus);

        // Tab moves focus to the chat, on the last (answer) block, which is not
        // foldable. Blocks are user(0), thinking(1), answer(2); the clean run
        // closed on the answer, so its total sits there, not on a closing line.
        assert!(matches!(
            panel.transcript().items().last(),
            Some(Item::Assistant {
                run_ms: Some(_),
                ..
            })
        ));
        panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
        assert!(panel.chat_focus);
        assert_eq!(panel.selected, 2);
        panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(panel.selected, 2);

        // Up walks to the reasoning block, which folds; Space expands it, again
        // folds it.
        panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(panel.selected, 1);
        assert!(!panel.transcript().any_expanded());
        panel.handle_key(chord(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(panel.transcript().any_expanded());
        panel.handle_key(chord(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(!panel.transcript().any_expanded());

        // Up walks to the user block; a printable key does not type.
        panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(panel.selected, 0);
        panel.handle_key(chord(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(panel.input_text().is_empty());

        // Tab returns focus to the input.
        panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
        assert!(!panel.chat_focus);
        type_text(&mut panel, "hi");
        assert_eq!(panel.input_text(), "hi");
    }

    #[test]
    fn file_completions_list_paths_and_at_mentions_insert_them() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("README.md"), "hi").unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "ref").unwrap();

        // The walker skips .git and offers files and directories.
        let all = file_completions(dir.path(), "");
        let values: Vec<&str> = all.iter().map(|i| i.value.as_str()).collect();
        assert!(values.contains(&"README.md"), "{values:?}");
        assert!(values.contains(&"src/"), "{values:?}");
        assert!(values.contains(&"src/main.rs"), "{values:?}");
        assert!(!values.iter().any(|v| v.contains(".git")), "{values:?}");
        // A prefix filters, name matches rank first.
        let main = file_completions(dir.path(), "main");
        assert_eq!(main.first().map(|i| i.value.as_str()), Some("src/main.rs"));

        // Typing @ opens the file popup; selecting a file replaces the token.
        let mut panel = AgentPanel::new(AgentPanelSetup {
            cwd: dir.path().to_path_buf(),
            ..setup(vec![])
        });
        type_text(&mut panel, "look at @READ");
        assert!(panel.completion.is_some(), "no @ completion popup");
        assert!(panel.completion_span.is_some());
        assert!(panel
            .completion
            .as_ref()
            .unwrap()
            .items()
            .iter()
            .any(|i| i.value == "README.md"));
        panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(panel.input_text(), "look at README.md ");
        assert!(panel.completion.is_none());

        // A directory keeps the popup open for its contents.
        type_text(&mut panel, "@src");
        panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(panel.input_text(), "look at README.md @src/");
        assert!(panel.completion.is_some(), "dir did not reopen the popup");
        assert!(panel
            .completion
            .as_ref()
            .unwrap()
            .items()
            .iter()
            .any(|i| i.value == "src/main.rs"));
    }

    #[test]
    fn slash_commands_expand_prompt_templates() {
        let mut expanding = panel(vec![reply("done")]);
        type_text(&mut expanding, "/review src/x.rs");
        expanding.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut expanding);
        let items = expanding.transcript().items();
        assert!(
            matches!(&items[0], Item::User { text, .. } if text == "Review src/x.rs carefully."),
            "{items:?}"
        );

        // An unknown command is refused with the names on offer; a path is text.
        let mut panel = panel(vec![reply("ok")]);
        type_text(&mut panel, "/nope");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        assert!(panel.transcript().items().iter().any(
            |item| matches!(item, Item::Notice { text, .. } if text.contains("available: review"))
        ));
        assert_eq!(panel.input_text(), "/nope", "the input is kept for editing");
        assert_eq!(slash_command("/usr/bin/ls -la"), None);
        assert_eq!(slash_command("/review a b"), Some(("review", "a b")));
        assert_eq!(slash_command("/"), None);

        // The picker puts the command into the input, ready for arguments.
        let events = panel.handle_status_action(PROMPTS_ACTION);
        let picker = events.first().expect("picker");
        let PanelEvent::ShowSelect { options, .. } = picker else {
            panic!("expected a picker, got {picker:?}");
        };
        assert_eq!(options, &["/review <path> · Review a file"]);
        select(&mut panel, picker, 0);
        assert_eq!(panel.input_text(), "/review ");
    }
    /// A tool with nothing behind it, standing in for one an MCP server sent.
    struct Late(&'static str);

    impl termide_agent_core::Tool for Late {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "late"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }
        fn execute(
            &self,
            call: &termide_agent_core::ToolCall,
            _ctx: &termide_agent_core::ToolContext,
            _on_update: &mut dyn FnMut(ToolUpdate),
            _cancel: &CancelToken,
        ) -> ToolResultMessage {
            ToolResultMessage::text(call, "late")
        }
    }

    #[test]
    fn late_tools_join_the_registry_and_failures_are_reported() {
        let (tx, rx) = mpsc::channel();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            late_tools: Some(rx),
            ..setup(vec![])
        });
        tx.send(LateTools::Ready {
            source: "github".into(),
            tools: vec![Arc::new(Late("github__search"))],
        })
        .unwrap();
        tx.send(LateTools::Failed {
            source: "ghost".into(),
            error: "cannot start ghost-server".into(),
        })
        .unwrap();
        let events = panel.tick();
        assert!(!events.is_empty());
        assert!(panel.tools.get("github__search").is_some());
        let notices: Vec<String> = panel
            .transcript()
            .items()
            .iter()
            .filter_map(|item| match item {
                Item::Notice { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            notices,
            [
                "mcp github: 1 tools connected",
                "mcp ghost: cannot start ghost-server"
            ]
        );
        // The worker got them too: the next run sees the tool in its registry.
        let runtime = std::mem::replace(&mut panel.runtime, Box::new(Idle));
        let agent = runtime.into_agent().expect("agent");
        assert!(agent.tools().get("github__search").is_some());
    }
    /// A backend with nothing behind it, to swap out of a panel in a test.
    struct Idle;

    impl Backend for Idle {
        fn prompt(&self, _message: UserMessage) -> Result<(), PromptError> {
            Err(PromptError::Stopped)
        }
        fn steer(&self, _message: UserMessage) {}
        fn queue_lens(&self) -> (usize, usize) {
            (0, 0)
        }
        fn abort(&self) {}
        fn is_busy(&self) -> bool {
            false
        }
        fn drain(&self) -> Vec<AgentEvent> {
            Vec::new()
        }
        fn update(&self, _update: Box<dyn FnOnce(&mut Agent) + Send>) -> Result<(), PromptError> {
            Err(PromptError::Unsupported)
        }
        fn compact(&self, _focus: Option<String>) -> Result<(), PromptError> {
            Err(PromptError::Unsupported)
        }
        fn into_agent(self: Box<Self>) -> Option<Agent> {
            None
        }
    }

    /// An "external agent" for tests: answers every prompt with one text
    /// message, through the same events as the real ACP backend.
    struct External {
        events: Mutex<Vec<AgentEvent>>,
    }

    impl External {
        fn new(_setup: BackendSetup) -> Self {
            Self {
                events: Mutex::new(Vec::new()),
            }
        }
    }

    impl Backend for External {
        fn prompt(&self, message: UserMessage) -> Result<(), PromptError> {
            let mut events = self.events.lock().unwrap();
            events.push(AgentEvent::AgentStart);
            events.push(AgentEvent::MessageEnd(Message::User(message)));
            // Like an ACP adapter: the message starts with its first text.
            events.push(AgentEvent::MessageStart);
            events.push(AgentEvent::MessageUpdate(StreamEvent::TextDelta(
                "from outside".into(),
            )));
            events.push(AgentEvent::MessageEnd(Message::Assistant(
                AssistantMessage {
                    content: vec![AssistantContent::Text {
                        text: "from outside".into(),
                    }],
                    stop_reason: StopReason::Stop,
                    usage: Usage::default(),
                    provider: "acp".into(),
                    model: "outside".into(),
                    error_message: None,
                    timestamp: 0,
                },
            )));
            events.push(AgentEvent::AgentEnd);
            Ok(())
        }
        fn steer(&self, _message: UserMessage) {}
        fn queue_lens(&self) -> (usize, usize) {
            (0, 0)
        }
        fn abort(&self) {}
        fn is_busy(&self) -> bool {
            false
        }
        fn drain(&self) -> Vec<AgentEvent> {
            std::mem::take(&mut *self.events.lock().unwrap())
        }
        fn update(&self, _update: Box<dyn FnOnce(&mut Agent) + Send>) -> Result<(), PromptError> {
            Err(PromptError::Unsupported)
        }
        fn compact(&self, _focus: Option<String>) -> Result<(), PromptError> {
            Err(PromptError::Unsupported)
        }
        fn into_agent(self: Box<Self>) -> Option<Agent> {
            None
        }
    }

    #[test]
    fn an_external_agent_replaces_the_loop_and_hides_its_knobs() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("native")])
        });
        type_text(&mut panel, "hello");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);

        assert!(panel.switch_agent("outside"));
        assert_eq!(chip(&panel, AGENT_ACTION), "outside");
        let texts: Vec<String> = panel
            .status_segments()
            .into_iter()
            .map(|s| s.text)
            .collect();
        assert!(!texts.iter().any(|t| t.starts_with("Mode")), "{texts:?}");
        assert!(!texts.iter().any(|t| t.starts_with("Model")), "{texts:?}");
        assert!(texts.contains(&" (acp)".to_string()));
        // The earlier conversation is shown, and marked as unknown to the agent.
        let items = panel.transcript().items();
        assert!(matches!(&items[0], Item::User { text, .. } if text == "hello"));
        assert!(items.iter().any(
            |i| matches!(i, Item::Notice { text, .. } if text.contains("not known to the external agent"))
        ));

        // Model and mode are not ours any more.
        let events = panel.handle_status_action(MODE_ACTION);
        assert!(!events
            .iter()
            .any(|e| matches!(e, PanelEvent::ShowSelect { .. })));
        panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(panel.mode.get(), Mode::Configured);

        type_text(&mut panel, "go");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        assert!(panel
            .transcript()
            .items()
            .iter()
            .any(|i| matches!(i, Item::Assistant { text, .. } if text == "from outside")));
        // It reports no tokens and times no prefill: no made-up indicators.
        assert!(panel.transcript().items().iter().all(
            |i| !matches!(i, Item::Assistant { text, cost: Some(_), .. } if text == "from outside")
        ));
        // The log records both the switch and the external agent's answer.
        let session = Session::open(panel.session_path().unwrap()).unwrap();
        assert_eq!(session.current_agent().as_deref(), Some("outside"));
        assert_eq!(session.context_messages().len(), 4);

        // Back to the built-in loop.
        assert!(panel.switch_agent("default"));
        assert!(!panel.external);
        assert_eq!(chip(&panel, MODE_ACTION), "configured");
    }

    /// An external agent that advertises two models and records the one picked.
    struct ModelBackend {
        picked: Arc<Mutex<Option<String>>>,
    }

    impl Backend for ModelBackend {
        fn prompt(&self, _message: UserMessage) -> Result<(), PromptError> {
            Ok(())
        }
        fn steer(&self, _message: UserMessage) {}
        fn queue_lens(&self) -> (usize, usize) {
            (0, 0)
        }
        fn abort(&self) {}
        fn is_busy(&self) -> bool {
            false
        }
        fn drain(&self) -> Vec<AgentEvent> {
            Vec::new()
        }
        fn update(&self, _update: Box<dyn FnOnce(&mut Agent) + Send>) -> Result<(), PromptError> {
            Err(PromptError::Unsupported)
        }
        fn compact(&self, _focus: Option<String>) -> Result<(), PromptError> {
            Err(PromptError::Unsupported)
        }
        fn available_models(&self) -> Vec<BackendModel> {
            vec![
                BackendModel {
                    id: "m-fast".into(),
                    name: "Fast".into(),
                },
                BackendModel {
                    id: "m-slow".into(),
                    name: "Slow".into(),
                },
            ]
        }
        fn current_model(&self) -> Option<String> {
            Some(
                self.picked
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| "m-fast".into()),
            )
        }
        fn select_model(&self, model_id: String) -> Result<(), String> {
            *self.picked.lock().unwrap() = Some(model_id);
            Ok(())
        }
        fn into_agent(self: Box<Self>) -> Option<Agent> {
            None
        }
    }

    #[test]
    fn an_external_agent_lists_and_switches_models() {
        let dir = tempfile::tempdir().unwrap();
        let picked = Arc::new(Mutex::new(None));
        let for_factory = Arc::clone(&picked);
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            backend: Some(Arc::new(move |_setup: BackendSetup| {
                Ok(Box::new(ModelBackend {
                    picked: Arc::clone(&for_factory),
                }) as Box<dyn Backend>)
            })),
            ..setup(vec![])
        });
        assert!(panel.external);
        // A tick adopts the advertised current model for the banner and chip.
        panel.tick();
        assert_eq!(panel.model.id, "m-fast");
        let texts: Vec<String> = panel
            .status_segments()
            .into_iter()
            .map(|s| s.text)
            .collect();
        assert!(texts.iter().any(|t| t == "m-fast"), "{texts:?}");

        // The Model chip opens the agent's model list.
        let events = panel.handle_status_action(MODEL_ACTION);
        let Some(PanelEvent::ShowSelect { options, .. }) = events.first() else {
            panic!("expected a model picker, got {events:?}");
        };
        assert_eq!(options.len(), 2);
        assert!(options.iter().any(|o| o.contains("Slow")), "{options:?}");

        // Picking the second switches it over ACP and updates the chip.
        panel.handle_command(PanelCommand::SelectionMade {
            action: MODEL_ACTION.to_string(),
            index: 1,
        });
        assert_eq!(*picked.lock().unwrap(), Some("m-slow".to_string()));
        assert_eq!(panel.model.id, "m-slow");
    }

    #[test]
    fn a_cli_provider_pre_selects_its_configured_model_on_start() {
        let dir = tempfile::tempdir().unwrap();
        let picked = Arc::new(Mutex::new(None));
        let for_factory = Arc::clone(&picked);
        let mut base = setup(vec![]);
        // A CLI provider with a configured model to pre-select.
        base.provider_kind = "claude_code".into();
        base.model = ModelSpec {
            id: "m-slow".into(),
            ..base.model
        };
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            backend: Some(Arc::new(move |_setup: BackendSetup| {
                Ok(Box::new(ModelBackend {
                    picked: Arc::clone(&for_factory),
                }) as Box<dyn Backend>)
            })),
            ..base
        });
        assert!(panel.external);
        // The agent starts on "m-fast"; a tick applies the configured "m-slow".
        panel.tick();
        assert_eq!(*picked.lock().unwrap(), Some("m-slow".to_string()));
        assert_eq!(panel.model.id, "m-slow");
    }

    #[test]
    fn arrow_keys_recall_earlier_requests_and_bring_the_draft_back() {
        let mut panel = panel(vec![reply("a"), reply("b")]);
        for request in ["first request", "second request"] {
            type_text(&mut panel, request);
            panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
            settle(&mut panel);
        }
        type_text(&mut panel, "half typ");
        panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(panel.input_text(), "second request");
        panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(panel.input_text(), "first request");
        // Past the oldest it stays; back down it returns to the draft.
        panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(panel.input_text(), "first request");
        panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
        panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(panel.input_text(), "half typ");
        assert!(panel.history_pos.is_none());
        // Typing ends browsing; the arrows then move inside a multi-line input.
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::SHIFT));
        type_text(&mut panel, "more");
        panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(panel.input_text(), "half typ\nmore");
        assert_eq!(format_tokens(32_000), "32k");
        assert_eq!(format_tokens(262_144), "262k");
        assert_eq!(format_tokens(1_000_000), "1M");
        assert_eq!(format_tokens(1_250_000), "1.2M");
        assert_eq!(format_tokens(512), "512");
    }

    #[test]
    fn typing_a_slash_offers_templates_and_tab_or_enter_completes() {
        let mut panel = panel(vec![reply("ok")]);
        type_text(&mut panel, "/re");
        let popup = panel.completion.as_ref().expect("popup");
        assert_eq!(popup.items()[0].value, "review");
        let rows = render_text(&mut panel, 60, 12);
        assert!(
            rows.iter()
                .any(|r| r.contains("/review <path>  Review a file")),
            "{rows:?}"
        );
        // Tab completes and closes the popup; the space invites arguments.
        panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(panel.input_text(), "/review ");
        assert!(panel.completion.is_none());

        // Enter on a partial name completes; on the full name it sends.
        panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
        type_text(&mut panel, "/rev");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(panel.input_text(), "/review ");
        assert!(panel.transcript().items().is_empty());
        panel.handle_key(chord(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(
            panel.completion.is_some(),
            "a lone /review shows the popup again"
        );
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        assert!(matches!(
            panel.transcript().items().first(),
            Some(Item::User { text, .. }) if text == "Review  carefully."
        ));

        // No match, no popup; Esc closes an open one.
        type_text(&mut panel, "/zzz");
        assert!(panel.completion.is_none());
        panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
        type_text(&mut panel, "/r");
        assert!(panel.completion.is_some());
        panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
        assert!(panel.completion.is_none());
        assert_eq!(
            panel.input_text(),
            "/r",
            "Esc closes the popup, not the input"
        );
    }
    #[test]
    fn slash_compact_is_built_in_and_reports_through_the_transcript() {
        let mut panel = panel(vec![]);
        type_text(&mut panel, "/comp");
        let popup = panel.completion.as_ref().expect("popup");
        assert!(popup.items().iter().any(|i| i.value == "compact"));
        panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(panel.input_text(), "/compact ");
        type_text(&mut panel, "the tests");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(panel.input_text(), "", "the command is consumed, not sent");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            panel.tick();
            let failed = panel.transcript().items().iter().any(|item| {
                matches!(item, Item::Notice { text, .. } if text.contains("too few messages"))
            });
            if failed {
                break;
            }
            assert!(Instant::now() < deadline, "no compaction notice");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(panel
            .transcript()
            .items()
            .iter()
            .all(|i| !matches!(i, Item::User { .. })));
    }
    #[cfg(unix)]
    #[test]
    fn command_scripts_run_and_a_project_one_asks_first() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = |name: &str, body: &str, trusted: bool| {
            let path = dir.path().join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            CommandScript::from_file(&path, trusted).unwrap()
        };
        let gather = script(
            "gather",
            "#!/bin/sh\n# description: Gather context\n# argument-hint: <topic>\necho \"Context about $1\"\n",
            true,
        );
        let project = script(
            "scan",
            "#!/bin/sh\n# description: Scan\necho scanned\n",
            false,
        );
        *COMMANDS.lock().unwrap() = vec![gather, project];
        let mut panel = panel(vec![reply("a"), reply("b")]);
        let wait_user = |panel: &mut AgentPanel, text: &str| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                panel.tick();
                if panel
                    .transcript()
                    .items()
                    .iter()
                    .any(|i| matches!(i, Item::User { text: t, .. } if t == text))
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "{:?}",
                    panel.transcript().items()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        };

        // The trusted script runs unasked and its output is the request.
        type_text(&mut panel, "/ga");
        assert!(panel
            .completion
            .as_ref()
            .unwrap()
            .items()
            .iter()
            .any(|i| i.value == "gather"));
        panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
        type_text(&mut panel, "parsers");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(panel.input_text(), "");
        wait_user(&mut panel, "Context about parsers");
        settle(&mut panel);

        // The project's script asks; "run for this session" runs it now and
        // next time without asking.
        type_text(&mut panel, "/scan");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        let form = panel.pending.as_ref().expect("a card asks").form();
        assert!(form.title().starts_with("Run the project command /scan"));
        assert_eq!(form.options().len(), 4);
        panel.handle_key(chord(KeyCode::Char('2'), KeyModifiers::NONE));
        assert!(panel.pending.is_none());
        wait_user(&mut panel, "scanned");
        settle(&mut panel);
        assert!(panel.allowed_commands.contains("scan"));

        // "Don't run" leaves nothing behind.
        *COMMANDS.lock().unwrap() = vec![script("other", "#!/bin/sh\necho x\n", false)];
        type_text(&mut panel, "/other");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        assert!(panel.pending.is_some());
        panel.handle_key(chord(KeyCode::Char('4'), KeyModifiers::NONE));
        assert!(panel.pending.is_none() && panel.command_run.is_none());
        *COMMANDS.lock().unwrap() = Vec::new();
    }
    #[test]
    fn undo_restores_the_files_and_rewinds_the_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions");
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "before").unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            cwd: dir.path().to_path_buf(),
            session_dir: Some(sessions),
            ..setup(vec![reply("a"), reply("b"), reply("c")])
        });
        type_text(&mut panel, "task one");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        // Nothing changed files yet: /undo has nothing to offer.
        type_text(&mut panel, "/undo");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        assert!(panel.pending.is_none());
        assert!(panel
            .transcript()
            .items()
            .iter()
            .any(|i| matches!(i, Item::Notice { text, .. } if text.contains("nothing to undo"))));

        // The second request "edits" the file: the store records it the way
        // the hook does for the edit tool.
        let leaf = panel
            .session
            .as_ref()
            .unwrap()
            .leaf_id()
            .map(str::to_string);
        {
            let store = panel.checkpoints.as_ref().unwrap();
            let mut store = store.lock().unwrap();
            store.begin_run(leaf);
            store.save(&file).unwrap();
            std::fs::write(&file, "after").unwrap();
            store.end_run();
        }
        type_text(&mut panel, "task two");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        assert_eq!(panel.session.as_ref().unwrap().context_messages().len(), 4);

        type_text(&mut panel, "/un");
        assert!(panel
            .completion
            .as_ref()
            .unwrap()
            .items()
            .iter()
            .any(|i| i.value == "undo"));
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE)); // completes
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE)); // sends /undo
        let form = panel.pending.as_ref().expect("undo card").form();
        assert!(form.title().contains("notes.txt"), "{}", form.title());
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        let events = panel.tick();
        assert!(events
            .iter()
            .any(|e| matches!(e, PanelEvent::FileChangedOnDisk(p) if p == &file)));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "before");
        // The conversation is back at the end of task one, on disk too.
        assert_eq!(panel.session.as_ref().unwrap().context_messages().len(), 2);
        assert!(!panel
            .transcript()
            .items()
            .iter()
            .any(|i| matches!(i, Item::User { text, .. } if text == "task two")));
        assert!(panel.transcript().items().iter().any(
            |i| matches!(i, Item::Notice { text, .. } if text.contains("undid the last request"))
        ));
        let reopened = Session::open(panel.session_path().unwrap()).unwrap();
        assert_eq!(reopened.context_messages().len(), 2);
    }
    #[test]
    fn plan_mode_adds_its_instructions_and_offers_to_carry_the_plan_out() {
        let dir = tempfile::tempdir().unwrap();
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            system_prompt: "Base prompt.".into(),
            plan_prompt: PlanPrompt::from_file("---\nrequest: Do it.\n---\nPlan first."),
            ..setup(vec![reply("1. change a\n2. change b"), reply("done")])
        });
        // ask → accept-edits → auto → plan
        for _ in 0..3 {
            panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
        }
        assert_eq!(panel.mode.get(), Mode::Plan);
        assert_eq!(chip(&panel, MODE_ACTION), "plan");
        let shown = panel.write_system_prompt().unwrap();
        assert_eq!(
            std::fs::read_to_string(shown).unwrap(),
            "Base prompt.\n\nPlan first."
        );

        type_text(&mut panel, "add a feature");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        let form = panel.pending.as_ref().expect("plan card").form();
        assert!(form.title().starts_with("Plan mode"), "{}", form.title());

        // Esc keeps planning; the next answer offers again.
        panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
        assert!(panel.pending.is_none());
        assert_eq!(panel.mode.get(), Mode::Plan);

        panel.offer_plan();
        panel.handle_key(chord(KeyCode::Char('1'), KeyModifiers::NONE));
        let _ = panel.tick();
        assert_eq!(panel.mode.get(), Mode::Edit);
        settle(&mut panel);
        let users: Vec<&str> = panel
            .transcript()
            .items()
            .iter()
            .filter_map(|i| match i {
                Item::User { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(users, ["add a feature", "Do it."]);
        let shown = panel.write_system_prompt().unwrap();
        assert_eq!(std::fs::read_to_string(shown).unwrap(), "Base prompt.");
        assert!(panel.pending.is_none(), "no card outside plan mode");
    }
}
