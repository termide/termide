//! An external agent over the Agent Client Protocol as a [`Backend`] of the
//! panel.
//!
//! termide is the ACP *client*: it starts the agent process, speaks
//! newline-delimited JSON-RPC 2.0 on its stdin/stdout and translates
//! `session/update` notifications into the same [`AgentEvent`]s the built-in
//! loop emits, so the panel renders both alike. The agent's own requests
//! are answered here: `session/request_permission` goes to the user through
//! the shared permission channel, `fs/read_text_file` and
//! `fs/write_text_file` are served from the working directory, terminals
//! are not offered.
//!
//! Starting an agent (`npx …` adapters take seconds) happens on a thread; a
//! prompt sent before the session exists waits in the queue and goes out as
//! soon as it does.

use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use termide_agent_core::{chars_for_tokens, handoff, recap};
use termide_agent_core::{
    companion_tools, parse_verdict, GoalPrompt, HandoffPrompt, IntentLog, ModelSpec, Provider,
    ReviewerSetup, SessionView, Spender, ThinkingLevel,
};
use termide_agent_core::{
    expand_env, now_millis, AcpConfig, AcpFlavor, Agent, AgentCommand, AgentEvent,
    AssistantContent, AssistantMessage, Backend, BackendModel, BackendOption, BackendSetup,
    BackgroundCall, CancelToken, ExternalSessionRef, Failure, FailureKind, Hooks, HostTools,
    Message, Mode, ModeHandle, PermissionHooks, PlanPrompt, PromptError, StopReason, StreamEvent,
    ToolCall, ToolContext, ToolDecision, ToolRegistry, ToolResultMessage, Usage, UserMessage,
    ACP_PROVIDER, RECAP_LIMIT,
};
use termide_agent_core::{CompactionCall, CompactionReason, SHELL_TOOL};
use termide_agent_mcp::{McpServer, SERVER_NAME};

mod provider;
mod windows;
pub use provider::{match_model, AcpProvider};
pub use windows::remember_windows_in;

/// How the calls of termide's tools reach the transcript from Claude Code:
/// named after the MCP server that serves them.
const HOST_TOOL_PREFIX: &str = "mcp__termide__";

/// What a retry tells an agent whose failed turn had already called tools,
/// in place of sending the request again.
const RETRY_NOTE: &str = "<system-reminder>\nYour previous turn ended with an error before it finished. Carry on with the last request from where it stopped.\n</system-reminder>";

/// The reset of a limit Claude Code reports rejected (`_claude/rateLimit`:
/// `status` and `resetsAt` in Unix seconds); `None` while it allows.
fn rejected_until(limit: &Value) -> Option<u64> {
    (limit["status"].as_str() == Some("rejected"))
        .then(|| limit["resetsAt"].as_u64())
        .flatten()
}

/// The protocol version requested.
pub const PROTOCOL_VERSION: u64 = 1;

/// How long Codex and Gemini CLI wait to open their session for termide's MCP
/// servers to answer: they list the tools of an MCP server once, when the
/// session starts, so tools that come later never reach them.
const MCP_SETTLE: Duration = Duration::from_secs(10);

type Pending = Arc<Mutex<HashMap<u64, Sender<Result<Value, RpcFailure>>>>>;

/// A request that failed: the error as the panel shows it, and its kind.
#[derive(Debug, Clone)]
struct RpcFailure {
    text: String,
    class: Failure,
}

impl RpcFailure {
    fn new(text: String, kind: FailureKind) -> Self {
        Self {
            text,
            class: Failure::new(kind),
        }
    }
}

/// Classify a JSON-RPC error of the agent. ACP has no codes of its own for
/// limits, so the adapters' `data.errorKind` (Claude Code's: the SDK's
/// assistant-message error) is read first, then the text — which is all
/// Codex and Gemini CLI give.
fn classify_rpc_error(error: &Value) -> Failure {
    let message = error["message"].as_str().unwrap_or_default();
    let from_text = Failure::from_text(message);
    // `authRequired` in ACP.
    if error["code"].as_i64() == Some(-32000) {
        return Failure::new(FailureKind::Auth);
    }
    let kind = match error["data"]["errorKind"].as_str() {
        // A subscription's usage limit is reported as a rate limit too.
        Some("rate_limit") if from_text.kind == FailureKind::QuotaExhausted => {
            FailureKind::QuotaExhausted
        }
        Some("rate_limit") => FailureKind::RateLimited,
        Some("billing_error" | "account_on_hold") => FailureKind::QuotaExhausted,
        Some(
            "authentication_failed"
            | "oauth_org_not_allowed"
            | "verification_required"
            | "cloud_credential_error",
        ) => FailureKind::Auth,
        Some("overloaded" | "server_error") => FailureKind::Transient,
        Some("invalid_request" | "model_not_found") => FailureKind::BadRequest,
        Some("transport_lost" | "worker_shutdown") => FailureKind::AgentDied,
        _ => from_text.kind,
    };
    Failure::new(kind).with_reset_from(message)
}
type SharedWriter = Arc<Mutex<Option<Box<dyn Write + Send>>>>;

/// How long a request that saw the agent hang up waits for the rest of what
/// it printed on stderr, which says why.
const STDERR_GRACE: Duration = Duration::from_millis(300);

/// The last line the agent's process printed on stderr. "Closed the
/// connection" alone does not say why it went: a launcher refused to run it
/// (`Permission denied`), a missing login, a crash — the line does.
#[derive(Default)]
struct StderrTail {
    /// The last non-empty line, and whether the stream has ended.
    state: Mutex<(Option<String>, bool)>,
    ended: Condvar,
}

impl StderrTail {
    /// Read `stderr` to its end on a thread of its own, logging each line.
    fn follow(self: &Arc<Self>, agent: String, stderr: impl Read + Send + 'static) {
        let tail = Arc::clone(self);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                log::debug!("acp {agent}: {line}");
                let line = line.trim();
                if !line.is_empty() {
                    tail.state.lock().unwrap_or_else(PoisonError::into_inner).0 =
                        Some(line.to_string());
                }
            }
            tail.state.lock().unwrap_or_else(PoisonError::into_inner).1 = true;
            tail.ended.notify_all();
        });
    }

    /// The last line, once the stream has ended or `grace` has passed.
    fn last(&self, grace: Duration) -> Option<String> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (state, _) = self
            .ended
            .wait_timeout_while(state, grace, |(_, ended)| !*ended)
            .unwrap_or_else(PoisonError::into_inner);
        state.0.clone()
    }
}

/// Where the connection stands.
enum Conn {
    Starting,
    Ready { session_id: String },
    Failed(String),
}

struct Shared {
    writer: SharedWriter,
    pending: Pending,
    next_id: AtomicU64,
    events: Sender<AgentEvent>,
    conn: Mutex<Conn>,
    /// Messages waiting for the session or for the current turn to end.
    queue: Mutex<Vec<UserMessage>>,
    busy: AtomicBool,
    cancel: CancelToken,
    /// The same permission logic the built-in agent runs: read-only commands
    /// and matching rules pass without a prompt, and grants (session or
    /// always) are remembered here so a request is asked only once.
    hooks: Mutex<PermissionHooks>,
    cwd: PathBuf,
    name: String,
    /// The assistant message being streamed, if any: text and thought count.
    open_message: Mutex<Option<String>>,
    /// The reasoning streamed into that message, kept with it in the log as
    /// the built-in loop keeps its own.
    open_thought: Mutex<String>,
    child: Mutex<Option<Child>>,
    /// What the agent's process prints on stderr, when termide started it.
    stderr: Option<Arc<StderrTail>>,
    /// Models the agent advertised at `session/new`, for the picker; empty when
    /// it advertises none.
    models: Mutex<Vec<BackendModel>>,
    /// The agent's current model id, from `session/new` and kept up to date by
    /// `current_model_update` notifications and `select_model`.
    current_model: Mutex<Option<String>>,
    /// The id of the session config option that picks the model, when the
    /// agent offers its models that way (`configOptions`) rather than as
    /// `models`; a switch then goes through `session/set_config_option`.
    model_option: Mutex<Option<String>>,
    /// The session's config options of the `select` type, the model's among
    /// them, as the agent last stated them all: in the session's result, in a
    /// `session/set_config_option` reply or a `config_option_update`.
    options: Mutex<Vec<BackendOption>>,
    /// The commands the agent offers, as it last listed them.
    commands: Mutex<Vec<AgentCommand>>,
    /// Which adapter this is.
    flavor: AcpFlavor,
    /// termide's system prompt, for an adapter that takes it.
    system_prompt: String,
    /// termide's tools, until the handshake serves them.
    host_tools: Mutex<Option<HostTools>>,
    /// The panel's session log, for the calls' [`SessionView`].
    session_log: Option<String>,
    /// The server of termide's tools, alive as long as the agent.
    mcp_server: Mutex<Option<McpServer>>,
    /// Calls of termide's tools the agent closed while the server still ran
    /// them (see [`Backend::background_calls`]), as their blocks have them.
    background: Mutex<Vec<ToolCall>>,
    /// Whether termide's MCP servers have all answered, which Codex and
    /// Gemini CLI wait on (for [`MCP_SETTLE`] at most) to open their session.
    mcp_settled: Mutex<bool>,
    mcp_settled_changed: Condvar,
    /// Calls announced without their arguments yet, by id.
    announced: Mutex<HashMap<String, Value>>,
    /// The running calls of termide's own tools, by id, with the tool's name
    /// and arguments: the agent may still ask for their permission, which
    /// termide judges on its server, their end may not name the tool again,
    /// and the details the server kept for the UI are found by them. The
    /// server knows each call it runs by its id here.
    host_calls: Arc<Mutex<HashMap<String, (String, Value)>>>,
    /// The calls on their way into the session log.
    calls: Mutex<CallLog>,
    /// The context's fill and size, `(used, size)`, as `usage_update` last
    /// reported them.
    context: Mutex<Option<(u64, u64)>>,
    /// The window the user limited the agent to, 0 for none: a reported
    /// size above it is taken as it.
    context_limit: AtomicU64,
    /// The panel's live permission mode.
    mode: ModeHandle,
    /// Plan mode's texts, for telling Claude Code of a switch.
    plan: PlanPrompt,
    /// Whether what Claude Code was last told — its system prompt, then the
    /// notes since — has plan mode on. The prompt is fixed for the session,
    /// so a later switch reaches it as a note before the next turn.
    told_plan: AtomicBool,
    /// termide's tools Claude Code was last told it has — those its session
    /// started with, then those a note named — and those served now, which
    /// change as the session switches tools or MCP servers come and go; a
    /// difference reaches it as a note before the next turn.
    told_tools: Mutex<Option<BTreeSet<String>>>,
    served_tools: Mutex<BTreeSet<String>>,
    /// The agent's own session to resume, when the log names one.
    resume: Option<ExternalSessionRef>,
    /// What the reviewer judges the agent's requests against: the user's
    /// messages and the agent's calls, as the built-in loop records them.
    intent: IntentLog,
    /// The agent's own subscription for the reviewer that runs on "the
    /// session's model", when the agent was started from a command.
    session_provider: OnceLock<Arc<dyn Provider>>,
    /// The conversation so far, until the handshake knows whether the
    /// agent resumed it or is to be told a recap of it.
    history: Mutex<Vec<Message>>,
    /// The conversation a new session of the agent is still to be told,
    /// before its first request: as a record cut down to half its window
    /// ([`handoff`]) when that is known, else as a recap of the decisions
    /// ([`recap`]). Made when the request goes, once the model is settled.
    untold: Mutex<Option<Vec<Message>>>,
    /// Where the windows agents report are remembered, see [`windows`].
    windows: OnceLock<PathBuf>,
    /// Set while `session/load` replays the conversation, which the panel
    /// already shows and the log already holds.
    replaying: AtomicBool,
    /// Whether the agent takes a message into the turn it is running
    /// (`_session/steering`, which its `initialize` result announces under
    /// `_meta.steering.supported`), as the built-in loop does at a step.
    steering: AtomicBool,
    /// Set while a `session/prompt` is out: the window in which a message
    /// can steer the turn.
    turn_running: AtomicBool,
    /// Messages the running turn took, waiting to be logged until the calls
    /// it has open are answered, so the log never puts a message between a
    /// call and its result.
    steered: Mutex<Vec<UserMessage>>,
    /// The `_session/steering` requests out, by id, with their message: the
    /// reply is taken in the reader, in order with the updates, so the
    /// message is logged before the text that answers it.
    steering_out: Mutex<HashMap<u64, UserMessage>>,
    /// A connection for side requests alone ([`AcpProvider`]): the
    /// handshake opens no session, each request opens its own.
    service: bool,
    /// Whether the agent forks a session (`session/fork`) and closes one
    /// (`session/close`), which a side request (the `/goal` judge, the
    /// `/handoff` brief) is sent in, so the conversation is left as it was.
    can_fork: AtomicBool,
    can_close: AtomicBool,
    /// Whether the agent takes image blocks in a prompt
    /// (`promptCapabilities.image`); without it attached images are left out.
    takes_images: AtomicBool,
    /// The side request in flight: the session it runs in and the answer
    /// so far, which reaches neither the panel nor the log.
    side: Mutex<HashMap<String, String>>,
    goal: GoalPrompt,
    handoff: HandoffPrompt,
    /// The blocks of the last turn sent, kept while it failed, for a retry
    /// to send them again.
    last_prompt: Mutex<Option<Vec<Value>>>,
    /// Whether the turn running (or the last one) called a tool: a failed
    /// turn that did is not sent again as it was, it is asked to go on.
    turn_acted: AtomicBool,
    /// A retry waits for the session like a message; its blocks are made
    /// when it goes, once a new agent's handshake has settled the recap.
    retry_pending: AtomicBool,
    /// When the account's limit resets (Unix seconds), as Claude Code last
    /// reported it rejected.
    limit_reset: Mutex<Option<u64>>,
    /// Set while the agent works with no turn of ours out: Claude Code goes
    /// on by itself when a task it moved to the background finishes. The
    /// panel sees it as a run (busy, its clock ticking) until the agent falls
    /// quiet, the user stops it or a message of theirs starts a turn.
    unsolicited: AtomicBool,
    /// When the agent last streamed something, and whether it was answer
    /// text: after the answer it is done sooner than after a call, which
    /// the model may take a while to follow up.
    last_update: Mutex<Instant>,
    last_was_text: AtomicBool,
    /// How long the agent must stay quiet to end a run of its own, after
    /// answer text and after anything else (milliseconds).
    quiet_after_text_ms: AtomicU64,
    quiet_ms: AtomicU64,
    /// The compaction of its context the agent reported started
    /// (`compaction_update`), until it reports how it ended.
    compaction: Mutex<Option<OpenCompaction>>,
    /// The id of the compaction that ended last: the agent may repeat its
    /// end, which must not reach the panel twice.
    compaction_ended: Mutex<Option<String>>,
}

/// A compaction the agent runs on its own context, as its
/// `compaction_update` and `compaction_summary_chunk` reported it so far.
struct OpenCompaction {
    id: String,
    /// The summary that replaces the history, as far as it came.
    summary: String,
    /// The context's fill when it began, as `usage_update` last stated it.
    tokens_before: u64,
    reason: CompactionReason,
    started: Instant,
}

/// How long an agent working by itself must stay quiet after its answer
/// text, and after anything else, before its run is taken as over.
const UNSOLICITED_QUIET_AFTER_TEXT: Duration = Duration::from_secs(5);
const UNSOLICITED_QUIET: Duration = Duration::from_secs(30);

/// The agent's tool calls as the session log keeps the built-in loop's: an
/// assistant message with the calls, then a result for each. The agent
/// reports a call when it starts and its result when it ends, so the calls
/// started are held back until the first of them ends — calls made together
/// share one message, as the built-in loop's do — and a call the turn left
/// without a result gets one saying so.
#[derive(Default)]
struct CallLog {
    /// Started calls not yet in the log.
    unlogged: Vec<ToolCall>,
    /// Started calls without a result yet.
    open: Vec<ToolCall>,
}

pub struct AcpRuntime {
    shared: Arc<Shared>,
    events: Receiver<AgentEvent>,
}

impl AcpRuntime {
    /// Start the agent named `name` as `config` says. Returns at once; the
    /// handshake runs on a thread and its failure reaches the panel as a
    /// failed assistant message when the first prompt goes out.
    pub fn start(name: &str, config: &AcpConfig, setup: BackendSetup) -> Result<Self, String> {
        Self::launch(name, config, setup, false)
    }

    /// Whether the handshake is still under way: neither a session nor a
    /// failure yet. The agent's models are known once it is over.
    #[must_use]
    pub fn is_starting(&self) -> bool {
        matches!(
            *self
                .shared
                .conn
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            Conn::Starting
        )
    }

    /// Start the agent named `name` as a service for side requests only
    /// ([`AcpProvider`]): the handshake opens no session of its own.
    fn start_service(name: &str, config: &AcpConfig, setup: BackendSetup) -> Result<Self, String> {
        Self::launch(name, config, setup, true)
    }

    fn launch(
        name: &str,
        config: &AcpConfig,
        setup: BackendSetup,
        service: bool,
    ) -> Result<Self, String> {
        let mut child = agent_command(config, &setup.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start {}: {error}", config.command))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take().map(|stderr| {
            let tail = Arc::new(StderrTail::default());
            tail.follow(name.to_string(), stderr);
            tail
        });
        let cwd = setup.cwd.clone();
        let runtime = Self::connect(
            name,
            stdout,
            stdin,
            setup,
            config.timeout_secs,
            config.flavor,
            service,
            stderr,
        );
        runtime
            .shared
            .context_limit
            .store(config.context_limit.unwrap_or(0), Ordering::Release);
        // The reviewer on "the session's model" asks the agent's own
        // subscription, in a process of its own.
        if !service {
            let _ = runtime.shared.session_provider.set(Arc::new(CurrentModel {
                inner: AcpProvider::new(name, config.clone(), cwd),
                shared: Arc::downgrade(&runtime.shared),
            }));
        }
        *runtime
            .shared
            .child
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(child);
        Ok(runtime)
    }

    /// Speak over any pair of streams (tests, other transports).
    pub fn from_streams(
        name: &str,
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        setup: BackendSetup,
        timeout_secs: u64,
        flavor: AcpFlavor,
    ) -> Self {
        Self::connect(
            name,
            reader,
            writer,
            setup,
            timeout_secs,
            flavor,
            false,
            None,
        )
    }

    /// [`Self::from_streams`], as a service when `service` says so, with the
    /// process's `stderr` when there is one.
    #[allow(clippy::too_many_arguments)]
    fn connect(
        name: &str,
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        setup: BackendSetup,
        timeout_secs: u64,
        flavor: AcpFlavor,
        service: bool,
        stderr: Option<Arc<StderrTail>>,
    ) -> Self {
        let (events_tx, events) = mpsc::channel();
        let shared = Arc::new(Shared {
            writer: Arc::new(Mutex::new(Some(Box::new(writer)))),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            events: events_tx,
            conn: Mutex::new(Conn::Starting),
            queue: Mutex::new(Vec::new()),
            busy: AtomicBool::new(false),
            cancel: setup.cancel.clone(),
            hooks: Mutex::new({
                let mut hooks = PermissionHooks::new(setup.rules, Box::new(setup.prompter))
                    .with_mode_handle(setup.mode.clone())
                    .with_classifier(Box::new(setup.reviewer.classifier(setup.cancel.clone())));
                if let Some(persist) = setup.persist {
                    hooks = hooks.with_persist(persist);
                }
                hooks
            }),
            cwd: setup.cwd,
            name: name.to_string(),
            open_message: Mutex::new(None),
            open_thought: Mutex::new(String::new()),
            child: Mutex::new(None),
            stderr,
            models: Mutex::new(Vec::new()),
            current_model: Mutex::new(None),
            model_option: Mutex::new(None),
            options: Mutex::new(Vec::new()),
            commands: Mutex::new(Vec::new()),
            flavor,
            system_prompt: setup.system_prompt,
            session_log: setup
                .host_tools
                .as_ref()
                .and_then(|host| host.session_id.clone()),
            // Codex and Gemini CLI keep their own tools: of termide's they
            // are served those they have no counterpart of (the project's
            // memory and skills, the panel's cards), and those of its MCP
            // servers, which the panel hands over as they connect.
            host_tools: Mutex::new(setup.host_tools.map(|host| {
                if matches!(flavor, AcpFlavor::Codex | AcpFlavor::GeminiCli) {
                    HostTools {
                        tools: companion_tools(&host.tools, &host.skills),
                        ..host
                    }
                } else {
                    host
                }
            })),
            mcp_server: Mutex::new(None),
            background: Mutex::new(Vec::new()),
            mcp_settled: Mutex::new(false),
            mcp_settled_changed: Condvar::new(),
            announced: Mutex::new(HashMap::new()),
            host_calls: Arc::default(),
            calls: Mutex::new(CallLog::default()),
            context: Mutex::new(None),
            context_limit: AtomicU64::new(0),
            told_plan: AtomicBool::new(setup.mode.get() == Mode::Plan),
            told_tools: Mutex::new(None),
            served_tools: Mutex::new(BTreeSet::new()),
            mode: setup.mode,
            plan: setup.plan,
            resume: setup.resume,
            intent: {
                let intent = IntentLog::new();
                for message in &setup.history {
                    match message {
                        Message::User(user) => intent.record_user(user),
                        Message::Assistant(assistant) => intent.push_calls(assistant.tool_calls()),
                        Message::ToolResult(_) => {}
                    }
                }
                intent
            },
            session_provider: OnceLock::new(),
            history: Mutex::new(setup.history),
            untold: Mutex::new(None),
            windows: windows::windows_file().map_or_else(OnceLock::new, OnceLock::from),
            replaying: AtomicBool::new(false),
            steering: AtomicBool::new(false),
            turn_running: AtomicBool::new(false),
            steered: Mutex::new(Vec::new()),
            steering_out: Mutex::new(HashMap::new()),
            service,
            can_fork: AtomicBool::new(false),
            can_close: AtomicBool::new(false),
            takes_images: AtomicBool::new(false),
            side: Mutex::new(HashMap::new()),
            goal: setup.goal,
            handoff: setup.handoff,
            last_prompt: Mutex::new(None),
            turn_acted: AtomicBool::new(false),
            retry_pending: AtomicBool::new(false),
            limit_reset: Mutex::new(None),
            unsolicited: AtomicBool::new(false),
            last_update: Mutex::new(Instant::now()),
            last_was_text: AtomicBool::new(false),
            compaction: Mutex::new(None),
            compaction_ended: Mutex::new(None),
            quiet_after_text_ms: AtomicU64::new(UNSOLICITED_QUIET_AFTER_TEXT.as_millis() as u64),
            quiet_ms: AtomicU64::new(UNSOLICITED_QUIET.as_millis() as u64),
        });
        let for_reader = Arc::clone(&shared);
        std::thread::spawn(move || for_reader.read_loop(reader));
        let for_handshake = Arc::clone(&shared);
        std::thread::spawn(move || for_handshake.handshake(Duration::from_secs(timeout_secs)));
        Self { shared, events }
    }
}

impl Backend for AcpRuntime {
    fn prompt(&self, message: UserMessage) -> Result<(), PromptError> {
        if matches!(
            *self
                .shared
                .conn
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            Conn::Failed(_)
        ) {
            return Err(PromptError::Stopped);
        }
        {
            // The turn is claimed and its message queued under the queue's
            // lock, which `kick` decides under: a kick from the handshake
            // finishing in between would otherwise find a claimed turn with
            // nothing to run and end it, stranding the message.
            let mut queue = self
                .shared
                .queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if self
                .shared
                .busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Err(PromptError::Busy);
            }
            self.shared.cancel.reset();
            let _ = self.shared.events.send(AgentEvent::AgentStart);
            queue.insert(0, message);
        }
        self.shared.kick();
        Ok(())
    }

    /// The failed turn goes again (see [`Shared::retry_blocks`]); a dead
    /// agent cannot take it — the panel starts a new one first.
    fn retry(&self) -> Result<(), PromptError> {
        if matches!(
            *self
                .shared
                .conn
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            Conn::Failed(_)
        ) {
            return Err(PromptError::Stopped);
        }
        {
            // Claimed under the queue's lock, as in `prompt`.
            let _queue = self
                .shared
                .queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if self
                .shared
                .busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Err(PromptError::Busy);
            }
            self.shared.retry_pending.store(true, Ordering::Release);
            self.shared.cancel.reset();
            let _ = self.shared.events.send(AgentEvent::AgentStart);
        }
        self.shared.kick();
        Ok(())
    }

    /// The message waits in the queue, shown as queued; an agent that takes
    /// a message into its running turn is handed it at once, else it goes
    /// as the next turn.
    fn steer(&self, message: UserMessage) {
        self.shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(message.clone());
        self.shared.report_queue();
        // The agent working by itself takes no steering: its run ends and
        // the message goes as a turn.
        if self.shared.unsolicited.load(Ordering::Acquire) {
            self.shared.finish_unsolicited(StopReason::Stop, None);
            return;
        }
        // Held while the agent compacts, see [`Shared::compacting`].
        if self.shared.steering.load(Ordering::Acquire)
            && self.shared.turn_running.load(Ordering::Acquire)
            && !self.shared.compacting()
        {
            self.shared.steer_now(message);
        }
    }

    fn take_queued(&self) -> Vec<UserMessage> {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // While the session is starting, the first message is the run's own
        // prompt, not one waiting; it stays.
        let keep = usize::from(!self.shared.busy.load(Ordering::Acquire) && !queue.is_empty());
        let keep = keep.min(queue.len());
        let taken = queue.split_off(keep);
        drop(queue);
        let lens = self.queue_lens();
        let _ = self.shared.events.send(AgentEvent::QueueUpdate {
            steering: lens.0,
            follow_up: lens.1,
        });
        taken
    }

    fn queue_lens(&self) -> (usize, usize) {
        let queued = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len();
        let running = self.shared.busy.load(Ordering::Acquire);
        // While a turn runs, its own message is not in the queue; while the
        // session is starting, the first message is.
        (
            queued.saturating_sub(usize::from(!running && queued > 0)) + self.shared.in_flight(),
            0,
        )
    }

    fn abort(&self) {
        if !self.is_busy() {
            return;
        }
        self.shared.cancel.cancel();
        if let Conn::Ready { session_id } = &*self
            .shared
            .conn
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            let _ = self
                .shared
                .notify("session/cancel", json!({ "sessionId": session_id }));
        }
        // A call the agent moved to the background is no longer its turn's,
        // so its cancel leaves it running: the stop takes it down here.
        if let Some(server) = &*self
            .shared
            .mcp_server
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            server.cancel_running();
        }
        // No turn of ours answers the cancel: the agent's own run ends here.
        self.shared
            .finish_unsolicited(StopReason::Aborted, Some("aborted".to_string()));
    }

    fn is_busy(&self) -> bool {
        self.shared.busy.load(Ordering::Acquire)
    }

    fn background_calls(&self) -> Vec<BackgroundCall> {
        self.shared.background_calls()
    }

    fn stop_background_call(&self, id: &str) -> bool {
        let Some(call) = self
            .shared
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|call| call.id == id)
            .cloned()
        else {
            return false;
        };
        let server = self
            .shared
            .mcp_server
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(server) = server.as_ref() else {
            return false;
        };
        let running = server.running_calls();
        running_one(&call, &running).is_some_and(|at| server.cancel_call(&running[at].id))
    }

    fn drain(&self) -> Vec<AgentEvent> {
        std::iter::from_fn(|| self.events.try_recv().ok()).collect()
    }

    fn update(&self, _update: Box<dyn FnOnce(&mut Agent) + Send>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }

    fn compact(&self, _focus: Option<String>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }

    /// The judge's instructions and request go to the agent as one side
    /// request; its answer is read as the built-in judge's is.
    fn judge(&self, goal: String) -> Result<(), PromptError> {
        self.shared.side_ready()?;
        let shared = Arc::clone(&self.shared);
        std::thread::spawn(move || {
            let text = format!(
                "{}\n\n{}",
                shared.goal.system_prompt(&goal),
                shared.goal.request
            );
            let (answer, usage) = shared.side_query(&text);
            shared.report_spent(Spender::GoalJudge, usage);
            let event = match answer {
                Ok(reply) if !reply.trim().is_empty() => match parse_verdict(&reply) {
                    Ok(verdict) => AgentEvent::GoalJudged {
                        outcome: verdict.outcome,
                        reason: verdict.reason,
                        reply,
                    },
                    Err(error) => AgentEvent::GoalJudgeFailed { error, reply },
                },
                Ok(_) => AgentEvent::GoalJudgeFailed {
                    error: "the judge call did not complete".to_string(),
                    reply: String::new(),
                },
                Err(error) => AgentEvent::GoalJudgeFailed {
                    error,
                    reply: String::new(),
                },
            };
            let _ = shared.events.send(event);
        });
        Ok(())
    }

    /// The brief's instructions and request go to the agent as one side
    /// request; its answer is the brief.
    fn handoff(&self) -> Result<(), PromptError> {
        self.shared.side_ready()?;
        let shared = Arc::clone(&self.shared);
        std::thread::spawn(move || {
            let text = format!(
                "{}\n\n{}",
                shared.handoff.instructions, shared.handoff.request
            );
            let (answer, usage) = shared.side_query(&text);
            shared.report_spent(Spender::Handoff, usage);
            let brief = match answer {
                Ok(reply) if !reply.trim().is_empty() => Ok(reply.trim().to_string()),
                Ok(_) => Err("the handoff call did not produce a brief".to_string()),
                Err(error) => Err(error),
            };
            let _ = shared.events.send(AgentEvent::Handoff { brief });
        });
        Ok(())
    }

    fn available_models(&self) -> Vec<BackendModel> {
        self.shared
            .models
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn current_model(&self) -> Option<String> {
        self.shared
            .current_model
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn is_starting(&self) -> bool {
        matches!(
            *self
                .shared
                .conn
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            Conn::Starting
        )
    }

    fn context_usage(&self) -> Option<(u64, u64)> {
        *self
            .shared
            .context
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Claude Code's calls of termide's tools, and its permission requests,
    /// are judged here; Codex's and Gemini CLI's modes are mapped from the
    /// panel's. Another agent answers to its own configuration.
    fn follows_mode(&self) -> bool {
        self.shared.flavor != AcpFlavor::Generic
    }

    /// Claude Code, while termide's tools are still to be served or the
    /// server of them runs: a server that failed to start leaves it on its
    /// own tools.
    fn runs_host_tools(&self) -> bool {
        self.shared.flavor == AcpFlavor::ClaudeCode && self.shared.serving()
    }

    /// Claude Code, which is handed termide's prompt for its own when its
    /// session starts.
    fn takes_system_prompt(&self) -> bool {
        self.shared.flavor == AcpFlavor::ClaudeCode && !self.shared.system_prompt.is_empty()
    }

    /// Codex and Gemini CLI, on the same terms as [`Self::runs_host_tools`],
    /// and only when they take an MCP server over HTTP.
    fn takes_mcp_tools(&self) -> bool {
        matches!(self.shared.flavor, AcpFlavor::Codex | AcpFlavor::GeminiCli)
            && self.shared.serving()
    }

    fn host_tools_settled(&self) {
        *self
            .shared
            .mcp_settled
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = true;
        self.shared.mcp_settled_changed.notify_all();
    }

    /// The set the server will start with, else the running server's. The
    /// locks are taken in the order `session/new` takes them, which holds the
    /// first while it starts the server, so no change falls between the two.
    fn update_host_tools(&self, tools: ToolRegistry) -> Result<(), PromptError> {
        let shared = &self.shared;
        *shared
            .served_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = tool_names(&tools);
        let mut pending = shared
            .host_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(host) = pending.as_mut() {
            host.tools = tools;
            return Ok(());
        }
        match &*shared
            .mcp_server
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            Some(server) => {
                server.set_tools(tools);
                Ok(())
            }
            None => Err(PromptError::Unsupported),
        }
    }

    fn set_mode(&self, mode: Mode) {
        // The hooks read the shared handle; Codex and Gemini CLI are told, off
        // the UI thread.
        if matches!(self.shared.flavor, AcpFlavor::Codex | AcpFlavor::GeminiCli) {
            let shared = Arc::clone(&self.shared);
            std::thread::spawn(move || shared.apply_mode(mode));
        }
    }

    /// Taken during a turn too: ACP lets a session's configuration change
    /// at any time, and the agent applies it as soon as it can.
    fn select_model(&self, model_id: String) -> Result<(), String> {
        let option = self
            .shared
            .model_option
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match option {
            Some(config_id) => self.shared.set_config_option(&config_id, &model_id)?,
            None => {
                let session_id = self.shared.session_id()?;
                self.shared.request(
                    "session/set_model",
                    json!({ "sessionId": session_id, "modelId": model_id }),
                    Duration::from_secs(30),
                )?;
            }
        }
        *self
            .shared
            .current_model
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(model_id);
        Ok(())
    }

    fn config_options(&self) -> Vec<BackendOption> {
        let model = self
            .shared
            .model_option
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        self.shared
            .options
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|option| Some(&option.id) != model.as_ref())
            .cloned()
            .collect()
    }

    fn set_option(&self, id: String, value: String) -> Result<(), String> {
        self.shared.set_config_option(&id, &value)
    }

    fn agent_commands(&self) -> Vec<AgentCommand> {
        self.shared
            .commands
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn into_agent(self: Box<Self>) -> Option<Agent> {
        None
    }
}

/// The agent's process, in `cwd` with its environment, in a session of its
/// own with no controlling terminal.
///
/// Left on termide's terminal, the agent or anything it runs could open
/// `/dev/tty`: a read takes keys and halves of escape sequences from
/// termide, and a mode change takes the terminal out of raw mode.
fn agent_command(config: &AcpConfig, cwd: &Path) -> Command {
    let mut command = Command::new(&config.command);
    command.args(&config.args).current_dir(cwd);
    for (key, value) in &config.env {
        command.env(key, expand_env(value, |var| std::env::var(var).ok()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and touches no Rust state.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    command
}

impl Drop for AcpRuntime {
    fn drop(&mut self) {
        self.shared.cancel.cancel();
        // Closing stdin tells a well-behaved agent to exit; kill the rest.
        self.shared
            .writer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(mut child) = self
            .shared
            .child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Shared {
    /// Whether termide's tools are still to be served, or the server of them
    /// runs.
    fn serving(&self) -> bool {
        self.host_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
            || self
                .mcp_server
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_some()
    }

    fn handshake(self: &Arc<Self>, timeout: Duration) {
        let outcome = self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": { "readTextFile": true, "writeTextFile": true },
                    "terminal": false,
                    // Its own compactions reported as such, not as a made-up
                    // "Compact conversation" tool call.
                    "session": { "compaction": {} }
                },
                "clientInfo": { "name": "termide", "version": env!("CARGO_PKG_VERSION") }
            }),
            timeout,
        );
        let steers = outcome
            .as_ref()
            .is_ok_and(|result| result["_meta"]["steering"]["supported"] == true);
        self.steering.store(steers, Ordering::Release);
        if let Ok(init) = &outcome {
            let sessions = &init["agentCapabilities"]["sessionCapabilities"];
            self.can_fork
                .store(sessions["fork"].is_object(), Ordering::Release);
            self.can_close
                .store(sessions["close"].is_object(), Ordering::Release);
            self.takes_images.store(
                init["agentCapabilities"]["promptCapabilities"]["image"] == true,
                Ordering::Release,
            );
        }
        if self.service {
            *self.conn.lock().unwrap_or_else(PoisonError::into_inner) = match outcome {
                Ok(_) => Conn::Ready {
                    session_id: String::new(),
                },
                Err(error) => Conn::Failed(error),
            };
            return;
        }
        // Claude Code's adapter takes HTTP servers, which the others say.
        let http = self.flavor == AcpFlavor::ClaudeCode
            || outcome
                .as_ref()
                .is_ok_and(|result| result["agentCapabilities"]["mcpCapabilities"]["http"] == true);
        if http && matches!(self.flavor, AcpFlavor::Codex | AcpFlavor::GeminiCli) {
            self.wait_for_mcp_servers();
        }
        let params = self.new_session_params(http);
        let result =
            outcome.and_then(|init| self.open_session(&init["agentCapabilities"], params, timeout));
        let conn = match result {
            Ok(value) => match value["sessionId"].as_str() {
                Some(id) => {
                    self.adopt_models(&value);
                    Conn::Ready {
                        session_id: id.to_string(),
                    }
                }
                None => Conn::Failed("session/new returned no sessionId".into()),
            },
            Err(error) => Conn::Failed(error),
        };
        let ready = matches!(conn, Conn::Ready { .. });
        *self.conn.lock().unwrap_or_else(PoisonError::into_inner) = conn;
        if ready && matches!(self.flavor, AcpFlavor::Codex | AcpFlavor::GeminiCli) {
            self.apply_mode(self.mode.get());
        }
        self.kick();
    }

    /// The key the agent's window is remembered under: its adapter and the
    /// model in use.
    fn window_key(&self) -> String {
        windows::key(
            self.flavor,
            &self.name,
            self.current_model
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_deref()
                .unwrap_or("default"),
        )
    }

    /// The agent's context window as far as it is known before it reports
    /// one in this session: reported, else remembered from an earlier run,
    /// else told from the model's name; the user's limit applies.
    fn known_window(&self) -> Option<u64> {
        let reported = self
            .context
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map(|(_, size)| size);
        let window = reported
            .or_else(|| {
                let path = self.windows.get()?;
                windows::remembered(path, &self.window_key())
            })
            .or_else(|| {
                let model = self
                    .current_model
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                // The model's label names its context when the id does not
                // (`default` is "Opus … 1M context" on some accounts).
                let label = model.as_ref().map(|id| {
                    let name = self
                        .models
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .iter()
                        .find(|m| &m.id == id)
                        .map(|m| m.name.clone())
                        .unwrap_or_default();
                    format!("{id} {name}")
                });
                windows::inferred(self.flavor, label.as_deref())
            })?;
        Some(match self.context_limit.load(Ordering::Acquire) {
            0 => window,
            limit => window.min(limit),
        })
    }

    /// What a new session is told of the conversation before its first
    /// request, once: the record within half the agent's window when that
    /// is known, else — or when even cut down it would not fit — the
    /// decisions within the same room or [`RECAP_LIMIT`].
    fn take_recap(&self) -> Option<String> {
        let history = self
            .untold
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()?;
        let budget = self
            .known_window()
            .map(|window| chars_for_tokens(window / 2));
        let told = budget.and_then(|budget| handoff(&history, budget));
        if let Some(text) = &told {
            log::info!(
                "acp {}: the conversation goes as a record of {} characters",
                self.name,
                text.chars().count()
            );
            return told;
        }
        recap(&history, budget.unwrap_or(RECAP_LIMIT))
    }

    /// Open the session the conversation goes on in: the agent's own, when
    /// the log names one of this agent's and the agent can take it up again
    /// — through `session/resume`, else `session/load`, whose replay of what
    /// the panel already shows is dropped — or else a new one, recorded in
    /// the log and told a recap of the conversation before its first request.
    fn open_session(
        &self,
        capabilities: &Value,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let history =
            std::mem::take(&mut *self.history.lock().unwrap_or_else(PoisonError::into_inner));
        let resume = self
            .resume
            .as_ref()
            .filter(|resume| resume.agent == self.name);
        let method = if capabilities["sessionCapabilities"]["resume"].is_object() {
            Some("session/resume")
        } else if capabilities["loadSession"] == true {
            Some("session/load")
        } else {
            None
        };
        if let (Some(resume), Some(method)) = (resume, method) {
            let mut resume_params = params.clone();
            resume_params["sessionId"] = json!(resume.session_id);
            self.replaying
                .store(method == "session/load", Ordering::Release);
            let result = self.request(method, resume_params, timeout);
            self.replaying.store(false, Ordering::Release);
            match result {
                Ok(mut value) => {
                    if !value.is_object() {
                        value = json!({});
                    }
                    value["sessionId"] = json!(resume.session_id);
                    return Ok(value);
                }
                // Gone from the agent's own store, most likely: the
                // conversation goes on in a new session, told the recap.
                Err(error) => {
                    log::warn!("acp {}: cannot resume the session: {error}", self.name)
                }
            }
        }
        let value = self.request("session/new", params, timeout)?;
        if let Some(id) = value["sessionId"].as_str() {
            let _ = self.events.send(AgentEvent::ExternalSession {
                agent: self.name.clone(),
                session_id: id.to_string(),
            });
            *self.untold.lock().unwrap_or_else(PoisonError::into_inner) =
                Some(history).filter(|history| !history.is_empty());
        }
        Ok(value)
    }

    /// Wait, for [`MCP_SETTLE`] at most, until the panel says termide's MCP
    /// servers have all answered and their tools are handed over, so an agent
    /// that lists them once has them. Claude Code lists them again as they
    /// change and does not wait.
    fn wait_for_mcp_servers(&self) {
        let offered = self
            .host_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        if !offered {
            return;
        }
        let settled = self
            .mcp_settled
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let _ = self
            .mcp_settled_changed
            .wait_timeout_while(settled, MCP_SETTLE, |settled| {
                !*settled && !self.cancel.is_cancelled()
            });
    }

    /// `session/new`'s parameters. Claude Code gets termide's tools from
    /// termide's MCP server in place of its own, termide's system prompt in
    /// place of its own, none of its own settings (their rules, hooks,
    /// `CLAUDE.md`, MCP servers), and leave to run termide's tools without
    /// asking, since termide judges each call as it runs it. Without the
    /// server it keeps its own tools, so it still has some. Codex and Gemini
    /// CLI keep their own tools and prompt, and get the server too when they
    /// take one over `http`, for the tools of termide's MCP servers.
    fn new_session_params(&self, http: bool) -> Value {
        let mut params = json!({ "cwd": self.cwd, "mcpServers": [] });
        let serves = match self.flavor {
            AcpFlavor::ClaudeCode | AcpFlavor::Codex | AcpFlavor::GeminiCli => http,
            AcpFlavor::Generic => false,
        };
        // Held until the server runs: a change of the tools meanwhile waits
        // and then goes to the server. An agent that takes no server drops
        // the tools here, and with them the claim that it runs them.
        let mut pending = self
            .host_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let host = pending.take().filter(|_| serves);
        if let Some(host) = &host {
            let names = tool_names(&host.tools);
            *self
                .served_tools
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = names.clone();
            if self.flavor == AcpFlavor::ClaudeCode {
                *self
                    .told_tools
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(names);
            }
        }
        let server = host.and_then(|host| {
            let context = ToolContext {
                cwd: self.cwd.clone(),
                session: Some(self.session_view()),
                ..host.context
            };
            McpServer::start(host.tools, host.hooks, context)
                .map_err(|error| {
                    log::warn!("cannot serve termide's tools to {}: {error}", self.name);
                })
                .ok()
        });
        let served = server.is_some();
        if let Some(server) = server {
            // A call is known by the id of its block in the panel, so the
            // panel's stop and a view of a delegate reach it.
            let calls = Arc::clone(&self.host_calls);
            server.set_call_ids(Box::new(move |name, arguments| {
                calls
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .iter()
                    .filter(|(_, (known, args))| known == name && args == arguments)
                    .map(|(id, _)| id.clone())
                    .collect()
            }));
            params["mcpServers"] = json!([{
                "type": "http",
                "name": SERVER_NAME,
                "url": server.url(),
                "headers": [
                    { "name": "Authorization", "value": format!("Bearer {}", server.token()) }
                ],
            }]);
            *self
                .mcp_server
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(server);
        }
        drop(pending);
        if self.flavor != AcpFlavor::ClaudeCode {
            return params;
        }
        // No predicted follow-up prompt: termide shows none, and each would
        // be one more request a turn.
        let mut options = json!({
            "settingSources": [], "strictMcpConfig": true, "promptSuggestions": false
        });
        if served {
            options["tools"] = json!([]);
            options["allowedTools"] = json!([format!("mcp__{SERVER_NAME}")]);
        }
        params["_meta"] = json!({
            "systemPrompt": self.system_prompt,
            "claudeCode": { "options": options },
        });
        params
    }

    /// Put the agent in the modes that match the panel's `mode`: Codex's
    /// approval preset, and its plan collaboration mode for `plan`; Gemini
    /// CLI's approval mode. `ask` and `configured` have it ask, so termide's
    /// rules and the user decide.
    fn apply_mode(&self, mode: Mode) {
        let session_id = match &*self.conn.lock().unwrap_or_else(PoisonError::into_inner) {
            Conn::Ready { session_id } => session_id.clone(),
            _ => return,
        };
        if self.flavor == AcpFlavor::GeminiCli {
            self.apply_gemini_mode(&session_id, mode);
            return;
        }
        let (approval, collaboration) = codex_modes(mode);
        for (config_id, value) in [("mode", approval), ("collaboration_mode", collaboration)] {
            if let Err(error) = self.set_config_option(config_id, value) {
                log::warn!(
                    "acp {}: cannot set {config_id} to {value}: {error}",
                    self.name
                );
            }
        }
    }

    /// Gemini CLI's approval mode for `mode`. Its `plan` mode exists only
    /// when the user enabled it in Gemini's settings; without it the panel's
    /// plan mode falls back to `default`, where Gemini asks before editing and
    /// termide refuses the edit.
    fn apply_gemini_mode(&self, session_id: &str, mode: Mode) {
        let set = |value: &str| {
            self.request(
                "session/set_mode",
                json!({ "sessionId": session_id, "modeId": value }),
                Duration::from_secs(30),
            )
        };
        let value = gemini_mode(mode);
        let mut outcome = set(value);
        if outcome.is_err() && value == "plan" {
            outcome = set("default");
        }
        if let Err(error) = outcome {
            log::warn!("acp {}: cannot set mode {value}: {error}", self.name);
        }
    }

    /// Record the settings an agent states in a `session/new`/`load`/`resume`
    /// result, so the panel can list and switch them: its `configOptions`,
    /// whose `model` entry, when there is one, lists the models — the way
    /// ACP prefers — else its `models` (`availableModels`, `currentModelId`).
    /// Missing or malformed data leaves the lists empty.
    fn adopt_models(&self, result: &Value) {
        if let Some(options) = result["configOptions"].as_array() {
            self.adopt_options(options);
        }
        let by_option = self
            .model_option
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        if !by_option && result["models"].is_object() {
            self.adopt_model_list(&result["models"]);
        }
    }

    /// The whole set of config options, as the agent states it each time:
    /// the `select` ones are kept, the model's among them read as the list of
    /// models and the model in use.
    fn adopt_options(&self, options: &[Value]) {
        let options: Vec<BackendOption> = options.iter().filter_map(option_of).collect();
        let model = options
            .iter()
            .find(|option| option.is("model") || option.id == "model");
        if let Some(model) = model {
            *self
                .model_option
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(model.id.clone());
            *self.models.lock().unwrap_or_else(PoisonError::into_inner) = model.values.clone();
            *self
                .current_model
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(model.current.clone());
        }
        *self.options.lock().unwrap_or_else(PoisonError::into_inner) = options;
    }

    /// The session's id, once it is open.
    fn session_id(&self) -> Result<String, String> {
        match &*self.conn.lock().unwrap_or_else(PoisonError::into_inner) {
            Conn::Ready { session_id } => Ok(session_id.clone()),
            Conn::Starting => Err("the agent is still starting".to_string()),
            Conn::Failed(error) => Err(error.clone()),
        }
    }

    /// Set config option `id` to `value` and take the options the agent
    /// replies with, which may change with it (the efforts a model offers);
    /// an agent that replies with none has the one value changed.
    fn set_config_option(&self, id: &str, value: &str) -> Result<(), String> {
        let session_id = self.session_id()?;
        let reply = self.request(
            "session/set_config_option",
            json!({ "sessionId": session_id, "configId": id, "value": value }),
            Duration::from_secs(30),
        )?;
        match reply["configOptions"].as_array() {
            Some(options) => self.adopt_options(options),
            None => {
                if let Some(option) = self
                    .options
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .iter_mut()
                    .find(|option| option.id == id)
                {
                    option.current = value.to_string();
                }
            }
        }
        Ok(())
    }

    fn adopt_model_list(&self, models: &Value) {
        let list: Vec<BackendModel> = models["availableModels"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|m| {
                        let id = m["modelId"].as_str()?.to_string();
                        let name = m["name"].as_str().unwrap_or(&id).to_string();
                        Some(BackendModel { id, name })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let current = models["currentModelId"].as_str().map(str::to_string);
        *self.models.lock().unwrap_or_else(PoisonError::into_inner) = list;
        if let Some(current) = current {
            *self
                .current_model
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(current);
        }
    }

    /// Start the next queued turn when the session is ready and no turn is
    /// running; report a failed connection as a failed answer.
    fn kick(self: &Arc<Self>) {
        if !self.busy.load(Ordering::Acquire) {
            return;
        }
        let session_id = match &*self.conn.lock().unwrap_or_else(PoisonError::into_inner) {
            Conn::Starting => return,
            Conn::Ready { session_id } => session_id.clone(),
            Conn::Failed(error) => {
                self.queue
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clear();
                self.retry_pending.store(false, Ordering::Release);
                let _ = self.events.send(AgentEvent::MessageEnd(Message::Assistant(
                    AssistantMessage::failed(
                        ACP_PROVIDER,
                        &self.name,
                        StopReason::Error,
                        error.clone(),
                    ),
                )));
                let _ = self.events.send(AgentEvent::AgentEnd);
                self.busy.store(false, Ordering::Release);
                return;
            }
        };
        // Decided under the queue's lock, which `prompt` and `retry` claim a
        // turn under: a turn seen claimed has its message or retry in place.
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        if self.retry_pending.swap(false, Ordering::AcqRel) {
            drop(queue);
            let prompt = self.retry_blocks();
            let this = Arc::clone(self);
            std::thread::spawn(move || this.run_retry(&session_id, prompt));
            return;
        }
        // What the user typed while the agent worked goes as one turn: it is
        // usually one thought written in pieces. A command's output or
        // another agent's message goes on its own, keeping its mark.
        let message = {
            let run = UserMessage::merged_run(queue.iter());
            UserMessage::merge(queue.drain(..run).collect())
        };
        drop(queue);
        let Some(message) = message else {
            let _ = self.events.send(AgentEvent::AgentEnd);
            self.busy.store(false, Ordering::Release);
            // A message a turn did not take back after all (see
            // [`Self::requeue`]) may have arrived meanwhile: it runs now.
            self.run_queued();
            // Output that came after the turn's answer, before it was
            // closed, is the agent going on by itself; so is a call it moved
            // to the background, which it takes up again once it ends.
            let late = self.serving_call()
                || self
                    .open_message
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_some()
                || !self
                    .calls
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .open
                    .is_empty();
            if late {
                self.begin_unsolicited();
            }
            return;
        };
        let this = Arc::clone(self);
        std::thread::spawn(move || this.run_turn(&session_id, message));
    }

    /// Claude Code runs on termide's system prompt, fixed when its session
    /// started, so a plan-mode switch since would leave it on the wrong
    /// instructions: the next turn opens with a note of the switch, kept out
    /// of the session log. Codex and Gemini CLI are switched to their own
    /// modes instead.
    fn plan_switch_note(&self) -> Option<String> {
        if self.flavor != AcpFlavor::ClaudeCode {
            return None;
        }
        let on = self.mode.get() == Mode::Plan;
        let told = self.told_plan.swap(on, Ordering::AcqRel);
        let note = self.plan.switch_note(told, on)?;
        Some(format!("<system-reminder>\n{note}\n</system-reminder>"))
    }

    /// Claude Code's prompt speaks of the tools its session started with: a
    /// change of termide's tools since — one switched off or on, an MCP
    /// server's that came or went — opens the next turn with a note of it,
    /// kept out of the session log like the plan note.
    fn tools_note(&self) -> Option<String> {
        let served = self
            .served_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut told = self
            .told_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let before = told.as_mut()?;
        let named = |names: Vec<&String>| {
            names
                .iter()
                .map(|name| format!("{HOST_TOOL_PREFIX}{name}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let added = named(served.difference(before).collect());
        let removed = named(before.difference(&served).collect());
        if added.is_empty() && removed.is_empty() {
            return None;
        }
        *before = served;
        let mut note = String::from("Your tools changed since your instructions were written.");
        if !added.is_empty() {
            note.push_str(&format!(" Now available: {added}."));
        }
        if !removed.is_empty() {
            note.push_str(&format!(
                " No longer available, do not call them: {removed}."
            ));
        }
        Some(format!("<system-reminder>\n{note}\n</system-reminder>"))
    }

    fn run_turn(self: &Arc<Self>, session_id: &str, message: UserMessage) {
        let _ = self.events.send(AgentEvent::TurnStart);
        self.intent.record_user(&message);
        let _ = self
            .events
            .send(AgentEvent::MessageEnd(Message::User(message.clone())));
        // What this turn took is no longer waiting: the panel's queue strip
        // drops it now, not when the turn ends.
        let queued = self
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len();
        let _ = self.events.send(AgentEvent::QueueUpdate {
            steering: queued,
            follow_up: 0,
        });
        let text = message.framed_text();
        let mut prompt = Vec::new();
        // Kept out of the log, like the plan note: the log holds the
        // conversation itself.
        if let Some(recap) = self.take_recap() {
            prompt.push(json!({ "type": "text", "text": recap }));
        }
        for note in [self.plan_switch_note(), self.tools_note()]
            .into_iter()
            .flatten()
        {
            prompt.push(json!({ "type": "text", "text": note }));
        }
        prompt.push(json!({ "type": "text", "text": text }));
        prompt.extend(self.image_blocks(&message));
        self.send_turn(session_id, prompt);
    }

    /// `message`'s images as ACP image blocks, when the agent takes them.
    fn image_blocks(&self, message: &UserMessage) -> Vec<Value> {
        if !self.takes_images.load(Ordering::Acquire) {
            return Vec::new();
        }
        message
            .images()
            .map(|(media_type, data)| {
                json!({ "type": "image", "mimeType": media_type, "data": data })
            })
            .collect()
    }

    /// A turn that tries the last failed one again: no new message is
    /// logged, the blocks go as they are.
    fn run_retry(self: &Arc<Self>, session_id: &str, prompt: Vec<Value>) {
        let _ = self.events.send(AgentEvent::TurnStart);
        self.send_turn(session_id, prompt);
    }

    /// What a retry sends: the failed turn's blocks again when it called no
    /// tool, so nothing it did is done twice; otherwise a note asking the
    /// agent to go on from where it stopped — after the recap of the
    /// conversation, when a restarted agent has not been told it yet.
    fn retry_blocks(&self) -> Vec<Value> {
        let last = self
            .last_prompt
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(prompt) = last.filter(|_| !self.turn_acted.load(Ordering::Acquire)) {
            return prompt;
        }
        let mut blocks = Vec::new();
        if let Some(recap) = self.take_recap() {
            blocks.push(json!({ "type": "text", "text": recap }));
        }
        blocks.push(json!({ "type": "text", "text": RETRY_NOTE }));
        blocks
    }

    /// Send a turn's blocks and close the turn when the agent answers.
    fn send_turn(self: &Arc<Self>, session_id: &str, prompt: Vec<Value>) {
        *self
            .last_prompt
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(prompt.clone());
        self.turn_acted.store(false, Ordering::Release);
        self.turn_running.store(true, Ordering::Release);
        let result = self.request_classified(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": prompt
            }),
            Duration::from_secs(60 * 60 * 24),
        );
        self.turn_running.store(false, Ordering::Release);
        if result.is_ok() {
            *self
                .last_prompt
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = None;
        }
        let stop = match &result {
            Ok(value) => match value["stopReason"].as_str() {
                Some("cancelled") => StopReason::Aborted,
                Some("max_tokens") => StopReason::Length,
                Some("refusal") => StopReason::Error,
                _ => StopReason::Stop,
            },
            Err(_) => StopReason::Error,
        };
        // A cancelled turn reads as the built-in loop's: an aborted message.
        let error = match &result {
            Err(failure) => Some(failure.text.clone()),
            Ok(_) if stop == StopReason::Aborted => Some("aborted".to_string()),
            Ok(_) => None,
        };
        let failure = result.as_ref().err().map(|failure| {
            let mut class = failure.class;
            // The reset Claude Code reported is exact; one read from the
            // text is not (a time of day without its zone).
            if matches!(
                class.kind,
                FailureKind::RateLimited | FailureKind::QuotaExhausted
            ) {
                if let Some(at) = *self
                    .limit_reset
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                {
                    class.resets_at = Some(at);
                }
            }
            class
        });
        let usage = result
            .as_ref()
            .map(|value| usage_of(value, self.flavor))
            .unwrap_or_default();
        if let Ok(value) = &result {
            for (model, beyond) in usage_beyond(value, &usage, &self.model_name()) {
                self.report_spent_on(Spender::AgentInternal, model, beyond);
            }
        }
        // A compaction the turn ended in the middle of: an adapter reporting
        // it as a made-up call never ends it then.
        self.end_compaction("cancelled", None, None);
        self.close_unfinished_calls();
        self.close_message_with(stop, error, failure, usage);
        self.flush_steered();
        let _ = self.events.send(AgentEvent::TurnEnd);
        if self.cancel.is_cancelled() {
            self.queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
        }
        let queued = self
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len();
        let _ = self.events.send(AgentEvent::QueueUpdate {
            steering: queued + self.in_flight(),
            follow_up: 0,
        });
        self.kick();
    }

    /// The session as the reviewer sees it: the log of what the user asked
    /// and the agent did, and the agent's own subscription as "the session's
    /// model" (its current model), or none when it was not started from a
    /// command — a reviewer on the session's model then cannot answer, and
    /// the user is asked.
    fn session_view(&self) -> SessionView {
        let provider = self
            .session_provider
            .get()
            .cloned()
            .unwrap_or_else(|| Arc::new(NoModel) as Arc<dyn Provider>);
        SessionView {
            id: self.session_log.clone(),
            intent: self.intent.clone(),
            provider,
            model: ModelSpec {
                provider: ACP_PROVIDER.into(),
                id: String::new(),
                context_window: 0,
                max_tokens: None,
                thinking: ThinkingLevel::Off,
            },
        }
    }

    /// Whether a side request can go now: the session is open and no turn
    /// or side request runs in it.
    fn side_ready(&self) -> Result<(), PromptError> {
        if self.busy.load(Ordering::Acquire)
            || !self
                .side
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
        {
            return Err(PromptError::Busy);
        }
        // An agent still starting (a reopen, a restart) is busy, not gone:
        // the caller asks again once its session is up.
        match &*self.conn.lock().unwrap_or_else(PoisonError::into_inner) {
            Conn::Ready { .. } => Ok(()),
            Conn::Starting => Err(PromptError::Busy),
            Conn::Failed(_) => Err(PromptError::Stopped),
        }
    }

    /// Ask the agent `text` aside from the conversation and return its
    /// answer's text: in a fork of the session when the agent forks one,
    /// closed afterwards when it closes one; else in the session itself, as
    /// a turn kept from the panel and the log (the agent's own session still
    /// holds it). Its tools are not asked for: a permission request from it
    /// is refused. What the request spent comes with the answer, or with
    /// the failure of a request that went out.
    fn side_query(self: &Arc<Self>, text: &str) -> (Result<String, String>, Usage) {
        let main = match self.session_id() {
            Ok(main) => main,
            Err(error) => return (Err(error), Usage::default()),
        };
        let forked = if self.can_fork.load(Ordering::Acquire) {
            let reply = self.request(
                "session/fork",
                json!({ "sessionId": main, "cwd": self.cwd, "mcpServers": [] }),
                Duration::from_secs(60),
            );
            match reply {
                Ok(reply) => reply["sessionId"].as_str().map(str::to_string),
                Err(error) => {
                    log::warn!("acp {}: cannot fork the session: {error}", self.name);
                    None
                }
            }
        } else {
            None
        };
        // In the session itself, no turn may start meanwhile.
        if forked.is_none()
            && self
                .busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return (Err("the agent is busy".to_string()), Usage::default());
        }
        let session_id = forked.clone().unwrap_or(main);
        let answer = self.side_prompt(&session_id, text, Duration::from_secs(60 * 60));
        match &forked {
            Some(fork) if self.can_close.load(Ordering::Acquire) => {
                if let Err(error) = self.request(
                    "session/close",
                    json!({ "sessionId": fork }),
                    Duration::from_secs(10),
                ) {
                    log::debug!("acp {}: cannot close the fork: {error}", self.name);
                }
            }
            Some(_) => {}
            None => {
                self.busy.store(false, Ordering::Release);
                // A request typed meanwhile waited for this.
                self.run_queued();
            }
        }
        answer
    }

    /// Send `text` as a side request in session `session_id` and return the
    /// answer's text, which reaches neither the panel nor the log, and what
    /// the request spent — a stopped one too, as far as the agent tells;
    /// several may run at once, each in its own session.
    fn side_prompt(
        &self,
        session_id: &str,
        text: &str,
        timeout: Duration,
    ) -> (Result<String, String>, Usage) {
        self.side
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(session_id.to_string(), String::new());
        let result = self.request(
            "session/prompt",
            json!({ "sessionId": session_id, "prompt": [{ "type": "text", "text": text }] }),
            timeout,
        );
        let answer = self
            .side
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(session_id)
            .unwrap_or_default();
        let reply = match result {
            Ok(reply) => reply,
            Err(error) => return (Err(error), Usage::default()),
        };
        let usage = usage_of(&reply, self.flavor);
        match reply["stopReason"].as_str() {
            Some(stop @ ("cancelled" | "refusal")) => {
                (Err(format!("the agent stopped: {stop}")), usage)
            }
            _ => (Ok(answer), usage),
        }
    }

    /// Take `update` of `session` for a side request in flight, when it is
    /// one's: its text is kept, the rest dropped.
    fn side_update(&self, session: Option<&str>, update: &Value) -> bool {
        let mut side = self.side.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(text) = session.and_then(|session| side.get_mut(session)) else {
            return false;
        };
        if update["sessionUpdate"] == "agent_message_chunk" {
            text.push_str(update["content"]["text"].as_str().unwrap_or_default());
        }
        true
    }

    /// Whether `params` come from a side request's session.
    fn is_side_request(&self, params: &Value) -> bool {
        params["sessionId"].as_str().is_some_and(|session| {
            self.side
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(session)
        })
    }

    /// Hand `message`, which waits in the queue, to the turn the agent is
    /// running. Taken, it is logged as the built-in loop logs a steering
    /// message; refused — the turn ended meanwhile, or the agent asks for a
    /// prompt — it waits for the next turn as before. A message the user or
    /// the turn's end took from the queue first is left alone.
    fn steer_now(self: &Arc<Self>, message: UserMessage) {
        {
            let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(index) = queue.iter().position(|queued| *queued == message) else {
                return;
            };
            if !self.turn_running.load(Ordering::Acquire) {
                return;
            }
            queue.remove(index);
        }
        let Ok(session_id) = self.session_id() else {
            self.requeue(message);
            return;
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut prompt = vec![json!({ "type": "text", "text": message.framed_text() })];
        prompt.extend(self.image_blocks(&message));
        self.steering_out
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, message);
        let request = json!({
            "jsonrpc": "2.0", "id": id, "method": "_session/steering",
            "params": {
                "sessionId": session_id,
                "prompt": prompt,
                "_meta": { "steering": { "idleBehavior": "promptRequired" } },
            },
        });
        if self.write(&request).is_err() {
            let message = self
                .steering_out
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
            if let Some(message) = message {
                self.requeue(message);
            }
        }
    }

    /// The agent's answer to a `_session/steering` request, `reply` being
    /// its result or error.
    fn steering_answered(self: &Arc<Self>, message: UserMessage, reply: &Value) {
        match reply["result"]["outcome"].as_str() {
            // `startedNewTurn`: an agent that ignores the idle behaviour
            // asked for began a turn of its own with it.
            Some("injected" | "startedNewTurn") => {
                self.steered
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(message);
                self.report_queue();
                self.flush_steered();
            }
            _ => {
                log::debug!("acp {}: steering refused: {reply}", self.name);
                self.requeue(message);
            }
        }
    }

    /// Log the messages the running turn took, once it has no call open:
    /// the text streamed before them closes first.
    fn flush_steered(&self) {
        let open = !self
            .calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .open
            .is_empty();
        if open {
            return;
        }
        let steered =
            std::mem::take(&mut *self.steered.lock().unwrap_or_else(PoisonError::into_inner));
        if steered.is_empty() {
            return;
        }
        self.close_message(StopReason::Stop, None);
        for message in steered {
            self.intent.record_user(&message);
            let _ = self
                .events
                .send(AgentEvent::MessageEnd(Message::User(message)));
        }
        self.report_queue();
    }

    /// The agent writes again while messages it took wait to be logged: it
    /// has gone on from the calls still open. Claude Code drops a call it
    /// was streaming when a message comes in, and never answers it; left
    /// open, it held the message out of the transcript until the turn's
    /// end, logged after the answer it got.
    fn steered_on(&self) {
        let waiting = !self
            .steered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty();
        if !waiting {
            return;
        }
        self.close_open_calls("The agent went on without this call.");
        self.flush_steered();
    }

    /// Put back a message the running turn did not take, first in line, and
    /// start a turn for it when none is running.
    fn requeue(self: &Arc<Self>, message: UserMessage) {
        self.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(0, message);
        self.report_queue();
        self.run_queued();
    }

    /// Start a turn for what waits in the queue, when no run is on.
    fn run_queued(self: &Arc<Self>) {
        let waiting = !self
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty();
        if waiting
            && self
                .busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            self.cancel.reset();
            let _ = self.events.send(AgentEvent::AgentStart);
            self.kick();
        }
    }

    /// Note what the agent streams; output with no run on starts one of the
    /// agent's own (see [`Self::unsolicited`]).
    fn note_update(self: &Arc<Self>, update: &Value) {
        let kind = update["sessionUpdate"].as_str().unwrap_or("");
        if self.service
            || self.replaying.load(Ordering::Acquire)
            || !matches!(
                kind,
                "agent_message_chunk"
                    | "agent_thought_chunk"
                    | "tool_call"
                    | "tool_call_update"
                    | "plan"
            )
        {
            return;
        }
        *self
            .last_update
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
        self.last_was_text
            .store(kind == "agent_message_chunk", Ordering::Release);
        if !self.busy.load(Ordering::Acquire)
            && matches!(
                *self.conn.lock().unwrap_or_else(PoisonError::into_inner),
                Conn::Ready { .. }
            )
        {
            self.begin_unsolicited();
        }
    }

    /// The calls the agent moved to the background that the server still
    /// runs, each matched to one running call; those it no longer runs are
    /// dropped.
    fn background_calls(&self) -> Vec<BackgroundCall> {
        let mut running = self
            .mcp_server
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(McpServer::running_calls)
            .unwrap_or_default();
        let mut calls = Vec::new();
        self.background
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|call| {
                let Some(at) = running_one(call, &running) else {
                    return false;
                };
                let elapsed = running.remove(at).elapsed;
                calls.push(BackgroundCall {
                    call: call.clone(),
                    elapsed,
                });
                true
            });
        calls.sort_by(|a, b| b.elapsed.cmp(&a.elapsed));
        calls
    }

    /// Whether termide's MCP server runs a call for the agent. Claude Code
    /// closes a call it moved to the background, but the command goes on.
    fn serving_call(&self) -> bool {
        self.mcp_server
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(McpServer::is_running)
    }

    /// Open a run for the agent working by itself, and watch for its end:
    /// the agent quiet long enough with no call open, its own or one
    /// termide's MCP server still runs for it.
    fn begin_unsolicited(self: &Arc<Self>) {
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        self.unsolicited.store(true, Ordering::Release);
        *self
            .last_update
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
        self.cancel.reset();
        let _ = self.events.send(AgentEvent::AgentStart);
        let this = Arc::clone(self);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(100));
            if !this.unsolicited.load(Ordering::Acquire) {
                return;
            }
            let failed = matches!(
                *this.conn.lock().unwrap_or_else(PoisonError::into_inner),
                Conn::Failed(_)
            );
            // A call termide still runs for the agent counts as its work:
            // the quiet is counted from its end, which the agent then takes up.
            let serving = this.serving_call();
            if serving {
                *this
                    .last_update
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Instant::now();
                this.last_was_text.store(false, Ordering::Release);
            }
            let quiet = Duration::from_millis(if this.last_was_text.load(Ordering::Acquire) {
                this.quiet_after_text_ms.load(Ordering::Acquire)
            } else {
                this.quiet_ms.load(Ordering::Acquire)
            });
            let open = !this
                .calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .open
                .is_empty()
                || !this
                    .announced
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_empty()
                || serving;
            let silent = this
                .last_update
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .elapsed()
                >= quiet;
            if failed || (silent && !open) {
                this.finish_unsolicited(StopReason::Stop, None);
                return;
            }
        });
    }

    /// Close the agent's own run, as a turn's end closes ours; a message
    /// that waits then goes as the next turn.
    fn finish_unsolicited(self: &Arc<Self>, stop: StopReason, error: Option<String>) {
        if !self.unsolicited.swap(false, Ordering::AcqRel) {
            return;
        }
        self.close_unfinished_calls();
        self.close_message(stop, error);
        self.flush_steered();
        if self.cancel.is_cancelled() {
            self.queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
        }
        self.report_queue();
        self.kick();
    }

    /// Tell the panel how many messages wait.
    fn report_queue(&self) {
        let queued = self
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len();
        let running = self.busy.load(Ordering::Acquire);
        // While the session is starting, the first message is the run's own.
        let steering =
            queued.saturating_sub(usize::from(!running && queued > 0)) + self.in_flight();
        let _ = self.events.send(AgentEvent::QueueUpdate {
            steering,
            follow_up: 0,
        });
    }

    /// The messages handed to the running turn and not yet logged: they
    /// still show as waiting, so a message typed during a turn stays in
    /// sight until it takes its place in the transcript.
    fn in_flight(&self) -> usize {
        self.steering_out
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
            + self
                .steered
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len()
    }

    /// Finish the assistant message being streamed, or make one for an
    /// error, so the transcript and the log get a complete message.
    fn close_message(&self, stop: StopReason, error: Option<String>) {
        self.close_message_with(stop, error, None, Usage::default());
    }

    /// [`Self::close_message`], with the failure classified and the turn's
    /// token usage when the agent reported it.
    fn close_message_with(
        &self,
        stop: StopReason,
        error: Option<String>,
        failure: Option<Failure>,
        usage: Usage,
    ) {
        let text = self
            .open_message
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let thought = std::mem::take(
            &mut *self
                .open_thought
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        if text.is_none() && error.is_none() {
            return;
        }
        let mut content = Vec::new();
        if !thought.is_empty() {
            content.push(AssistantContent::thinking(thought));
        }
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            content.push(AssistantContent::Text { text });
        }
        let message = AssistantMessage {
            content,
            stop_reason: stop,
            usage,
            provider: ACP_PROVIDER.into(),
            model: self.model_name(),
            error_message: error,
            failure,
            timestamp: now_millis(),
        };
        let _ = self
            .events
            .send(AgentEvent::MessageEnd(Message::Assistant(message)));
    }

    /// Tell the panel what a side request of `by`'s spent, credited to the
    /// session's model, so the log keeps it.
    fn report_spent(&self, by: Spender, usage: Usage) {
        self.report_spent_on(by, self.model_name(), usage);
    }

    /// [`Self::report_spent`] on `model`.
    fn report_spent_on(&self, by: Spender, model: String, usage: Usage) {
        if usage.total() > 0 {
            let _ = self.events.send(AgentEvent::Spent { by, model, usage });
        }
    }

    /// The model to credit a message to: the agent's current one, or the
    /// agent's name while it has named none.
    fn model_name(&self) -> String {
        self.current_model
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| self.name.clone())
    }

    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        self.request_classified(method, params, timeout)
            .map_err(|failure| failure.text)
    }

    /// [`Self::request`], its failure classified.
    fn request_classified(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, RpcFailure> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, tx);
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if let Err(error) = self.write(&message) {
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
            let shut_down = self
                .writer
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_none();
            // An agent that exited before reading fails the write with a
            // broken pipe: the same end as a closed connection, whose last
            // stderr line says why.
            return Err(if shut_down {
                RpcFailure::new(error, FailureKind::AgentDied)
            } else {
                self.connection_closed(method)
            });
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&id);
                return Err(RpcFailure::new(
                    format!("{method}: no reply within {} s", timeout.as_secs()),
                    FailureKind::Transient,
                ));
            }
            match rx.recv_timeout(left.min(Duration::from_millis(100))) {
                Ok(reply) => {
                    return reply.map_err(|failure| RpcFailure {
                        text: format!("{method}: {}", failure.text),
                        ..failure
                    })
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err(self.connection_closed(method)),
            }
        }
    }

    /// The failure of a request the agent hung up on, with the last line it
    /// printed to stderr.
    fn connection_closed(&self, method: &str) -> RpcFailure {
        let mut text = format!("{method}: the agent closed the connection");
        if let Some(line) = self.stderr.as_ref().and_then(|t| t.last(STDERR_GRACE)) {
            text = format!("{text}: {line}");
        }
        RpcFailure::new(text, FailureKind::AgentDied)
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    fn write(&self, message: &Value) -> Result<(), String> {
        let mut line = message.to_string();
        line.push('\n');
        let mut guard = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let writer = guard.as_mut().ok_or("the agent was shut down")?;
        writer
            .write_all(line.as_bytes())
            .and_then(|()| writer.flush())
            .map_err(|error| format!("cannot write to the agent: {error}"))
    }

    fn read_loop(self: Arc<Self>, reader: impl Read) {
        for line in BufReader::new(reader).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let message: Value = match serde_json::from_str(&line) {
                Ok(message) => message,
                Err(error) => {
                    log::debug!("acp {}: not JSON ({error})", self.name);
                    continue;
                }
            };
            match (message["id"].as_u64(), message.get("method")) {
                (Some(id), None) => {
                    let steered = self
                        .steering_out
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .remove(&id);
                    if let Some(steered) = steered {
                        self.steering_answered(steered, &message);
                        continue;
                    }
                    let reply = match message.get("error") {
                        Some(error) => Err(RpcFailure {
                            text: format!(
                                "{} (code {})",
                                error["message"].as_str().unwrap_or("error"),
                                error["code"]
                            ),
                            class: classify_rpc_error(error),
                        }),
                        None => Ok(message["result"].clone()),
                    };
                    if let Some(tx) = self
                        .pending
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .remove(&id)
                    {
                        let _ = tx.send(reply);
                    }
                }
                (Some(id), Some(method)) => {
                    let method = method.as_str().unwrap_or("").to_string();
                    self.serve_request(id, &method, &message["params"]);
                }
                (None, Some(method)) => {
                    if method == "session/update" {
                        let params = &message["params"];
                        if self.side_update(params["sessionId"].as_str(), &params["update"]) {
                            continue;
                        }
                        self.note_update(&params["update"]);
                        self.on_update(&params["update"]);
                    } else {
                        log::debug!("acp {}: notification {method}", self.name);
                    }
                }
                (None, None) => {}
            }
        }
        // The agent is gone: a pending prompt learns it, later ones are refused.
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        let mut conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        if !matches!(*conn, Conn::Failed(_)) {
            *conn = Conn::Failed("the agent exited".into());
        }
    }

    /// Answer the agent's requests: permissions through the user, files
    /// from the working directory, nothing else.
    fn serve_request(self: &Arc<Self>, id: u64, method: &str, params: &Value) {
        match method {
            // A side request is answered from what the agent knows.
            "session/request_permission" if self.is_side_request(params) => {
                let options = params["options"].as_array().cloned().unwrap_or_default();
                let reject = ["reject_once", "reject_always"].iter().find_map(|kind| {
                    options
                        .iter()
                        .find(|o| o["kind"].as_str() == Some(kind))
                        .and_then(|o| o["optionId"].as_str())
                });
                let outcome = match reject {
                    Some(option) => json!({ "outcome": "selected", "optionId": option }),
                    None => json!({ "outcome": "cancelled" }),
                };
                let _ = self.write(
                    &json!({ "jsonrpc": "2.0", "id": id, "result": { "outcome": outcome } }),
                );
            }
            "session/request_permission" => {
                let this = Arc::clone(self);
                let params = params.clone();
                std::thread::spawn(move || {
                    let result = this.ask_permission(&params);
                    let _ = this.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                });
            }
            "fs/read_text_file" => {
                let reply = read_text_file(&self.cwd, params);
                let _ = self.write(&rpc_reply(id, reply));
            }
            "fs/write_text_file" => {
                let reply = write_text_file(&self.cwd, params);
                if reply.is_ok() {
                    if let Some(path) = params["path"].as_str() {
                        // Surfaces as an edit so an open editor reloads the file.
                        let call = ToolCall {
                            id: format!("fs-{id}"),
                            name: "write".into(),
                            arguments: json!({ "path": path }),
                            extra_content: None,
                        };
                        let _ = self
                            .events
                            .send(AgentEvent::ToolExecutionStart { call: call.clone() });
                        let _ = self.events.send(AgentEvent::ToolExecutionEnd {
                            result: ToolResultMessage::text(&call, format!("Wrote {path}"))
                                .with_details(json!({ "path": absolute(&self.cwd, path) })),
                        });
                    }
                }
                let _ = self.write(&rpc_reply(id, reply));
            }
            _ => {
                let _ = self.write(&json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": { "code": -32601, "message": format!("{method} is not supported by termide") }
                }));
            }
        }
    }

    fn ask_permission(&self, params: &Value) -> Value {
        let options = params["options"].as_array().cloned().unwrap_or_default();
        let pick = |kinds: &[&str]| {
            kinds.iter().find_map(|kind| {
                options
                    .iter()
                    .find(|o| o["kind"].as_str() == Some(kind))
                    .and_then(|o| o["optionId"].as_str())
                    .map(str::to_string)
            })
        };
        let selected = |chosen: Option<String>| match chosen {
            Some(option_id) => {
                json!({ "outcome": { "outcome": "selected", "optionId": option_id } })
            }
            None => json!({ "outcome": { "outcome": "cancelled" } }),
        };
        // A call of termide's own tool (Codex asks before every MCP call) is
        // judged when it runs on termide's server; asking here too would
        // put up a second card for it.
        let id = params["toolCall"]["toolCallId"].as_str().unwrap_or("");
        if self
            .host_calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(id)
        {
            return selected(pick(&["allow_once", "allow_always"]));
        }
        // Codex asks about a call it has already announced by its id alone,
        // with no kind: what the request leaves out is the announced call's.
        let mut tool_call = params["toolCall"].clone();
        if let Some(announced) = self
            .announced
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
        {
            for (key, value) in announced.as_object().into_iter().flatten() {
                if tool_call.get(key).is_none_or(Value::is_null) {
                    tool_call[key] = value.clone();
                }
            }
        }
        let call = permission_call(&tool_call);
        let ctx = ToolContext {
            session: Some(self.session_view()),
            ..ToolContext::new(self.cwd.clone())
        };
        // The hooks decide the request as they would for the built-in agent:
        // a read-only command or a matching rule passes without a prompt, an
        // unknown one reaches the user, and a session or always grant is
        // recorded so it is asked only once. The user is never troubled with
        // anything the built-in agent would have let through silently.
        let decision = self
            .hooks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .before_tool_call(&call, &ctx);
        // Always answer "once": termide stays the source of truth for grants,
        // so the agent keeps asking and termide keeps deciding silently,
        // rather than the agent remembering a rule of its own.
        let chosen = match decision {
            ToolDecision::Block { .. } => pick(&["reject_once", "reject_always"]),
            // The permission hooks only ever allow or block; any allowing
            // verdict answers the request "once".
            _ => pick(&["allow_once", "allow_always"]),
        };
        selected(chosen)
    }

    /// One `session/update` into the events the panel understands.
    fn on_update(self: &Arc<Self>, update: &Value) {
        let kind = update["sessionUpdate"].as_str().unwrap_or("");
        if self.replaying.load(Ordering::Acquire)
            && matches!(
                kind,
                "user_message_chunk"
                    | "agent_message_chunk"
                    | "agent_thought_chunk"
                    | "tool_call"
                    | "tool_call_update"
                    | "plan"
                    | "compaction_update"
                    | "compaction_summary_chunk"
            )
        {
            return;
        }
        if matches!(kind, "tool_call" | "tool_call_update") && self.on_compaction_call(update) {
            return;
        }
        if kind == "tool_call" {
            self.turn_acted.store(true, Ordering::Release);
        }
        match kind {
            "agent_message_chunk" => {
                self.steered_on();
                let text = update["content"]["text"].as_str().unwrap_or("").to_string();
                let mut open = self
                    .open_message
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if open.is_none() {
                    *open = Some(String::new());
                    let _ = self.events.send(AgentEvent::MessageStart {
                        prompt_tokens: None,
                    });
                }
                if let Some(buffer) = open.as_mut() {
                    buffer.push_str(&text);
                }
                let _ = self
                    .events
                    .send(AgentEvent::MessageUpdate(StreamEvent::TextDelta(text)));
            }
            "agent_thought_chunk" => {
                self.steered_on();
                let text = update["content"]["text"].as_str().unwrap_or("").to_string();
                if self
                    .open_message
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_none()
                {
                    *self
                        .open_message
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(String::new());
                    let _ = self.events.send(AgentEvent::MessageStart {
                        prompt_tokens: None,
                    });
                }
                self.open_thought
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push_str(&text);
                let _ = self
                    .events
                    .send(AgentEvent::MessageUpdate(StreamEvent::ThinkingDelta(text)));
            }
            "tool_call" => {
                // Text streamed so far becomes its own message before the call.
                self.close_message(StopReason::ToolUse, None);
                // A call announced before its arguments are known (Claude Code
                // streams them) waits for the update that brings them, so it
                // shows with its command or path.
                if !has_arguments(update) && !is_finished(update) {
                    let id = update["toolCallId"].as_str().unwrap_or("").to_string();
                    self.announced
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(id, update.clone());
                    return;
                }
                self.note_host_call(update);
                self.start_tool_call(tool_call_of(update));
                if is_finished(update) {
                    self.finish_tool_call(update);
                }
            }
            "tool_call_update" => {
                let id = update["toolCallId"].as_str().unwrap_or("");
                if has_arguments(update) || is_finished(update) {
                    let announced = self
                        .announced
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .remove(id);
                    if let Some(mut call) = announced {
                        for (key, value) in update.as_object().into_iter().flatten() {
                            if !value.is_null() {
                                call[key] = value.clone();
                            }
                        }
                        self.note_host_call(&call);
                        self.start_tool_call(tool_call_of(&call));
                    } else if has_arguments(update) {
                        // Claude Code streams the raw input: the call started
                        // on its first keys, and later updates bring the rest.
                        self.update_tool_arguments(id, update);
                    }
                }
                if is_finished(update) {
                    self.finish_tool_call(update);
                }
            }
            "usage_update" => {
                if let (Some(used), Some(size)) = (update["used"].as_u64(), update["size"].as_u64())
                {
                    // Kept as the agent states it; the user's limit is
                    // applied where it is read.
                    if let Some(path) = self.windows.get() {
                        windows::remember(path, &self.window_key(), size);
                    }
                    let size = match self.context_limit.load(Ordering::Acquire) {
                        0 => size,
                        limit => size.min(limit),
                    };
                    *self.context.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some((used, size));
                }
                let limit = &update["_meta"]["_claude/rateLimit"];
                if limit.is_object() {
                    *self
                        .limit_reset
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = rejected_until(limit);
                }
            }
            "compaction_update" => self.on_compaction_update(update),
            "compaction_summary_chunk" => {
                let text = update["content"]["text"].as_str().unwrap_or("");
                let Some(id) = update["compactionId"].as_str() else {
                    return;
                };
                if self.open_compaction(id) {
                    if let Some(open) = self
                        .compaction
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .as_mut()
                    {
                        open.summary.push_str(text);
                    }
                    let _ = self
                        .events
                        .send(AgentEvent::CompactionUpdate(StreamEvent::TextDelta(
                            text.to_string(),
                        )));
                }
            }
            "current_model_update" => {
                if let Some(id) = update["modelId"].as_str() {
                    *self
                        .current_model
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(id.to_string());
                }
            }
            "available_commands_update" => {
                let commands = update["availableCommands"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|command| {
                        Some(AgentCommand {
                            name: command["name"]
                                .as_str()?
                                .trim_start_matches('/')
                                .to_string(),
                            description: command["description"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            hint: command["input"]["hint"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                        })
                    })
                    .collect();
                *self.commands.lock().unwrap_or_else(PoisonError::into_inner) = commands;
            }
            // The agent changed its options itself: the model among them.
            "config_option_update" => {
                if let Some(options) = update["configOptions"].as_array() {
                    self.adopt_options(options);
                }
            }
            other => log::debug!("acp {}: update {other} ignored", self.name),
        }
    }

    /// Make the compaction `id` the open one, starting it when it is new;
    /// `false` for one that already ended, whose late reports are dropped.
    fn open_compaction(&self, id: &str) -> bool {
        if self
            .compaction_ended
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_deref()
            == Some(id)
        {
            return false;
        }
        if self
            .compaction
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|open| open.id == id)
        {
            return true;
        }
        // Text streamed so far is its own message, before the compaction.
        self.close_message(StopReason::ToolUse, None);
        let tokens_before = self
            .context
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map_or(0, |(used, _)| used);
        let reason = if self.prompted_compact() {
            CompactionReason::Manual
        } else {
            CompactionReason::Threshold
        };
        *self
            .compaction
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(OpenCompaction {
            id: id.to_string(),
            summary: String::new(),
            tokens_before,
            reason,
            started: Instant::now(),
        });
        let _ = self.events.send(AgentEvent::CompactionStart {
            reason,
            prompt_tokens: tokens_before,
        });
        true
    }

    /// Whether the turn running was asked for with the agent's `/compact`.
    fn prompted_compact(&self) -> bool {
        self.last_prompt
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|blocks| blocks.last())
            .and_then(|block| block["text"].as_str())
            .and_then(|text| text.trim_start().strip_prefix("/compact"))
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
    }

    /// A `compaction_update`: the compaction it names starts, takes the
    /// summary it carries, and ends on a final status. A finished one
    /// replaces the history with its summary, as the built-in loop's does.
    fn on_compaction_update(self: &Arc<Self>, update: &Value) {
        let Some(id) = update["compactionId"].as_str() else {
            return;
        };
        if !self.open_compaction(id) {
            return;
        }
        // A summary given replaces what came in chunks; `null` or `[]`
        // clears it, and its absence keeps it.
        if let Some(summary) = update.get("summary") {
            if let Some(open) = self
                .compaction
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_mut()
            {
                open.summary = blocks_text(summary);
            }
        }
        let status = update["status"].as_str().unwrap_or("");
        if !matches!(status, "completed" | "failed" | "cancelled") {
            // `in_progress`, or a status this client does not know.
            return;
        }
        let error = update["error"].as_str().filter(|error| !error.is_empty());
        self.end_compaction(status, error, None);
    }

    /// The made-up "Compact conversation" call an adapter that does not know
    /// the compaction capability reports its compaction as: it starts the
    /// compaction and its end ends it, with no summary, so the log keeps the
    /// history whole. `false` for any other call.
    fn on_compaction_call(self: &Arc<Self>, update: &Value) -> bool {
        let Some(id) = update["toolCallId"].as_str() else {
            return false;
        };
        let known = self
            .compaction
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|open| open.id == id)
            || self
                .compaction_ended
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_deref()
                == Some(id);
        let fresh = update["sessionUpdate"] == "tool_call" && is_compaction_call(update);
        if !(known || fresh) {
            return false;
        }
        if !self.open_compaction(id) || !is_finished(update) {
            return true;
        }
        let status = update["status"].as_str().unwrap_or("");
        // Its error comes as the call's result, after a fixed prefix.
        let text = content_text(&update["content"]);
        let error = text.trim();
        let error = error
            .strip_prefix("Compaction failed:")
            .unwrap_or(error)
            .trim();
        let tokens_before = update["rawOutput"]["preTokens"].as_u64();
        self.end_compaction(status, Some(error).filter(|e| !e.is_empty()), tokens_before);
        true
    }

    /// End the open compaction with `status` (`completed`, `failed` or
    /// `cancelled`) and hand the turn the messages held while it ran.
    /// `tokens_before` is the agent's own count of the context it began
    /// with, when it gives one.
    fn end_compaction(
        self: &Arc<Self>,
        status: &str,
        error: Option<&str>,
        tokens_before: Option<u64>,
    ) {
        let Some(open) = self
            .compaction
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return;
        };
        *self
            .compaction_ended
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(open.id.clone());
        let tokens_before = tokens_before
            .filter(|tokens| *tokens > 0)
            .unwrap_or(open.tokens_before);
        let summary = open.summary.trim().to_string();
        let event = if status == "completed" {
            AgentEvent::Compacted {
                // Claude Code keeps nothing of the history verbatim.
                kept: 0,
                tokens_before,
                // Corrected by the `usage_update` that follows.
                tokens_after: (summary.chars().count() / 4) as u64,
                call: CompactionCall {
                    reason: open.reason,
                    stop_reason: StopReason::Stop,
                    duration_ms: open.started.elapsed().as_millis() as u64,
                    usage: Usage::default(),
                    thinking_chars: 0,
                    tool_calls: Vec::new(),
                    input_cut: Vec::new(),
                },
                summary,
            }
        } else {
            AgentEvent::CompactionFailed {
                error: error.unwrap_or(status).to_string(),
                tokens_before,
                call: None,
                reply: String::new(),
            }
        };
        let _ = self.events.send(event);
        self.steer_held();
    }

    /// Whether the agent is compacting its context: a message steered into
    /// the turn now would abort the compaction (Claude Code delivers a steer
    /// by interrupting the model call running), so it waits for the end.
    fn compacting(&self) -> bool {
        self.compaction
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Hand the running turn the messages that waited for its compaction.
    fn steer_held(self: &Arc<Self>) {
        if !self.steering.load(Ordering::Acquire) || !self.turn_running.load(Ordering::Acquire) {
            return;
        }
        let held = self
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for message in held {
            self.steer_now(message);
        }
    }

    /// Show a call that starts, and hold it for the log.
    fn start_tool_call(&self, call: ToolCall) {
        self.intent.push_calls(std::iter::once(&call));
        let _ = self
            .events
            .send(AgentEvent::ToolExecutionStart { call: call.clone() });
        let mut calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        calls.unlogged.push(call.clone());
        calls.open.push(call);
    }

    /// Take the fuller raw input of a running call: its log entry, the
    /// arguments a call of termide's own tool is known by, and the shown
    /// block. Nothing happens for a call not started or with nothing new.
    fn update_tool_arguments(&self, id: &str, update: &Value) {
        let input = &update["rawInput"];
        let call = {
            let mut calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(open) = calls.open.iter_mut().find(|call| call.id == id) else {
                return;
            };
            let mut host_calls = self
                .host_calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let arguments = match host_calls.get_mut(id) {
                // termide's own tool: the raw input is its arguments, or
                // carries them when Codex names the server and the tool.
                Some((_, known)) => {
                    let fresh = match host_tool_of(update) {
                        Some((_, arguments)) => arguments,
                        None => input.clone(),
                    };
                    *known = fresh.clone();
                    fresh
                }
                // The agent's own tool keeps the title beside its raw input.
                None => {
                    let mut arguments = open.arguments.clone();
                    for (key, value) in input.as_object().into_iter().flatten() {
                        arguments[key] = value.clone();
                    }
                    arguments
                }
            };
            if arguments == open.arguments {
                return;
            }
            open.arguments = arguments.clone();
            let shown = open.clone();
            if let Some(unlogged) = calls.unlogged.iter_mut().find(|call| call.id == id) {
                unlogged.arguments = arguments;
            }
            shown
        };
        let _ = self.events.send(AgentEvent::ToolCallUpdate { call });
    }

    /// Log the calls started and not yet logged, as one assistant message.
    fn log_started_calls(&self) {
        let calls = std::mem::take(
            &mut self
                .calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .unlogged,
        );
        if calls.is_empty() {
            return;
        }
        let message = AssistantMessage {
            content: calls.into_iter().map(AssistantContent::ToolCall).collect(),
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
            provider: ACP_PROVIDER.into(),
            model: self.model_name(),
            error_message: None,
            failure: None,
            timestamp: now_millis(),
        };
        let _ = self
            .events
            .send(AgentEvent::MessageEnd(Message::Assistant(message)));
    }

    /// Log `result` after the calls it answers; a result for a call that was
    /// never shown starting stays out of the log, which has no call for it.
    fn log_tool_result(&self, result: ToolResultMessage) {
        let started = {
            let mut calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
            let index = calls
                .open
                .iter()
                .position(|call| call.id == result.tool_call_id);
            index.map(|index| calls.open.remove(index)).is_some()
        };
        if !started {
            return;
        }
        self.log_started_calls();
        let _ = self
            .events
            .send(AgentEvent::MessageEnd(Message::ToolResult(result)));
        self.flush_steered();
    }

    /// At the turn's end, give each call still without a result one saying
    /// it did not finish, so the log holds no call left unanswered.
    fn close_unfinished_calls(&self) {
        self.close_open_calls("The turn ended before this call finished.");
    }

    /// Give each call still open a result saying `why` it has none.
    fn close_open_calls(&self, why: &str) {
        let open = std::mem::take(
            &mut self
                .calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .open,
        );
        if open.is_empty() {
            return;
        }
        self.log_started_calls();
        for call in open {
            let result = ToolResultMessage::error(&call, why);
            let _ = self
                .events
                .send(AgentEvent::MessageEnd(Message::ToolResult(result)));
        }
    }

    /// Remember a call of termide's own tool by its id, until it finishes.
    fn note_host_call(&self, update: &Value) {
        if let Some(host) = host_tool_of(update) {
            let id = update["toolCallId"].as_str().unwrap_or("").to_string();
            self.host_calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(id, host);
        }
    }

    fn finish_tool_call(&self, update: &Value) {
        let mut call = tool_call_of(update);
        let host = self
            .host_calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&call.id);
        // termide ran it: the details and pictures its tool gave, which the
        // agent's report leaves out, so its block draws as for the built-in
        // agent.
        let host_finished = host.and_then(|(name, arguments)| {
            call.name = name;
            let server = self
                .mcp_server
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let server = server.as_ref()?;
            // Closed while termide still runs it: the agent moved it to the
            // background, and the panel shows it until it ends.
            if server
                .running_calls()
                .iter()
                .any(|running| running.name == call.name && running.arguments == arguments)
            {
                self.background
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(ToolCall {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: arguments.clone(),
                        extra_content: None,
                    });
            }
            server.take_finished(&call.name, &arguments)
        });
        let (host_details, mut images) = match host_finished {
            Some(finished) => (finished.details, finished.images),
            None => (None, Vec::new()),
        };
        if images.is_empty() {
            images = content_images(&update["content"]);
        }
        // Codex reports an MCP call's result as the MCP result, not as content.
        let mut text = content_text(&update["content"]);
        if text.is_empty() {
            text = mcp_result_text(&update["rawOutput"]["result"]);
        }
        let failed = update["status"].as_str() == Some("failed");
        let mut result = if failed {
            ToolResultMessage::error(
                &call,
                if text.is_empty() {
                    "failed".into()
                } else {
                    text
                },
            )
        } else {
            ToolResultMessage::text(&call, text).with_images(images)
        };
        if let Some(details) = host_details {
            result = result.with_details(details);
        } else if let Some(path) = update["locations"][0]["path"].as_str() {
            result = result.with_details(json!({ "path": absolute(&self.cwd, path) }));
        }
        let _ = self.events.send(AgentEvent::ToolExecutionEnd {
            result: result.clone(),
        });
        self.log_tool_result(result);
    }
}

/// Where among the server's `running` calls the call `call` is: by its id,
/// which the server knows it by once the agent announced it before it ran;
/// else by its tool and arguments.
fn running_one(call: &ToolCall, running: &[termide_agent_mcp::RunningCall]) -> Option<usize> {
    running.iter().position(|r| r.id == call.id).or_else(|| {
        running
            .iter()
            .position(|r| r.name == call.name && r.arguments == call.arguments)
    })
}

/// The ACP tool call as the panel's [`ToolCall`]: the kind stands as the
/// name (`edit`, `execute`, `read`, …), so the transcript line and the
/// editor reload behave as for built-in tools, and the title travels in the
/// arguments when the agent gives no raw input.
fn tool_call_of(update: &Value) -> ToolCall {
    // A call of termide's own tool shows as that tool, as the built-in loop
    // shows it.
    if let Some((name, arguments)) = host_tool_of(update) {
        return ToolCall {
            id: update["toolCallId"].as_str().unwrap_or("").to_string(),
            name,
            arguments,
            extra_content: None,
        };
    }
    let title = update["title"].as_str().unwrap_or("").to_string();
    let mut arguments = update
        .get("rawInput")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let kind = update["kind"].as_str().unwrap_or("other");
    // `other` names no tool: the title does — what the model called, such as
    // a `shell` it made up in place of `mcp__termide__shell` — so the block
    // reads as that call, not as the ACP kind.
    let name = if kind == "other" && !title.is_empty() {
        title
    } else {
        fill_own_call(update, kind, &title, &mut arguments);
        if !title.is_empty() {
            arguments["title"] = json!(title);
        }
        match kind {
            "execute" => SHELL_TOOL.to_string(),
            kind => kind.to_string(),
        }
    };
    ToolCall {
        id: update["toolCallId"].as_str().unwrap_or("").to_string(),
        name,
        arguments,
        extra_content: None,
    }
}

/// Give an agent's own command, read or edit of ACP `kind` the arguments
/// termide's tool of the kind takes, where its raw input `arguments` lacks
/// them: Codex gives a command as `command` words, or only as the call's
/// `title`, and the file of a read or an edit only in the call's locations or
/// its diff.
fn fill_own_call(call: &Value, kind: &str, title: &str, arguments: &mut Value) {
    match kind {
        "execute" if arguments["command"].as_str().is_none() => {
            let command = match &arguments["command"] {
                Value::Array(words) => words
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => title.to_string(),
            };
            arguments["command"] = json!(command);
        }
        "read" | "edit" if arguments["path"].as_str().is_none() => {
            let path = call["locations"][0]["path"].as_str().or_else(|| {
                call["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|block| block["type"] == "diff")
                    .and_then(|diff| diff["path"].as_str())
            });
            if let Some(path) = path {
                arguments["path"] = json!(path);
            }
        }
        _ => {}
    }
}

/// The name and arguments of a call of a tool termide's MCP server serves:
/// Claude Code titles it `mcp__termide__<tool>` with the arguments as its raw
/// input, Codex names the server and the tool in its raw input.
fn host_tool_of(update: &Value) -> Option<(String, Value)> {
    let input = &update["rawInput"];
    let object = |value: &Value| {
        Some(value)
            .filter(|v| v.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}))
    };
    if let Some(name) = update["title"]
        .as_str()
        .and_then(|title| title.strip_prefix(HOST_TOOL_PREFIX))
    {
        return Some((name.to_string(), object(input)));
    }
    let tool = input["tool"]
        .as_str()
        .filter(|_| input["server"] == SERVER_NAME)?;
    Some((tool.to_string(), object(&input["arguments"])))
}

/// The agent's subscription for a side call on "the session's model": a
/// request that names no model runs on the model the agent is on now.
struct CurrentModel {
    inner: AcpProvider,
    shared: std::sync::Weak<Shared>,
}

impl Provider for CurrentModel {
    fn name(&self) -> &str {
        ACP_PROVIDER
    }

    fn stream(
        &self,
        request: &termide_agent_core::Request<'_>,
        on_event: &mut dyn FnMut(StreamEvent),
        cancel: &CancelToken,
    ) -> AssistantMessage {
        let current = self
            .shared
            .upgrade()
            .and_then(|shared| {
                shared
                    .current_model
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
            })
            .filter(|_| request.model.id.is_empty());
        match current {
            Some(id) => {
                let model = ModelSpec {
                    id,
                    ..request.model.clone()
                };
                let request = termide_agent_core::Request {
                    model: &model,
                    ..*request
                };
                self.inner.stream(&request, on_event, cancel)
            }
            None => self.inner.stream(request, on_event, cancel),
        }
    }
}

/// "The session's model" of an agent that has no subscription to ask.
struct NoModel;

impl Provider for NoModel {
    fn name(&self) -> &str {
        ACP_PROVIDER
    }

    fn stream(
        &self,
        request: &termide_agent_core::Request<'_>,
        _on_event: &mut dyn FnMut(StreamEvent),
        _cancel: &CancelToken,
    ) -> AssistantMessage {
        AssistantMessage::failed(
            ACP_PROVIDER,
            &request.model.id,
            StopReason::Error,
            "the agent has no model to review with",
        )
    }
}

/// The names of `tools`.
fn tool_names(tools: &ToolRegistry) -> BTreeSet<String> {
    tools.names().into_iter().map(str::to_string).collect()
}

/// An ACP config option of the `select` type, its values flat or in groups;
/// `None` for another type, which a client that does not know it ignores.
fn option_of(option: &Value) -> Option<BackendOption> {
    if option["type"].as_str().is_some_and(|kind| kind != "select") {
        return None;
    }
    let id = option["id"].as_str()?.to_string();
    let entry = |o: &Value| {
        let id = o["value"].as_str()?.to_string();
        let name = o["name"].as_str().unwrap_or(&id).to_string();
        Some(BackendModel { id, name })
    };
    let values = option["options"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|o| match o["options"].as_array() {
            Some(group) => group.iter().filter_map(entry).collect::<Vec<_>>(),
            None => entry(o).into_iter().collect(),
        })
        .collect();
    Some(BackendOption {
        name: option["name"].as_str().unwrap_or(&id).to_string(),
        category: option["category"].as_str().map(str::to_string),
        current: option["currentValue"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        values,
        id,
    })
}

/// The text of an MCP `tools/call` result's content.
fn mcp_result_text(result: &Value) -> String {
    let texts: Vec<&str> = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block["text"].as_str())
        .collect();
    texts.join("\n")
}

/// The token usage a `session/prompt` result reports, when it does.
fn usage_of(result: &Value, flavor: AcpFlavor) -> Usage {
    let usage = &result["usage"];
    let count = |key: &str| usage[key].as_u64().unwrap_or(0);
    let mut usage = Usage {
        input: count("inputTokens"),
        output: count("outputTokens"),
        cache_read: count("cachedReadTokens"),
        cache_write: count("cachedWriteTokens"),
        // Codex's reasoning, within its output; Claude Code never counts
        // its thinking apart.
        thinking: count("thoughtTokens"),
    };
    // Gemini CLI passes on the Gemini API's counts as they are: the prompt
    // with its cached part in it, and the output without the thoughts.
    if flavor == AcpFlavor::GeminiCli {
        usage.input = usage.input.saturating_sub(usage.cache_read);
        usage.output += usage.thinking;
    }
    usage
}

/// What a `session/prompt` result's `_meta.quota.model_usage` counts beyond
/// its `usage`, by model. Claude Code's `usage` is its conversation's own
/// turns, while its rows by model also count its subagents, summaries and
/// helper calls; Codex's single row is its `usage`, so nothing is beyond.
///
/// The turns are taken out of one row: among those that hold them whole, one
/// on `current` (the session's model, named its own way) or else the
/// largest; with none holding them, the largest row.
fn usage_beyond(result: &Value, usage: &Usage, current: &str) -> Vec<(String, Usage)> {
    let Some(rows) = result["_meta"]["quota"]["model_usage"].as_array() else {
        return Vec::new();
    };
    let mut rows: Vec<(String, Usage)> = rows
        .iter()
        .filter_map(|row| {
            Some((
                bare_model(row["model"].as_str()?),
                token_count(&row["token_count"]),
            ))
        })
        .collect();
    let covers = |row: &Usage| {
        row.input >= usage.input
            && row.output >= usage.output
            && row.cache_read >= usage.cache_read
            && row.cache_write >= usage.cache_write
    };
    let current = bare_model(current).to_lowercase();
    let named = |model: &str| {
        let model = model.to_lowercase();
        !current.is_empty() && (model.contains(&current) || current.contains(&model))
    };
    let host = rows
        .iter()
        .enumerate()
        .filter(|(_, (_, row))| covers(row))
        .max_by_key(|(_, (model, row))| (named(model), row.total()))
        .or_else(|| {
            rows.iter()
                .enumerate()
                .max_by_key(|(_, (_, row))| row.total())
        })
        .map(|(index, _)| index);
    if let Some(index) = host {
        rows[index].1 = rows[index].1.less(usage);
    }
    rows.retain(|(_, row)| row.total() > 0);
    rows
}

/// A model id without the context-size suffix Claude Code decorates it
/// with, `claude-sonnet-5[1m]` read as `claude-sonnet-5`.
fn bare_model(model: &str) -> String {
    match model.find('[') {
        Some(at) if model.ends_with(']') => model[..at].to_string(),
        _ => model.to_string(),
    }
}

/// A `_meta.quota` `token_count`: Codex's shape, which Claude Code's
/// follows but for spelling its cache writes its own way.
fn token_count(count: &Value) -> Usage {
    let count = |key: &str| count[key].as_u64().unwrap_or(0);
    Usage {
        input: count("inputTokens"),
        output: count("outputTokens"),
        cache_read: count("cachedInputTokens"),
        cache_write: count("cachedWriteTokens").max(count("cacheWriteInputTokens")),
        thinking: count("reasoningOutputTokens"),
    }
}

/// Whether a tool call update carries the call's arguments.
fn has_arguments(update: &Value) -> bool {
    update["rawInput"]
        .as_object()
        .is_some_and(|input| !input.is_empty())
}

/// Whether a `tool_call` is the "Compact conversation" call an adapter
/// without the compaction capability stands in for its compaction: Claude
/// Code's names its tool `compact`; failing that, its title and kind with
/// no arguments.
fn is_compaction_call(update: &Value) -> bool {
    update["_meta"]["claudeCode"]["toolName"] == "compact"
        || (update["title"] == "Compact conversation"
            && update["kind"] == "think"
            && !has_arguments(update))
}

/// Whether a tool call update reports the call done.
fn is_finished(update: &Value) -> bool {
    matches!(
        update["status"].as_str(),
        Some("completed") | Some("failed")
    )
}

/// Codex's approval preset and collaboration mode for the panel's `mode`.
/// `auto` has it ask like `configured`, so every request reaches termide's
/// rules and, in `auto`, termide's reviewer rather than Codex's own.
fn codex_modes(mode: Mode) -> (&'static str, &'static str) {
    match mode {
        Mode::Ask | Mode::Configured | Mode::Auto => ("read-only", "default"),
        Mode::Plan => ("read-only", "plan"),
        // Its `agent` preset is its own reviewer ("Auto review"), which
        // termide's stands in for: edits in the workspace pass, the rest asks.
        Mode::Edit => ("workspace-write", "default"),
        Mode::All => ("agent-full-access", "default"),
    }
}

/// Gemini CLI's approval mode for the panel's `mode`.
fn gemini_mode(mode: Mode) -> &'static str {
    match mode {
        Mode::Ask | Mode::Configured | Mode::Auto => "default",
        Mode::Plan => "plan",
        Mode::Edit => "autoEdit",
        Mode::All => "yolo",
    }
}

/// A `session/request_permission` `toolCall` as a termide [`ToolCall`], its
/// ACP `kind` translated to the tool name the permission rules speak so the
/// same logic judges it: `execute` becomes `shell` (the command placed under
/// `command`, from the raw input or the title), `read`/`edit` keep their
/// names, and anything else stands as its kind, which no built-in rule covers
/// and so reaches the user.
fn permission_call(tool_call: &Value) -> ToolCall {
    let title = tool_call["title"].as_str().unwrap_or("").to_string();
    let kind = tool_call["kind"].as_str().unwrap_or("other");
    let name = match kind {
        "execute" => SHELL_TOOL,
        other => other,
    };
    let mut arguments = tool_call
        .get("rawInput")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    // `shell` rules match on the command line and `edit` ones on the path;
    // if the agent gave neither in the raw input, they are read off the rest
    // of the call — for a command, its title is the best stand-in.
    fill_own_call(tool_call, kind, &title, &mut arguments);
    if !title.is_empty() {
        arguments["title"] = json!(title);
    }
    ToolCall {
        id: tool_call["toolCallId"].as_str().unwrap_or("").to_string(),
        name: name.to_string(),
        arguments,
        extra_content: None,
    }
}

/// The text of a tool call's content blocks: plain content as is, diffs as
/// a unified-looking summary.
fn content_text(content: &Value) -> String {
    let mut out = String::new();
    for block in content.as_array().into_iter().flatten() {
        let piece = match block["type"].as_str() {
            Some("content") => block["content"]["text"].as_str().unwrap_or("").to_string(),
            Some("diff") => format!(
                "--- {path}\n+++ {path}\n{}",
                block["newText"].as_str().unwrap_or(""),
                path = block["path"].as_str().unwrap_or("")
            ),
            _ => String::new(),
        };
        if !piece.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&piece);
        }
    }
    out
}

/// The pictures among a tool call's ACP content, as `(media type, base64
/// data)`.
fn content_images(content: &Value) -> Vec<(String, String)> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "content" && block["content"]["type"] == "image")
        .filter_map(|block| {
            let image = &block["content"];
            Some((
                image["mimeType"].as_str()?.to_string(),
                image["data"].as_str()?.to_string(),
            ))
        })
        .collect()
}

/// The text of a list of ACP content blocks, its text blocks one after
/// another; empty for anything else.
fn blocks_text(blocks: &Value) -> String {
    blocks
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block["text"].as_str())
        .collect()
}

fn absolute(cwd: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn rpc_reply(id: u64, result: Result<Value, String>) -> Value {
    match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(message) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": message } })
        }
    }
}

/// Most bytes one `fs/read_text_file` returns: a whole file larger than this
/// is refused, and a range stops at the last whole line within it.
const READ_TEXT_MAX_BYTES: usize = 16 * 1024 * 1024;

fn read_text_file(cwd: &Path, params: &Value) -> Result<Value, String> {
    use std::io::Read as _;
    let path = absolute(cwd, params["path"].as_str().ok_or("path missing")?);
    let cannot_read = |error: std::io::Error| format!("cannot read {}: {error}", path.display());
    let file = std::fs::File::open(&path).map_err(cannot_read)?;
    let line = params["line"].as_u64().map(|l| l.max(1) as usize - 1);
    let limit = params["limit"].as_u64().map(|l| l as usize);
    let content = match (line, limit) {
        (None, None) => {
            let mut bytes = Vec::new();
            file.take(READ_TEXT_MAX_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(cannot_read)?;
            if bytes.len() > READ_TEXT_MAX_BYTES {
                return Err(format!(
                    "{} is larger than {} MiB; read it in parts with `line` and `limit`",
                    path.display(),
                    READ_TEXT_MAX_BYTES / (1024 * 1024)
                ));
            }
            String::from_utf8(bytes).map_err(|_| {
                format!(
                    "cannot read {}: stream did not contain valid UTF-8",
                    path.display()
                )
            })?
        }
        (line, limit) => termide_agent_core::read_line_window(
            file,
            termide_agent_core::WindowSpec {
                first: line.unwrap_or(0),
                max_lines: limit.unwrap_or(usize::MAX),
                max_bytes: READ_TEXT_MAX_BYTES,
                max_line_bytes: READ_TEXT_MAX_BYTES,
                count_total: false,
            },
            &|| false,
        )
        .map_err(cannot_read)?
        .lines
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n"),
    };
    Ok(json!({ "content": content }))
}

fn write_text_file(cwd: &Path, params: &Value) -> Result<Value, String> {
    let path = absolute(cwd, params["path"].as_str().ok_or("path missing")?);
    let content = params["content"].as_str().ok_or("content missing")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    termide_fsutil::write_atomic(&path, content.as_bytes())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(json!({}))
}

#[cfg(test)]
mod tests;
